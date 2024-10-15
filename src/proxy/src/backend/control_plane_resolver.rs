use common::ShutdownMessage;
use dashmap::DashMap;
use futures::StreamExt;
use itertools::Itertools;
use serde::Deserialize;
use std::cmp::PartialEq;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Condvar};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_rustls::rustls::lock::Mutex;
use tonic::codegen::http::uri;
use tonic::transport::Channel;
use tracing::{error, info};

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct CpBackend {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Generation")]
    generation: u64,
    #[serde(rename = "Role")]
    role: u32,
    #[serde(rename = "GossipAddr")]
    gossip_addr: String,
    #[serde(rename = "ServiceAddr")]
    service_addr: String,
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Ready")]
    ready: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CpChannelStatus {
    Unavailable,
    Available,
}

#[derive(Debug, Clone)]
pub struct CpChannel {
    pub status: CpChannelStatus,
    pub generation: u64,
    pub channel: Channel,
    pub backend_name: String,
    pub service: String,
    pub endpoint: String,
}

impl CpChannel {
    pub async fn new(cp_backend: &CpBackend) -> anyhow::Result<Self> {
        let rpc_addr = format!("http://{}", cp_backend.service_addr);
        let channel = Channel::from_shared(rpc_addr.clone())
            .map_err(|e| anyhow::anyhow!("Failed to create channel: {:?}", e))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to channel: {:?}", e))?;
        Ok(Self {
            status: CpChannelStatus::Available,
            generation: cp_backend.generation,
            channel,
            backend_name: cp_backend.name.clone(),
            service: cp_backend.service_addr.clone(),
            endpoint: cp_backend.address.clone(),
        })
    }
}

type SharedCpChannel = Arc<RwLock<CpChannel>>;

pub struct CpResolver {
    cp_service: String,
    interval: Duration,
    cp_backends: DashMap<String, SharedCpChannel>,
    started: Arc<(Mutex<bool>, Condvar)>,
    pick_index: AtomicUsize,
}

impl CpResolver {
    pub fn new(cp_service: String, interval: Option<Duration>) -> Self {
        let default_interval = interval.unwrap_or_else(|| Duration::from_secs(1));
        let cp_srv_uri =
            uri::Uri::try_from(cp_service.clone()).expect("CP_ADDR is not a valid uri");

        let cp_addr = match cp_srv_uri.scheme_str().filter(|&s| s == "http") {
            Some(_) => cp_service,
            None => format!("http://{}", cp_service),
        };
        Self {
            cp_service: cp_addr,
            interval: default_interval,
            cp_backends: DashMap::new(),
            started: Arc::new((Mutex::new(false), Condvar::new())),
            pick_index: AtomicUsize::new(0),
        }
    }

    pub async fn wait_for_backends_ready(&self) {
        let (lock, cvar) = &*self.started;
        let mut started = lock.lock().unwrap();
        while !*started {
            started = cvar.wait(started).unwrap();
        }
        info!("CpServiceResolver backends are ready");
    }

    pub async fn start(
        &self,
        mut shutdown_rx: Box<tokio::sync::watch::Receiver<ShutdownMessage>>,
    ) -> anyhow::Result<()> {
        let mut resolver_ticker = tokio::time::interval(self.interval);
        info!(
            "CpServiceResolver started ticker with interval {:?}",
            self.interval
        );
        let cluster_members_uri = format!("{}/api/v1/cluster-members", &self.cp_service);
        let client = reqwest::ClientBuilder::new()
            .no_proxy()
            .connect_timeout(Duration::from_secs(1))
            .build()?;
        let started_copy = Arc::clone(&self.started);
        let (lock, cvar) = &*started_copy;
        loop {
            tokio::select! {
                _ = resolver_ticker.tick() => {
                     self.refresh_backends(&client, &cluster_members_uri).await?;
                     // let check_started = *lock.lock().unwrap();
                     if !self.cp_backends.is_empty() && !(*lock.lock().unwrap()) {
                         let mut started = lock.lock().unwrap();
                         *started = true;
                         cvar.notify_all();
                         info!("CpServiceResolver first refresh complete.");
                     }
                }
                _ = shutdown_rx.changed() => {
                    return Ok(());
                }
            }
        }
    }

    pub async fn get_cp_backend(&self) -> CpChannel {
        let available_channels = tokio_stream::iter(self.cp_backends.iter())
            .filter_map(|entry| async move {
                let backend = entry.value();
                let backend_read_guard = backend.read().await;
                if backend_read_guard.status == CpChannelStatus::Available {
                    Some(backend.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<SharedCpChannel>>()
            .await;
        self.pick_index
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let available_arc = Arc::new(available_channels);
        let index =
            if self.pick_index.load(std::sync::atomic::Ordering::Relaxed) >= available_arc.len() {
                self.pick_index
                    .store(0, std::sync::atomic::Ordering::Relaxed);
                0
            } else {
                self.pick_index.load(std::sync::atomic::Ordering::Relaxed)
            };
        let shared_channel_ref = available_arc.get(index).unwrap();
        let channel = shared_channel_ref.read().await;
        channel.clone()
    }

    pub async fn mark_backend_unavailable(&self, backend_name: &str) {
        if let Some(backend) = self.cp_backends.get(backend_name) {
            let mut backend = backend.write().await;
            backend.status = CpChannelStatus::Unavailable;
        }
    }

    pub async fn update_backends(&self, new_cp_backend: Vec<CpBackend>) -> anyhow::Result<()> {
        if !self.cp_backends.is_empty() {
            let remove_keys = self
                .cp_backends
                .iter()
                .filter_map(|entry| {
                    let backend_name = entry.key();
                    if new_cp_backend.iter().any(|b| b.name == *backend_name) {
                        None
                    } else {
                        Some(backend_name.clone())
                    }
                })
                .collect_vec();
            remove_keys.iter().for_each(|k| {
                self.cp_backends.remove(k);
            });
        }
        tokio_stream::iter(new_cp_backend.into_iter())
            .for_each(|new_cp_backend| async move {
                let backend_name = new_cp_backend.name.clone();
                let cp_channel = CpChannel::new(&new_cp_backend).await;
                match cp_channel {
                    Ok(new_channel) => {
                        if let Some(old_channel) = self.cp_backends.get(&backend_name) {
                            let mut old_channel = old_channel.write().await;
                            match old_channel.status {
                                CpChannelStatus::Unavailable => {
                                    old_channel.status = CpChannelStatus::Available;
                                    old_channel.generation = new_cp_backend.generation;
                                    old_channel.channel = new_channel.channel;
                                }
                                CpChannelStatus::Available => {
                                    if new_cp_backend.generation > old_channel.generation {
                                        old_channel.generation = new_cp_backend.generation;
                                        old_channel.channel = new_channel.channel;
                                    }
                                }
                            }
                        } else {
                            self.cp_backends
                                .insert(backend_name, Arc::new(RwLock::new(new_channel)));
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to create channel for backend {:?}, cause by {:?}",
                            &backend_name, e
                        );
                        if self.cp_backends.contains_key(&backend_name) {
                            self.cp_backends.remove(&backend_name);
                        }
                    }
                }
            })
            .await;
        Ok(())
    }

    pub async fn refresh_backends(
        &self,
        http_client: &reqwest::Client,
        cluster_member_uri: &str,
    ) -> anyhow::Result<()> {
        let result = http_client
            .get(cluster_member_uri)
            .send()
            .await?
            .json::<Vec<CpBackend>>()
            .await;
        match result {
            Ok(backends) => self.update_backends(backends).await,
            Err(e) => {
                error!(
                    "Failed to get cp backends: cp_addr={:?} members_endpoints={:?} {:?}",
                    &self.cp_service, cluster_member_uri, e
                );
                anyhow::bail!("Failed to get cp backends {:?}", e);
            }
        }
    }
}
