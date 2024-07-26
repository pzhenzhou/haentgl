use crate::backend::backend_mgr::BackendManagerOptions;
use crate::prost::common_proto::{ClusterName, DBLocation, ServiceStatus};
use crate::backend::router::{BackendLoadBalancerType, BackendRouterType};
use crate::backend::BackendInstance;

use clap::{Parser, Subcommand};
use itertools::Itertools;
use std::collections::VecDeque;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::LazyLock;

pub static TEST_BACKEND_ADDRS: LazyLock<VecDeque<BackendInstance>> = LazyLock::new(|| {
    VecDeque::from(vec![BackendInstance {
        location: DBLocation {
            region: "_NONE_".to_string(),
            available_zone: "_NONE_".to_string(),
            namespace: "_NONE_".to_string(),
            node_name: "_NONE_".to_string(),
        },
        addr: "127.0.0.1:3315".to_string(),
        status: ServiceStatus::Ready,
        cluster: ClusterName::default(),
    }])
});

#[derive(Parser, Default, Debug, Clone)]
#[clap(
    name = "my-proxy",
    version = "0.1.0",
    about = "mysql proxy for serverless database."
)]
pub struct ProxyServerArgs {
    #[clap(long, value_name = "WORKS", default_value_t = 4)]
    pub works: usize,
    #[clap(long, value_name = "PORT", default_value_t = 3310)]
    pub port: u16,
    #[clap(long, value_name = "HTTP_PORT", default_value_t = 9000)]
    pub http_port: u16,
    #[clap(long, value_name = "TLS", default_value_t = false)]
    pub tls: bool,
    #[clap(long, value_name = "ENABLE METRICS COLLECTOR", default_value_t = false)]
    pub enable_metrics: bool,
    #[clap(long, value_name = "ENABLE REST API", default_value_t = false)]
    pub enable_rest: bool,
    #[clap(long, value_name = "ROUTE_NAME")]
    pub router: Option<String>,
    #[clap(long, value_name = "BALANCE")]
    pub balance: Option<String>,
    #[clap(long, value_name = "LOG_LEVEL")]
    pub log_level: Option<String>,
    #[clap(long, value_name = "Control Plane Grpc Address")]
    pub cp_addr: Option<String>,
    #[clap(long, value_name = "NODE_ID")]
    pub node_id: Option<String>,
    #[clap(subcommand)]
    pub backend: Option<BackendConfigArgs>,
    #[clap(flatten)]
    pub cp_args: ControlPlaneArgs,
}

#[derive(clap::Parser, Debug, Default, Clone)]
pub struct ControlPlaneArgs {
    #[clap(long)]
    pub enable_cp: bool,
    #[clap(long)]
    pub cp_target_addr: Option<String>,
}

impl ControlPlaneArgs {
    pub fn cp_listen_addr(&self) -> Option<String> {
        if self.enable_cp {
            match &self.cp_target_addr {
                Some(addr) => Some(addr.clone()),
                None => Some("0.0.0.0:19001".to_string()),
            }
        } else {
            None
        }
    }
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
#[command(next_line_help = true)]
pub enum BackendConfigArgs {
    #[command(long_about = "Proxy only a specific backend. For testing purposes.")]
    Backend {
        #[clap(long)]
        backend_addr: String,
    },
    #[command(long_about = "Proxy only a specific cluster. For testing purposes.")]
    Cluster {
        #[clap(long)]
        namespace: String,
        #[clap(long)]
        cluster_name: Option<String>,
    },
}

impl ProxyServerArgs {
    pub fn get_node_id(&self) -> String {
        self.node_id
            .clone()
            .unwrap_or_else(|| "default".to_string())
    }

    pub fn new_backend_opts(&self) -> BackendManagerOptions {
        let balancer_type_str = self.balancer_type();

        BackendManagerOptions {
            static_router: if let Some(router) = self.router.as_ref() {
                router.eq_ignore_ascii_case("static")
            } else {
                true
            },
            balance_type: BackendLoadBalancerType::from_str(balancer_type_str.as_str()).unwrap(),
            ..Default::default()
        }
    }

    pub fn balancer_type(&self) -> String {
        if let Some(balance) = self.balance.as_ref() {
            balance.clone().to_lowercase()
        } else {
            "random".to_lowercase()
        }
    }

    pub fn router_type(&self) -> Option<BackendRouterType> {
        if let Some(router_str) = &self.router {
            let router = BackendRouterType::from_str(router_str.as_str()).unwrap();
            Some(router)
        } else {
            None
        }
    }

    // only for testing purposes.
    pub fn static_backend_list(&self) -> VecDeque<BackendInstance> {
        if let Some(backend_cmd) = &self.backend {
            match backend_cmd {
                BackendConfigArgs::Backend {
                    backend_addr: addrs,
                    ..
                } => {
                    let backend_list = addrs
                        .split(',')
                        .map(|addr| BackendInstance {
                            location: DBLocation::default(),
                            addr: addr.to_string(),
                            status: ServiceStatus::Ready,
                            cluster: ClusterName::default(),
                        })
                        .collect_vec();
                    VecDeque::from(backend_list)
                }
                _ => TEST_BACKEND_ADDRS.deref().clone(),
            }
        } else {
            TEST_BACKEND_ADDRS.deref().clone()
        }
    }
}
