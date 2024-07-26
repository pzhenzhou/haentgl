use crate::async_packet_read;
use crate::backend::pool::{BackendIO, PooledConn};
use crate::backend::{BackendInstance, DbConnPhase, DbUserConnLifeCycle};
use crate::protocol::mysql::packet::writers::write_reset_connection;
use crate::server::default_capabilities;

use dashmap::DashMap;
use deadpool::managed::{Metrics, RecycleError, RecycleResult};
use futures::FutureExt;
use mysql_common::io::ParseBuf;
use mysql_common::proto::MyDeserialize;
use nanoid::nanoid;
use std::future::Future;
use std::ops::DerefMut;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

pub struct PooledConnMgr {
    backend_addr: Arc<Mutex<BackendInstance>>,
    conn_life_cycle_store: DashMap<String, DbUserConnLifeCycle>,
}

impl PooledConnMgr {
    pub fn new(backend_addr: BackendInstance) -> Self {
        Self {
            backend_addr: Arc::new(Mutex::new(backend_addr)),
            conn_life_cycle_store: Default::default(),
        }
    }

    pub async fn get_addr(&self) -> String {
        self.backend_addr.lock().await.addr.clone()
    }
    pub fn conn_life_cycle(&self, conn_uid: String) -> Option<DbUserConnLifeCycle> {
        self.conn_life_cycle_store
            .get(&conn_uid)
            .map(|v| v.value().clone())
    }

    pub fn save_conn_life_cycle(&self, conn_id: String, conn_life_cycle: DbUserConnLifeCycle) {
        self.conn_life_cycle_store.insert(conn_id, conn_life_cycle);
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
        _metrics: &Metrics,
    ) -> impl Future<Output = RecycleResult<Self::Error>> + Send {
        async {
            let conn_life_cycle = &pooled_conn.conn_life_cycle.lock().await;
            if conn_life_cycle.is_none() {
                return Err(RecycleError::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the connection auth failed",
                )));
            }
            let conn_phase = conn_life_cycle.conn_phase().unwrap();
            match conn_phase {
                DbConnPhase::Command => {
                    let backend_client = &pooled_conn.inner_conn;
                    let mut backend_client_guard = backend_client.lock().await;
                    let (br, bw) = backend_client_guard.deref_mut();
                    write_reset_connection(bw).await?;
                    let (_, pkt) = async_packet_read!(br);

                    if pkt.is_err_packet() {
                        let err_packet = mysql_common::packets::ErrPacket::deserialize(
                            default_capabilities(),
                            &mut ParseBuf(&pkt),
                        )
                        .unwrap();
                        warn!(
                            "ProxySrv Failed recycled backend-end id={:?} ERR={:?}",
                            &pooled_conn.id,
                            err_packet.server_error().message_str()
                        );
                        Err(RecycleError::from(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "reset connection failed",
                        )))
                    } else {
                        Ok(())
                    }
                }
                _ => {
                    warn!(
                        "ProxySrv Failed recycled backend-end id={:?} conn_phase={:?}",
                        &pooled_conn.id, conn_phase
                    );
                    Err(RecycleError::from(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the connection auth failed",
                    )))
                }
            }
        }
        .boxed()
    }

    fn detach(&self, pooled_conn: &mut PooledConn) {
        futures::executor::block_on(async {
            let conn_id = pooled_conn.id.clone();
            self.conn_life_cycle_store.remove(&conn_id);
            let close_rs = pooled_conn.close().await;
            debug!(
                "ProxySrv Detached backend-end id={:?} {:?}",
                conn_id, close_rs
            );
        })
    }
}
