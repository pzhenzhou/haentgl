pub const PROCESS_MEM_RSS_SIZE: &str = "proxy_process_mem_rss_bytes";
pub const PROCESS_VIRTUAL_MEM_SIZE: &str = "proxy_process_mem_virtual_bytes";
pub const CPU_CORE_NUM: &str = "proxy_process_cpu_core_num";
pub const CPU_TOTAL: &str = "proxy_process_cpu_seconds_total";
pub const PROXY_MAX_CONN: &str = "proxy_max_connections";
pub const PROXY_CURR_CONN: &str = "proxy_curr_connections";
pub const PROXY_COM_LATENCY: &str = "proxy_com_latency";

#[macro_export]
macro_rules! metrics_const {
    ($({$metric_name:ident, $init_fn:ident, $metric_type:expr, $name:expr, $desc:expr}),*) => {
        use metrics::KeyName;
        use std::sync::OnceLock;
        use $crate::metrics::MetricType;

        #[derive(Debug, Clone)]
        pub enum MetricsConsts {
           $($metric_name(&'static str, &'static str, KeyName, MetricType),)*
        }

        pub fn list_all_metrics() -> &'static Vec<MetricsConsts> {
           static ALL_METRICS: OnceLock<Vec<MetricsConsts>> = OnceLock::new();
           ALL_METRICS.get_or_init(|| {
              vec![$(MetricsConsts::$metric_name($name, $desc, KeyName::from_const_str($name), $metric_type),)*]
           })
        }

        impl MetricsConsts {
            $(
            #[inline]
            pub fn $init_fn() -> Self {
                MetricsConsts::$metric_name($name, $desc, KeyName::from_const_str($name), $metric_type)
            })*

            pub fn get_name(&self) -> String {
                let (name, _,_,_) = self.get_metrics_pair();
                name.to_string()
            }

            pub fn get_metrics_pair(&self) -> (&'static str, &'static str, &KeyName, &MetricType){
                match self {
                    $(
                    MetricsConsts::$metric_name(name, desc, key_name, metrics_type) => (name, desc, key_name, metrics_type),
                    )*
                }
            }
        }
    };
}

metrics_const!(
    { ProcessRssMemSize, rss_mem_size, MetricType::Gauge, PROCESS_MEM_RSS_SIZE, "Process resident memory size in bytes"},
    { ProcessVirtralMemSize, virtual_mem_size,MetricType::Gauge, PROCESS_VIRTUAL_MEM_SIZE, "Process virtual memory size in bytes"},
    { CpuCoreNum, cpu_core_num, MetricType::Gauge, CPU_CORE_NUM, "cpu core num."},
    { CpuTotal, cpu_total, MetricType::Gauge, CPU_TOTAL, "total user and system cpu time spend in seconds."},
    { ProxyMaxConnections, max_connections, MetricType::Gauge, PROXY_MAX_CONN, "The max number of connections allowed by the Proxy."},
    { ProxyCurrentConnections, current_connections, MetricType::Gauge, PROXY_CURR_CONN, "The current connection count by the Proxy."},
    { ProxyComLatency, com_latncy, MetricType::Histogram, PROXY_COM_LATENCY, "Latency of command execution."}
);
