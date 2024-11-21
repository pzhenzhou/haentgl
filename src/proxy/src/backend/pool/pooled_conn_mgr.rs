use crate::backend::pool::{BackendIO, PooledConn};
use crate::backend::{BackendInstance, DbConnPhase, DbUserConnLifeCycle};

use deadpool::managed::{Metrics, RecycleError, RecycleResult};
use futures::FutureExt;
use nanoid::nanoid;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

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
        info!("ProxySrv recycle metrics={:?}", metrics);
        async move {
            let conn_life_cycle = &pooled_conn.conn_life_cycle.lock().await;
            if conn_life_cycle.is_none() {
                info!(
                    "ProxySrv conn_id={:?} back into pool.",
                    &pooled_conn.id
                );
                Ok(())
            } else {
                let conn_phase = conn_life_cycle.conn_phase.clone().unwrap();
                info!(
                    "ProxySrv recycled conn_id={:?} {:?}",
                    &pooled_conn.id, conn_life_cycle
                );
                match conn_phase {
                    DbConnPhase::Connection => Err(RecycleError::from(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "connection is illegal",
                    ))),
                    _ => Ok(()),
                }
            }
        }
        .boxed()
    }

    fn detach(&self, pooled_conn: &mut PooledConn) {
        futures::executor::block_on(async {
            let conn_id = pooled_conn.id.clone();
            let close_rs = pooled_conn.close().await;
            info!(
                "ProxySrv Detached backend-end id={:?} close_rs is err {:?}",
                conn_id, close_rs
            );
        })
    }
}
