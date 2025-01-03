use crate::protocol::mysql::constants::CommandCode as ComInfo;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;

use hashbrown::HashMap;
use mysql_common::constants::{CapabilityFlags, StatusFlags};
use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite};
use winnow::binary::{le_u16, le_u32, le_u8};
use winnow::combinator::{alt, preceded, rest};
use winnow::prelude::*;
use winnow::token::{literal, take, take_until};
use winnow::{Parser, Partial};

#[derive(Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Query(&'a [u8]),
    ListFields(&'a [u8]),
    Close(u32),
    Prepare(&'a [u8]),
    Init(&'a [u8]),
    Execute {
        stmt: u32,
        params: &'a [u8],
    },
    SendLongData {
        stmt: u32,
        param: u16,
        data: &'a [u8],
    },
    Ping,
    Quit,
}

#[pin_project]
pub struct PacketIO<R, W> {
    pub reader: PacketReader<R>,
    pub writer: PacketWriter<W>,
}

impl<R: AsyncRead + Send + Unpin, W: AsyncWrite + Send + Unpin> PacketIO<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: PacketReader::new(reader),
            writer: PacketWriter::new(writer),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// This column's associated table.
    ///
    /// Note: that this is *technically* the table's alias.
    pub table: String,
    /// This column's name.
    ///
    /// Note: that this is *technically* the column's alias.
    pub column: String,
    /// This column's type>
    pub column_type: mysql_common::constants::ColumnType,
    /// Any flags associated with this column.
    ///
    /// Of particular interest are `ColumnFlags::UNSIGNED_FLAG` and `ColumnFlags::NOT_NULL_FLAG`.
    pub column_flags: mysql_common::constants::ColumnFlags,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OkPacket {
    /// header
    pub header: u8,
    /// affected rows in update/insert
    pub affected_rows: u64,
    /// insert_id in update/insert
    pub last_insert_id: u64,
    /// StatusFlags associated with this query
    pub status_flags: StatusFlags,
    /// Warnings
    pub warnings: u16,
    /// Extra information
    pub info: String,
    /// session state change information
    pub session_state_info: String,
}

/// `HandshakeResponse` represents the client's reply to the handshake response packet.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HandshakeResponse {
    pub client_flag: CapabilityFlags,
    pub max_packet_len: u32,
    pub collation: u16,
    pub tenant_key: Option<Vec<u8>>,
    pub username: Option<Vec<u8>>,
    pub auth_response: Vec<u8>,
    pub auth_plugin: Vec<u8>,
    pub database: Option<Vec<u8>>,
    pub connect_attributes: Option<HashMap<String, String>>,
}

impl HandshakeResponse {
    pub fn full_tenant_name(&self) -> String {
        let tenant_encode_str = match &self.tenant_key {
            Some(tenant_key) => std::str::from_utf8(tenant_key).unwrap(),
            _ => "NONE",
        };
        let user_name = match &self.username {
            Some(username) => {
                let username_str = std::str::from_utf8(username).unwrap();
                username_str
            }
            None => "NONE",
        };
        format!("{}.{}", tenant_encode_str, user_name)
    }

    pub fn db_user_string(&self) -> String {
        match &self.username {
            Some(username) => String::from_utf8_lossy(username).to_string(),
            None => "_NONE".to_string(),
        }
    }

    pub fn change_tenant_if_need(&mut self) {
        let is_not_static = std::env::var(crate::server::PROXY_ENV_SYNC_ROUTER)
            .unwrap_or("false".to_string())
            .eq("true");

        if is_not_static {
            if let Some(conn_user) = &self.username {
                let conn_user_slice = conn_user.as_slice();
                if let Some(pos) = conn_user_slice.iter().rposition(|&c| c == b'.') {
                    let tenant_keys = &conn_user[..pos];
                    let user_name = &conn_user[pos + 1..];
                    self.tenant_key = Some(tenant_keys.to_vec());
                    self.username = Some(user_name.to_vec());
                }
            }
        }
    }
}

fn read_length_encoded_string(i: &[u8]) -> IResult<&[u8], &[u8]> {
    let (input, len) = read_length_encoded_number(i)?;
    take(len).parse_peek(input)
}

pub fn eof_server_status(i: &[u8]) -> IResult<&[u8], StatusFlags> {
    let status_flag_slice = &i[3..i.len()];
    let (i, status_flags_code) = le_u16.parse_peek(status_flag_slice)?;
    Ok((i, StatusFlags::from_bits_truncate(status_flags_code)))
}

pub fn read_length_encoded_number(i: &[u8]) -> IResult<&[u8], u64> {
    let (i, b) = le_u8.parse_peek(i)?;
    let r_size: usize = match b {
        0xfb => return Ok((i, 0)),
        0xfc => 2,
        0xfd => 3,
        0xfe => 8,
        _ => return Ok((i, b as u64)),
    };
    let mut bytes = [0u8; 8];
    let (i, b) = take(r_size).parse_peek(i)?;
    bytes[..r_size].copy_from_slice(b);
    Ok((i, u64::from_le_bytes(bytes)))
}

pub fn ok_packet(i: &[u8], capabilities: CapabilityFlags) -> winnow::IResult<&[u8], OkPacket> {
    let (i, header) = le_u8.parse_peek(i)?;
    let (i, affected_rows) = read_length_encoded_number(i)?;
    let (i, last_insert_id) = read_length_encoded_number(i)?;
    let (i, status_flags_value) = le_u16.parse_peek(i)?;

    let status_flags = StatusFlags::from_bits_retain(status_flags_value);
    // info!("from bytes to OKPacket header={header},status_flag={status_flags:?}");
    let (i, warnings) = le_u16.parse_peek(i)?;
    let (info, session_state_info) =
        if !i.is_empty() && capabilities.contains(CapabilityFlags::CLIENT_SESSION_TRACK) {
            let (i, info_size) = read_length_encoded_number(i)?;
            let (i, info) = take(info_size).parse_peek(i)?;

            let session_state_info =
                if status_flags.contains(StatusFlags::SERVER_SESSION_STATE_CHANGED) {
                    let (i, s_t_size) = read_length_encoded_number(i)?;
                    let (_i, session_state_info) = take(s_t_size).parse_peek(i)?;
                    std::str::from_utf8(session_state_info).unwrap_or("")
                } else {
                    ""
                };
            (
                std::str::from_utf8(info).unwrap_or("").to_string(),
                session_state_info.to_string(),
            )
        } else {
            ("".to_string(), "".to_string())
        };

    Ok((
        i,
        OkPacket {
            header,
            affected_rows,
            last_insert_id,
            status_flags,
            warnings,
            info,
            session_state_info,
        },
    ))
}

pub fn client_handshake_response(
    i: &[u8],
    is_after_tls: bool,
) -> IResult<&[u8], HandshakeResponse> {
    let (i, capability_flags) = le_u16.parse_peek(i)?;
    let mut capabilities = CapabilityFlags::from_bits_truncate(capability_flags as u32);
    if capabilities.contains(CapabilityFlags::CLIENT_PROTOCOL_41) {
        // HandshakeResponse41
        let (i, cap2) = le_u16.parse_peek(i)?;
        let cap = (cap2 as u32) << 16 | capability_flags as u32;

        capabilities = CapabilityFlags::from_bits_truncate(cap);

        let (i, max_packet_len) = le_u32.parse_peek(i)?;
        let (i, collation) = take(1u8).parse_peek(i)?;

        let (i, _) = take(23u8).parse_peek(i)?;

        if !is_after_tls && capabilities.contains(CapabilityFlags::CLIENT_SSL) {
            return Ok((
                i,
                HandshakeResponse {
                    client_flag: capabilities,
                    max_packet_len,
                    collation: u16::from(collation[0]),
                    tenant_key: None,
                    username: None,
                    auth_response: vec![],
                    auth_plugin: vec![],
                    database: None,
                    connect_attributes: None,
                },
            ));
        }

        let (i, username) = if is_after_tls || !capabilities.contains(CapabilityFlags::CLIENT_SSL) {
            let (i, user) = take_until(1.., "\0").parse_peek(i)?;
            let (i, _) = literal(b"\0").parse_peek(i)?;
            (i, Some(user.to_owned()))
        } else {
            (i, None)
        };
        let (i, auth_response) =
            if capabilities.contains(CapabilityFlags::CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA) {
                let (i, size) = read_length_encoded_number(i)?;
                take(size).parse_peek(i)?
            } else if capabilities.contains(CapabilityFlags::CLIENT_SECURE_CONNECTION) {
                let (i, size) = le_u8.parse_peek(i)?;
                take(size).parse_peek(i)?
            } else {
                take_until(1.., "\0").parse_peek(i)?
            };

        let (i, db) =
            if capabilities.contains(CapabilityFlags::CLIENT_CONNECT_WITH_DB) && !i.is_empty() {
                let (i, db) = take_until(1.., "\0").parse_peek(i)?;
                let (i, _) = literal(b"\0").parse_peek(i)?;
                (i, Some(db))
            } else {
                (i, None)
            };

        let (i, auth_plugin) =
            if capabilities.contains(CapabilityFlags::CLIENT_PLUGIN_AUTH) && !i.is_empty() {
                let (i, auth_plugin) = take_until(1.., "\0").parse_peek(i)?;

                let (i, _) = literal("\0").parse_peek(i)?;
                (i, auth_plugin)
            } else {
                (i, &b""[..])
            };

        let (i, connect_attributes) =
            if capabilities.contains(CapabilityFlags::CLIENT_CONNECT_ATTRS) && !i.is_empty() {
                let (i, data_len) = read_length_encoded_number(i)?;
                let (i, data) = take(data_len).parse_peek(i)?;
                let mut input = data;
                let mut connect_attributes = HashMap::new();
                while !input.is_empty() {
                    let (remaining, k) = read_length_encoded_string(input)?;
                    // Parse value
                    let (remaining, v) = read_length_encoded_string(remaining)?;
                    let conn_attr_key = std::str::from_utf8(k).unwrap().to_string();
                    let conn_attr_val = std::str::from_utf8(v).unwrap().to_string();
                    connect_attributes.insert(conn_attr_key, conn_attr_val);
                    input = remaining;
                }
                (i, Some(connect_attributes))
            } else {
                (i, None)
            };

        Ok((
            i,
            HandshakeResponse {
                client_flag: capabilities,
                max_packet_len,
                collation: u16::from(collation[0]),
                tenant_key: None,
                username,
                auth_response: auth_response.to_vec(),
                auth_plugin: auth_plugin.to_vec(),
                database: db.map(|c| c.to_vec()),
                connect_attributes,
            },
        ))
    } else {
        // HandshakeResponse320
        let (i, max_packet_len_v1) = le_u16.parse_peek(i)?;
        let (i, max_packet_len_v2) = le_u8.parse_peek(i)?;
        let max_packet_len = (max_packet_len_v2 as u32) << 16 | max_packet_len_v1 as u32;
        let (i, username) = take_until(1.., "\0").parse_peek(i)?;
        let (i, _) = literal(b"\0").parse_peek(i)?;

        let (i, auth_response, db) =
            if capabilities.contains(CapabilityFlags::CLIENT_CONNECT_WITH_DB) {
                let (i, auth_response) = literal(b"\0").parse_peek(i)?;
                let (i, _) = literal(b"\0").parse_peek(i)?;

                let (i, db) = literal(b"\0").parse_peek(i)?;
                let (i, _) = literal(b"\0").parse_peek(i)?;

                (i, auth_response, Some(db))
            } else {
                (&b""[..], i, None)
            };

        Ok((
            i,
            HandshakeResponse {
                client_flag: capabilities,
                max_packet_len,
                collation: 0,
                tenant_key: None,
                username: Some(username.to_vec()),
                auth_response: auth_response.to_vec(),
                auth_plugin: vec![],
                database: db.map(|c| c.to_vec()),
                connect_attributes: None,
            },
        ))
    }
}

fn send_long_data(i: Partial<&[u8]>) -> IResult<Partial<&[u8]>, Command<'_>> {
    let (remaining, stmt) = le_u32.parse_peek(i)?;
    let (remaining, param) = le_u16.parse_peek(remaining)?;
    let data = *remaining; // Get the remaining data slice
    Ok((remaining, Command::SendLongData { stmt, param, data }))
}

