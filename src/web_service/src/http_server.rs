use crate::metrics_handler::*;
use crate::proxy_handler::*;

use anyhow::anyhow;
use axum::routing::{get, post};
use axum::Router;
use common::profiling::head_profiler::{HeapProfileOpts, HeapProfiler};
use common::profiling::prof::Prof;
use proxy::backend::backend_discovery::{get_backend_discovery, BackendDiscovery};
use proxy::backend::backend_mgr::BackendMgr;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};

#[derive(Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub code: u16,
    pub message: String,
    pub data: T,
}

pub struct HaentglProxyRest;

#[derive(Clone)]
pub struct HaentglProxyRestState {
    backend_mgr: Arc<BackendMgr>,
}

impl HaentglProxyRestState {
    pub fn new(backend_mgr: Arc<BackendMgr>) -> Self {
        Self { backend_mgr }
    }
    pub fn backend_mgr_ref(&self) -> Arc<BackendMgr> {
        Arc::clone(&self.backend_mgr)
    }

    pub fn backend_discovery_ref(&self) -> Arc<BackendDiscovery> {
        let my_node = if let Ok(node_id) = std::env::var("NODE_ID") {
            node_id
        } else {
            "local_node".to_string()
        };
        get_backend_discovery(my_node)
    }

    pub fn cpu_profile(&self) -> &'static Prof {
        static CPU_PROF: std::sync::OnceLock<Prof> = std::sync::OnceLock::new();

        CPU_PROF.get_or_init(Prof::default)
    }

    pub fn memory_profile(&self) -> Result<&HeapProfiler, anyhow::Error> {
        static HEAP_PROF: std::sync::OnceLock<HeapProfiler> = std::sync::OnceLock::new();

        HEAP_PROF.get_or_try_init(|| HeapProfiler::new_with_opts(HeapProfileOpts::default()))
    }
}

impl HaentglProxyRest {
    pub async fn start_server<F>(
        addr: String,
        port: u16,
        enable_metric: bool,
        app_state: HaentglProxyRestState,
        shutdown: F,
    ) -> anyhow::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut app = Router::new()
            .route("/", get("Hi I'm Haentgl Proxy WebService"))
            .route("/mem_dump", get(dump_mem_profile))
            .route("/mem_prof_analysis/:dump_path", get(heap_analysis))
            .route("/start_cpu_prof", get(start_cpu_prof))
            .route("/stop_cpu_prof", get(stop_cpu_prof))
            .route("/list_cpu_profile", get(list_cpu_profile))
            .route("/print_cpu_prof", get(print_cpu_prof))
            .route("/tenant", post(add_tenant))
            .route(
                "/tenant/:region/:az/:namespace/:cluster/status",
                get(tenant_status),
            )
            .with_state(app_state);

        if enable_metric {
            app = app.nest("", route_metrics(MetricsHandler {}));
        }

        app = app.layer(TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::new()));
        // .layer(TimeoutLayer::new(Duration::from_secs(10)));
        let listener = tokio::net::TcpListener::bind(format!("{addr}:{port}"))
            .await
            .unwrap();

        match axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(shutdown)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                println!("Failed to start Haentgl Proxy WebService {e:?}");
                Err(anyhow!(e.to_string()))
            }
        }
    }
}
