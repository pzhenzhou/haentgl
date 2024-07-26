use crate::protocol::mysql::constants;
#[allow(unused_imports)]
use bitflags::Flags;
use byteorder::{ByteOrder, LittleEndian};

use pin_project::pin_project;
use std::io;
use std::io::prelude::*;
use std::io::IoSlice;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncWrite, AsyncWriteExt};

#[derive(Clone)]
#[pin_project]
pub struct PacketWriter<W> {
    // buf: bytes::BytesMut,
    buf: Vec<u8>,
    seq: u8,
    #[pin]
    pub inner_writer: W,
}

impl<W> PacketWriter<W> {
    pub fn new(write: W) -> Self {
        Self {
            buf: Vec::new(),
            seq: 0,
            inner_writer: write,
        }
    }

    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    fn take_buffer(&mut self) -> Vec<u8> {
        // TODO: bench vec<u8> vs bytes
        std::mem::take(&mut self.buf)
        // let limit = self.buf.len();
        // let take_buf = self.buf.as_mut().take(limit);
        // take_buf.into_inner().to_vec()
    }

    pub fn set_seq(&mut self, seq: u8) {
        self.seq = seq;
    }

    fn increase_seq(&mut self) {
        self.seq = self.seq.wrapping_add(1);
    }

    pub fn reset_seq(&mut self) {
        self.seq = 0;
    }

    pub fn seq(&self) -> u8 {
        self.seq
    }
}

impl<W: AsyncWrite> AsyncWrite for PacketWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(self.project().inner_writer, cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_flush(self.project().inner_writer, cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        AsyncWrite::poll_shutdown(self.project().inner_writer, cx)
    }
}
impl<W> Write for PacketWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len();
        self.buf.extend_from_slice(buf);
        // self.buf.extend(buf);
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: AsyncWrite + Unpin> PacketWriter<W> {
    pub async fn end_packet(&mut self) -> io::Result<()> {
        let mut header = [0; constants::PACKET_HEADER_LEN];
        if !self.is_empty() {
            let raw_packet = self.take_buffer();
            // split the raw buffer at the boundary of size MAX_PAYLOAD_LEN
            let chunks = raw_packet.chunks(constants::MAX_PAYLOAD_LEN);
            for chunk in chunks {
                // prepare the header
                LittleEndian::write_u24(&mut header, chunk.len() as u32);
                header[3] = self.seq();
                self.increase_seq();
                // write out the header and payload.
                //
                // depends on the AsyncWrite provided, this may trigger
                // real system call or not (for examples, if AsyncWrite is buffered stream)
                let written = self
                    .inner_writer
                    .write_vectored(&[IoSlice::new(&header), IoSlice::new(chunk)])
                    .await?;

                // if write buffer is not drained, fall back to write_all
                if written != constants::PACKET_HEADER_LEN + chunk.len() {
                    let remaining: Vec<u8> = header
                        .iter()
                        .chain(chunk.iter())
                        .skip(written)
                        .cloned()
                        .collect();
                    self.inner_writer.write_all(&remaining).await?
                }
            }
            Ok(())
        } else {
            // Packet with empty payload. Usually, the payload is not empty. Currently, only the password is empty.
            LittleEndian::write_u24(&mut header, 0);
            header[3] = self.seq();
            // info!(
            //     "PacketWriter::end_packet: write empty packet. seq: {}",
            //     header[3]
            // );
            let _size = self
                .inner_writer
                .write_vectored(&[IoSlice::new(&header), IoSlice::new(&[])])
                .await?;
            Ok(())
        }
    }

    pub async fn flush_all(&mut self) -> io::Result<()> {
        self.inner_writer.flush().await
    }
}
