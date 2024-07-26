## Haentgl

Haentgl is a Rust-based proxy designed for MySQL databases, oriented towards cloud serverless environments. It
serves as a unified access layer for serverless databases, providing seamless command forwarding, cluster autodiscovery,
connection maintenance, and topology awareness. Additionally, it includes control plane functions such as overload
control and database cluster state observation.

## Features

### Core Functions

#### Command Forwarding

- Efficiently forwards commands to the appropriate back-end MySQL database cluster.

#### Cluster Autodiscovery

- Automatically redirects connections when the proxied back-end database cluster expands or when a new cluster is
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