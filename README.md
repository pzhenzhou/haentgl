## Haentgl

> haentgl is a combination of Hegel + Kant. Pronunciation: /ˈhæntəgəl/

Haentgl is a Rust-based proxy designed for MySQL databases, oriented towards cloud serverless environments. It
serves as a unified access layer for serverless databases, providing seamless command forwarding, cluster autodiscovery,
connection maintenance, and topology awareness. Additionally, it includes control plane functions such as overload
control and database cluster state observation.

## Architecture

```text
        +---------------------+
        |        Users        |
        +---------------------+
                  |
                  v
        +---------------------+
        |     Proxy Layer     | <---------------------+
        +---------------------+                       |
                  |                                   |
                  v                                   |
+---------------------------------+                   |
|   MySQL-Compatible Distributed  |                   |
|            Database             |                   |
+---------------------------------+                   |
                  ^                                   |
                  |                                   |
        +---------------------+                       |
        |    Control Plane    | ----------------------+
        +---------------------+

```

1. Users
    - Connect to the Proxy Layer using the MySQL protocol to access database services.
2. Proxy Layer
    - Acts as the sole access point to the database, maintaining user connections and sessions.
    - Monitors backend database status (e.g., connection counts, basic metrics).Perceiving the topology of clusters to
      better enable **"applications to be closer to data, or data locality."**
    - **Always Online**: Enables seamless upgrades and maintenance of the backend databases without user impact by migrating
      connections and maintaining sessions.
    - **Automatic Scaling**: Works with the Control Plane to detect changes in the database cluster, facilitating
      auto-scaling and Scale-to-zero, a key feature of serverless products.
3. Control Plane
    - Manages deployment and scaling of the Distributed Database and other components.

## Project status

### Not implemented at the moment

1. connection migration.

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