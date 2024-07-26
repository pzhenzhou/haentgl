use crate::protocol::mysql::basic::{Column, OkPacket};
use crate::protocol::mysql::charset::DEFAULT_COLLATION_ID;
use crate::protocol::mysql::constants::AuthPluginName::AuthNativePassword;
use crate::protocol::mysql::constants::{CommandCode, AUTH_PLUGIN_DATA_PART_1_LENGTH};
use crate::protocol::mysql::error_codes::ErrorKind;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;

use crate::server::default_capabilities;
use byteorder::{LittleEndian, WriteBytesExt};
use mysql_common::constants::{CapabilityFlags, StatusFlags};
use mysql_common::io::WriteMysqlExt;
use rustls::server::ServerConfig;
use std::io::{self, Write};
use tokio::io::AsyncWrite;
use tokio_rustls::rustls;

pub const CONN_ID: u32 = u32::from_le_bytes([0x08, 0x00, 0x00, 0x00]);

pub async fn write_err_packet<W: AsyncWrite + Unpin>(
    err: ErrorKind,
    msg: &[u8],
    w: &mut PacketWriter<W>,
) -> io::Result<()> {
    w.write_u8(0xff)?;
    w.write_u16::<LittleEndian>(err as u16)?;
    w.write_u8(b'#')?;
    w.write_all(err.sqlstate())?;
    w.write_all(msg)?;
    w.end_packet().await
}

pub async fn write_eof_packet<W: AsyncWrite + Unpin>(
    w: &mut PacketWriter<W>,
    s: StatusFlags,
) -> io::Result<()> {
    w.write_all(&[0xfe, 0x00, 0x00])?;
    w.write_u16::<LittleEndian>(s.bits())?;
    w.end_packet().await
}

pub async fn write_ok_packet<W: AsyncWrite + Unpin>(
    w: &mut PacketWriter<W>,
    rows: u64,
    last_insert_id: u64,
    s: StatusFlags,
) -> io::Result<()> {
    // [7, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0]
    w.write_u8(0x00)?; // OK packet type
    w.write_lenenc_int(rows)?;
    w.write_lenenc_int(last_insert_id)?;
    w.write_u16::<LittleEndian>(s.bits())?;
    w.write_all(&[0x00, 0x00])?; // no warnings
    w.end_packet().await
}

pub async fn write_ok_packet_with_client_flags<W: AsyncWrite + Unpin>(
    w: &mut PacketWriter<W>,
    client_capabilities: CapabilityFlags,
    ok_packet: OkPacket,
) -> io::Result<()> {
    w.write_u8(ok_packet.header)?; // OK packet type
    w.write_lenenc_int(ok_packet.affected_rows)?;
    w.write_lenenc_int(ok_packet.last_insert_id)?;
    if client_capabilities.contains(CapabilityFlags::CLIENT_PROTOCOL_41) {
        w.write_u16::<LittleEndian>(ok_packet.status_flags.bits())?;
        w.write_all(&[0x00, 0x00])?; // no warnings
    } else if client_capabilities.contains(CapabilityFlags::CLIENT_TRANSACTIONS) {
        w.write_u16::<LittleEndian>(ok_packet.status_flags.bits())?;
    }

    if client_capabilities.contains(CapabilityFlags::CLIENT_SESSION_TRACK) {
        w.write_lenenc_str(ok_packet.info.as_bytes())?;
        if ok_packet
            .status_flags
            .contains(StatusFlags::SERVER_SESSION_STATE_CHANGED)
        {
            w.write_lenenc_str(ok_packet.session_state_info.as_bytes())?;
        }
    } else {
        w.write_all(ok_packet.info.as_bytes())?;
    }
    w.end_packet().await
}

pub async fn write_prepare_ok<'a, PI, CI, W>(
    id: u32,
    params: PI,
    columns: CI,
    w: &mut PacketWriter<W>,
    client_capabilities: CapabilityFlags,
) -> io::Result<()>
where
    PI: IntoIterator<Item = &'a Column>,
    CI: IntoIterator<Item = &'a Column>,
    <PI as IntoIterator>::IntoIter: ExactSizeIterator,
    <CI as IntoIterator>::IntoIter: ExactSizeIterator,
    W: AsyncWrite + Unpin,
{
    let pi = params.into_iter();
    let ci = columns.into_iter();

    // first, write out COM_STMT_PREPARE_OK
    w.write_u8(0x00)?;
    w.write_u32::<LittleEndian>(id)?;
    w.write_u16::<LittleEndian>(ci.len() as u16)?;
    w.write_u16::<LittleEndian>(pi.len() as u16)?;
    w.write_u8(0x00)?;
    w.write_u16::<LittleEndian>(0)?; // number of warnings
    w.end_packet().await?;

    if pi.len() > 0 {
        write_column_definitions_41(pi, w, client_capabilities, false).await?;
    }
    if ci.len() > 0 {
        write_column_definitions_41(ci, w, client_capabilities, false).await?;
    }
    Ok(())
}

pub async fn write_column_definitions<'a, I, W>(
    i: I,
    w: &mut PacketWriter<W>,
    client_capabilities: CapabilityFlags,
) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Column>,
    <I as IntoIterator>::IntoIter: ExactSizeIterator,
    W: AsyncWrite + Unpin,
{
    let i = i.into_iter();
    w.write_lenenc_int(i.len() as u64)?;
    w.end_packet().await?;
    write_column_definitions_41(i, w, client_capabilities, false).await
}

