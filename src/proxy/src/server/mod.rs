use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::constants::SqlComInfo;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::Packet;
use async_trait::async_trait;
use common::metrics::common_labels;
use mysql_common::constants::CapabilityFlags;
use std::sync::OnceLock;

use rustls::server::ServerConfig;
use std::collections::HashMap;
use std::vec;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio_rustls::rustls;

pub mod auth;
pub mod cmd_handler;
mod forwarder;
pub mod haentgl_server;
pub mod proxy_cli_args;
#[allow(unused_variables)]
pub mod static_proxy;

#[macro_export]
macro_rules! parse_err_packet {
    ($capabilities:expr, $packet:expr,$err_msg:expr) => {
        use mysql_common::io::ParseBuf;
        use mysql_common::proto::MyDeserialize;
        use tracing::warn;

        let err_packet =
            mysql_common::packets::ErrPacket::deserialize($capabilities, &mut ParseBuf(&$packet))
                .unwrap();
        let server_error = err_packet.server_error();
        let server_err_msg = server_error.message_str();
        warn!("{:?} {:?}", $err_msg, server_err_msg);
    };
}

// FIXME: may be get from cp.
pub const DEFAULT_BACKEND_VERSION: &[u8] = b"11.1.2-MariaDB-1:11.1.2+maria~ubu2204";
pub const PROXY_COM_METRIC_LABEL_KEY: &str = "proxy_com";
pub const PROXY_CONN_METRIC_LABEL_KEY: &str = "proxy_conn";
pub const PROXY_ENV_SYNC_ROUTER: &str = "PROXY_SYNC_ROUTER";

pub static DEFAULT_CAPABILITIES_ONCE: OnceLock<CapabilityFlags> = OnceLock::new();

static PROXY_COM: OnceLock<HashMap<u8, Vec<(&'static str, String)>>> = OnceLock::new();

pub fn init_sql_com_labels() -> &'static HashMap<u8, Vec<(&'static str, String)>> {
    PROXY_COM.get_or_init(|| {
        let process_labels = common_labels();
        let code_and_str = SqlComInfo::all_sql_com();
        let all_labels = code_and_str
            .iter()
            .map(|(com_code, com_str)| {
                (
                    *com_code,
                    [
                        &vec![(PROXY_COM_METRIC_LABEL_KEY, com_str.to_string())][..],
                        &process_labels[..],
                    ]
                    .concat(),
                )
            })
            .collect::<HashMap<u8, Vec<(&'static str, String)>>>();
        all_labels
    })
}

// CLIENT_QUERY_ATTRIBUTES new capability flag.
// MariaDB 10.6: not include this attribute.
// MySQL 8.0.34: default include this attribute.
// COM_QUERY: https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_com_query.html
pub fn default_capabilities() -> CapabilityFlags {
    *DEFAULT_CAPABILITIES_ONCE.get_or_init(|| {
        CapabilityFlags::CLIENT_CAN_HANDLE_EXPIRED_PASSWORDS
            | CapabilityFlags::CLIENT_CONNECT_ATTRS
            | CapabilityFlags::CLIENT_CONNECT_WITH_DB
            | CapabilityFlags::CLIENT_DEPRECATE_EOF
            | CapabilityFlags::CLIENT_FOUND_ROWS
            | CapabilityFlags::CLIENT_IGNORE_SIGPIPE
            | CapabilityFlags::CLIENT_IGNORE_SPACE
            | CapabilityFlags::CLIENT_INTERACTIVE
            | CapabilityFlags::CLIENT_LOCAL_FILES
            | CapabilityFlags::CLIENT_LONG_FLAG
            | CapabilityFlags::CLIENT_LONG_PASSWORD
            | CapabilityFlags::CLIENT_MULTI_RESULTS
            | CapabilityFlags::CLIENT_MULTI_STATEMENTS
            | CapabilityFlags::CLIENT_NO_SCHEMA
            | CapabilityFlags::CLIENT_ODBC
            | CapabilityFlags::CLIENT_OPTIONAL_RESULTSET_METADATA
            | CapabilityFlags::CLIENT_PLUGIN_AUTH
            | CapabilityFlags::CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA
            | CapabilityFlags::CLIENT_PROTOCOL_41
            | CapabilityFlags::CLIENT_PS_MULTI_RESULTS
            // | CapabilityFlags::CLIENT_QUERY_ATTRIBUTES
            | CapabilityFlags::CLIENT_REMEMBER_OPTIONS
            | CapabilityFlags::CLIENT_RESERVED
            | CapabilityFlags::CLIENT_SECURE_CONNECTION
            | CapabilityFlags::CLIENT_SESSION_TRACK
            | CapabilityFlags::CLIENT_TRANSACTIONS
    })
}

/// `ProxyServer` is the abstract core feature of the MySQL proxy server including:
/// 1. Connect MySQL client with Backend (Backend Instance), authenticate and forward commands.
/// 2. Serve as the access layer for Serverless to reduce the impact of Backend changes on customers,
///    including but not limited to - node upgrades, node downgrades, suspensions, and resumptions.
#[async_trait]
pub trait ProxyServer {
    /// Route to an available Backend Instance cluster backend and authenticate.
    ///
    /// Returns the HandshakeResponse (client's handshake information)
    /// as well as the client's tcp reader (TcpStream reader)
    async fn on_conn<R, W>(
        &self,
        client_reader: R,
        client_writer: &mut W,
        scramble: [u8; 20],
        #[cfg(feature = "tls")] tls_conf: &Option<std::sync::Arc<ServerConfig>>,
    ) -> Result<(u8, HandshakeResponse, Packet, PacketReader<R>), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin;

    /// Forwards packets between the client and the Backend.
    /// If the backend connection fails, it redirects to an available backend connection.
    async fn on_com<'a, R, W>(
        &self,
        client_reader: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        handshake_response: &'a HandshakeResponse,
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin;

    async fn close(&self);
}
