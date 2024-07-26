use crate::backend::backend_mgr::{BackendManagerOptions, BackendMgr};
use crate::backend::router::BackendRouterTrait;
use crate::backend::{DbConnPhase, DbUserConnLifeCycle};
use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::constants::CommandCode;
use crate::protocol::mysql::error_codes::ErrorKind;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::*;
use crate::server::auth::{gen_user_salt, Authenticator};
use crate::server::forwarder::query_forward::QueryForwarder;
use crate::server::forwarder::reset_conn_forward::ResetConnForwarder;
use crate::server::forwarder::stmt_prepare_forward::StmtPrepareForwarder;
use crate::server::forwarder::{change_user_forward, ComForwarder, GenericComForwarder};
use crate::server::{init_sql_com_labels, ProxyServer};

use async_trait::async_trait;
use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::io::Error;
use std::ops::DerefMut;
use std::sync::Arc;

use common::metrics::common_labels;
use common::metrics::metric_def::PROXY_COM_LATENCY;
use num_traits::FromPrimitive;
use rustls::server::ServerConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::RwLock;
use tokio_rustls::rustls;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct ProxyConnStatus {
    max_connections: u64,
    conn_id: u64,
}

impl Default for ProxyConnStatus {
    fn default() -> Self {
        Self {
            max_connections: 1000,
            conn_id: 0,
        }
    }
}

pub struct HaentglServer<A> {
    sql_com_labels: HashMap<u8, Vec<(&'static str, String)>>,
    conn_status: RwLock<ProxyConnStatus>,
    backend_mgr: BackendMgr,
    authenticator: A,
}

impl<A: Authenticator> HaentglServer<A> {
    pub fn new(
        options: BackendManagerOptions,
        router: BackendRouterTrait,
        authenticator: A,
    ) -> Self {
        let proxy_conn_status = ProxyConnStatus::default();
        common::metrics::gauge(
            common::metrics::metric_def::PROXY_MAX_CONN,
            proxy_conn_status.max_connections as f64,
            Some(common_labels()),
        );
        Self {
            sql_com_labels: init_sql_com_labels().clone(),
            conn_status: RwLock::new(proxy_conn_status),
            backend_mgr: BackendMgr::new(router, options),
            authenticator,
        }
    }

    pub async fn connect<'a, R, W>(
        &'a self,
        reader: R,
        mut writer: W,
        #[cfg(feature = "tls")] tls_conf: &Option<Arc<ServerConfig>>,
    ) -> Result<(), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        let conn_id = self.update_server_status(&mut writer).await?;
        let salt = gen_user_salt();
        info!("ProxySrv on_conn conn_id={conn_id}");
        // record curr connect
        common::metrics::gauge_inc(
            common::metrics::metric_def::PROXY_CURR_CONN,
            1_f64,
            Some(common_labels()),
        );
        #[cfg(feature = "tls")]
        let (seq, handshake_response, handshake_pkt, mut reader) =
            self.on_conn(reader, &mut writer, salt, tls_conf).await?;
        #[cfg(not(feature = "tls"))]
        let (seq, handshake_response, handshake_pkt, mut reader) =
            self.on_conn(reader, &mut writer, salt, None).await?;

        let pool_ref = self
            .backend_mgr
            .connect_to_backend(&handshake_response)
            .await?;

        let pool_conn_mgr = pool_ref.manager();
        // FIXME: when pool is full, it will block here.
        let pooled_conn = pool_ref.get().await.unwrap();
        let conn_uid = &pooled_conn.id;

        let conn_life_cycle = pool_conn_mgr
            .conn_life_cycle(conn_uid.clone())
            .unwrap_or_default();
        let backend_conn = &pooled_conn.inner_conn;
        let mut backend_client_guard = backend_conn.lock().await;
        let (backend_reader, backend_writer) = backend_client_guard.deref_mut();
        backend_writer.reset_seq();

        let mut mut_writer = PacketWriter::new(writer);
        let auth_result = if let Some(conn_phase) = conn_life_cycle.conn_phase() {
            match conn_phase {
                DbConnPhase::Command => {
                    debug!("ProxySrv  ConnPhase == Command  {conn_uid:?}.");
                    self.authenticator
                        .continue_auth::<R, W>(
                            backend_writer,
                            backend_reader,
                            &mut mut_writer,
                            &mut reader,
                            seq,
                            &handshake_response,
                        )
                        .await
                }
                _ => {
                    debug!("ProxySrv ConnPhase == Connection {conn_uid:?}.");
                    self.authenticator
                        .reply_handshake_response::<R, W>(
                            backend_writer,
                            backend_reader,
                            &mut mut_writer,
                            &mut reader,
                            seq,
                            (&handshake_pkt, &handshake_response),
                        )
                        .await
                }
            }
        } else {
            debug!("ProxySrv First authentication on current conn {conn_uid:?}.");
            self.authenticator
                .reply_handshake_response::<R, W>(
                    backend_writer,
                    backend_reader,
                    &mut mut_writer,
                    &mut reader,
                    seq,
                    (&handshake_pkt, &handshake_response),
                )
                .await
        };
        let db_user = handshake_response.db_user_string();
        match auth_result {
            Ok(()) => {
                pool_conn_mgr.save_conn_life_cycle(
                    conn_uid.clone(),
                    DbUserConnLifeCycle::new_conn_life_cycle(db_user, DbConnPhase::Command),
                );
            }
            Err(_e) => {
                pool_conn_mgr.save_conn_life_cycle(
                    conn_uid.clone(),
                    DbUserConnLifeCycle::new_conn_life_cycle(db_user, DbConnPhase::Connection),
                );
                debug!("Authentication failure does not execute the command");
                return Ok(());
            }
        }

