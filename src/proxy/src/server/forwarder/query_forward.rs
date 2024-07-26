use crate::parse_err_packet;
use crate::protocol::mysql::basic::{eof_server_status, ok_packet, HandshakeResponse};
use crate::protocol::mysql::constants::CommandCode;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::Packet;
use crate::server::forwarder::ComForwarder;

use async_trait::async_trait;
use byteorder::ByteOrder;
use mysql_common::constants::{CapabilityFlags, StatusFlags};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub struct QueryForwarder {
    pub com_code: CommandCode,
}

impl QueryForwarder {
    async fn forward_query<W>(
        &self,
        handshake: &HandshakeResponse,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
    ) -> Result<(), std::io::Error>
    where
        W: AsyncWrite + Send + Unpin,
    {
        let capabilities = handshake.client_flag;
        loop {
            let response_packet = self
                .forward_one_packet(client_writer, backend_reader, false)
                .await?;
            // debug!(
            //     "ProxySrv forward_query start header = {:?}",
            //     response_packet[0]
            // );
            let status_flag = if response_packet.is_ok_packet() {
                client_writer.flush_all().await?;
                let (_, ok_pkt) = ok_packet(&response_packet, capabilities).unwrap();
                ok_pkt.status_flags
            } else if response_packet.is_err_packet() {
                parse_err_packet!(capabilities, response_packet, "forward_query ERR");
                client_writer.flush_all().await?;
                return Ok(());
            } else if response_packet.is_local_in_file_packet() {
                //TODO: supported it
                unimplemented!("not supported LocalInFileHeader");
            } else {
                self.forward_result(handshake, backend_reader, client_writer)
                    .await?
            };
            if !status_flag.contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS) {
                break;
            }
        }
        Ok(())
    }

    async fn forward_result<W>(
        &self,
        handshake: &HandshakeResponse,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
    ) -> Result<StatusFlags, std::io::Error>
    where
        W: AsyncWrite + Send + Unpin,
    {
        let client_capability = handshake.client_flag;
        let client_deprecate_eof =
            client_capability.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF);
        if !client_deprecate_eof {
            let resp_packet = loop {
                let response_packet = self
                    .forward_one_packet(client_writer, backend_reader, false)
                    .await?;
                if response_packet.is_eof_packet() {
                    break response_packet;
                }
            };
            let status_code = byteorder::LittleEndian::read_u16(&resp_packet[3..]);
            if let Some(status_flags) = StatusFlags::from_bits(status_code) {
                if status_flags.contains(StatusFlags::SERVER_STATUS_CURSOR_EXISTS) {
                    // debug!("ProxySrv forward_result SERVER_STATUS_CURSOR_EXISTS ");
                    client_writer.flush_all().await?;
                    return Ok(status_flags);
                }
            }
        }
        self.forward_until_result_end(handshake, backend_reader, client_writer)
            .await
    }

    async fn forward_until_result_end<W>(
        &self,
        handshake: &HandshakeResponse,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
    ) -> Result<StatusFlags, std::io::Error>
    where
        W: AsyncWrite + Send + Unpin,
    {
        let client_capability = handshake.client_flag;
        let client_deprecate_eof =
            client_capability.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF);
        loop {
            let response_packet = self
                .forward_one_packet(client_writer, backend_reader, false)
                .await?;

            if response_packet.is_err_packet() {
                parse_err_packet!(
                    client_capability,
                    response_packet,
                    "ComQuery forward_until_result_end ERR"
                );
                client_writer.flush_all().await?;
                break;
            }
            if !client_deprecate_eof {
                if response_packet.is_eof_packet() {
                    client_writer.flush_all().await?;
                    let (_, status_flag) = eof_server_status(&response_packet).unwrap();
                    return Ok(status_flag);
                }
            } else if response_packet.is_result_set_eof_packet() {
                let (_, ok_pkt) = ok_packet(&response_packet, client_capability).unwrap();
                client_writer.flush_all().await?;
                return Ok(ok_pkt.status_flags);
            }
        }
        Ok(StatusFlags::default())
    }
}

#[async_trait]
impl<R, W> ComForwarder<R, W> for QueryForwarder
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin,
{
    async fn forward(
        &self,
        _: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        _: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        handshake: &HandshakeResponse,
    ) -> Result<Option<Packet>, std::io::Error> {
        let query_rs = match self.com_code {
            CommandCode::ComQuery | CommandCode::ComStmtExecute | CommandCode::ComProcessInfo => {
                self.forward_query(handshake, backend_reader, client_writer)
                    .await
            }
            CommandCode::ComFieldList | CommandCode::ComStmtFetch => self
                .forward_until_result_end(handshake, backend_reader, client_writer)
                .await
                .map(|_| ()),
            _ => {
                unreachable!("not supported com_code = {:?}", self.com_code);
            }
        };
        match query_rs {
            Ok(()) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
