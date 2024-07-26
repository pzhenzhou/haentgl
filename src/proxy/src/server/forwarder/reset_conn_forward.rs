use crate::async_packet_read;
use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::writers::write_reset_connection;
use crate::protocol::mysql::packet::{packet_reader, Packet};
use crate::server::forwarder::ComForwarder;

use async_trait::async_trait;
use packet_reader::PacketReader;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

pub struct ResetConnForwarder;

#[async_trait]
impl<R, W> ComForwarder<R, W> for ResetConnForwarder
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin,
{
    async fn forward(
        &self,
        _: &mut PacketReader<R>,
        _: &mut PacketWriter<W>,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        _: &HandshakeResponse,
    ) -> Result<Option<Packet>, std::io::Error> {
        write_reset_connection(backend_writer).await?;
        let (_be_seq, _be_rsp_pkt) = async_packet_read!(backend_reader);

        Ok(None)
    }
}
