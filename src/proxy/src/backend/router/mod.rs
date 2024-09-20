mod static_router;
mod sync_router;

use crate::backend::router::static_router::StaticRouter;
use crate::backend::router::sync_router::SyncRouter;
use crate::backend::BackendInstance;
use crate::prost::common_proto::TenantKey;
use crate::server::proxy_cli_args::ProxyServerArgs;
use async_trait::async_trait;
use chrono::Utc;
use common::ShutdownMessage;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::VecDeque;
use std::future::Future;
use std::io::Error;
use std::sync::Mutex;
use strum_macros::EnumString;
use tokio::sync::watch::Receiver;
use tracing::info;

#[derive(Debug, Clone, EnumString)]
pub enum BackendRouterType {
    #[strum(serialize = "static")]
    Static,
    #[strum(serialize = "sync-with-cp")]
    SyncWithCp,
}

pub enum BackendRouterTrait {
    Static(Box<StaticRouter>),
    Sync(Box<SyncRouter>),
}

impl BackendRouterTrait {
    pub fn is_static(&self) -> bool {
        match self {
            BackendRouterTrait::Static(_) => true,
            BackendRouterTrait::Sync(_) => false,
        }
    }
}

#[async_trait]
impl BackendRouter for BackendRouterTrait {
    async fn status_change_notify<F, Fut>(&self, f: F) -> Result<(), Error>
    where
        F: Fn(BackendInstance) -> Fut + Send,
        Fut: Future<Output = Result<(), Error>> + Send + Sync,
    {
        match self {
            BackendRouterTrait::Static(s_router) => {
                info!("Static router does need to notify status change. Just initialize.");
                s_router.status_change_notify(f).await
            }
            BackendRouterTrait::Sync(sync_router) => sync_router.status_change_notify(f).await,
        }
    }

    async fn selector(
        &self,
        backend_location: &TenantKey,
        backend_selector: &BackendLoadBalancerType,
    ) -> Result<BackendInstance, Error> {
        match self {
            BackendRouterTrait::Static(router) => {
                router.selector(backend_location, backend_selector).await
            }
            BackendRouterTrait::Sync(router) => {
                router.selector(backend_location, backend_selector).await
            }
        }
    }

    async fn load_backends(
        &self,
        backend_location: Option<TenantKey>,
    ) -> Result<VecDeque<BackendInstance>, Error> {
        match self {
            BackendRouterTrait::Static(router) => router.load_backends(backend_location).await,
            BackendRouterTrait::Sync(router) => router.load_backends(backend_location).await,
        }
    }
}

#[derive(Debug, Clone, EnumString)]
pub enum BackendLoadBalancerType {
    #[strum(serialize = "random")]
    Random,
    #[strum(serialize = "p2c")]
    P2C,
}

pub trait BackendLoadBalancer: Send + Sync {
    // fn balance<T: Clone>(&self, backends: &VecDeque<T>) -> usize;
    fn balance(&self, backends: usize) -> usize;
}

pub struct RandomBalancer {
    rand: Mutex<StdRng>,
}

impl Default for RandomBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomBalancer {
    pub fn new() -> Self {
        Self {
            rand: Mutex::new(StdRng::seed_from_u64(
                Utc::now().timestamp_subsec_nanos().into(),
            )),
        }
    }
}

impl BackendLoadBalancer for RandomBalancer {
    fn balance(&self, backends: usize) -> usize {
        let mut mut_rand = self.rand.lock().unwrap();
        mut_rand.gen_range(0..backends)
    }
}

/// `BackendRouter` responsible for maintaining the list of users and their corresponding backends. Its functions include
///  1. Selecting appropriate backends according to the load balancing policy.
///  2. Updating the list status of backends when their topology changes.
#[async_trait]
pub trait BackendRouter: Send + Sync {
    //async fn status_change_notify(&self, f: BoxedStatusChangeNotify) -> Result<(), Error>;
    async fn status_change_notify<F, Fut>(&self, f: F) -> Result<(), Error>
    where
        F: Fn(BackendInstance) -> Fut + Send,
        Fut: Future<Output = Result<(), Error>> + Send + Sync;

    async fn selector(
        &self,
        backend_location: &TenantKey,
        backend_selector: &BackendLoadBalancerType,
    ) -> Result<BackendInstance, Error>;

    async fn load_backends(
        &self,
        backend_location: Option<TenantKey>,
    ) -> Result<VecDeque<BackendInstance>, Error>;
}

pub async fn new_backend_router(
    proxy_args: &ProxyServerArgs,
    shutdown_rx: &Receiver<ShutdownMessage>,
) -> BackendRouterTrait {
    let be_router = proxy_args.router_type();
    if let Some(router) = be_router {
        match router {
            BackendRouterType::Static => {
                let test_backend_list = proxy_args.static_backend_list();
                BackendRouterTrait::Static(Box::new(StaticRouter::new(test_backend_list)))
            }
            BackendRouterType::SyncWithCp => {
                BackendRouterTrait::Sync(Box::new(SyncRouter::new(proxy_args, shutdown_rx).await))
            }
        }
    } else {
        let test_backend_list = proxy_args.static_backend_list();
        BackendRouterTrait::Static(Box::new(StaticRouter::new(test_backend_list)))
    }
}

pub fn new_balancer(
    balancer_type_opt: Option<BackendLoadBalancerType>,
) -> impl BackendLoadBalancer {
    if let Some(balancer_type) = balancer_type_opt {
        match balancer_type {
            BackendLoadBalancerType::Random => RandomBalancer::new(),
            // for now only support random.
            _ => unreachable!(),
        }
    } else {
        RandomBalancer::new()
    }
}
