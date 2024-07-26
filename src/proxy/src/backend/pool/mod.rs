use crate::backend::DbUserConnLifeCycle;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

pub mod pooled_conn_mgr;

pub const BACKEND_CLIENT_DEFAULT_IDLE: Duration = Duration::from_secs(60 * 60);
#[derive(Debug, Clone)]
pub struct BackendPoolConfig {
    pub initial_size: u32,
    pub max_size: u32,
    pub time_to_idle: Duration,
}

impl Default for BackendPoolConfig {
    fn default() -> Self {
        Self {
            initial_size: 5,
            max_size: 50,
            time_to_idle: BACKEND_CLIENT_DEFAULT_IDLE,
        }
    }
}

pub type BackendConn = (PacketReader<OwnedReadHalf>, PacketWriter<OwnedWriteHalf>);

pub type SafeBackendConn = Arc<Mutex<BackendConn>>;

#[derive(Clone)]
pub struct PooledConn {
    pub id: String,
    pub inner_conn: SafeBackendConn,
    pub conn_life_cycle: Arc<Mutex<DbUserConnLifeCycle>>,
}

impl PooledConn {
    pub async fn close(&self) -> Result<(), std::io::Error> {
        let mut inner_guard = self.inner_conn.lock().await;
        let (_, writer) = inner_guard.deref_mut();
        writer.shutdown().await
    }
}
#[derive(Clone)]
pub struct BackendIO {
    backend_client: SafeBackendConn,
    backend_addr: String,
}

impl BackendIO {
    pub async fn new(backend_addr: String) -> Result<Self, std::io::Error> {
        let std_tcp_stream = std::net::TcpStream::connect(backend_addr.clone())?;
        std_tcp_stream.set_nonblocking(true)?;
        let tcp_stream_rs = tokio::net::TcpStream::from_std(std_tcp_stream)?;
        let (reader, writer) = tcp_stream_rs.into_split();
        Ok(Self {
            backend_client: Arc::new(Mutex::new((
                PacketReader::new(reader),
                PacketWriter::new(writer),
            ))),
            backend_addr,
        })
    }

    pub fn backend_addr(&self) -> String {
        self.backend_addr.clone()
    }

    pub fn get_backend_client(&self) -> Arc<Mutex<BackendConn>> {
        Arc::clone(&self.backend_client)
    }
}
