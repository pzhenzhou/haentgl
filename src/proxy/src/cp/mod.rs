use crate::cp::active_users::UserActivityWindow;
use common::ShutdownMessage;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch::Receiver;
use tracing::{error, info};

pub mod active_users;
mod cp_target;
// pub mod prost;

pub async fn start_cp_target_reporter(
    active_users: Arc<UserActivityWindow>,
    cp_addr: impl Into<String>,
    mut shutdown_rx: Box<Receiver<ShutdownMessage>>,
) {
    let cp_addr = cp_addr.into();
    let cp_srv_impl = crate::cp::cp_target::ControlPlaneServiceImpl::new(active_users);
    let cp_addr_socket = cp_addr.parse().unwrap();
    tokio::task::spawn(async move {
        tonic::transport::Server::builder()
            .timeout(Duration::from_secs(5))
            .add_service(crate::prost::control_plane::control_plane_service_server::ControlPlaneServiceServer::new(cp_srv_impl))
            .serve_with_shutdown(cp_addr_socket, async move {
                tokio::select! {
                    res = shutdown_rx.changed() => {
                        match res {
                            Ok(_) => info!("ControlPlaneService shutdown."),
                            Err(_) => error!("ControlPlaneService shutdown sender dropped!"),
                        }
                    }
                }
            })
            .await
            .expect("Failed to build ControlPlaneService.");
    });
    info!("ControlPlaneService started at {:?}", cp_addr_socket);
}
