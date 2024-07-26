use anyhow::anyhow;
use parking_lot::RwLock;
use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs};
use tikv_jemalloc_ctl::{epoch as jemalloc_epoch, prof as jemalloc_prof, stats as jemalloc_stats};
use tracing::{info, warn};

const OPT_PROF: &[u8] = b"opt.prof\0";

const HEAP_FILE_PREFIX: &str = "%Y-%m-%d-%H-%M-%S";

pub async fn heap_analysis(heap_file_path: String) -> anyhow::Result<()> {
    let executable_path = env::current_exe()?;
    let jeprof_path = env::current_dir()?.join("jeprof");
    info!("jeprof_run executable_path = {:?}", executable_path);

    let prof_cmd = move || {
        std::process::Command::new(jeprof_path)
            .arg("--collapsed")
            .arg(executable_path)
            .arg(Path::new(&heap_file_path))
            .output()
    };

    match tokio::task::spawn_blocking(prof_cmd).await.unwrap() {
        Ok(output) => {
            info!("jeprof_run command exec ok = {:?}", output);
            if output.status.success() {
                fs::write(Path::new("collapsed"), &output.stdout)?;
                Ok(())
            } else {
                info!("jeprof_run command exec err: {:?}", output);
                Err(anyhow!(
                    "jeprof exit with an error. stdout: {}, stderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        }
        Err(e) => {
            warn!("ProxySrv heap_profile jeprof_error cause by {:?}", e);
            Err(e.into())
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeapProfileOpts {
    auto_profile: Arc<AtomicBool>,
    tick: Option<Duration>,
    profile_dir: String,
    threshold: f32,
}

impl Default for HeapProfileOpts {
    fn default() -> Self {
        HeapProfileOpts {
            auto_profile: Arc::new(AtomicBool::new(false)),
            tick: None,
            profile_dir: "/tmp/mono_proxy_heap".to_string(),
            threshold: 0.7,
        }
    }
}

pub struct HeapProfiler {
    profile_opts: Arc<RwLock<HeapProfileOpts>>,
    jemalloc_dump_mib: jemalloc_prof::dump_mib,
    jemalloc_allocated_mib: jemalloc_stats::allocated_mib,
    jemalloc_epoch_mib: tikv_jemalloc_ctl::epoch_mib,
    state_rx: tokio::sync::watch::Receiver<bool>,
    state_tx: tokio::sync::watch::Sender<bool>,
}

impl HeapProfiler {
    pub fn new_with_opts(profile_opts: HeapProfileOpts) -> anyhow::Result<Self> {
        let opt_rs = unsafe {
            tikv_jemalloc_ctl::raw::read::<bool>(OPT_PROF)
                .map_err(|e| format!("Failed to read opt.prof. cause by {}", e))
        };
        info!("ProxySrv HeapProfile read opt.prof {:?}", opt_rs);
        let opt_prof = if let Ok(opt_prof) = opt_rs {
            opt_prof
        } else {
            false
        };
        if opt_prof {
            Err(anyhow!(
                "opt.prof != ON please start the mono-proxy \
                with proper MALLOC ENV. e.g. MALLOC_CONF=prof:true"
            ))
        } else {
            fs::create_dir_all(&profile_opts.profile_dir)?;
            let jemalloc_dump_mib = jemalloc_prof::dump::mib().unwrap();
            let jemalloc_allocated_mib = jemalloc_stats::allocated::mib().unwrap();
            let jemalloc_epoch_mib = jemalloc_epoch::mib().unwrap();
            let profile_opts = Arc::new(RwLock::new(profile_opts));
            let (tx, rx) = tokio::sync::watch::channel(false);
            Ok(Self {
                profile_opts,
                jemalloc_dump_mib,
                jemalloc_allocated_mib,
                jemalloc_epoch_mib,
                state_rx: rx,
                state_tx: tx,
            })
        }
    }

    pub fn dump_profile(&self) -> anyhow::Result<String> {
        let file_prefix = chrono::Local::now().format(HEAP_FILE_PREFIX);
        let file_name = format!("{}.mono_proxy", file_prefix);
        let profile_opts = self.profile_opts.read();
        let dir = &profile_opts.profile_dir;
        let file_path = Path::new(dir)
            .join(file_name)
            .to_str()
            .expect("file path is not valid utf8")
            .to_owned();

        let file_path_c = CString::new(file_path).expect("0 byte in file path");
        let dump_rs = self
            .jemalloc_dump_mib
            .write(unsafe { &*(file_path_c.as_c_str() as *const _) });
        match dump_rs {
            Ok(_) => Ok(dir.clone()),
            Err(e) => {
                warn!("ProxySrv HeapProfile {e}");
                Err(anyhow!(e))
            }
        }
    }

    fn change_auto_profile_state(&self, curr: bool, new: bool) -> bool {
        let opts = self.profile_opts.write();
        let cas_rs = opts
            .auto_profile
            .compare_exchange(curr, new, Ordering::Acquire, Ordering::Relaxed)
            .unwrap_or(false);
        cas_rs == curr
    }

    pub fn stop_auto_profile(&self) {
        if self.change_auto_profile_state(true, false) {
            self.state_tx
                .send(true)
                .expect("Can't stop auto heap profile.")
        }
    }

    pub fn start_auto_profile(&'static mut self, total_memory_size: usize) {
        let started = self.change_auto_profile_state(false, true);
        if started {
            let tick = self
                .profile_opts
                .read()
                .tick
                .unwrap_or_else(|| Duration::from_secs(3));

            let threshold = self.profile_opts.read().threshold;
            let threshold_dump_heap = (total_memory_size as f64 * threshold as f64) as usize;
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(tick);
                let mut prev_jemalloc_allocated_bytes = 0;
                // let threshold_dump_heap = self.auto_dump_threshold;
                loop {
                    tokio::select! {
                        rx_changed = self.state_rx.changed() => {
                            if rx_changed.is_ok() && *self.state_rx.borrow_and_update(){
                               info!("ProxySrv HeapProfiler receive stop signal {:?}", rx_changed);
                               return ;
                            }
                        }
                        _ =  interval.tick() => {
                        }
                    }
                    let jemalloc_allocated_bytes = if self.jemalloc_epoch_mib.advance().is_ok() {
                        self.jemalloc_allocated_mib.read().unwrap_or_else(|e| {
                            warn!("ProxySrv HeapProfiler Failed to read allocated {e}");
                            prev_jemalloc_allocated_bytes
                        })
                    } else {
                        prev_jemalloc_allocated_bytes
                    };

                    if jemalloc_allocated_bytes > threshold_dump_heap
                        && prev_jemalloc_allocated_bytes <= threshold_dump_heap
                    {
                        self.dump_profile().expect("filed to heap dump");
                    }
                    prev_jemalloc_allocated_bytes = jemalloc_allocated_bytes;
                }
            });
        } else {
            warn!("ProxySrv HeapProfiler already started!!");
        }
    }
}
