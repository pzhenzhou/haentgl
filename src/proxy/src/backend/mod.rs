use crate::backend::backend_discovery::{get_backend_discovery, BackendDiscovery};
use crate::backend::prost::common_proto::{ClusterName, DBLocation, ServiceStatus, TenantKey};
use std::sync::Arc;
use tracing::{error, info};

pub mod backend_discovery;
pub mod backend_mgr;
pub mod pool;
pub mod prost;
pub mod router;
// only for test.
pub fn test_tenant_key() -> TenantKey {
    TenantKey {
        region: "_NONE_".to_string(),
        available_zone: "_NONE_".to_string(),
        namespace: "_NONE_".to_string(),
        cluster_name: "_NONE_".to_string(),
    }
}

fn obfuscate_char(c: char, index: usize) -> char {
    if c.is_ascii_alphabetic() {
        let shift = ((index * 3 + 10) % 26) as u8;
        let base = if c.is_ascii_lowercase() { b'a' } else { b'A' };
        ((c as u8 - base + 26 - shift) % 26 + base) as char
    } else {
        c
    }
}

fn restore_char(c: char, index: usize) -> char {
    if c.is_ascii_alphabetic() {
        let shift = ((index * 3 + 10) % 26) as u8;
        let base = if c.is_ascii_lowercase() { b'a' } else { b'A' };
        ((c as u8 - base + shift) % 26 + base) as char
    } else {
        c
    }
}

pub fn obfuscate_string(original_string: &str) -> String {
    original_string
        .chars()
        .enumerate()
        .map(|(i, c)| obfuscate_char(c, i))
        .collect()
}

pub fn restore_string(obfuscated_string: &str) -> String {
    obfuscated_string
        .chars()
        .enumerate()
        .map(|(i, c)| restore_char(c, i))
        .collect()
}

pub fn encode_tenant_key(tenant_key: &TenantKey) -> String {
    let encode_region = obfuscate_string(&tenant_key.region);
    let encode_az = obfuscate_string(&tenant_key.available_zone);
    let encode_s = obfuscate_string(&tenant_key.namespace);
    let encode_cluster_name = obfuscate_string(&tenant_key.cluster_name);

    let encode_len = [
        encode_region.len() as u8,
        encode_az.len() as u8,
        encode_s.len() as u8,
        encode_cluster_name.len() as u8,
    ];
    format!(
        "{}{}{}{}{}",
        hex::encode(encode_len),
        encode_region,
        encode_az,
        encode_s,
        encode_cluster_name
    )
}

pub fn decode_tenant_key(tenant_key: &str) -> TenantKey {
    let key_string = tenant_key.to_string();
    let header = &key_string[0..8];
    let key_len = hex::decode(header).unwrap();

    let mut tenant_key = TenantKey::default();
    key_len.iter().enumerate().for_each(|(idx, len)| {
        let start = 8 + key_len[..idx].iter().sum::<u8>() as usize;
        let end = start + *len as usize;
        let decode_string = &key_string[start..end];
        let field = restore_string(decode_string);
        match idx {
            0 => tenant_key.region = field,
            1 => tenant_key.available_zone = field,
            2 => tenant_key.namespace = field,
            3 => tenant_key.cluster_name = field,
            _ => {}
        }
    });
    tenant_key
}
#[derive(Clone, Default, Debug, Eq, PartialEq, Hash)]
pub struct BackendInstance {
    pub location: DBLocation,
    pub addr: String,
    pub status: ServiceStatus,
    pub cluster: ClusterName,
}

impl BackendInstance {
    pub fn new(
        location: DBLocation,
        addr: String,
        status: ServiceStatus,
        cluster: ClusterName,
    ) -> Self {
        Self {
            location,
            addr,
            status,
            cluster,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum DbConnPhase {
    None,
    Connection,
    Command,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Hash)]
pub struct DbUserConnLifeCycle {
    db_user: Option<String>,
    conn_phase: Option<DbConnPhase>,
}

impl DbUserConnLifeCycle {
    pub fn is_none(&self) -> bool {
        self.db_user.is_none() && self.conn_phase.is_none()
    }

    pub fn new_conn_life_cycle(db_user: String, conn_phase: DbConnPhase) -> Self {
        Self {
            db_user: Some(db_user),
            conn_phase: Some(conn_phase),
        }
    }

    pub fn conn_phase(&self) -> Option<DbConnPhase> {
        self.conn_phase.clone()
    }

    pub fn db_user(&self) -> Option<String> {
        self.db_user.clone()
    }
}

pub async fn start_backend_discovery(
    curr_node: String,
    topology_srv_addr: String,
) -> Arc<BackendDiscovery> {
    let backend_discovery = get_backend_discovery(curr_node);
    let be_discovery_arc = Arc::clone(&backend_discovery);
    tokio::task::spawn(async move {
        let res = be_discovery_arc
            .discover(topology_srv_addr.as_str(), None)
            .await;
        match res {
            Ok(_) => info!("DbInstanceDiscoveryService shutdown."),
            Err(e) => error!("DbInstanceDiscoveryService error: {:?}", e),
        }
    });
    info!("DbInstanceDiscoveryService started.");
    backend_discovery
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn test_tenant_key() {
        let tenant_key = crate::backend::prost::common_proto::TenantKey {
            region: "ap-northeast-1".to_string(),
            available_zone: "ap-northeast-1a".to_string(),
            namespace: "mono-system-test".to_string(),
            cluster_name: "cluster-name-1-XxxxxxxXXXXXX".to_string(),
        };

        let encoded_tenant_key = crate::backend::encode_tenant_key(&tenant_key);
        println!("encoded_tenant_key: {}", encoded_tenant_key);
        let decode_tenant_key = crate::backend::decode_tenant_key(&encoded_tenant_key);
        println!("decode_tenant_key: {:?}", decode_tenant_key);
        assert_eq!(tenant_key, decode_tenant_key);
    }
}
