use crate::protocol::mysql::error_codes::ErrorKind::ER_ACCESS_DENIED_NO_PASSWORD_ERROR;
use crate::protocol::mysql::packet::packet_reader::PacketReader;
use crate::protocol::mysql::packet::packet_writer::PacketWriter;
use crate::protocol::mysql::packet::writers;
use crate::protocol::mysql::packet::writers::write_ok_packet_with_client_flags;
use crate::server::cmd_handler::CmdHandler;

use crate::server::auth::default_salt;
use mysql_common::constants::CapabilityFlags;
use rustls::ServerConfig;
use std::borrow::BorrowMut;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::protocol::mysql::basic::{
    client_handshake_response, from_packet, Command, HandshakeResponse, OkPacket,
};
use crate::protocol::mysql::constants::AuthPluginName::AuthNativePassword;
use crate::server::{default_capabilities, DEFAULT_BACKEND_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::rustls;
use tracing::{error, info, warn};
use winnow::error::ErrMode;

macro_rules! auth_pkt_reader {
    ($pkt_reader:expr, $err_msg:expr) => {{
        $pkt_reader
            .next_async()
            .await?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::ConnectionAborted, $err_msg))?
    }};
}

macro_rules! client_handshake_err {
    ($handshake_rs:expr) => {{
        $handshake_rs
            .map_err(|e| match e {
                ErrMode::Incomplete(_) => std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "backend sent incomplete handshake",
                ),
                ErrMode::Backtrack(winnow_error) | ErrMode::Cut(winnow_error) => {
                    if let winnow::error::ErrorKind::Eof = winnow_error.kind {
                        std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            format!(
                                "backend did not complete handshake; got {:?}",
                                winnow_error.input
                            ),
                        )
                    } else {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "bad backend handshake; got {:?} ({:?})",
                                winnow_error.input, winnow_error.kind
                            ),
                        )
                    }
                }
            })?
            .1
    }};
}
/// The [`StaticProxyServer`] is a demo to show how to implement a backend service compatible
/// with the MySQL protocol. Its purpose is to forward commands from the client to the proxy's backend.
pub struct StaticProxyServer;

