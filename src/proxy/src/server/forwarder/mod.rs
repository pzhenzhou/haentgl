pub mod change_user_forward;
pub mod query_forward;
pub mod reset_conn_forward;
pub mod stmt_prepare_forward;

use crate::async_packet_read;
use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::constants::AuthPluginName::UnKnowPluginName;
use crate::protocol::mysql::constants::CommandCode;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::Packet;

use async_trait::async_trait;
use my_common::io::ParseBuf;
use my_common::packets::{AuthPlugin, ComChangeUserMoreData};
use my_common::proto::{MyDeserialize, MySerialize};
use mysql_common as my_common;
use std::borrow::Cow;
use std::io::{Error, Write};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

#[async_trait]
pub trait ComForwarder<R, W>: Send + Sync
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin,
{
    async fn forward_one_packet(
        &self,
        dest_writer: &mut PacketWriter<W>,
        src_reader: &mut PacketReader<R>,
        is_flush: bool,
    ) -> Result<Packet, Error> {
        let (seq, src_rsp) = async_packet_read!(src_reader);
        dest_writer.set_seq(seq);
        dest_writer.write_all(&src_rsp)?;
        dest_writer.end_packet().await?;
        if is_flush {
            dest_writer.flush_all().await?
        }
        Ok(src_rsp)
    }

    /// The packets from the client are sent to the backend of the proxy;
    /// since SqlProxy has connection pooling, we need a special handle for ComQuit ComChangeUser
    /// because of the connection lifecycle management.
    async fn write_to_backend(
        &self,
        seq: u8,
        com_code: CommandCode,
        handshake_response: &HandshakeResponse,
        client_packet: Packet,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
    ) -> Result<(), Error> {
        let pkt_option = match com_code {
            CommandCode::ComQuit => None,
            CommandCode::ComChangeUser => {
                let client_capability = handshake_response.client_flag;
                let com_change_user = mysql_common::packets::ComChangeUser::deserialize(
                    client_capability,
                    &mut ParseBuf(&client_packet),
                )
                    .unwrap();
                let change_user_more_data = ComChangeUserMoreData::new(
                    handshake_response.collation,
                )
                    .with_auth_plugin(Some(AuthPlugin::Other(Cow::from(
                        UnKnowPluginName.as_ref().as_bytes(),
                    ))));

                let updated_change_user_com = com_change_user
                    .clone()
                    .with_auth_plugin_data(None::<&[u8]>)
                    .with_more_data(Some(change_user_more_data));
                let mut new_client_packet = vec![];
                updated_change_user_com.serialize(&mut new_client_packet);
                Some(Packet::from_vec(new_client_packet))
            }
            _ => Some(client_packet),
        };
        if let Some(pkt) = pkt_option {
            backend_writer.set_seq(seq);
            backend_writer.write_all(&pkt)?;
            backend_writer.end_packet().await?;
            backend_writer.flush_all().await
        } else {
            Ok(())
        }
    }

    /// Forwarding logic for specific commands
    async fn forward(
        &self,
        client_reader: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        handshake_response: &HandshakeResponse,
    ) -> Result<Option<Packet>, Error>;
}

pub(crate) struct GenericComForwarder;

#[async_trait]
impl<R, W> ComForwarder<R, W> for GenericComForwarder
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
        _: &HandshakeResponse,
    ) -> Result<Option<Packet>, Error> {
        Ok(self
            .forward_one_packet(client_writer, backend_reader, true)
            .await
            .map(Some)?)
    }
}
