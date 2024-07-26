use crate::parse_err_packet;
use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::Packet;
use crate::server::forwarder::ComForwarder;

use async_trait::async_trait;
use std::io::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub struct ChangeUserForwarder;

#[async_trait]
impl<R, W> ComForwarder<R, W> for ChangeUserForwarder
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin,
{
    async fn forward(
        &self,
        client_reader: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        handshake_response: &HandshakeResponse,
    ) -> Result<Option<Packet>, Error> {
        loop {
            let rsp_pkt = self
                .forward_one_packet(client_writer, backend_reader, true)
                .await?;

            if rsp_pkt.is_err_packet() {
                parse_err_packet!(
                    handshake_response.client_flag,
                    rsp_pkt,
                    "forward_change_user ERR"
                );
                return Err(Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Error packet from backend",
                ));
            } else if rsp_pkt.is_ok_packet() {
                return Ok(Some(rsp_pkt));
            } else {
                // backend sends a switch-auth request. forward auth data to backend.
                let pkt = self
                    .forward_one_packet(backend_writer, client_reader, true)
                    .await?;
                // debug!("ProxySrv ChangeUserForwarder client_rsp={}", pkt.len());
                if pkt.is_err_packet() {
                    parse_err_packet!(
                        handshake_response.client_flag,
                        pkt,
                        "forward_change_user ERR"
                    );
                    return Err(Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Error packet from client",
                    ));
                }
            }
        }
    }
}
