## Haentgl

> haentgl is a combination of Hegel + Kant. Pronunciation: /ˈhæntəgəl/

Haentgl is a Rust-based proxy designed for MySQL databases, oriented towards cloud serverless environments. It
serves as a unified access layer for serverless databases, providing seamless command forwarding, cluster autodiscovery,
connection maintenance, and topology awareness. Additionally, it includes control plane functions such as overload
control and database cluster state observation.

## Project status

### Not implemented at the moment

1. overload control.
2. connection migration.

## Features

### Core Functions

#### Command Forwarding

- Efficiently forwards commands to the appropriate back-end MySQL database cluster.

#### Cluster Autodiscovery

- Automatically redirects connections when the proxied back-end database cluster scale-out or when a new cluster is
  deployed.

#### Connection Maintenance

- Maintains user connections during back-end database cluster upgrades, restarts, or reclamations (scaling to zero),
  ensuring no impact on user's applications and automatic connection redirection.

#### Cluster Topology Awareness

- Provides awareness of cluster topology and supports affinity scheduling for optimized performance.

### Control Plane Functions

#### Overload Control

- Manages and controls overload situations to maintain optimal performance and stability.

#### Cluster State Observation

- Observes and reports the internal state of the database cluster for monitoring and troubleshooting purposes.

### How to test locally

```shell
cargo build 
./target/debug/my-proxy  --works 4 --port 3311  backend --backend-addr ${YOUR_BACKEND_MYSQL_ADDR}
 ```

```text
./target/debug/my-proxy  --works 4 --port 3311  backend --backend-addr 0.0.0.0:3315
2024-07-26T10:32:58.551490Z  INFO my_proxy: 149: ProxySrv running config args=ProxyServerArgs { works: 4, port: 3311, http_port: 9000, tls: false, enable_metrics: false, enable_rest: false, router: None, balance: None, log_level: None, cluster_watcher_addr: None, node_id: None, backend: Some(Backend { backend_addr: "0.0.0.0:3315" }), cp_args: ControlPlaneArgs { enable_cp: false, cp_target_addr: None } }
2024-07-26T10:32:58.552883Z  INFO proxy::backend::backend_mgr: 82: ProxySrv backend_mgr prepare_backend_pool start max_size=50, all_backend_list=[BackendInstance { location: DBLocation { region: "", available_zone: "", namespace: "", node_name: "" }, addr: "0.0.0.0:3315", status: Ready, cluster: ClusterName { namespace: "", cluster_name: "" } }]
2024-07-26T10:32:58.552985Z  INFO proxy::backend::backend_mgr: 63: ProxySrv backend_mgr conn pool initialized successfully. BackendInstance { location: DBLocation { region: "", available_zone: "", namespace: "", node_name: "" }, addr: "0.0.0.0:3315", status: Ready, cluster: ClusterName { namespace: "", cluster_name: "" } }
2024-07-26T10:33:09.974797Z  INFO proxy::server::haentgl_server: 88: ProxySrv on_conn conn_id=1
2024-07-26T10:33:09.975898Z DEBUG proxy::server::haentgl_server: 151: ProxySrv First authentication on current conn "PEWNlDDoT_QVY8LVixmwV".
2024-07-26T10:33:09.979788Z DEBUG proxy::server::auth::authenticator: 76: ProxySrv Auth continue_auth auth success=0
```