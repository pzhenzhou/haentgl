use crate::backend::router::{
    BackendLoadBalancer, BackendLoadBalancerType, BackendRouter, RandomBalancer,
};
use crate::backend::BackendInstance;
use crate::prost::common_proto::TenantKey;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::future::Future;
use std::io::Error;

/// StaticRouter Only for testing purposes.
pub struct StaticRouter {
    backend_addrs: VecDeque<BackendInstance>,
    balancer: RandomBalancer,
}

impl StaticRouter {
    pub fn new(backend_addrs: VecDeque<BackendInstance>) -> Self {
        Self {
            backend_addrs,
            balancer: RandomBalancer::new(),
        }
    }
}

#[async_trait]
impl BackendRouter for StaticRouter {
    async fn status_change_notify<F, Fut>(&self, f: F) -> Result<(), Error>
    where
        F: Fn(BackendInstance) -> Fut + Send,
        Fut: Future<Output = Result<(), Error>> + Send + Sync,
    {
        for backend in self.backend_addrs.iter() {
            f(backend.clone()).await.unwrap();
        }
        Ok(())
    }

    async fn selector(
        &self,
        _backend_location: &TenantKey,
        _backend_selector: &BackendLoadBalancerType,
    ) -> Result<BackendInstance, Error> {
        let backend_deque = &self.backend_addrs;
        let backend_count = backend_deque.len();
        let selected_idx = self.balancer.balance(backend_count);
        let backend_addr = backend_deque.get(selected_idx).unwrap();
        Ok(backend_addr.clone())
    }

    async fn load_backends(
        &self,
        _backend_location: Option<TenantKey>,
    ) -> Result<VecDeque<BackendInstance>, Error> {
        Ok(self.backend_addrs.clone())
    }
}
