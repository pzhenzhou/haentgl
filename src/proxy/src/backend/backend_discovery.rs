use crate::backend::control_plane_resolver::{CpChannel, CpResolver};
use crate::backend::BackendInstance;
use crate::prost::common_proto::response::Payload;
use crate::prost::common_proto::{ClusterName, DBLocation, Response, SubscribeId, TenantKey};
use crate::prost::topology::topology_client::TopologyClient;
use crate::prost::topology::SubscribeNamespaceRequest;

use anyhow::anyhow;
use common::ShutdownMessage;
use dashmap::DashMap;
use futures_async_stream::stream;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use thiserror::Error;
use tokio::sync;
use tokio::sync::{Mutex, RwLock};
use tonic::{Request, Streaming};
use tracing::{info, warn};

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

pub fn get_backend_discovery(node_id: String, namespace: String) -> Arc<BackendDiscovery> {
    Arc::clone(DB_INSTANCE_ONCE.get_or_init(|| Arc::new(BackendDiscovery::new(node_id, namespace))))
}

type SharedBackendList = Arc<RwLock<VecDeque<BackendInstance>>>;

pub struct BackendDiscovery {
    node_id: String,
    namespace: String,
    tenant_key_tx: sync::mpsc::UnboundedSender<TenantKey>,
    tenant_key_rx: Arc<Mutex<sync::mpsc::UnboundedReceiver<TenantKey>>>,
    tenants: DashMap<TenantKey, SharedBackendList>,
    db_instance_tx: sync::watch::Sender<BackendInstance>,
    db_instance_rx: sync::watch::Receiver<BackendInstance>,
}

impl BackendDiscovery {
    pub fn new(node_id: String, namespace: String) -> Self {
        let (tx, rx) = sync::mpsc::unbounded_channel();
        let (db_instance_tx, db_instance_rx) = sync::watch::channel(BackendInstance::default());

        Self {
            node_id,
            namespace,
            tenant_key_tx: tx,
            tenant_key_rx: Arc::new(Mutex::new(rx)),
            tenants: DashMap::new(),
            db_instance_tx,
            db_instance_rx,
        }
    }

    pub async fn discover_with_retry(
        &self,
        cp_srv_resolver: Arc<CpResolver>,
        shutdown_rx: Box<sync::watch::Receiver<ShutdownMessage>>,
    ) -> anyhow::Result<()> {
        cp_srv_resolver.wait_for_backends_ready().await;
        let mut cp_channel = cp_srv_resolver.get_cp_backend().await;
        info!(
            "BackendDiscovery start to subscribe.The cp backend {:?} {:?}",
            cp_channel.backend_name, cp_channel.service
        );
        loop {
            let subscribe_rs = self.send_subscribe(&cp_channel, shutdown_rx.clone()).await;
            match subscribe_rs {
                Ok(_) => return Ok(()),
                Err(_e) => {
                    cp_srv_resolver
                        .mark_backend_unavailable(&cp_channel.backend_name)
                        .await;
                    cp_channel = cp_srv_resolver.get_cp_backend().await;
                    info!(
                        "BackendDiscovery retry to subscribe. The new cp backend {:?}",
                        cp_channel.backend_name,
                    );
                    self.tenants.iter().for_each(|entry| {
                        let tenant_key = entry.key();
                        self.subscribe_for(tenant_key.clone()).unwrap();
                    });
                    // send subscribe request with new cp_channel
                }
            }
        }
    }

    pub fn all_cluster_list(&self) -> &DashMap<TenantKey, SharedBackendList> {
        &self.tenants
    }

    pub fn unsubscribed_tenants(&self, tenant_key: &TenantKey) {
        self.tenants
            .insert(tenant_key.clone(), Arc::new(RwLock::new(VecDeque::new())));
    }

    pub fn subscribe_for(&self, tenant_key: TenantKey) -> anyhow::Result<()> {
        let rs = self.tenant_key_tx.send(tenant_key.clone());
        if rs.is_err() {
            return Err(anyhow!("Failed to send tenant_key to backend discovery"));
        }
        info!("BackendDiscovery subscribe for success.");
        Ok(())
    }

