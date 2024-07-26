use crate::prost::control_plane::UserCom;

use crate::prost::common_proto::TenantKey;
use dashmap::DashMap;
use std::hash::Hasher;
use std::mem;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

fn user_com_hash(user_com: &UserCom) -> u64 {
    let mut hasher = twox_hash::xxh3::Hash64::default();
    let user_ref = user_com.cluster.as_ref().unwrap();
    hasher.write_str(&user_ref.region);
    hasher.write_str(&user_ref.available_zone);
    hasher.write_str(&user_ref.cluster_name);
    hasher.write_str(&user_ref.namespace);
    hasher.write_str(&user_com.user);
    hasher.finish()
}

/// SwitchableMaps are concurrency-safe data structures that support snapshot-like features.
pub struct SwitchableMaps {
    switchable_table: [DashMap<u64, UserCom>; 2],
    active_idx: AtomicUsize,
}

impl SwitchableMaps {
    fn new() -> Self {
        Self {
            switchable_table: [DashMap::new(), DashMap::new()],
            active_idx: AtomicUsize::new(0),
        }
    }

    pub fn active_len(&self) -> usize {
        let curr_idx = self.active_idx.load(Ordering::Acquire);
        self.switchable_table[curr_idx].len()
    }

    /// Switch the active data structure and return the iterator of the old active data structure.
    /// The memory release depends on the call from caller, so clean_inactive must be called after the switch.
    fn switch(&self) -> Option<dashmap::iter::Iter<u64, UserCom>> {
        let curr_idx = self.active_idx.load(Ordering::Acquire);
        let new_active_idx = (curr_idx + 1) % 2;
        if self
            .active_idx
            .compare_exchange(
                curr_idx,
                new_active_idx,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            Some(self.switchable_table[curr_idx].iter())
        } else {
            None
        }
    }

    fn clean_inactive(&self) {
        let inactive_idx = (self.active_idx.load(Ordering::Acquire) + 1) % 2;
        self.switchable_table[inactive_idx].clear();
    }

    fn put(&self, key: u64, value: UserCom) {
        let active_idx = self.active_idx.load(Ordering::Acquire);
        self.switchable_table[active_idx].insert(key, value);
    }

    pub fn get(&self, key: u64) -> Option<UserCom> {
        let active_idx = self.active_idx.load(Ordering::Acquire);
        self.switchable_table[active_idx]
            .get(&key)
            .map(|v| v.clone())
    }

    pub fn iter_active(&self) -> dashmap::iter::Iter<u64, UserCom> {
        let active_idx = self.active_idx.load(Ordering::Acquire);
        self.switchable_table[active_idx].iter()
    }
}

/// The active users are saved for a certain time window, within which the data is growing,
/// and once the control plane has successfully crawled the data, the memory is freed. By design,
/// freeze calls are low cost, except for CAS of atomic variables whose purpose is to have no blocks in the write path.
pub struct UserActivityWindow {
    data: Arc<SwitchableMaps>,
    size: Arc<AtomicU64>,
    count: Arc<AtomicU64>,
    buf: mpsc::UnboundedSender<(TenantKey, String, u8, u64)>,
}

impl Default for UserActivityWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl UserActivityWindow {
    pub fn new() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let active_pkt = Arc::new(SwitchableMaps::new());
        let size = Arc::new(AtomicU64::new(0));
        let count = Arc::new(AtomicU64::new(0));

        let moved_size = Arc::clone(&size);
        let moved_count = Arc::clone(&count);
        let moved_active_pkt = Arc::clone(&active_pkt);
        tokio::spawn(async move {
            loop {
                let active_user_triple = rx.recv().await;
                if let Some((cluster, user, com_code, ts)) = active_user_triple {
                    let user_com = &UserCom {
                        cluster: Some(cluster),
                        user,
                        com: vec![com_code],
                        com_ts: ts,
                    };
                    let sized = mem::size_of_val(user_com);
                    let user_key = user_com_hash(user_com);
                    moved_active_pkt.put(user_key, user_com.clone());
                    moved_count.fetch_add(1, Ordering::AcqRel);
                    moved_size.fetch_add(sized as u64, Ordering::AcqRel);
                }
            }
        });
        Self {
            data: active_pkt,
            size,
            count,
            buf: tx,
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Acquire)
    }

    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    pub fn freeze(&self) -> Vec<UserCom> {
        let old_data = self.data.switch();
        if let Some(it) = old_data {
            let mut frozen_data = Vec::new();
            for entry in it {
                let bytes = entry.value();
                frozen_data.push(bytes.clone());
            }
            self.data.clean_inactive();
            self.size.store(0, Ordering::Release);
            frozen_data
        } else {
            Vec::new()
        }
    }

    pub fn add_active_users(
        &self,
        cluster: TenantKey,
        active_user: String,
        com_code: u8,
        com_ts: u64,
    ) {
        let tx = &self.buf;
        // TODO avoid user clone.
        let send_rs = tx.send((cluster, active_user, com_code, com_ts));
        if send_rs.is_err() {
            warn!("add_active_users failed {:?}", send_rs.err().unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cp::active_users::UserActivityWindow;
    use crate::prost::common_proto::TenantKey;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    pub async fn test_active_user_com() {
        let region_dic = ["us-east-2", "ap-northeast-1", "us-west-2"];
        let az_dic = ["us-east-2a", "ap-northeast-1a", "us-west-2a"];
        let max_user = 1000;
        let data_of_time_window_arc = Arc::new(UserActivityWindow::new());
        let interval_data_arc = Arc::clone(&data_of_time_window_arc);

        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_micros(10));
            let mut freeze = false;
            loop {
                let active_pkt_len = interval_data_arc.data.active_len();
                if !freeze && active_pkt_len >= 40 {
                    let frozen_data = interval_data_arc.freeze();
                    assert_eq!(active_pkt_len, frozen_data.len());
                    println!(
                        "freeze complete. curr in_active_len={:?}, frozen_size={:?} active_len={}",
                        active_pkt_len,
                        frozen_data.len(),
                        interval_data_arc.data.active_len(),
                    );
                    freeze = true
                }
                let msg_count = interval_data_arc.count() as usize;
                if msg_count == max_user {
                    println!("msg_count = {}", msg_count);
                    break;
                }
                interval.tick().await;
            }
        });
        for i in 0..max_user {
            let region = region_dic[i % 3];
            let az = az_dic[i % 3];
            let cluster = TenantKey {
                region: region.to_string(),
                available_zone: az.to_string(),
                namespace: "default".to_string(),
                cluster_name: "cluster-1".to_string(),
            };
            let user = format!("user-{}", i);
            data_of_time_window_arc.add_active_users(cluster, user, 1, 0);
        }
        let _ = join.await;
    }
}
