use crate::async_packet_read;
use crate::protocol::mysql::basic::{client_handshake_response, HandshakeResponse};
use crate::protocol::mysql::charset::UTF8_MB4_GENERAL_CI;
use crate::protocol::mysql::constants::AuthPluginName::UnKnowPluginName;
use crate::protocol::mysql::constants::HeaderInfo;
use crate::protocol::mysql::error_codes::ErrorKind;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::{writers, Packet};
use crate::server::auth::Authenticator;
use crate::server::{default_capabilities, DEFAULT_BACKEND_VERSION};

use async_trait::async_trait;
use mysql_common::io::ParseBuf;
use mysql_common::packets::{AuthPlugin, ComChangeUserMoreData, ErrPacket};
use mysql_common::proto::{MyDeserialize, MySerialize};
use rustls::server::ServerConfig;
use std::borrow::Cow;
use std::io::{Error, Write};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio_rustls::rustls;
use tracing::{debug, warn};

const AUTH_SWITCH_REQUEST: u8 = 0xfe;

pub struct ProxyAuthenticator;

impl ProxyAuthenticator {
    async fn process_auth_switch_plugin<R, W>(
        &self,
        client_seq: u8,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
        client_reader: &mut PacketReader<R>,
    ) -> Result<(), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        let (be_seq, pkt) = async_packet_read!(backend_reader);
        assert_eq!(AUTH_SWITCH_REQUEST, pkt[0]);
        client_writer.set_seq(client_seq + 1);
        client_writer.write_all(&pkt)?;
        client_writer.end_packet().await?;
        client_writer.flush_all().await?;

        // read auth_response from client (password_len + hash(password, salt))
        let (_c_seq, auth_response) = async_packet_read!(client_reader);
        backend_writer.set_seq(be_seq + 1);
        backend_writer.write_all(&auth_response)?;
        backend_writer.end_packet().await?;
        backend_writer.flush_all().await?;

        let (l_seq, be_auth_pkt) = async_packet_read!(backend_reader);
        client_writer.set_seq(l_seq);
        client_writer.write_all(&be_auth_pkt)?;
        client_writer.end_packet().await?;
        client_writer.flush_all().await?;
        if be_auth_pkt[0] == HeaderInfo::ErrHeader as u8 {
            let err_packet =
                ErrPacket::deserialize(default_capabilities(), &mut ParseBuf(&be_auth_pkt))
                    .unwrap();
            let srv_error = err_packet.server_error();
            warn!(" {:?}", srv_error.message_str());
            let msg_err = srv_error.message_ref();
            let err_msg_str = String::from_utf8(msg_err.to_vec()).unwrap();
            Err(Error::new(
                std::io::ErrorKind::PermissionDenied,
                err_msg_str,
            ))
        } else {
            debug!(
                "ProxySrv Auth continue_auth auth success={:?}",
                be_auth_pkt[0]
            );
            Ok(())
        }
    }
}

/// `reset_handshake_plugin` Reset the plugin_name of HandshakeResponse.
///
/// The authentication phase involves  - client, proxy, and backend. The proxy has to route to the
/// correct backend (from backend connection pool) based on the client's handshake information,
/// so it needs to send a Handshake to the client first (including salt/scramble).
/// After handshaking with the backend, the proxy then forwards the client's HandshakeResponse to
/// the backend for authentication. The authentication fails because the scramble (random string)
/// generated during the two handshakes is different. So, we need to trigger an AuthSwitchRequest to
/// get the correct auth response from the client.
fn reset_handshake_plugin(
    packet: &[u8],
    handshake_response: &HandshakeResponse,
) -> Result<Vec<u8>, Error> {
    let curr_user = handshake_response.username.clone();
    let max_pkt_len = handshake_response.max_packet_len;
    mysql_common::packets::HandshakeResponse::deserialize((), &mut ParseBuf(packet))
        .map(|pkt| {
            let un_know_plugin_rsp = mysql_common::packets::HandshakeResponse::new(
                Some(pkt.scramble_buf()),
                (8, 0, 36),
                curr_user,
                pkt.db_name(),
                Some(AuthPlugin::Other(Cow::from(
                    UnKnowPluginName.as_ref().as_bytes(),
                ))),
                pkt.capabilities(),
                pkt.connect_attributes(),
                max_pkt_len,
            );
            let mut new_packet = Vec::new();
            un_know_plugin_rsp.serialize(&mut new_packet);
            new_packet
        })
        .map_err(|e| Error::new(std::io::ErrorKind::InvalidData, e))
}

