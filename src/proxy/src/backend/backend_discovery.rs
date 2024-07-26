use crate::backend::prost::common_proto::response::Payload;
use crate::backend::prost::common_proto::{ClusterName, DBLocation, SubscribeId, TenantKey};
use crate::backend::prost::topology;
use crate::backend::BackendInstance;

use anyhow::anyhow;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::ops::Sub;
use std::sync::{Arc, OnceLock};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::Request;
use tracing::{info, warn};

const DEFAULT_SUBSCRIBE_CHAN_SIZE: usize = 256;
const DEFAULT_MAX_RETRY: time::Duration = time::Duration::from_secs(60 * 3);
const SQL_PORT: &str = "sql-port";

#[derive(Error, Debug)]
pub enum BackendDiscoveryError {
    #[error("Failed to connect to topology service. Address {0} cause by {1}")]
    ConnectTopologyServiceError(String, anyhow::Error),
    #[error(
        "Failed to send subscribe request to topology service. \
        service addr is {0}, Request {1} cause by {2}"
    )]
    SendSubscribeRequestError(String, String, anyhow::Error),
    #[error("Retry failed timeout {0} topology service is {1}")]
    RetryTimeout(i64, String),
    #[error("Failed to subscribe topology service. service addr is {0}, cause by{1}")]
    SubscribeError(String, String),
}

static DB_INSTANCE_ONCE: OnceLock<Arc<BackendDiscovery>> = OnceLock::new();

pub fn get_backend_discovery(node_id: String) -> Arc<BackendDiscovery> {
    Arc::clone(DB_INSTANCE_ONCE.get_or_init(|| Arc::new(BackendDiscovery::new(node_id))))
}

type ClusterInstanceList = Arc<RwLock<VecDeque<BackendInstance>>>;

pub struct BackendDiscovery {
    node_id: String,
    db_location_tx: mpsc::UnboundedSender<DBLocation>,
    db_location_rx: Arc<Mutex<mpsc::UnboundedReceiver<DBLocation>>>,
    backend_instance_list: DashMap<TenantKey, ClusterInstanceList>,
}

