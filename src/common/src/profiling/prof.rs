use anyhow::anyhow;
use chrono::Local;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::{fs, thread};
use tracing::{debug, info, warn};

pub struct Prof {
    starting: AtomicBool,
}

impl Default for Prof {
    fn default() -> Self {
        Prof {
            starting: AtomicBool::new(false),
        }
    }
}

impl Prof {
    pub fn start(&'static self, duration: u64, profile_path: String) -> anyhow::Result<bool> {
        let started =
            self.starting
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Acquire);
        if let Ok(rs) = started {
            if !rs {
                match self.prof_sample(duration, profile_path) {
                    Ok(_) => Ok(true),
                    Err(e) => Err(anyhow!(
                        "ProxySrv CpuProfiler start start profiler error {e:?}"
                    )),
                }
            } else {
                Ok(false)
            }
        } else {
            warn!("ProxySrv CpuProfiler started error {started:?}");
            Ok(false)
        }
    }

    fn prof_sample(&'static self, duration: u64, profile_path: String) -> anyhow::Result<()> {
        let profile_path = PathBuf::from(profile_path);
        let starting = Arc::new(&self.starting);
        if fs::create_dir_all(&profile_path).is_ok() {
            let prof_thread = thread::Builder::new().name("SQL_PROXY_PROF_THD".to_string());
            let _rs = prof_thread.spawn(move || loop {
                debug!("ProxySrv CpuProfiler prof_sample running!");
                if !starting.load(Ordering::Acquire) {
                    debug!("ProxySrv CpuProfiler state stopped prof_sample exit.");
                    break;
                }
                let guard = pprof::ProfilerGuardBuilder::default()
                    .blocklist(&["libc", "libgcc", "pthread", "vdso"])
                    .build()
                    .unwrap();
                thread::sleep(Duration::from_secs(duration));
                let now_date = Local::now();
                let time_prefix = format!("{}", now_date.format("%Y-%m-%d-%H-%M-%S"));
                match guard.report().build() {
                    Ok(report) => {
                        let profile_svg = profile_path.join(format!("mp_cpu_{}.svg", time_prefix));
                        let file = fs::File::create(&profile_svg).unwrap();
                        report.flamegraph(file).unwrap();
                        info!("ProxySrv CpuProfiler save report to {:?}", profile_svg);
                    }
                    Err(err) => {
                        warn!(
                            "ProxySrv CpuProfiler. Failed to generate flamegraph: {}",
                            err
                        );
                    }
                }
            });

            Ok(())
        } else {
            Err(anyhow!(
                "ProxySrv CpuProfiler. Failed to create profile_path dir {profile_path:?}"
            ))
        }
    }

    pub fn stop(&self) -> anyhow::Result<bool> {
        let rs = self
            .starting
            .compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed);
        match rs {
            Ok(r) => Ok(r),
            Err(e) => {
                tracing::debug!("ProxySrv CpuProfiler stop error cause by {e:?}");
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn test_date_time_format() {
        let now_date = chrono::Local::now();
        let formatted = format!("{}", now_date.format("%Y-%m-%d-%H:%M:%S"));

        println!("formatted = {formatted:?}");
    }
}