#[async_trait]
impl Authenticator for ProxyAuthenticator {
    async fn continue_auth<R, W>(
        &self,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
        client_reader: &mut PacketReader<R>,
        client_seq: u8,
        handshake_resp: &HandshakeResponse,
    ) -> Result<(), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        let un_know_plugin_data =
            AuthPlugin::Other(Cow::from(UnKnowPluginName.as_ref().as_bytes()));

        let com_change_user = mysql_common::packets::ComChangeUser::new()
            .with_user(handshake_resp.username.as_deref())
            .with_database(handshake_resp.database.as_deref())
            .with_more_data(Some(
                ComChangeUserMoreData::new(UTF8_MB4_GENERAL_CI as u16)
                    .with_auth_plugin(Some(un_know_plugin_data.clone())),
            ));
        let mut change_user_data = Vec::new();
        com_change_user.serialize(&mut change_user_data);

        backend_writer.write_all(&change_user_data)?;
        backend_writer.end_packet().await?;
        backend_writer.flush_all().await?;
        // see: https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_connection_phase.html#sect_protocol_connection_phase_com_change_user_auth
        self.process_auth_switch_plugin(
            client_seq,
            backend_writer,
            backend_reader,
            client_writer,
            client_reader,
        )
            .await
    }

    async fn initial_handshake<R, W>(
        &self,
        conn_id: u64,
        scramble: [u8; 20],
        client_reader: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        #[cfg(feature = "tls")] tls_conf: &Option<std::sync::Arc<ServerConfig>>,
    ) -> Result<(u8, HandshakeResponse, Packet), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        let server_version_bytes = DEFAULT_BACKEND_VERSION;
        // 1. The ProxyServer sends an initial handshake packet to the client.
        #[cfg(feature = "tls")]
        writers::write_initial_handshake(
            client_writer,
            conn_id,
            scramble,
            server_version_bytes,
            tls_conf,
        )
            .await?;
        #[cfg(not(feature = "tls"))]
        writers::write_initial_handshake(client_writer, conn_id, scramble, server_version_bytes)
            .await?;
        // 2. The ProxyServer reads the client's HandshakeResponse.
        if let Some((seq, client_handshake_rsp_pkt)) = client_reader.next_async().await? {
            let (_, handshake_resp) =
                client_handshake_response(&client_handshake_rsp_pkt, false).unwrap();
            Ok((seq, handshake_resp, client_handshake_rsp_pkt))
        } else {
            warn!("ProxySrv Failed to read client HandshakeResponse");
            writers::write_err_packet(
                ErrorKind::ER_ACCESS_DENIED_ERROR,
                "peer terminated connection".as_bytes(),
                client_writer,
            )
                .await?;
            Err(Error::new(
                std::io::ErrorKind::PermissionDenied,
                "peer terminated connection",
            ))
        }
    }

    async fn reply_handshake_response<R, W>(
        &self,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
        client_reader: &mut PacketReader<R>,
        client_seq: u8,
        handshake_resp_pair: (&[u8], &HandshakeResponse),
    ) -> Result<(), Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        // 1. ProxyServer reads initial handshake packets from the backend.
        let (_seq_val, _handshake_init) = async_packet_read!(backend_reader);
        let (packet_bytes, client_handshake_rsp) = handshake_resp_pair;
        let new_packet = reset_handshake_plugin(packet_bytes, client_handshake_rsp)?;

        backend_writer.set_seq(client_seq);
        backend_writer.write_all(&new_packet)?;
        backend_writer.end_packet().await?;
        backend_writer.flush_all().await?;
        self.process_auth_switch_plugin(
            client_seq,
            backend_writer,
            backend_reader,
            client_writer,
            client_reader,
        )
            .await
    }
}
