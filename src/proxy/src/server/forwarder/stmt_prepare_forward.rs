use crate::parse_err_packet;
use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::Packet;
use crate::server::forwarder::ComForwarder;

use crate::protocol::mysql::constants::CommandCode;
use async_trait::async_trait;
use byteorder::ByteOrder;
use mysql_common::constants::CapabilityFlags;
use std::io::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub struct StmtPrepareForwarder {
    pub com_code: CommandCode,
    pub request: Packet,
}

impl StmtPrepareForwarder {
    async fn forward_prepare_stmt<W>(
        &self,
        client_writer: &mut PacketWriter<W>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        handshake: &HandshakeResponse,
    ) -> Result<Option<Packet>, Error>
    where
        W: AsyncWrite + Send + Unpin,
    {
        let packet = self
            .forward_one_packet(client_writer, backend_reader, false)
            .await?;
        let capabilities = handshake.client_flag;
        let is_client_deprecate_eof = capabilities.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF);
        if packet.is_err_packet() {
            parse_err_packet!(capabilities, packet, "stmt_prepare_forward ERR");
            if let Err(e) = client_writer.flush_all().await {
                Err(e)
            } else {
                Ok(None)
            }
        } else if packet.is_ok_packet() {
            let column = byteorder::LittleEndian::read_u16(&packet[5..]);
            let params = byteorder::LittleEndian::read_u16(&packet[7..]);
            let mut expected_packets = column + params;
            if !is_client_deprecate_eof {
                if column > 0 {
                    expected_packets += 1
                }
                if params > 0 {
                    expected_packets += 1
                }
            }
            for _idx in 0..expected_packets {
                self.forward_one_packet(client_writer, backend_reader, false)
                    .await?;
            }
            match client_writer.flush_all().await {
                Ok(()) => Ok(None),
                Err(e) => Err(e),
            }
        } else {
            unreachable!()
        }
    }

    async fn forward_close_stmt(&self) -> Result<Option<Packet>, Error> {
        let _status = self.request[0];
        let _stmt_id = byteorder::LittleEndian::read_u32(&self.request[1..5]);
        // TODO update prepared statement status
        Ok(None)
    }
}

#[async_trait]
impl<R, W> ComForwarder<R, W> for StmtPrepareForwarder
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
    ) -> Result<Option<Packet>, Error> {
        match self.com_code {
            CommandCode::ComStmtPrepare => {
                self.forward_prepare_stmt(client_writer, backend_reader, handshake)
                    .await
            }
            CommandCode::ComStmtClose => self.forward_close_stmt().await,
            _ => unreachable!(),
        }
    }
}