impl StaticProxyServer {
    pub async fn on_recv<R, W, C>(
        &self,
        mut rx: tokio::sync::mpsc::Receiver<(R, W, C)>,
        mut exit_send: tokio::sync::watch::Receiver<()>,
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
        C: CmdHandler + Unpin,
    {
        let conn_id = AtomicU64::new(0);
        loop {
            tokio::select! {
                _ = exit_send.changed() => {
                    return Ok(());
                }
                Some((inbound, mut outbound, mut cmd_handler)) = rx.recv() => {
                    info!("StaticProxy recv inbound, outbound");
                    let new_id = conn_id.fetch_add(1, Ordering::Relaxed);
                    let (_, (handshake, seq, client_flags, pkt_reader)) = Self::initial_handshake(new_id, inbound,&mut outbound,#[cfg(feature = "tls")]&None,).await?;
                    let mut reader = PacketReader::new(pkt_reader);
                    let mut writer = PacketWriter::new(outbound);

                    let borrow_reader = reader.borrow_mut();
                    let borrow_writer = writer.borrow_mut();
                    Self::respond_client_handshake_rsp(borrow_reader,borrow_writer,&mut cmd_handler,handshake,seq,).await?;
                    Self::on_cmd(borrow_reader, borrow_writer, client_flags, &mut cmd_handler).await?;
                }
            }
        }
    }

    pub async fn run<R, W>(
        inbound: R,
        mut outbound: W,
        mut cmd_handler: impl CmdHandler,
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        // TODO: conn_id should from CmdHandler
        let (_, (handshake, seq, client_flags, pkt_reader)) = Self::initial_handshake(
            0,
            inbound,
            &mut outbound,
            #[cfg(feature = "tls")]
            &None,
        )
        .await?;

        let mut reader = PacketReader::new(pkt_reader);
        let mut writer = PacketWriter::new(outbound);

        let borrow_reader = reader.borrow_mut();
        let borrow_writer = writer.borrow_mut();

        Self::respond_client_handshake_rsp(
            borrow_reader,
            borrow_writer,
            &mut cmd_handler,
            handshake,
            seq,
        )
        .await?;

        Self::on_cmd(borrow_reader, borrow_writer, client_flags, &mut cmd_handler).await?;
        Ok(())
    }

    async fn on_cmd<R, W>(
        reader: &mut PacketReader<R>,
        writer: &mut PacketWriter<W>,
        client_flags: CapabilityFlags,
        cmd_handler: &mut impl CmdHandler,
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        while let Some((seq, packet)) = reader.next_async().await? {
            writer.set_seq(seq + 1);
            let cmd_pkt_rs = from_packet(&packet);
            match cmd_pkt_rs {
                Ok((_, cmd)) => match cmd {
                    Command::Query(q) => cmd_handler.on_query(q, writer).await?,
                    Command::Prepare(prepare) => cmd_handler.on_prepare(prepare, writer).await?,
                    Command::Execute { stmt, params } => {
                        cmd_handler.on_execute(params, writer).await?
                    }
                    Command::SendLongData { stmt, param, data } => {}
                    Command::Close(stmt) => {
                        write_ok_packet_with_client_flags(
                            writer,
                            client_flags,
                            OkPacket::default(),
                        )
                        .await?;
                    }
                    Command::ListFields(_) => {
                        let ok_packet = OkPacket {
                            header: 0xfe,
                            ..Default::default()
                        };
                        write_ok_packet_with_client_flags(writer, client_flags, ok_packet).await?;
                    }
                    Command::Init(schema) => {
                        cmd_handler.on_init(schema, writer).await?;
                    }
                    Command::Ping => {
                        write_ok_packet_with_client_flags(
                            writer,
                            client_flags,
                            OkPacket::default(),
                        )
                        .await?;
                    }
                    Command::Quit => {
                        break;
                    }
                },
                Err(e) => {
                    write_ok_packet_with_client_flags(writer, client_flags, OkPacket::default())
                        .await?
                }
            }
        }
        Ok(())
    }
    async fn initial_handshake<R, W>(
        conn_id: u64,
        reader: R,
        writer: &mut W,
        #[cfg(feature = "tls")] tls_conf: &Option<std::sync::Arc<ServerConfig>>,
    ) -> Result<
        (
            bool,
            (HandshakeResponse, u8, CapabilityFlags, PacketReader<R>),
        ),
        std::io::Error,
    >
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        let mut pkt_reader = PacketReader::new(reader);
        let mut pkt_writer = PacketWriter::new(writer);

        let salt = default_salt();
        let server_version = DEFAULT_BACKEND_VERSION;
        #[cfg(feature = "tls")]
        writers::write_initial_handshake(&mut pkt_writer, conn_id, salt, server_version, tls_conf)
            .await?;

        #[cfg(not(feature = "tls"))]
        writers::write_initial_handshake(&mut pkt_writer, conn_id, salt, server_version).await?;
        let (seq, handshake_pkt) = pkt_reader.next_async().await?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "peer terminated connection",
            )
        })?;

        let handshake_rs = client_handshake_response(&handshake_pkt, false);
        let client_handshake_rsp = client_handshake_err!(handshake_rs);
        pkt_writer.set_seq(seq + 1);
        #[cfg(not(feature = "tls"))]
        if client_handshake_rsp
            .client_flag
            .contains(CapabilityFlags::CLIENT_SSL)
        {
            error!("MySQLStaticProxy init_handshake Error.backend requested SSL despite us not advertising support for it. {client_handshake_rsp:?}");
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "backend requested SSL despite us not advertising support for it",
            )
            .into());
        }
        let server_capabilities = default_capabilities();
        #[cfg(feature = "tls")]
        if client_handshake_rsp
            .client_flag
            .contains(CapabilityFlags::CLIENT_SSL)
        {
            info!("MySQLStaticProxy init_handshake success. TSL=true");
            return Ok((
                true,
                (client_handshake_rsp, seq, server_capabilities, pkt_reader),
            ));
        }
        info!("MySQLStaticProxy init_handshake success.TSL=false");
        Ok((
            false,
            (client_handshake_rsp, seq, server_capabilities, pkt_reader),
        ))
    }

    async fn respond_client_handshake_rsp<R, W>(
        reader: &mut PacketReader<R>,
        writer: &mut PacketWriter<W>,
        cmd_handler: &mut impl CmdHandler,
        #[cfg(feature = "tls")] mut client_handshake: HandshakeResponse,
        #[cfg(not(feature = "tls"))] client_handshake: HandshakeResponse,
        mut seq: u8,
    ) -> Result<(), std::io::Error>
    where
        R: AsyncRead + Send + Unpin,
        W: AsyncWrite + Send + Unpin,
    {
        #[cfg(feature = "tls")]
        if client_handshake
            .client_flag
            .contains(CapabilityFlags::CLIENT_SSL)
        {
            let (r_seq, handshake_pkt) = auth_pkt_reader!(reader, "backend terminated connection");
            seq = r_seq;
            let handshake_rs = client_handshake_response(&handshake_pkt, true);
            client_handshake = client_handshake_err!(handshake_rs);
            writer.set_seq(seq + 1);
        }

        let salt = default_salt(); //conn_stat.get_slat();
        {
            if !client_handshake
                .client_flag
                .contains(CapabilityFlags::CLIENT_PROTOCOL_41)
            {
                error!("StaticProxy Server protocol Incompatibilities {client_handshake:?}");
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "Protocol Incompatibilities. Required: CLIENT_PROTOCOL_41.\
                                    Please upgrade your mysql backend.",
                ));
            };
            // self.client_flags = client_handshake.client_flag;
            // Determining Authentication Method:
            // In order to reduce the number of round-trips,
            // identity verification information is exchanged during the handshake phase.
            // When the backend and server use different authentication methods,
            // it is necessary to switch the authentication method.
            let mut auth_rsp = client_handshake.auth_response;
            if let Some(username) = client_handshake.username.as_ref() {
                // TODO: for user plugin
                let desired_plugin = AuthNativePassword.as_ref(); //self.default_auth_plugin();
                if !desired_plugin.is_empty()
                    && auth_rsp.is_empty()
                    && client_handshake.auth_plugin != desired_plugin.as_bytes()
                {
                    info!("StaticProxy switch the authentication method.");
                    writer.set_seq(seq + 1);
                    writer.write_all(&[0xfe])?;
                    writer.write_all(desired_plugin.as_bytes())?;
                    writer.write_all(&[0x00])?;
                    writer.write_all(salt.as_slice())?;
                    writer.write_all(&[0x00])?;
                    writer.end_packet().await?;
                    writer.flush_all().await?;
                    {
                        let (rr_seq, auth_response_data) =
                            auth_pkt_reader!(reader, "backend terminated connection");
                        seq = rr_seq;
                        auth_rsp = auth_response_data.to_vec();
                    }
                }

                writer.set_seq(seq + 1);
                let auth_rs = cmd_handler
                    .auth(
                        desired_plugin,
                        username,
                        salt.as_slice(),
                        auth_rsp.as_slice(),
                    )
                    .await?;
                // if authentication fails
                if !auth_rs {
                    warn!("StaticProxy Authenticate failed, current user = {username:?}");
                    let auth_failed_err = format!(
                        "Authenticate failed, user {username:?}, auth_plugin: {desired_plugin:?}"
                    );
                    writers::write_err_packet(
                        ER_ACCESS_DENIED_NO_PASSWORD_ERROR,
                        auth_failed_err.as_bytes(),
                        writer,
                    )
                    .await?;
                    writer.flush_all().await?;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        auth_failed_err,
                    ));
                }
                if let Some(Ok(curr_database)) = client_handshake
                    .database
                    .as_ref()
                    .map(|db_bytes| std::str::from_utf8(db_bytes))
                {
                    cmd_handler
                        .on_init(curr_database.as_bytes(), writer)
                        .await?;
                } else {
                    let client_capabilities = client_handshake.client_flag;
                    info!("StaticProxy send client_ok_pkt.");
                    write_ok_packet_with_client_flags(
                        writer,
                        client_capabilities,
                        OkPacket::default(),
                    )
                    .await?;
                }
            }
            writer.flush_all().await?;
        };
        info!("StaticProxy response to backend handshake message successful.");
        Ok(())
    }
}
