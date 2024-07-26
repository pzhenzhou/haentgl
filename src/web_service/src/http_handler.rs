use crate::http_server::MonoProxyRestState;
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{http, Json, Router};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use tracing::debug;
use walkdir::WalkDir;

#[derive(Clone, Copy)]
pub struct MetricsHandler;

impl MetricsHandler {
    pub fn render(&self) -> String {
        if let Some(prometheus_handle) = common::metrics::try_handle() {
            prometheus_handle.render()
        } else {
            "Please initialize the prometheus context first.".to_string()
        }
    }
}

pub fn route_metrics<S>(metrics_handler: MetricsHandler) -> Router<S> {
    Router::new()
        .route("/metrics", get(metrics_get))
        .with_state(metrics_handler)
}

pub async fn list_cpu_profile(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    if let Some(profile_path) = params.get("profile_path") {
        let mut files = vec![];
        for file in WalkDir::new(profile_path)
            .into_iter()
            .filter_map(|file| file.ok())
        {
            if file.metadata().unwrap().is_dir() {
                continue;
            }
            if let Some(file_name) = file.path().file_name() {
                let file_name_string = file_name.to_os_string().into_string().unwrap();
                debug!("MonoProxyRest list_cpu_profile = {file_name_string:?}");
                if file_name_string.starts_with("mp_cpu_") {
                    files.push(file_name_string);
                }
            }
        }
        (
            http::StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            Json(files).into_response(),
        )
    } else {
        (
            http::StatusCode::BAD_REQUEST,
            [(CONTENT_TYPE, "application/json")],
            "Please set query string. For example list_cpu_profile?profile_path=XXX "
                .into_response(),
        )
    }
}

pub async fn stop_cpu_prof(
    State(state): State<MonoProxyRestState>,
) -> Json<HashMap<String, String>> {
    let cpu_prof = state.cpu_profile();
    let stop_rs = match cpu_prof.stop() {
        Ok(r) => r.to_string(),
        Err(e) => e.to_string(),
    };
    Json(HashMap::from([("cpu_prof_stop".to_string(), stop_rs)]))
}

pub async fn start_cpu_prof(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<MonoProxyRestState>,
) -> Json<HashMap<String, String>> {
    let duration = params
        .get("duration")
        .unwrap_or(&"60".to_string())
        .parse()
        .unwrap_or(60_u64);

    let default_path = "/tmp/mono_proxy_cpu_prof/".to_string();
    let profile_path = params.get("profile_path").unwrap_or(&default_path);

    let cpu_prof = state.cpu_profile();
    match cpu_prof.start(duration, profile_path.to_string()) {
        Ok(r) => Json(HashMap::from([(
            "cpu_prof_start".to_string(),
            r.to_string(),
        )])),
        Err(e) => Json(HashMap::from([(
            "cpu_prof_start".to_string(),
            e.to_string(),
        )])),
    }
}

pub async fn print_cpu_prof(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    if let Some(file_path_string) = params.get("profile_file") {
        // let file_path_string = format!("/tmp/{mono_cpu_fg}");
        let mono_cpu_fg_path = std::path::Path::new(file_path_string.as_str());
        if mono_cpu_fg_path.exists() {
            if let Ok(mut mono_cpu_fg_file) = File::open(mono_cpu_fg_path) {
                let meta = mono_cpu_fg_file.metadata().unwrap();
                let file_size = meta.len();
                let mut file_buf = Vec::with_capacity(file_size as usize);
                mono_cpu_fg_file
                    .read_to_end(&mut file_buf)
                    .expect("Failed to read profile size");
                (
                    http::StatusCode::OK,
                    [(CONTENT_TYPE, "image/svg+xml")],
                    file_buf.into_response(),
                )
            } else {
                (
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    [(CONTENT_TYPE, "application/json")],
                    format!("Failed to open {mono_cpu_fg_path:?} ").into_response(),
                )
            }
        } else {
            (
                http::StatusCode::BAD_REQUEST,
                [(CONTENT_TYPE, "application/json")],
                format!("{mono_cpu_fg_path:?} not exists.").into_response(),
            )
        }
    } else {
        (
            http::StatusCode::BAD_REQUEST,
            [(CONTENT_TYPE, "application/json")],
            "Please set query string. For example print_cpu_prof?profile_path=XXX ".into_response(),
        )
    }
}

pub async fn heap_analysis(Path(dump_path): Path<String>) {
    let _dump_heap_rs = common::profiling::head_profiler::heap_analysis(dump_path).await;
}

pub async fn dump_mem_profile(
    State(state): State<MonoProxyRestState>,
) -> Json<HashMap<String, String>> {
    let mem_profiler_rs = state.memory_profile();
    if let Ok(mem_prof) = mem_profiler_rs {
        match mem_prof.dump_profile() {
            Ok(dir) => Json(HashMap::from([("mem_heap_dump".to_string(), dir)])),
            Err(err) => Json(HashMap::from([(
                "mem_heap_dump_err".to_string(),
                err.to_string(),
            )])),
        }
    } else {
        Json(HashMap::from([(
            "mem_heap_dump_err".to_string(),
            mem_profiler_rs.err().unwrap().to_string(),
        )]))
    }
}

#[axum_macros::debug_handler]
async fn metrics_get(state: State<MetricsHandler>) -> String {
    state.render()
}
