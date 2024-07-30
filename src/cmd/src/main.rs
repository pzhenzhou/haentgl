use clap::Parser;
use common::metrics::process_unix::ProcessRecorder;
use common::ShutdownMessage;
use proxy::backend::router::new_backend_router;
use proxy::cp;
use proxy::cp::active_users::UserActivityWindow;
use proxy::server::auth::authenticator::ProxyAuthenticator;
use proxy::server::haentgl_server::HaentglServer;
use proxy::server::proxy_cli_args::ProxyServerArgs;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::watch;
use tokio::sync::watch::Receiver;
use tracing::{info, warn, Level};
use tracing_subscriber::EnvFilter;

#[cfg(unix)]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

async fn shutdown_await(mut shutdown_rx: Box<Receiver<ShutdownMessage>>) {
    let changed_rs = &shutdown_rx.changed().await;
    if changed_rs.is_ok() {
        let canceled = shutdown_rx.borrow_and_update().clone();
        if let ShutdownMessage::Cancel(msg) = canceled {
            info!("ProxySrv process receive shutdown msg {msg}");
        }
    }
}

async fn shutdown_signal() -> ShutdownMessage {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        ctrl_c_v = ctrl_c => {
            let msg = format!("ProxySrv receive ctrl_c signal  {ctrl_c_v:?}");
            ShutdownMessage::Cancel(msg)
        },
        v = terminate => {
            let msg =  format!("ProxySrv receive terminate signal  {v:?}");
            ShutdownMessage::Cancel(msg)
        },
    }
}

#[allow(dead_code)]
async fn start_cp_target(
    proxy_config: ProxyServerArgs,
    shutdown_rx: &Receiver<ShutdownMessage>,
) -> Option<Arc<UserActivityWindow>> {
    let cp_args = proxy_config.cp_args;
    if cp_args.enable_cp {
        let rpc_server_addr = cp_args.cp_listen_addr().unwrap();
        let shutdown_rx_clone = Box::new(shutdown_rx.clone());
        let active_users = Arc::new(UserActivityWindow::default());
        let borrow_moved_active_users = Arc::clone(&active_users);
        cp::prost::start_cp_target_reporter(
            borrow_moved_active_users,
            rpc_server_addr,
            shutdown_rx_clone,
        )
            .await;
        Some(active_users)
    } else {
        None
    }
}

fn start_metrics_and_rest(
    proxy_config: ProxyServerArgs,
    runtime: &Runtime,
    shutdown_rx: &Receiver<ShutdownMessage>,
) {
    let http_port = proxy_config.http_port;
    if proxy_config.enable_metrics {
        common::metrics::init_metrics_context();
        let mut process_recorder = ProcessRecorder::new(
            common::metrics::common_labels().clone(),
            shutdown_rx.clone(),
        );
        runtime.spawn(async move {
            process_recorder.start_auto_collect().await;
        });
        let shutdown_rx_clone = Box::new(shutdown_rx.clone());
        runtime.spawn(async move {
            web_service::http_server::HaentglProxyRest::start_server(
                "0.0.0.0".to_string(),
                http_port,
                true,
                shutdown_await(shutdown_rx_clone),
            )
                .await
        });
    }
    if proxy_config.enable_rest {
        let shutdown_rx_clone = Box::new(shutdown_rx.clone());
        runtime.spawn(async move {
            web_service::http_server::HaentglProxyRest::start_server(
                "0.0.0.0".to_string(),
                http_port,
                false,
                shutdown_await(shutdown_rx_clone),
            )
                .await
        });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proxy_config = ProxyServerArgs::parse();
    let log_level_string = proxy_config
        .log_level
        .clone()
        .unwrap_or("DEBUG".to_string());
    let level = Level::from_str(log_level_string.as_str())?;
    // setup tracing, disable grpc debug log.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("DEBUG,hyper=INFO,tower=INFO,h2=INFO,chitchat=INFO"))
        .add_directive(level.into())
        .add_directive("hyper=INFO".parse().unwrap())
        .add_directive("h2=INFO".parse().unwrap())
        .add_directive("tower=INFO".parse().unwrap())
        .add_directive("chitchat=INFO".parse().unwrap());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_line_number(true)
        .init();

    let works = proxy_config.works;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("MONO_PROXY")
        .worker_threads(works)
        .build()?;

    info!("ProxySrv running config args={:?}", proxy_config);
    // start metrics service
    let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownMessage::Init);
    start_metrics_and_rest(proxy_config.clone(), &runtime, &shutdown_rx);
    runtime.block_on(async {
        let backend_options = proxy_config.new_backend_opts();
        let router = new_backend_router(&proxy_config).await;

        let mut proxy_srv = HaentglServer::new(
            backend_options,
            router,
            ProxyAuthenticator,
        );
        proxy_srv.initialize().await.unwrap();

        let port = proxy_config.port;
        let tcp_listener = TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
        let proxy_srv_arc = Arc::new(proxy_srv);
        loop {
            tokio::select! {
                shutdown_msg = shutdown_signal() => {
                    shutdown_tx.send(shutdown_msg.clone()).unwrap();
                    break ;
                }
                rs = tcp_listener.accept() => {
                   match rs {
                      Ok((stream,  _addr)) => {
                         let (client_reader, client_writer) = stream.into_split();
                         let proxy_arc_clone = Arc::clone(&proxy_srv_arc);
                         runtime.spawn(async move {proxy_arc_clone.connect(client_reader, client_writer, &None).await});
                      }
                      Err(e)=> {
                          warn!("ProxySrv accept connection err. cause by {e:?}");
                      }
                   }
                }
            }
        };
        Ok(())
    })
}
