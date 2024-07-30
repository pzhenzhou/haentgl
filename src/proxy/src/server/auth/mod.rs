use crate::protocol::mysql::basic::HandshakeResponse;
use crate::protocol::mysql::constants::SCRAMBLE_SIZE;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::Packet;
use std::io::ErrorKind;

use async_trait::async_trait;
use itertools::Itertools;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha1::Digest;
use sha2::Sha256;

use rustls::server::ServerConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio_rustls::rustls;


pub mod authenticator;

// Only for test purpose.
pub fn default_salt() -> [u8; SCRAMBLE_SIZE] {
    let bs = ";X,po_k}>o6^Wz!/kM}N".as_bytes();
    let mut salt: [u8; SCRAMBLE_SIZE] = [0; SCRAMBLE_SIZE];
    for i in 0..SCRAMBLE_SIZE {
        salt[i] = bs[i];
        if salt[i] == b'\0' || salt[i] == b'$' {
            salt[i] += 1;
        }
    }
    salt
}

fn val(c: u8, idx: usize) -> Result<u8, std::io::Error> {
    match c {
        b'A'..=b'F' => Ok(c - b'A' + 10),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'0'..=b'9' => Ok(c - b'0'),
        _ => Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("Invalid hex character {}", idx),
        )),
    }
}

pub fn hex_string_decode<T: AsRef<[u8]>>(data: T) -> Result<Vec<u8>, std::io::Error> {
    let data_ref = data.as_ref();
    if data_ref.len() % 2 != 0 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "Input hex string's length needs to be even, as two digits correspond to one byte.",
        ));
    }

    data_ref
        .chunks(2)
        .enumerate()
        .map(|(i, pair)| {
            Ok::<u8, std::io::Error>(val(pair[0], 2 * i)? << 4 | val(pair[1], 2 * i + 1)?)
        })
        .try_collect()
}

/// Generate a random string user ASCII but avoid separator character.
/// https://github.com/mysql/mysql-server/blob/8.0/mysys/crypt_genhash_impl.cc#L427
#[inline]
pub fn gen_user_salt() -> [u8; SCRAMBLE_SIZE] {
    let mut salt: [u8; SCRAMBLE_SIZE] = [0; SCRAMBLE_SIZE];
    let mut r = StdRng::from_entropy();
    for salt_item in salt.iter_mut() {
        let salt_rand = r.gen_range(0..127) as u8;
        *salt_item = salt_rand;
        if *salt_item == b'\0' || *salt_item == b'$' {
            *salt_item += 1;
        }
    }
    salt
}

fn to_u8_32(bytes: impl AsRef<[u8]>) -> [u8; 32] {
    let mut out = [0; 32];
    (out[..]).copy_from_slice(bytes.as_ref());
    out
}

pub fn sha256_1(bytes: impl AsRef<[u8]>) -> [u8; 32] {
    let mut hasher = Sha256::default();
    hasher.update(bytes.as_ref());
    to_u8_32(hasher.finalize())
}

pub fn sha256_2(bytes1: impl AsRef<[u8]>, bytes2: impl AsRef<[u8]>) -> [u8; 32] {
    let mut hasher = Sha256::default();
    hasher.update(bytes1.as_ref());
    hasher.update(bytes2.as_ref());
    to_u8_32(hasher.finalize())
}

pub fn sha1_1(bytes: impl AsRef<[u8]>) -> [u8; 20] {
    sha1::Sha1::digest(bytes).into()
}

pub fn xor<T, U>(mut left: T, right: U) -> T
where
    T: AsMut<[u8]>,
    U: AsRef<[u8]>,
{
    left.as_mut()
        .iter_mut()
        .zip(right.as_ref().iter())
        .map(|(l, r)| *l ^= r)
        .last();
    left
}

pub fn sha1_2(bytes1: impl AsRef<[u8]>, bytes2: impl AsRef<[u8]>) -> [u8; 20] {
    let mut hasher = sha1::Sha1::new();
    hasher.update(bytes1.as_ref());
    hasher.update(bytes2.as_ref());
    hasher.finalize().into()
}

/// The Authenticator is an abstraction of the connection phase of the MySQL protocol.
///
/// 1. exchange the capabilities of client and server （setup TSL if requested）
/// 2. Authenticate the client's user information
///
/// Since the [MySQL connection lifecycle](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_connection_lifecycle.html) requires SqlProxy authentication when SqlProxy enables
/// connection pooling.
#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn continue_auth<R, W>(
        &self,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
        client_reader: &mut PacketReader<R>,
        client_seq: u8,
        client_handshake_rsp_pkt: &HandshakeResponse,
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin;

    /// Reads Backend's HandshakePacket and forwards it to the client
    async fn initial_handshake<R, W>(
        &self,
        conn_id: u64,
        scramble: [u8; 20],
        client_reader: &mut PacketReader<R>,
        client_writer: &mut PacketWriter<W>,
        #[cfg(feature = "tls")] tls_conf: &Option<std::sync::Arc<ServerConfig>>,
    ) -> Result<(u8, HandshakeResponse, Packet), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin;

    /// Responds to the client's HandshakePacket, which is forwarded to the backend.
    async fn reply_handshake_response<R, W>(
        &self,
        backend_writer: &mut PacketWriter<OwnedWriteHalf>,
        backend_reader: &mut PacketReader<OwnedReadHalf>,
        client_writer: &mut PacketWriter<W>,
        client_reader: &mut PacketReader<R>,
        seq: u8,
        client_handshake_rsp_pkt: (&[u8], &HandshakeResponse),
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin;
}
