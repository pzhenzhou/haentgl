use crate::backend::pool::pooled_conn_mgr::PooledConnMgr;
use crate::backend::pool::BackendPoolConfig;

use crate::prost::common_proto::{ServiceStatus, TenantKey};
use crate::backend::router::{BackendLoadBalancerType, BackendRouter, BackendRouterTrait};
use crate::backend::{decode_tenant_key, test_tenant_key, BackendInstance};
use crate::protocol::mysql::basic::HandshakeResponse;

use dashmap::DashMap;
use deadpool::managed::{Object, Pool};
use itertools::Itertools;
use std::io::ErrorKind;
use std::str;
use std::sync::{Arc, OnceLock};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct BackendManagerOptions {
    pub tls: bool,
    pub pool_size: u16,
    pub static_router: bool,
    pub balance_type: BackendLoadBalancerType,
    pub pool_config: BackendPoolConfig,
}

impl Default for BackendManagerOptions {
    fn default() -> Self {
        Self {
            tls: false,
            pool_size: 100,
            static_router: true,
            balance_type: BackendLoadBalancerType::Random,
            pool_config: BackendPoolConfig::default(),
        }
    }
}

static BE_MGR_ONCE: OnceLock<Arc<BackendMgr>> = OnceLock::new();

pub fn get_or_init_backend_mgr(
    router: BackendRouterTrait,
    mgr_options: BackendManagerOptions,
) -> Arc<BackendMgr> {
    Arc::clone(BE_MGR_ONCE.get_or_init(|| Arc::new(BackendMgr::new(router, mgr_options))))
}

pub struct BackendMgr {
    mgr_options: BackendManagerOptions,
    router: BackendRouterTrait,
    be_conn_pool: DashMap<BackendInstance, Pool<PooledConnMgr>>,
}

impl BackendMgr {
    pub fn new(router: BackendRouterTrait, mgr_options: BackendManagerOptions) -> Self {
        Self {
            mgr_options,
            router,
            be_conn_pool: DashMap::new(),
        }
    }

    async fn init_backend_pool(
        &self,
        backend_instance: BackendInstance,
    ) -> Result<(), std::io::Error> {
        let backend_status = backend_instance.status;
        let max_size = self.mgr_options.pool_config.max_size;
        match backend_status {
            ServiceStatus::Ready => {
                let conn_mgr = PooledConnMgr::new(backend_instance.clone());
                let inner_pool_rs = Pool::builder(conn_mgr).max_size(max_size as usize).build();
                match inner_pool_rs {
                    Ok(inner_pool) => {
                        info!(
                            "ProxySrv backend_mgr conn pool initialized successfully. {:?}",
                            backend_instance.addr
                        );
                        self.be_conn_pool.insert(backend_instance, inner_pool);
                        Ok(())
                    }
                    Err(e) => {
                        warn!("ProxySrv backend_mgr init backend_conn_pool Err {:?}", e);
                        Err(std::io::Error::new(
                            ErrorKind::ConnectionRefused,
                            e.to_string(),
                        ))
                    }
                }
            }
            ServiceStatus::Offline => {
                if let Some(entry) = self.be_conn_pool.get(&backend_instance) {
                    let pool = entry.value();
                    pool.close();
                }
                self.be_conn_pool.remove(&backend_instance);
                Ok(())
            }
            ServiceStatus::UnKnowStatus | ServiceStatus::NotReady => Ok(()),
        }
    }

    pub async fn prepare_backend_conn_pool(&self) -> Result<(), std::io::Error> {
        self.router
            .status_change_notify(|be| async move {
                let rs = self.init_backend_pool(be.clone()).await;
                if rs.is_err() {
                    warn!("Failed to notify backend instance change");
                }
                rs
            })
            .await
    }

    pub async fn connect_to_backend(
        &self,
        client_handshake_rsp: &HandshakeResponse,
    ) -> Result<Pool<PooledConnMgr, Object<PooledConnMgr>>, std::io::Error> {
        let balancer_type = &self.mgr_options.balance_type;
        // 1. get BackendAddr list by user
        let tenant = if let Some(tenant_encode_key) = &client_handshake_rsp.tenant_key {
            let tenant_encode_str = str::from_utf8(tenant_encode_key).unwrap();
            decode_tenant_key(tenant_encode_str)
        } else {
            test_tenant_key()
        };
        debug!(
            "ProxySrv backend_mgr connect_to_backend tenant {:?}",
            &tenant
        );
        let backend_addr = self.router.selector(&tenant, balancer_type).await?;
        // debug!(
        //     "ProxySrv backend_mgr selected backend_addr {:?}",
        //     &backend_addr.addr
        // );
        return if let Some(pool) = self.be_conn_pool.get(&backend_addr) {
            let pool_values = pool.value().clone();
            Ok(pool_values)
        } else {
            Err(std::io::Error::new(
                ErrorKind::NotConnected,
                "no backend_addr found",
            ))
        };
    }

    pub fn tenant_status(&self, tenant: TenantKey) -> ServiceStatus {
        let be_list = self
            .be_conn_pool
            .iter()
            .map(|e| e.key().clone())
            .collect_vec();
        be_list
            .iter()
            .find(|be| {
                let cluster = &be.cluster;
                cluster.cluster_name.eq(tenant.cluster_name.as_str())
                    && cluster.namespace.eq(tenant.namespace.as_str())
            })
            .map(|be| be.status)
            .unwrap_or(ServiceStatus::UnKnowStatus)
    }
}