impl BackendDiscovery {
    pub fn new(node_id: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            node_id,
            db_location_tx: tx,
            db_location_rx: Arc::new(Mutex::new(rx)),
            backend_instance_list: DashMap::new(),
        }
    }

    pub async fn discover(
        &self,
        topology_srv_addr: &str,
        max_retry: Option<time::Duration>,
    ) -> anyhow::Result<()> {
        let default_retry = max_retry.unwrap_or(DEFAULT_MAX_RETRY);
        let mut retry = default_retry;
        let delay = time::Duration::from_secs(1);
        while let Err(e) = self.send_subscribe(topology_srv_addr).await {
            if retry.as_secs() == 0 {
                let retry_timeout_err = BackendDiscoveryError::RetryTimeout(
                    default_retry.as_secs() as i64,
                    topology_srv_addr.to_string(),
                );
                warn!(
                    "Retry failed timeout {} topology service is {}",
                    retry.as_secs(),
                    topology_srv_addr
                );
                return Err(retry_timeout_err.into());
            }
            if let Some(BackendDiscoveryError::ConnectTopologyServiceError(_addr, _e)) =
                e.downcast_ref::<BackendDiscoveryError>()
            {
                retry = retry.sub(delay);
                tokio::time::sleep(delay).await;
            }
        }
        Ok(())
    }

    pub fn all_cluster_list(&self) -> &DashMap<TenantKey, ClusterInstanceList> {
        &self.backend_instance_list
    }

    pub fn subscribe_for(&self, db_location: DBLocation) -> anyhow::Result<()> {
        Ok(self.db_location_tx.send(db_location)?)
    }

    async fn send_subscribe(&self, srv_addr: &str) -> anyhow::Result<()> {
        let subscribe_client_rs =
            topology::topology_client::TopologyClient::connect(format!("http://{}", srv_addr))
                .await;
        match subscribe_client_rs {
            Ok(mut client) => {
                let (sub_tx, sub_rx) = mpsc::channel(DEFAULT_SUBSCRIBE_CHAN_SIZE);
                let stream_ack = ReceiverStream::new(sub_rx);
                let response = client
                    .subscribe_namespace(Request::new(stream_ack))
                    .await
                    .unwrap();
                let mut response_streaming = response.into_inner();
                let mut mut_sub_rx = self.db_location_rx.lock().await;
                loop {
                    tokio::select! {
                        Some(location) = mut_sub_rx.recv() => {
                            let sub_ns_request = topology::SubscribeNamespaceRequest {
                                db_location: Some(location),
                                subscribe_id: Some(SubscribeId {
                                    id: self.node_id.clone(),
                                })
                            };
                            if let Err(send_err) = sub_tx.send(sub_ns_request.clone()).await {
                                warn!("Failed to send subscribe request to topology service {}. cause by {}", srv_addr, send_err);
                                let send_subscribe_reqeust_err  = BackendDiscoveryError::SendSubscribeRequestError(
                                    srv_addr.to_string(),
                                    format!("{:?}", sub_ns_request.subscribe_id),
                                    anyhow!(send_err),
                                );
                                return Err(send_subscribe_reqeust_err.into());
                            }
                        }
                        Some(response_rs) = response_streaming.next() => {
                            if let Ok(response) = response_rs {
                                if response.status != 200 {
                                    warn!("Failed to subscribe topology service {}. cause by {:?}", srv_addr, response);
                                    let subscribe_rsp_err = BackendDiscoveryError::SubscribeError(
                                        srv_addr.to_string(),
                                        format!("Response Code{:?}, Msg {:?}", response.status, response.message));
                                    return Err(subscribe_rsp_err.into());
                                }
                                if let Some(payload) = response.payload {
                                   match payload {
                                      Payload::ChangeEvent(event) => {
                                           let db_service = event.service.unwrap();
                                           info!("DbInstanceDiscovery recv={:?}", db_service);
                                           let endpoint_opt = db_service
                                              .endpoints
                                              .iter()
                                              .find(|e| e.port_name.eq(SQL_PORT));
                                           assert!(endpoint_opt.is_some());
                                           let endpoint = endpoint_opt.unwrap();
                                           let addr = format!("{}:{}", endpoint.address, endpoint.port);
                                           let location = db_service.location.clone().unwrap();
                                           let namespace = location.namespace.clone();
                                           let cluster_name = &db_service.cluster;
                                           let db_service_status = db_service.status();
                                           let tenant_key = TenantKey {
                                              region: location.region.clone(),
                                              available_zone: location.available_zone.clone(),
                                              namespace: namespace.clone(),
                                              cluster_name:cluster_name.clone()
                                            };
                                           let backend_instance = BackendInstance::new(
                                                location.clone(),addr,db_service_status, ClusterName {namespace ,cluster_name: cluster_name.to_string()}
                                           );
                                           // let tenant_ken_encode = encode_tenant_key(&tenant_key);
                                           if self.backend_instance_list.contains_key(&tenant_key) {
                                              let cluster_db_instance_list_ref = self.backend_instance_list.get_mut(&tenant_key).unwrap();
                                              let mut cluster_db_instance_list = cluster_db_instance_list_ref.write().await;
                                              cluster_db_instance_list.push_back(backend_instance);
                                           } else {
                                              let mut cluster_db_instance_list = VecDeque::new();
                                              cluster_db_instance_list.push_back(backend_instance);
                                              self.backend_instance_list.insert(tenant_key, Arc::new(RwLock::new(cluster_db_instance_list)));
                                           }
                                      }
                                      _ => {
                                        unreachable!()
                                      }
                                   }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to connect to topology service {}. cause by {:?}",
                    srv_addr, e
                );
                let connect_failed = BackendDiscoveryError::ConnectTopologyServiceError(
                    srv_addr.to_string(),
                    e.into(),
                );
                Err(connect_failed.into())
            }
        }
    }
}