// works for Protocol::ColumnDefinition41 is set
// see: https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_com_query_response_text_resultset_column_definition.html
pub async fn write_column_definitions_41<'a, I, W>(
    i: I,
    w: &mut PacketWriter<W>,
    client_capabilities: CapabilityFlags,
    is_com_field_list: bool,
) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Column>,
    W: AsyncWrite + Unpin,
{
    for c in i {
        // let c = c.borrow();
        // use crate::myc::constants::UTF8_GENERAL_CI;
        w.write_lenenc_str(b"def")?;
        w.write_lenenc_str(b"")?;
        w.write_lenenc_str(c.table.as_bytes())?;
        w.write_lenenc_str(b"")?;
        w.write_lenenc_str(c.column.as_bytes())?;
        w.write_lenenc_str(b"")?;
        w.write_lenenc_int(0xC)?;
        w.write_u16::<LittleEndian>(33)?;
        w.write_u32::<LittleEndian>(1024)?;
        w.write_u8(c.column_type as u8)?;
        w.write_u16::<LittleEndian>(c.column_flags.bits())?;
        w.write_all(&[0x00])?; // decimals
        w.write_all(&[0x00, 0x00])?; // unused

        if is_com_field_list {
            w.write_all(&[0xfb])?;
        }
        w.end_packet().await?;
    }

    if !client_capabilities.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF) {
        write_eof_packet(w, mysql_common::constants::StatusFlags::empty()).await
    } else {
        Ok(())
    }
}

pub async fn write_err<W: AsyncWrite + Unpin>(
    err: ErrorKind,
    msg: &[u8],
    w: &mut PacketWriter<W>,
) -> io::Result<()> {
    w.write_u8(0xff)?;
    w.write_u16::<LittleEndian>(err as u16)?;
    w.write_u8(b'#')?;
    w.write_all(err.sqlstate())?;
    w.write_all(msg)?;
    w.end_packet().await
}

pub async fn write_initial_handshake<W: AsyncWrite + Unpin>(
    writer: &mut PacketWriter<W>,
    conn_id: u64,
    scramble: [u8; 20],
    server_version: &[u8],
    #[cfg(feature = "tls")] tls_conf: &Option<std::sync::Arc<ServerConfig>>,
) -> io::Result<()> {
    writer.write_all(&[10])?; // protocol 10

    writer.write_all(server_version)?;
    writer.write_all(&[0x00])?;
    // connection_id (4 bytes)
    let conn_id_bytes = &[
        conn_id as u8,
        (conn_id >> 8) as u8,
        (conn_id >> 16) as u8,
        (conn_id >> 24) as u8,
    ];
    writer.write_all(conn_id_bytes)?;
    let server_capabilities = default_capabilities();
    #[cfg(feature = "tls")]
    let server_capabilities = if tls_conf.is_some() {
        server_capabilities | CapabilityFlags::CLIENT_SSL
    } else {
        server_capabilities
    };
    let server_capabilities_vec = server_capabilities.bits().to_le_bytes();

    writer.write_all(&scramble[0..AUTH_PLUGIN_DATA_PART_1_LENGTH])?; // auth-plugin-data-part-1
    writer.write_all(&[0x00])?;

    writer.write_all(&server_capabilities_vec[..2])?; // The lower 2 bytes of the Capabilities Flags, 0x42

    writer.write_all(&DEFAULT_COLLATION_ID.to_le_bytes())?; // UTF8_GENERAL_CI
    writer.write_all(&StatusFlags::SERVER_STATUS_AUTOCOMMIT.bits().to_le_bytes())?; // status_flags
    writer.write_all(&server_capabilities_vec[2..4])?; // The upper 2 bytes of the Capabilities Flags

    writer.write_all(&((scramble.len() + 1) as u8).to_le_bytes())?;

    writer.write_all(&[0x00; 10][..])?; // 10 bytes filler
                                        // Part2 of the auth_plugin_data
                                        // $len=MAX(13, length of auth-plugin-data - 8)
    writer.write_all(&scramble[AUTH_PLUGIN_DATA_PART_1_LENGTH..])?; // 12 bytes
    writer.write_all(&[0x00])?;

    // Plugin name
    writer.write_all(AuthNativePassword.as_ref().as_bytes())?;
    writer.write_all(&[0x00])?;
    writer.end_packet().await?;
    writer.flush_all().await
}

pub async fn write_query_request<W: AsyncWrite + Unpin>(
    w: &mut PacketWriter<W>,
    data: &[u8],
) -> io::Result<()> {
    let query_com = CommandCode::ComQuery as u8;
    w.write_u8(query_com)?;
    w.write_all(data)?;
    w.end_packet().await
}

pub async fn write_reset_connection<W: AsyncWrite + Unpin>(
    w: &mut PacketWriter<W>,
) -> io::Result<()> {
    let reset_com = CommandCode::ComResetConnection as u8;
    w.write_u8(reset_com)?;
    w.write_u32::<LittleEndian>(CONN_ID)?;
    w.end_packet().await?;
    w.flush_all().await
}
