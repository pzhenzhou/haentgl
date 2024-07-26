use crate::http_server::{ApiResponse, HaentglProxyRestState};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use proxy::prost::common_proto::TenantKey;

pub async fn add_tenant(
    State(state): State<HaentglProxyRestState>,
    Json(payload): Json<TenantKey>,
) -> impl IntoResponse {
    let be_discovery = state.backend_discovery_ref();
    let mut resp = ApiResponse {
        code: u16::from(StatusCode::CREATED),
        message: "success".to_string(),
        data: "",
    };
    if let Err(e) = be_discovery.subscribe_for(payload) {
        resp.code = u16::from(StatusCode::INTERNAL_SERVER_ERROR);
        resp.message = format!("failed to add tenant: {:?}", e);
    }
    Json(resp)
}

pub async fn tenant_status(
    State(state): State<HaentglProxyRestState>,
    Path((region, az, namespace, cluster)): Path<(String, String, String, String)>,
) -> impl IntoResponse {
    let tenant_key = TenantKey {
        region,
        available_zone: az,
        namespace,
        cluster_name: cluster,
    };
    let service_status = state.backend_mgr_ref().tenant_status(tenant_key);
    let resp = ApiResponse {
        code: u16::from(StatusCode::OK),
        message: "success".to_string(),
        data: service_status.as_str_name(),
    };
    Json(resp)
}