fn execute(i: Partial<&[u8]>) -> IResult<Partial<&[u8]>, Command<'_>> {
    let (remaining, stmt) = le_u32.parse_peek(i)?;
    let (remaining, _flags) = take(1u8).parse_peek(remaining)?;
    let (remaining, _iterations) = le_u32.parse_peek(remaining)?;
    let params = *remaining;
    Ok((remaining, Command::Execute { stmt, params }))
}

pub fn from_packet(pkt: &[u8]) -> IResult<Partial<&[u8]>, Command<'_>> {
    alt((
        preceded(literal([ComInfo::ComQuery as u8]), rest).map(Command::Query),
        preceded(literal([ComInfo::ComFieldList as u8]), rest).map(Command::ListFields),
        preceded(literal([ComInfo::ComInitDB as u8]), rest).map(Command::Init),
        preceded(literal([ComInfo::ComStmtPrepare as u8]), rest).map(Command::Prepare),
        preceded(
            literal([ComInfo::ComStmtExecute as u8]),
            winnow::unpeek(execute),
        ),
        preceded(
            literal([ComInfo::ComStmtSendLongData as u8]),
            winnow::unpeek(send_long_data),
        ),
        preceded(literal([ComInfo::ComStmtClose as u8]), le_u32).map(Command::Close),
        literal([ComInfo::ComQuit as u8]).map(|_| Command::Quit),
        literal([ComInfo::ComPing as u8]).map(|_| Command::Ping),
    ))
    .parse_peek(Partial::new(pkt))
}

