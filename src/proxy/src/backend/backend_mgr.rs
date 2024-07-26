use crate::backend::pool::pooled_conn_mgr::PooledConnMgr;
use crate::backend::pool::BackendPoolConfig;
use crate::backend::router::{BackendLoadBalancerType, BackendRouter, BackendRouterTrait};
use crate::backend::{test_tenant_key, BackendInstance};
use crate::protocol::mysql::basic::HandshakeResponse;

use crate::backend::prost::common_proto::TenantKey;
use dashmap::DashMap;
use deadpool::managed::{Object, Pool};
use std::io::ErrorKind;
use tracing::{info, warn};

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
            static_router: false,
            balance_type: BackendLoadBalancerType::Random,
            pool_config: BackendPoolConfig::default(),
        }
    }
}

// TODO: impl tenant key parser.
fn user_extract(_is_static: bool) -> impl Fn(&[u8]) -> TenantKey {
    |_conn_user: &[u8]| -> TenantKey { test_tenant_key() }
}

pub struct BackendMgr {
    mgr_options: BackendManagerOptions,
    router: BackendRouterTrait,
    backend_pool: DashMap<BackendInstance, Pool<PooledConnMgr>>,
}

impl BackendMgr {
    pub fn new(router: BackendRouterTrait, mgr_options: BackendManagerOptions) -> Self {
        Self {
            mgr_options,
            router,
            backend_pool: DashMap::new(),
        }
    }

    async fn init_backend_pool(
        &self,
        backend_instance: BackendInstance,
        max_size: u32,
    ) -> Result<Pool<PooledConnMgr>, std::io::Error> {
        let conn_mgr = PooledConnMgr::new(backend_instance.clone());
        let inner_pool_rs = Pool::builder(conn_mgr).max_size(max_size as usize).build();
        match inner_pool_rs {
            Ok(inner_pool) => {
                info!(
                    "ProxySrv backend_mgr conn pool initialized successfully. {:?}",
                    backend_instance
                );
                Ok(inner_pool)
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

    pub async fn prepare_backend_pool(&self) -> Result<(), std::io::Error> {
        let all_backend_list = self.router.load_backends(None).await?;
        let max_size = &self.mgr_options.pool_config.max_size;
        info!("ProxySrv backend_mgr prepare_backend_pool start max_size={max_size:?}, all_backend_list={all_backend_list:?}");
        for backend_addr in all_backend_list.iter() {
            let inner_pool = self
                .init_backend_pool(backend_addr.clone(), *max_size)
                .await?;
            self.backend_pool.insert(backend_addr.clone(), inner_pool);
        }
        Ok(())
    }

    pub async fn connect_to_backend(
        &self,
        client_handshake_rsp: &HandshakeResponse,
    ) -> Result<Pool<PooledConnMgr, Object<PooledConnMgr>>, std::io::Error> {
        let is_static = &self.mgr_options.static_router;
        let balancer_type = &self.mgr_options.balance_type;
        // 1. get BackendAddr list by user
        let form_user_fn = user_extract(*is_static);
        let conn_user = client_handshake_rsp.username.as_ref().unwrap();
        let backend_location = form_user_fn(conn_user.as_slice());
        let backend_addr = self
            .router
            .selector(&backend_location, balancer_type)
            .await?;

        return if let Some(pool) = self.backend_pool.get(&backend_addr) {
            let pool_values = pool.value().clone();
            Ok(pool_values)
        } else {
            Err(std::io::Error::new(
                ErrorKind::NotConnected,
                "no backend_addr found",
            ))
        };
    }
}
