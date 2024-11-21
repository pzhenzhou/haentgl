use crate::backend::pool::{BackendIO, PooledConn};
use crate::backend::{BackendInstance, DbUserConnLifeCycle};

use deadpool::managed::{Metrics, RecycleError, RecycleResult};
use futures::FutureExt;
use nanoid::nanoid;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Clone)]
pub struct PooledConnMgr {
    backend_addr: Arc<Mutex<BackendInstance>>,
}

impl PooledConnMgr {
    pub fn new(backend_addr: BackendInstance) -> Self {
        Self {
            backend_addr: Arc::new(Mutex::new(backend_addr)),
        }
    }

    pub async fn get_addr(&self) -> String {
        self.backend_addr.lock().await.addr.clone()
    }

    pub async fn get_backend_instance(&self) -> BackendInstance {
        let backend_addr_guard = self.backend_addr.lock().await;
        backend_addr_guard.clone()
    }
}

#[async_trait::async_trait]
impl deadpool::managed::Manager for PooledConnMgr {
    type Type = PooledConn;
    type Error = std::io::Error;

    fn create(&self) -> impl Future<Output = Result<Self::Type, Self::Error>> + Send {
        async move {
            let backed_addr = self.get_addr().await;
            let backend_io = BackendIO::new(backed_addr.to_owned()).await.unwrap();
            let backend_conn = backend_io.get_backend_client();
            Ok(PooledConn {
                id: nanoid!(),
                inner_conn: backend_conn,
                conn_life_cycle: Arc::new(Mutex::new(DbUserConnLifeCycle::default())),
            })
        }
        .boxed()
    }

    fn recycle(
        &self,
        pooled_conn: &mut Self::Type,
        metrics: &Metrics,
    ) -> impl Future<Output = RecycleResult<Self::Error>> + Send {
        let conn_id = pooled_conn.id.clone();
        info!("ProxySrv try recycle conn_id={:?} {:?}", conn_id, metrics);
        async move {
            let conn_life_cycle = &pooled_conn.conn_life_cycle.lock().await;
            if conn_life_cycle.is_none() {
                info!(
                    "ProxySrv Failed recycled backend-end id={:?} conn_life_cycle is none",
                    &pooled_conn.id
                );
                return Err(RecycleError::from(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "connection is not used",
                )));
            }
            let rs = pooled_conn.close().await;
            if let Err(e) = rs {
                warn!("ProxySrv recycle conn_id={:?} failed {:?}", conn_id, e);
            }
            Ok(())
        }
        .boxed()
    }
}
