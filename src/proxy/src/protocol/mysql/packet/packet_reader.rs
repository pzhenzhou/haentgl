use crate::protocol::mysql::packet::{packet, Packet};

use std::io;
use std::io::prelude::*;

use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;

const PACKET_BUFFER_SIZE: usize = 4096;
const PACKET_LARGE_BUFFER_SIZE: usize = 1048576;

#[macro_export]
macro_rules! async_packet_read {
    ($reader: expr) => {{
        use tracing::warn;
        let rs = $reader.next_async().await;
        if rs.is_err() {
            warn!("ProxySrv read pkg err = {:?}", rs);
        }
        rs?.ok_or_else(|| {
            warn!("ProxySrv pkg is none");
            std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "connection disconnect.",
            )
        })?
    }};
}

/// [PacketReader] represents reading data from a TcpStream and parsing it into a MySQL [`Packet`](Packet)
#[derive(Clone)]
pub struct PacketReader<R> {
    bytes: Vec<u8>,
    start: usize,
    remaining: usize,
    pub r: R,
}

impl<R> PacketReader<R> {
    pub fn new(r: R) -> Self {
        PacketReader {
            bytes: Vec::new(),
            start: 0,
            remaining: 0,
            r,
        }
    }
}

impl<R: Read> PacketReader<R> {
    // #[allow(dead_code)]
    pub fn next_read(&mut self) -> io::Result<Option<(u8, Packet)>> {
        self.start = self.bytes.len() - self.remaining;

        loop {
            if self.remaining != 0 {
                let bytes = {
                    // NOTE: this is all sorts of unfortunate. what we really want to do is to give
                    // &self.bytes[self.start..] to `packet()`, and the lifetimes should all work
                    // out. however, without NLL, borrowck doesn't realize that self.bytes is no
                    // longer borrowed after the match, and so can be mutated.
                    let bytes = &self.bytes[self.start..];
                    unsafe { ::std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) }
                };

                match packet(bytes) {
                    Ok((rest, p)) => {
                        self.remaining = rest.len();
                        return Ok(Some(p));
                    }
                    Err(nom::Err::Incomplete(_)) | Err(nom::Err::Error(_)) => {}
                    Err(nom::Err::Failure(ctx)) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{:?}", ctx),
                        ));
                    }
                }
            }

            // we need to read some more
            self.bytes.drain(0..self.start);
            self.start = 0;
            let end = self.bytes.len();
            self.bytes.resize(std::cmp::max(4096, end * 2), 0);
            let read = {
                let buf = &mut self.bytes[end..];
                self.r.read(buf)?
            };
            self.bytes.truncate(end + read);
            self.remaining = self.bytes.len();

            if read == 0 {
                if self.bytes.is_empty() {
                    return Ok(None);
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("{} unhandled bytes", self.bytes.len()),
                    ));
                }
            }
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for PacketReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        if self.remaining != 0 {
            buf.put_slice(&self.bytes[self.start..]);
            self.bytes.clear();
            self.start = 0;
            self.remaining = 0;
            std::task::Poll::Ready(Ok(()))
        } else {
            std::pin::Pin::new(&mut self.r).poll_read(cx, buf)
        }
    }
}

impl<R: AsyncRead + Unpin> PacketReader<R> {
    pub async fn next_async(&mut self) -> io::Result<Option<(u8, Packet)>> {
        self.start = self.bytes.len() - self.remaining;

        let mut buffer_size = PACKET_BUFFER_SIZE;
        loop {
            if self.remaining != 0 {
                let bytes = {
                    // NOTE: this is all sorts of unfortunate. what we really want to do is to give
                    // &self.bytes[self.start..] to `packet()`, and the lifetimes should all work
                    // out. however, without NLL, borrowck doesn't realize that self.bytes is no
                    // longer borrowed after the match, and so can be mutated.
                    let bytes = &self.bytes[self.start..];
                    unsafe { ::std::slice::from_raw_parts(bytes.as_ptr(), self.remaining) }
                };
                match packet(bytes) {
                    Ok((rest, p)) => {
                        self.remaining = rest.len();
                        if self.remaining > 0 {
                            self.bytes = rest.to_vec();
                            self.start = 0;
                        }
                        return Ok(Some(p));
                    }
                    Err(nom::Err::Incomplete(_)) | Err(nom::Err::Error(_)) => {}
                    Err(nom::Err::Failure(ctx)) => {
                        self.bytes.truncate(self.remaining);
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{:?}", ctx),
                        ));
                    }
                }
            }

            // we need to read some more
            self.bytes.drain(0..self.start);
            self.start = 0;
            let end = self.remaining;

            if self.bytes.len() - end < buffer_size {
                let new_len = std::cmp::max(buffer_size, end * 2);
                self.bytes.resize(new_len, 0);
            }
            let read = {
                let buf = &mut self.bytes[end..];
                self.r.read(buf).await?
            };
            self.remaining = end + read;
            // use a larger buffer size to reduce bytes resize times.
            buffer_size = PACKET_LARGE_BUFFER_SIZE;
            if read == 0 {
                self.bytes.truncate(self.remaining);
                if self.bytes.is_empty() {
                    return Ok(None);
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("{} unhandled bytes", self.bytes.len()),
                    ));
                }
            }
        }
    }
}
