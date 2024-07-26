use crate::backend::backend_discovery::BackendDiscovery;
use crate::backend::prost::common_proto::TenantKey;
use crate::backend::router::{
    BackendLoadBalancer, BackendLoadBalancerType, BackendRouter, RandomBalancer,
};
use crate::backend::{start_backend_discovery, BackendInstance};
use crate::server::proxy_cli_args::ProxyServerArgs;

use async_trait::async_trait;
use std::collections::VecDeque;
use std::io::Error;
use std::sync::Arc;

pub struct SyncRouter {
    be_discovery: Arc<BackendDiscovery>,
    balancer: Box<dyn BackendLoadBalancer>,
}

impl SyncRouter {
    pub async fn new(proxy_cli: &ProxyServerArgs) -> Self {
        let node_id = proxy_cli.get_node_id();
        let topology_srv_addr = proxy_cli.cluster_watcher_addr.clone().unwrap();
        Self {
            be_discovery: start_backend_discovery(node_id, topology_srv_addr).await,
            balancer: Box::new(RandomBalancer::new()),
        }
    }
}

#[async_trait]
impl BackendRouter for SyncRouter {
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
            return if let Some(entry) = all_be_list.get(&tenant) {
                let cluster_list_read_guard = entry.value().read().await;
                Ok(cluster_list_read_guard.clone())
            } else {
                Err(Error::new(
                    std::io::ErrorKind::NotFound,
                    "No backends found",
                ))
            };
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
