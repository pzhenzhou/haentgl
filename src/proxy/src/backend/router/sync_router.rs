use crate::backend::backend_discovery::BackendDiscovery;
use crate::backend::router::{
    BackendLoadBalancer, BackendLoadBalancerType, BackendRouter, RandomBalancer,
};
use crate::backend::{start_backend_discovery, BackendInstance};
use crate::prost::common_proto::TenantKey;
use crate::server::proxy_cli_args::ProxyServerArgs;

use async_trait::async_trait;
use common::ShutdownMessage;
use std::collections::VecDeque;
use std::future::Future;
use std::io::Error;
use std::sync::Arc;
use tokio::sync::watch::Receiver;
use tracing::warn;

pub struct SyncRouter {
    be_discovery: Arc<BackendDiscovery>,
    balancer: Box<dyn BackendLoadBalancer>,
}

impl SyncRouter {
    pub async fn new(proxy_cli: &ProxyServerArgs, shutdown_rx: &Receiver<ShutdownMessage>) -> Self {
        let node_id = proxy_cli.get_node_id();
        let namespace = proxy_cli.get_namespace();
        let topology_srv_addr = proxy_cli.cp_addr.clone().unwrap();
        Self {
            be_discovery: start_backend_discovery(
                node_id,
                namespace,
                topology_srv_addr,
                shutdown_rx,
            )
            .await,
            balancer: Box::new(RandomBalancer::new()),
        }
    }
}

#[async_trait]
impl BackendRouter for SyncRouter {
    async fn status_change_notify<F, Fut>(&self, f: F) -> Result<(), Error>
    where
        F: Fn(BackendInstance) -> Fut + Send,
        Fut: Future<Output = Result<(), Error>> + Send + Sync,
    {
        let rx_change = &mut self.be_discovery.backend_instance_change_notify().await;
        loop {
            if rx_change.changed().await.is_err() {
                return Err(Error::new(
                    std::io::ErrorKind::NotFound,
                    "The backend discovery service has been shutdown",
                ));
            }
            let db_instance = rx_change.borrow_and_update().clone();
            let rs = f(db_instance).await;
            if rs.is_err() {
                warn!("Failed to notify backend instance change");
            }
        }
    }

    async fn selector(
        &self,
        tenant_key: &TenantKey,
        lb: &BackendLoadBalancerType,
    ) -> Result<BackendInstance, Error> {
        if let Some(entry) = self.be_discovery.all_cluster_list().get(tenant_key) {
            let cluster_list_read_guard = entry.value().read().await;

            if cluster_list_read_guard.is_empty() {
                return Err(Error::new(
                    std::io::ErrorKind::NotFound,
                    "No backends found",
                ));
            } else if cluster_list_read_guard.len() == 1 {
                return Ok(cluster_list_read_guard.front().unwrap().clone());
            }
            match lb {
                BackendLoadBalancerType::Random => {
                    let backend_count = cluster_list_read_guard.len();
                    let selected_idx = self.balancer.balance(backend_count);
                    let backend_instance = cluster_list_read_guard.get(selected_idx).unwrap();
                    Ok(backend_instance.clone())
                }
                BackendLoadBalancerType::P2C => {
                    unreachable!()
                }
            }
        } else {
            Err(Error::new(std::io::ErrorKind::NotFound, "Tenant not found"))
        }
    }

    async fn load_backends(
        &self,
        tenant_key: Option<TenantKey>,
    ) -> Result<VecDeque<BackendInstance>, Error> {
        let all_be_list = self.be_discovery.all_cluster_list();
        if let Some(tenant) = tenant_key {
            if let Some(entry) = all_be_list.get(&tenant) {
                let cluster_list_read_guard = entry.value().read().await;
                Ok(cluster_list_read_guard.clone())
            } else {
                Err(Error::new(
                    std::io::ErrorKind::NotFound,
                    "No backends found",
                ))
            }
        } else {
            let mut result = VecDeque::new();
            for entry in all_be_list.iter() {
                let cluster_list_read_guard = entry.value().read().await;
                result.extend(cluster_list_read_guard.clone());
            }
            Ok(result)
        }
    }
}
