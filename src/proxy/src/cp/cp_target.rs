use crate::cp::active_users::UserActivityWindow;
use crate::prost::control_plane;
use crate::prost::control_plane::control_plane_response::PacketData;
use crate::prost::control_plane::control_plane_service_server::*;
use crate::prost::control_plane::{ActiveUsers, ControlPlaneResponse, PacketHeader};
use itertools::Itertools;
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, warn};

const DEFAULT_CHUNK_SIZE: usize = 15;

pub struct ControlPlaneServiceImpl {
    active_users: Arc<UserActivityWindow>,
}

impl ControlPlaneServiceImpl {
    pub fn new(active_users: Arc<UserActivityWindow>) -> Self {
        Self { active_users }
    }
}

#[async_trait::async_trait]
impl ControlPlaneService for ControlPlaneServiceImpl {
    type ActiveUsersStream =
        Pin<Box<dyn Stream<Item = Result<ControlPlaneResponse, Status>> + Send>>;

    async fn active_users(
        &self,
        request: Request<Streaming<()>>,
    ) -> Result<Response<Self::ActiveUsersStream>, Status> {
        let mut stream_request = request.into_inner();
        let active_user_arcs = Arc::clone(&self.active_users);
        let (tx, rx) = tokio::sync::mpsc::channel(DEFAULT_CHUNK_SIZE);
        tokio::task::spawn(async move {
            while (stream_request.next().await).is_some() {
                let active_users = active_user_arcs.freeze();
                if active_users.is_empty() {
                    debug!("No active users found.");
                    let none_active_users = ControlPlaneResponse {
                        header: Some(PacketHeader {
                            packet_type: control_plane::PacketType::ActiveUser as i32,
                            package_count: 0,
                            size_pre_package: 0,
                            size: 0,
                        }),
                        packet_data: None,
                    };
                    if let Err(e) = tx.send(Ok(none_active_users)).await {
                        warn!("Failed to send active user response: {:?}", e);
                        break;
                    } else {
                        continue;
                    }
                }
                let total_size = active_users.len();
                let grouped_users = active_users
                    .chunks(DEFAULT_CHUNK_SIZE)
                    .map(|chunk| chunk.to_vec())
                    .collect_vec();

                let mut header = PacketHeader {
                    packet_type: control_plane::PacketType::ActiveUser as i32,
                    package_count: total_size as u32,
                    size_pre_package: DEFAULT_CHUNK_SIZE as u32,
                    size: 0,
                };
                for batch in grouped_users {
                    header.size = batch.len() as u32;
                    let response = ControlPlaneResponse {
                        header: Some(header),
                        packet_data: Some(PacketData::ActiveUser(ActiveUsers {
                            active_user_com: batch,
                        })),
                    };
                    if let Err(send_err) = tx.send(Ok(response)).await {
                        warn!("Failed to send active user response: {:?}", send_err);
                        break;
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}
