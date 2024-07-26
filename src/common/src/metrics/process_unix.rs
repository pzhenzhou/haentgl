use crate::metrics::metric_def::MetricsConsts;
use crate::{sys_utils, ShutdownMessage};
#[cfg(target_os = "linux")]
use metrics::GaugeValue;
use metrics::{describe_gauge, gauge, Gauge};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::info;

#[cfg(target_os = "linux")]
static LINUX_PAGE_SIZE: std::sync::LazyLock<i64> =
    std::sync::LazyLock::new(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) });

#[cfg(target_os = "linux")]
static CLOCK_TICK: std::sync::LazyLock<u64> =
    std::sync::LazyLock::new(|| unsafe { libc::sysconf(libc::_SC_CLK_TCK) as u64 });

pub struct ProcessRecorder {
    mem_rss: Gauge,
    mem_virtual_size: Gauge,
    cpu_total: Gauge,
    cpu_core_num: Gauge,
    stop_rx: watch::Receiver<ShutdownMessage>,
    #[cfg(target_os = "linux")]
    cpu_total_value: GaugeValue,
}

macro_rules! register_process_metric {
    ($metric_const:expr, $labels:expr) => {{
        let (name, desc, _, _) = $metric_const.get_metrics_pair();
        describe_gauge!(name, desc);
        gauge!(name, $labels)
    }};
}

impl ProcessRecorder {
    pub fn new(
        labels: Vec<(&'static str, String)>,
        stop_rx: watch::Receiver<ShutdownMessage>,
    ) -> Self {
        let mem_rss_metric = MetricsConsts::rss_mem_size();
        let mem_virtual_size_metric = MetricsConsts::virtual_mem_size();
        let cpu_total_metric = MetricsConsts::cpu_total();
        let cpu_core_num_metric = MetricsConsts::cpu_core_num();

        Self {
            mem_rss: register_process_metric!(mem_rss_metric, &labels),
            mem_virtual_size: register_process_metric!(mem_virtual_size_metric, &labels),
            cpu_total: register_process_metric!(cpu_total_metric, &labels),
            cpu_core_num: register_process_metric!(cpu_core_num_metric, &labels),
            stop_rx,
            #[cfg(target_os = "linux")]
            cpu_total_value: GaugeValue::Absolute(0_f64),
        }
    }

    #[cfg(target_os = "linux")]
    pub fn collect(&self) {
        let p = match procfs::process::Process::myself() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "ProxySrv basic metrics collect error [target_os=macos] cause by {e:?}"
                );
                // we can't construct a Process object, so there's no stats to gather
                return;
            }
        };
        let stat = match p.stat() {
            Ok(stat) => stat,
            Err(e) => {
                tracing::warn!(
                    "ProxySrv basic metrics collect error [target_os=linux] cause by {e:?}"
                );
                // we can't get the stat, so there's no stats to gather
                return;
            }
        };

        // memory
        self.mem_virtual_size.set(stat.vsize as f64);
        self.mem_rss
            .set((stat.rss as i64 * *LINUX_PAGE_SIZE) as f64);

        // cpu
        let total = (stat.utime + stat.stime) / *CLOCK_TICK;
        let past = match self.cpu_total_value {
            GaugeValue::Absolute(f) => f,
            _ => 0_f64,
        };
        self.cpu_total.increment(total as f64 - past);
        self.cpu_total_value.update_value(total as f64 - past);
        self.cpu_core_num
            .set(sys_utils::cpu::get_total_cpu() as f64);
    }

    #[cfg(target_os = "macos")]
    pub fn collect(&self) {
        let my_pid = unsafe { libc::getpid() };
        let clock_tick = unsafe {
            let mut info = mach2::mach_time::mach_timebase_info::default();
            let errno = mach2::mach_time::mach_timebase_info(&mut info as *mut _);
            if errno != 0 {
                1_f64
            } else {
                (info.numer / info.denom) as f64
            }
        };

        let proc_info = match darwin_libproc::task_info(my_pid) {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(
                    "ProxySrv basic metrics collect error [target_os=macos] cause by {e:?}"
                );
                return;
            }
        };

        // memory collect
        self.mem_rss.set(proc_info.pti_resident_size as f64);
        self.mem_virtual_size.set(proc_info.pti_virtual_size as f64);

        // cpu metric collect
        let cpu_total =
            (proc_info.pti_total_user + proc_info.pti_total_system) as f64 * clock_tick / 1e9;
        self.cpu_total.increment(cpu_total);
        self.cpu_core_num
            .set(sys_utils::cpu::get_total_cpu() as f64)
    }

    pub async fn start_auto_collect(&mut self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        info!("ProxySrv process metrics collector auto collect start.");
        let stop_rx_rc = Arc::new(self.stop_rx.clone());
        loop {
            tokio::select! {
               rs = async {stop_rx_rc.has_changed().unwrap()} => {
                  if rs {
                      //let shutdown_msg = self.stop_rx.borrow_and_update().clone();
                      if let ShutdownMessage::Cancel(_) = self.stop_rx.borrow_and_update().clone() {
                        break;
                     }
                  }
               }
               _= interval.tick() => {
                   self.collect();
               }
            }
        }
    }
}
