use crate::backend::BackendInstance;
use crate::prost::common_proto::response::Payload;
use crate::prost::common_proto::{ClusterName, DBLocation, SubscribeId, TenantKey};
use crate::prost::topology;

use anyhow::anyhow;
use common::ShutdownMessage;
use dashmap::DashMap;
use futures_async_stream::stream;
use std::collections::VecDeque;
use std::ops::Sub;
use std::sync::{Arc, OnceLock};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tokio::{sync, time};
use tonic::transport::Channel;
use tonic::Request;
use tracing::{info, warn};

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
    tenant_key_tx: sync::mpsc::UnboundedSender<TenantKey>,
    tenant_key_rx: Arc<Mutex<sync::mpsc::UnboundedReceiver<TenantKey>>>,
    backend_instance_list: DashMap<TenantKey, ClusterInstanceList>,
    db_instance_tx: sync::watch::Sender<BackendInstance>,
    db_instance_rx: sync::watch::Receiver<BackendInstance>,
}

impl BackendDiscovery {
    pub fn new(node_id: String) -> Self {
        let (tx, rx) = sync::mpsc::unbounded_channel();
        let (db_instance_tx, db_instance_rx) = sync::watch::channel(BackendInstance::default());

        Self {
            node_id,
            tenant_key_tx: tx,
            tenant_key_rx: Arc::new(Mutex::new(rx)),
            backend_instance_list: DashMap::new(),
            db_instance_tx,
            db_instance_rx,
        }
    }

    pub async fn discover(
        &self,
        topology_srv_addr: &str,
        max_retry: Option<time::Duration>,
        shutdown_rx: Box<sync::watch::Receiver<ShutdownMessage>>,
    ) -> anyhow::Result<()> {
        let default_retry = max_retry.unwrap_or(DEFAULT_MAX_RETRY);
        let mut retry = default_retry;
        let delay = time::Duration::from_secs(1);
        while let Err(e) = self
            .send_subscribe(topology_srv_addr, shutdown_rx.clone())
            .await
        {
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
            } else {
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn all_cluster_list(&self) -> &DashMap<TenantKey, ClusterInstanceList> {
        &self.backend_instance_list
    }

    pub fn subscribe_for(&self, tenant_key: TenantKey) -> anyhow::Result<()> {
        let rs = self.tenant_key_tx.send(tenant_key.clone());
        if rs.is_err() {
            return Err(anyhow!("Failed to send tenant_key to backend discovery"));
        }
        Ok(())
    }

    pub async fn db_instance_change_notify(&self) -> sync::watch::Receiver<BackendInstance> {
        self.db_instance_rx.clone()
    }

    #[stream(item = topology::SubscribeNamespaceRequest)]
    async fn send_stream(
        tenant_key_rx: Arc<Mutex<sync::mpsc::UnboundedReceiver<TenantKey>>>,
        my_node_id: String,
    ) {
        loop {
            let tenant_key_opt = {
                let mut rx = tenant_key_rx.lock().await;
                rx.recv().await
            };
            if let Some(tenant_key) = tenant_key_opt {
                let location = DBLocation {
                    region: tenant_key.region.clone(),
                    available_zone: tenant_key.available_zone.clone(),
                    namespace: tenant_key.namespace.clone(),
                    node_name: "".to_string(),
                };
                let sub_ns_request = topology::SubscribeNamespaceRequest {
                    db_location: Some(location),
                    subscribe_id: Some(SubscribeId {
                        id: my_node_id.clone(),
                    }),
                    force: true,
                    label: std::collections::HashMap::default(),
                };
                yield sub_ns_request;
            } else {
                break;
            }
        }
    }

    async fn send_subscribe(
        &self,
        srv_addr: &str,
        mut shutdown_rx: Box<sync::watch::Receiver<ShutdownMessage>>,
    ) -> anyhow::Result<()> {
        let cp_addr = format!("http://{}", srv_addr);
        let channel_rs = Channel::from_shared(cp_addr).unwrap().connect().await;
        let channel = match channel_rs {
            Ok(channel) => channel,
            Err(e) => {
                warn!(
                    "Failed to connect to topology service {}. cause by {:?}",
                    srv_addr, e
                );
                let connect_failed = BackendDiscoveryError::ConnectTopologyServiceError(
                    srv_addr.to_string(),
                    e.into(),
                );
                return Err(connect_failed.into());
            }
        };
        let mut client = topology::topology_client::TopologyClient::new(channel);
        let tenant_key_rx = Arc::clone(&self.tenant_key_rx);
        let my_node_id = self.node_id.clone();

        let request_stream = BackendDiscovery::send_stream(tenant_key_rx, my_node_id);
        let response_rs = client
            .subscribe_namespace(Request::new(request_stream))
            .await;

        let db_instance_tx_arc = Arc::new(&self.db_instance_tx);
        match response_rs {
            Ok(response) => {
                let mut streaming = response.into_inner();
                loop {
                    tokio::select! {
                        Ok(Some(resp))  = streaming.message() => {
                            match resp.payload {
                                Some(Payload::ChangeEvent(event)) => {
                                   let db_service = event.service.unwrap();
                                   let endpoint_opt = db_service.endpoints.
                                        iter().find(|e| e.port_name.eq(SQL_PORT));
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
                                      location.clone(), addr.clone(),
                                      db_service_status, ClusterName {namespace ,cluster_name: cluster_name.to_string()}
                                   );
                                   if self.backend_instance_list.contains_key(&tenant_key) {
                                      let cluster_db_instance_list_ref = self.backend_instance_list.get_mut(&tenant_key).unwrap();
                                      let mut cluster_db_instance_list = cluster_db_instance_list_ref.write().await;
                                      cluster_db_instance_list.retain(|e| e.addr != addr);
                                      cluster_db_instance_list.push_back(backend_instance.clone());
                                   } else {
                                      let mut cluster_db_instance_list = VecDeque::new();
                                      cluster_db_instance_list.push_back(backend_instance.clone());
                                      self.backend_instance_list.insert(tenant_key, Arc::new(RwLock::new(cluster_db_instance_list)));
                                   }
                                   db_instance_tx_arc.send(backend_instance).unwrap();
                                }
                                _=> unreachable!()
                            }
                        },
                        shut_rs = shutdown_rx.changed() => {
                            match shut_rs {
                                Ok(_) => info!("BackendDiscovery shutdown."),
                                Err(e) => warn!("BackendDiscovery shutdown sender dropped! {:?}", e),
                            }
                            return Ok(());
                        }
                    }
                }
            }
            Err(e) => {
                let send_subscribe_request_err = BackendDiscoveryError::SendSubscribeRequestError(
                    srv_addr.to_string(),
                    format!("{:?}", self.node_id.clone()),
                    anyhow!(e),
                );
                Err(send_subscribe_request_err.into())
            }
        }
    }
}
