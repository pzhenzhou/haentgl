pub mod metric_def;
pub mod process_unix;

use crate::sys_utils::sys::hostname;
use metrics::{
    describe_counter, describe_gauge, describe_histogram, histogram, Histogram, IntoLabels,
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
// use once_cell::sync::Lazy;
use parking_lot::RwLock;

use crate::metrics::metric_def::MetricsConsts;
use std::sync::{Arc, Once};
use std::{fmt, vec};
use tracing::{info, warn};

static DEFAULT_BUCKETS: &[f64; 26] = &[
    1e+2, 2e+2, 4e+2, 6e+2, 8e+2, 1e+3, 2e+3, 4e+3, 6e+3, 8e+3, 1e+4, 2e+4, 4e+4, 6e+4, 8e+4, 1e+5,
    2e+5, 4e+5, 6e+5, 8e+5, 1e+6, 2e+6, 4e+6, 6e+6, 8e+6, 1e+7,
];

const DEFAULT_QUANTILES: &[f64; 9] = &[0.0, 0.5, 0.7, 0.8, 0.9, 0.95, 0.99, 0.999, 1.0];

#[derive(Debug, Clone, Copy)]
pub enum MetricType {
    Gauge,
    Counter,
    Histogram,
}

#[macro_export]
macro_rules! unix_process_recorder {
    ($recorder:expr, $metric_init:ident, $unit:expr,$labels: expr) => {{
        let metrics_value = MetricsConsts::$metric_init();
        let (_name, desc, key_name, _metric_type) = metrics_value.get_metrics_pair();

        let metrics_key = Key::from_parts(key_name.clone(), $labels.clone());
        $recorder.describe_gauge(key_name.clone(), $unit, SharedString::from(desc));
        $recorder.register_gauge(&metrics_key)
    }};
}

static PROMETHEUS_HANDLE: std::sync::LazyLock<Arc<RwLock<Option<PrometheusHandle>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(None)));

pub fn init_metrics_context() {
    static START: Once = Once::new();
    START.call_once(init_metrics)
}

fn init_metrics() {
    let recorder = PrometheusBuilder::new()
        .set_quantiles(DEFAULT_QUANTILES)
        .expect("can't set quantiles")
        .set_buckets(DEFAULT_BUCKETS)
        .expect("can't set buckets")
        .build_recorder();
    let mut prometheus_handle = PROMETHEUS_HANDLE.as_ref().write();
    *prometheus_handle = Some(recorder.handle());
    // unsafe {
    //     metrics::clear_recorder();
    // }
    match metrics::set_global_recorder(recorder) {
        Ok(_) => {
            init_process_metrics();
            info!("SqlProxy init prometheus metrics context successfully!");
        }
        Err(e) => {
            warn!(
                "SqlProxy init prometheus metrics context error.cause by {:?}",
                e.to_string()
            );
        }
    }
}

fn init_process_metrics() {
    let common_resource_metrics = vec![
        MetricsConsts::cpu_total(),
        MetricsConsts::virtual_mem_size(),
        MetricsConsts::cpu_core_num(),
        MetricsConsts::rss_mem_size(),
    ]; //init_all_metrics();
    let common_labels = common_labels();
    for metric in common_resource_metrics.iter() {
        let (name, desc, _, metric_type) = metric.get_metrics_pair();
        describe_and_register_metrics(*metric_type, name, desc, common_labels);
    }
}

pub fn try_handle() -> Option<PrometheusHandle> {
    PROMETHEUS_HANDLE.as_ref().read().clone()
}

pub struct MetricsTimer {
    start: coarsetime::Instant,
    histogram: Histogram,
    observed: bool,
}

impl From<Histogram> for MetricsTimer {
    fn from(histogram: Histogram) -> MetricsTimer {
        MetricsTimer::from_histogram(histogram)
    }
}

impl fmt::Debug for MetricsTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetricsTimer")
            .field("start", &self.start)
            .field("observed", &self.observed)
            .finish()
    }
}

impl Drop for MetricsTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_millis();
        if !self.observed {
            self.histogram.record(elapsed as f64);
        }
    }
}

impl MetricsTimer {
    pub fn from_histogram(histogram: Histogram) -> Self {
        Self {
            start: coarsetime::Instant::now(),
            histogram,
            observed: false,
        }
    }

    pub fn new(name: &'static str) -> Self {
        Self {
            start: coarsetime::Instant::now(),
            histogram: histogram!(name), //register_histogram!(name),
            observed: false,
        }
    }

    pub fn new_with_labels<L: IntoLabels>(name: &'static str, labels: L) -> Self {
        Self {
            start: coarsetime::Instant::now(),
            histogram: histogram!(name, labels), //register_histogram!(name, labels),
            observed: false,
        }
    }

    pub fn elapsed(&self) -> u64 {
        self.start.elapsed().as_millis()
    }

    pub fn discard(mut self) {
        self.observed = true;
    }
}

#[inline]
pub fn gauge(name: &'static str, value: f64, labels: Option<&Vec<(&'static str, String)>>) {
    let gauge_obj = if let Some(label) = labels {
        metrics::gauge!(name, label)
    } else {
        metrics::gauge!(name)
    };
    gauge_obj.set(value);
}

#[inline]
pub fn common_labels() -> &'static Vec<(&'static str, String)> {
    static COMMON_LABELS: std::sync::OnceLock<Vec<(&'static str, String)>> =
        std::sync::OnceLock::new();
    COMMON_LABELS.get_or_init(|| {
        let hostname = hostname();
        vec![("node_name", hostname)]
    })
}

#[inline]
pub fn gauge_inc(name: &'static str, value: f64, labels: Option<&Vec<(&'static str, String)>>) {
    let gauge = if let Some(label) = labels {
        metrics::gauge!(name, label)
    } else {
        metrics::gauge!(name)
    };
    gauge.increment(value);
}

#[inline]
pub fn gauge_dec(name: &'static str, value: f64, labels: Option<&Vec<(&'static str, String)>>) {
    let gauge = if let Some(label) = labels {
        metrics::gauge!(name, label)
    } else {
        metrics::gauge!(name)
    };
    gauge.decrement(value)
}

pub fn describe_and_register_metrics(
    metric_type: MetricType,
    name: &'static str,
    desc: &'static str,
    _labels: &[(&'static str, String)],
) {
    match metric_type {
        MetricType::Gauge => {
            describe_gauge!(name, desc);
            //gauge!(name, labels);
        }
        MetricType::Counter => {
            describe_counter!(name, desc);
            // counter!(name, labels);
        }
        MetricType::Histogram => {
            describe_histogram!(name, desc);
            // histogram!(name, labels);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::metrics::{common_labels, gauge};

    #[test]
    pub fn test_fast_instant() {
        for _idx in 0..10 {
            gauge("test_fast_instant", 1.0, None);
            gauge("test_fast_instant", 1.0, Some(common_labels()));
        }
    }
}
