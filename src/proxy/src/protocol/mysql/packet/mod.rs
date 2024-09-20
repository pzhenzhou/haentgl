pub mod packet_reader;
pub mod packet_writer;
pub mod writers;

use crate::protocol::mysql::constants;
use crate::protocol::mysql::constants::HeaderInfo;
use std::ops::Deref;
use winnow::token::take;
use winnow::Parser;

/// `Packet` Represents the packet format of the MySql wire protocol.
/// The maximum size of a MySQL packet is 16M; if the data is >16M, it needs to be split
/// until it is less than 16 M.[MySQL Packet](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_basic_packets.html)
#[derive(Clone, Debug)]
//TODO: bench Vec vs BytesMut
pub struct Packet(Vec<u8>);

impl Packet {
    pub fn from_vec(vec: Vec<u8>) -> Self {
        Packet(vec)
    }
}

#[inline]
fn full_packet(i: &[u8]) -> winnow::IResult<&[u8], (u8, &[u8])> {
    let (i, _) = winnow::token::literal(&[0xff, 0xff, 0xff]).parse_peek(i)?;
    let (i, seq) = take(1u8).parse_peek(i)?;
    let (i, bytes) = take(constants::MAX_PAYLOAD_LEN).parse_peek(i)?;
    Ok((i, (seq[0], bytes)))
}

#[inline]
fn one_packet(i: &[u8]) -> winnow::IResult<&[u8], (u8, &[u8])> {
    let (i, length) = winnow::binary::le_u24.parse_peek(i)?;
    let (i, seq) = take(1u8).parse_peek(i)?;
    let (i, bytes) = take(length).parse_peek(i)?;
    Ok((i, (seq[0], bytes)))
}

impl Packet {
    pub fn extend(&mut self, bytes: &[u8]) {
        self.0.extend(bytes);
    }

    /// See [MySQL EOF_Packet](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_basic_eof_packet.html)
    pub fn is_eof_packet(&self) -> bool {
        let pkt_len = self.0.len();
        !self.0.is_empty() && self.0[0] == (HeaderInfo::EOFHeader as u8) && pkt_len <= 5
    }

    /// See: [MariaDB](https://mariadb.com/kb/en/result-set-packets/) or [MySQL](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_basic_ok_packet.html)
    /// Packet header is 0xfe, and we need check the packet length.
    /// return true OK packet after the result set when CLIENT_DEPRECATE_EOF is enabled
    pub fn is_result_set_eof_packet(&self) -> bool {
        let pkt_len = self.0.len();
        !self.0.is_empty()
            && self.0[0] == (HeaderInfo::EOFHeader as u8)
            && (7..0xFFFFFF).contains(&pkt_len)
    }

    pub fn is_ok_packet(&self) -> bool {
        !self.0.is_empty() && self.0[0] == (HeaderInfo::OKHeader as u8)
    }

    pub fn is_err_packet(&self) -> bool {
        !self.0.is_empty() && self.0[0] == (HeaderInfo::ErrHeader as u8)
    }

    pub fn is_local_in_file_packet(&self) -> bool {
        !self.0.is_empty() && self.0[0] == (HeaderInfo::LocalInFileHeader as u8)
    }
}

impl AsRef<[u8]> for Packet {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsMut<[u8]> for Packet {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Deref for Packet {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

pub fn packet(i: &[u8]) -> winnow::IResult<&[u8], (u8, Packet)> {
    let mut input = i;
    let mut full_packets = Vec::new();
    // Manually parse zero or more full packets
    while let Ok((next_input, result)) = full_packet(input) {
        full_packets.push(result);
        input = next_input;
    }
    // Parse one final packet
    let (input, (last_seq, last_p)) = one_packet(input)?;
    // Combine the full packets with the last packet
    let mut pkt_data = Vec::new();
    let mut prev_seq = None;
    for (seq, p) in full_packets {
        if let Some(prev_seq) = prev_seq {
            // Ensure sequence numbers are consecutive
            assert_eq!(seq, prev_seq + 1);
        }
        pkt_data.extend_from_slice(p);
        prev_seq = Some(seq);
    }

    pkt_data.extend_from_slice(last_p);
    let pkt = Packet(pkt_data);
    Ok((input, (last_seq, pkt)))
}

#[cfg(test)]
mod tests {
    use crate::protocol::mysql::packet::*;

    #[test]
    fn test_one_ping() {
        let one_pkg_rs = one_packet(&[0x01, 0, 0, 0, 0x10]);
        println!("{one_pkg_rs:?}");
        assert!(one_pkg_rs.is_ok());
        let pkg = one_pkg_rs.unwrap().1;
        assert_eq!(pkg.1, &[0x10]);
    }

    #[test]
    fn test_ping() {
        let p = packet(&[0x01, 0, 0, 0, 0x10]).unwrap().1;
        assert_eq!(p.0, 0);
        assert_eq!(&*p.1, &[0x10][..]);
    }

    #[test]
    fn test_long_exact() {
        bitflags::bitflags! {}
        let mut data = vec![0xff, 0xff, 0xff, 0];
        data.extend(&[0; constants::MAX_PAYLOAD_LEN][..]);
        data.push(0x00);
        data.push(0x00);
        data.push(0x00);
        data.push(1);

        let mut play_load_slice = [0x00; 4];
        play_load_slice.clone_from_slice(&data[0..4]);
        let payload_len = u32::from_le_bytes(play_load_slice);
        println!("mysql packet max payload_len = {payload_len:}");
        assert_eq!(payload_len as usize, constants::MAX_PAYLOAD_LEN);
        let (rest, p) = packet(&data[..]).unwrap();
        assert!(rest.is_empty());
        assert_eq!(p.0, 1);
        assert_eq!(p.1.len(), constants::MAX_PAYLOAD_LEN);
        // assert_eq!(&*p.0, &[0; constants::MAX_PAYLOAD_LEN][..]);
    }

    #[test]
    fn test_long_more() {
        let mut data = vec![0xff, 0xff, 0xff, 0];
        data.extend(&[0; constants::MAX_PAYLOAD_LEN][..]);
        data.push(0x01);
        data.push(0x00);
        data.push(0x00);
        data.push(1);
        data.push(0x10);

        let (rest, p) = packet(&data[..]).unwrap();
        assert!(rest.is_empty());
        assert_eq!(p.0, 1);
        assert_eq!(p.1.len(), constants::MAX_PAYLOAD_LEN + 1);
        assert_eq!(
            &p.1[..constants::MAX_PAYLOAD_LEN],
            &[0; constants::MAX_PAYLOAD_LEN][..]
        );
        assert_eq!(&p.1[constants::MAX_PAYLOAD_LEN..], &[0x10]);
    }
}