    pub async fn backend_instance_change_notify(&self) -> sync::watch::Receiver<BackendInstance> {
        self.db_instance_rx.clone()
    }

    #[stream(item = SubscribeNamespaceRequest)]
    async fn send_stream(
        tenant_key_rx: Arc<Mutex<sync::mpsc::UnboundedReceiver<TenantKey>>>,
        my_node_id: String,
        namespace: String,
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
                let sub_ns_request = SubscribeNamespaceRequest {
                    db_location: Some(location),
                    subscribe_id: Some(SubscribeId {
                        id: my_node_id.clone(),
                        namespace: namespace.clone(),
                        name: my_node_id.clone(),
                    }),
                    force: true,
                    label: std::collections::HashMap::default(),
                };
                info!("BackendDiscovery send_stream {:?}", tenant_key);
                yield sub_ns_request;
            } else {
                break;
            }
        }
    }

    async fn recv_stream(&self, streaming: &mut Streaming<Response>) -> anyhow::Result<()> {
        while let Some(resp) = streaming.message().await? {
            match resp.payload {
                Some(Payload::ChangeEvent(event)) => {
                    let db_service = event.service.unwrap();
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
                        region: "".to_string(),
                        available_zone: "".to_string(),
                        // region: location.region.clone(),
                        // available_zone: location.available_zone.clone(),
                        namespace: namespace.clone(),
                        cluster_name: cluster_name.clone(),
                    };
                    let backend_instance = BackendInstance::new(
                        location.clone(),
                        addr.clone(),
                        db_service_status,
                        ClusterName {
                            namespace,
                            cluster_name: cluster_name.to_string(),
                        },
                    );
                    let cluster_db_instance_list_ref = self.tenants.get(&tenant_key).unwrap();
                    let mut cluster_db_instance_list = cluster_db_instance_list_ref.write().await;
                    cluster_db_instance_list.retain(|e| e.addr != addr);
                    cluster_db_instance_list.push_back(backend_instance.clone());
                    self.db_instance_tx.send(backend_instance)?;
                }
                None => {
                    warn!("BackendDiscovery receive empty payload.");
                    return Err(anyhow!("BackendDiscovery receive empty payload."));
                }
                _ => {
                    unreachable!()
                }
            }
        }
        Ok(())
    }

    async fn send_subscribe(
        &self,
        cp_channel: &CpChannel,
        mut shutdown_rx: Box<sync::watch::Receiver<ShutdownMessage>>,
    ) -> anyhow::Result<()> {
        let srv_addr = cp_channel.service.clone();
        let mut client = TopologyClient::new(cp_channel.channel.clone());
        let tenant_key_rx = Arc::clone(&self.tenant_key_rx);
        let my_node_id = self.node_id.clone();
        let namespace = self.namespace.clone();
        let request_stream = BackendDiscovery::send_stream(tenant_key_rx, my_node_id, namespace);
        let response_rs = client
            .subscribe_namespace(Request::new(request_stream))
            .await;
        info!(
            "BackendDiscovery subscribe_namespace successfully. cp_srv_addr={:?}",
            srv_addr
        );
        // let db_instance_tx_arc = Arc::new(&self.db_instance_tx);
        match response_rs {
            Ok(response) => {
                let mut streaming = response.into_inner();
                tokio::select! {
                    recv_rs = self.recv_stream(&mut streaming) => {
                        match recv_rs {
                            Err(e)=> {
                                warn!("Failed to receive topology service. StatusCode={:?}", e);
                                Err(e)
                            }
                            Ok(_) => {
                                info!("BackendDiscovery receive topology service.");
                                Ok(())
                            }
                        }
                    },
                    shut_rs = shutdown_rx.changed() => {
                        match shut_rs {
                            Ok(_) => info!("BackendDiscovery shutdown."),
                            Err(e) => warn!("BackendDiscovery shutdown sender dropped! {:?}", e),
                        }
                        Ok(())
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to subscribe topology service. StatusCode={:?}",
                    e.code()
                );
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
