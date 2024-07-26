use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use tokio::io::AsyncWrite;

/// [`CmdHandler`] implements a storage backend compatible with the MySQL wire protocol.
#[async_trait::async_trait]
pub trait CmdHandler: Send + Sync {
    async fn auth(
        &mut self,
        auth_plugin: &str,
        user: &[u8],
        salt: &[u8],
        auth_data: &[u8],
    ) -> Result<bool, std::io::Error>;

    async fn on_init<'a, W>(
        &mut self,
        database: &[u8],
        pkt_writer: &mut PacketWriter<W>,
    ) -> Result<(), std::io::Error>
    where
        W: AsyncWrite + Send + Unpin;

    async fn on_prepare<'a, W>(
        &mut self,
        packet: &[u8],
        pkt_writer: &mut PacketWriter<W>,
    ) -> Result<(), std::io::Error>
    where
        W: AsyncWrite + Send + Unpin;

    async fn on_query<'a, W>(
        &mut self,
        packet: &[u8],
        pkt_writer: &mut PacketWriter<W>,
    ) -> Result<(), std::io::Error>
    where
        W: AsyncWrite + Send + Unpin;

    async fn on_execute<'a, W>(
        &mut self,
        packet: &[u8],
        pkt_writer: &mut PacketWriter<W>,
    ) -> Result<(), std::io::Error>
    where
        W: AsyncWrite + Send + Unpin;
}