#[cfg(test)]
mod tests {
    use crate::protocol::mysql::basic::client_handshake_response;
    use crate::protocol::mysql::charset::collation_names;
    use crate::protocol::mysql::packet::packet_reader::PacketReader;
    use mysql_common::constants::CapabilityFlags;
    use std::io::Cursor;

    #[test]
    pub fn test_handshake_parse() {
        let bytes = &[
            0x5b, 0x00, 0x00, 0x01, 0x8d, 0xa6, 0xff, 0x09, 0x00, 0x00, 0x00, 0x01, 0x21, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x65, 0x66, 0x61, 0x75, 0x6c,
            0x74, 0x00, 0x14, 0xf7, 0xd1, 0x6c, 0xe9, 0x0d, 0x2f, 0x34, 0xb0, 0x2f, 0xd8, 0x1d,
            0x18, 0xc7, 0xa4, 0xe8, 0x98, 0x97, 0x67, 0xeb, 0xad, 0x64, 0x65, 0x66, 0x61, 0x75,
            0x6c, 0x74, 0x00, 0x6d, 0x79, 0x73, 0x71, 0x6c, 0x5f, 0x6e, 0x61, 0x74, 0x69, 0x76,
            0x65, 0x5f, 0x70, 0x61, 0x73, 0x73, 0x77, 0x6f, 0x72, 0x64, 0x00,
        ];
        let cursor = Cursor::new(&bytes[..]);
        let mut packet_reader = PacketReader::new(cursor);
        let (_, packet) = packet_reader.next_read().unwrap().unwrap();

        let handshake_rs = client_handshake_response(&packet, false);
        assert!(handshake_rs.is_ok());
        let handshake = handshake_rs.unwrap().1;
        println!("handshakeRsp = {handshake:?}");
        assert!(handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_LONG_PASSWORD));
        assert!(handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_MULTI_RESULTS));
        assert!(handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_CONNECT_WITH_DB));
        assert!(handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_DEPRECATE_EOF));
        assert_eq!(
            handshake.collation,
            *collation_names().get("utf8_general_ci").unwrap() as u16
        );
        assert_eq!(handshake.username.unwrap(), &b"default"[..]);
        assert_eq!(handshake.max_packet_len, 16777216);
    }

    #[test]
    pub fn test_handshake_parse_with_ssl() {
        let binary = &[
            0x25, 0x00, 0x00, 0x01, 0x85, 0xae, 0x3f, 0x20, 0x00, 0x00, 0x00, 0x01, 0x21, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x6a, 0x6f, 0x6e, 0x00, 0x00, 0x05,
        ];
        let cursor = Cursor::new(&binary[..]);
        let mut packet_reader = PacketReader::new(cursor);
        let (_, packet) = packet_reader.next_read().unwrap().unwrap();
        let (_, handshake) = client_handshake_response(&packet, true).unwrap();
        println!("{handshake:?}");
        assert!(handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_LONG_PASSWORD));
        assert!(handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_MULTI_RESULTS));
        assert!(!handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_CONNECT_WITH_DB));
        assert!(!handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_DEPRECATE_EOF));
        assert!(handshake.client_flag.contains(CapabilityFlags::CLIENT_SSL));
        assert_eq!(
            handshake.collation,
            *collation_names().get("utf8_general_ci").unwrap() as u16
        );
        assert_eq!(handshake.username.unwrap(), &b"jon"[..]);
        assert_eq!(handshake.max_packet_len, 16777216);
    }
}