        let borrow_writer = mut_writer.borrow_mut();
        self.on_com(
            &mut reader,
            borrow_writer,
            backend_writer,
            backend_reader,
            &handshake_response,
        )
            .await
    }

    pub async fn initialize(&mut self) -> Result<(), Error> {
        self.backend_mgr.prepare_backend_pool().await
    }

    async fn update_server_status<W>(&self, client_writer: &mut W) -> Result<u64, Error>
    where
        W: AsyncWrite + Send + Unpin,
    {
        let mut write_guard = self.conn_status.write().await;
        let curr_conn = write_guard.conn_id;
        if curr_conn > write_guard.max_connections {
            let mut writer = PacketWriter::new(client_writer);
            writers::write_err_packet(
                ErrorKind::ER_TOO_MANY_USER_CONNECTIONS,
                "too many connections.".as_bytes(),
                &mut writer,
            )
                .await?;
            return Err(Error::new(
                std::io::ErrorKind::TooManyLinks,
                "too many connections".to_string(),
            ));
        }
        write_guard.conn_id += 1;
        let conn_id = write_guard.conn_id;
        Ok(conn_id)
    }
}

#[async_trait]
impl<A: Authenticator> ProxyServer for HaentglServer<A> {
    async fn on_conn<R, W>(
        &self,
        r: R,
        w: &mut W,
        scramble: [u8; 20],
        #[cfg(feature = "tls")] tls_conf: &Option<Arc<ServerConfig>>,
    ) -> Result<(u8, HandshakeResponse, Packet, PacketReader<R>), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        let mut client_reader = PacketReader::new(r);
        let mut client_writer = PacketWriter::new(w);
        let conn_id = self.conn_status.read().await.conn_id;
        #[cfg(feature = "tls")]
        let (seq, handshake_response, pkt) = self
            .authenticator
            .initial_handshake(
                conn_id,
                scramble,
                &mut client_reader,
                &mut client_writer,
                tls_conf,
            )
            .await?;
        #[cfg(not(feature = "tls"))]
        let (seq, handshake_response, pkt) = self
            .authenticator
            .initial_handshake(
                conn_id,
                scramble,
                &mut client_reader,
                &mut client_writer,
                &None,
            )
            .await?;
        Ok((seq, handshake_response, pkt, client_reader))
    }

    async fn on_com<'a, R, W>(
        &self,
        client_reader: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        handshake_response: &'a HandshakeResponse,
    ) -> Result<(), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        backend_writer.reset_seq();
        loop {
            let pkt_opt = client_reader.next_async().await?;
            if pkt_opt.is_none() {
                warn!("ProxySrv Receive EMPTY PKT: Malform packet error ");
                return Err(Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Malform packet error".to_string(),
                ));
            }
            let (seq, client_packet) = pkt_opt.unwrap();
            let recv_com_code = client_packet[0];
            let com_code = CommandCode::from_u8(recv_com_code).unwrap();
            // info!("ProxySrv on_com receive ComCode={:?} from client", com_code);
            let com_forwarder: Box<dyn ComForwarder<R, W>> = match com_code {
                CommandCode::ComStmtPrepare | CommandCode::ComStmtClose => {
                    Box::new(StmtPrepareForwarder {
                        com_code,
                        request: client_packet.clone(),
                    })
                }
                CommandCode::ComQuery
                | CommandCode::ComStmtExecute
                | CommandCode::ComProcessInfo
                | CommandCode::ComFieldList
                | CommandCode::ComStmtFetch => Box::new(QueryForwarder { com_code }),
                CommandCode::ComQuit => Box::new(ResetConnForwarder),
                CommandCode::ComChangeUser => Box::new(change_user_forward::ChangeUserForwarder),
                _ => Box::new(GenericComForwarder),
            };
            com_forwarder
                .write_to_backend(
                    seq,
                    com_code,
                    handshake_response,
                    client_packet,
                    backend_writer,
                )
                .await?;

            let labels = self.sql_com_labels.get(&recv_com_code).unwrap();
            let _com_latency =
                common::metrics::MetricsTimer::new_with_labels(PROXY_COM_LATENCY, labels);
            let _pkt = com_forwarder
                .forward(
                    client_reader,
                    client_writer,
                    backend_writer,
                    backend_reader,
                    handshake_response,
                )
                .await?;
            if com_code == CommandCode::ComQuit {
                common::metrics::gauge_dec(
                    common::metrics::metric_def::PROXY_CURR_CONN,
                    1_f64,
                    Some(common_labels()),
                );
                break;
            }
        }
        Ok(())
    }

    async fn close(&self) {}
}
