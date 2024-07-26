use crate::http_handler::*;

use anyhow::anyhow;
use axum::routing::get;
use axum::Router;
// use once_cell::sync::OnceCell;
use std::future::Future;
// use std::time::Duration;
// use tower_http::timeout::TimeoutLayer;
use common::profiling::head_profiler::{HeapProfileOpts, HeapProfiler};
use common::profiling::prof::Prof;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};

pub struct MonoProxyRest;

#[derive(Clone)]
pub struct MonoProxyRestState;

impl MonoProxyRestState {
    pub fn cpu_profile(&self) -> &'static Prof {
        static CPU_PROF: std::sync::OnceLock<Prof> = std::sync::OnceLock::new();

        CPU_PROF.get_or_init(Prof::default)
    }

    pub fn memory_profile(&self) -> Result<&HeapProfiler, anyhow::Error> {
        static HEAP_PROF: std::sync::OnceLock<HeapProfiler> = std::sync::OnceLock::new();

        HEAP_PROF.get_or_try_init(|| HeapProfiler::new_with_opts(HeapProfileOpts::default()))
    }
}

impl MonoProxyRest {
    pub async fn start_server<F>(
        addr: String,
        port: u16,
        enable_metric: bool,
        shutdown: F,
    ) -> anyhow::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let app_state = MonoProxyRestState {};
        let mut app = Router::new()
            .route("/", get("Hi I'm MonoProxyREST"))
            .route("/mem_dump", get(dump_mem_profile))
            .route("/mem_prof_analysis/:dump_path", get(heap_analysis))
            .route("/start_cpu_prof", get(start_cpu_prof))
            .route("/stop_cpu_prof", get(stop_cpu_prof))
            .route("/list_cpu_profile", get(list_cpu_profile))
            .route("/print_cpu_prof", get(print_cpu_prof))
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
                println!("Failed to start MonoProxyRest {e:?}");
                Err(anyhow!(e.to_string()))
            }
        }
    }
}
