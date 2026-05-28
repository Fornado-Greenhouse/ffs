//! Per-peer mount tracking: which atoms the daemon has pulled from
//! which peer, so the `from/<peer>/` projection can filter to that
//! peer's contributions and revocation can unmount cleanly.
//!
//! Pulled atoms enter the local store with their original
//! signatures preserved (we trust them via the capability check that
//! happened at the source). What lives here is the *attribution*
//! layer: a `peer_id → set of atom hashes` map. On revocation, the
//! daemon drops the peer's mount; the atoms themselves remain in
//! the store (they're cryptographic facts) but no longer surface
//! under `from/<peer>/`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use ffs_core::Multihash;

#[async_trait]
pub trait PeerMountStore: Send + Sync {
    /// Record that `atom_hash` was pulled from `peer_id`.
    async fn record(&self, peer_id: &str, atom_hash: Multihash);
    /// List every atom hash currently mounted from `peer_id`.
    async fn list(&self, peer_id: &str) -> Vec<Multihash>;
    /// Drop every atom attribution for `peer_id`. The atoms remain
    /// in the underlying store; only their `from/<peer>/`
    /// surfacing goes away.
    async fn unmount(&self, peer_id: &str);
    /// Count atoms currently mounted from `peer_id`.
    async fn count(&self, peer_id: &str) -> usize;
}

#[derive(Debug, Default)]
pub struct InMemoryPeerMount {
    mounts: Mutex<HashMap<String, HashSet<Multihash>>>,
}

impl InMemoryPeerMount {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl PeerMountStore for InMemoryPeerMount {
    async fn record(&self, peer_id: &str, atom_hash: Multihash) {
        self.mounts
            .lock()
            .await
            .entry(peer_id.to_string())
            .or_default()
            .insert(atom_hash);
    }

    async fn list(&self, peer_id: &str) -> Vec<Multihash> {
        let guard = self.mounts.lock().await;
        match guard.get(peer_id) {
            Some(set) => set.iter().cloned().collect(),
            None => Vec::new(),
        }
    }

    async fn unmount(&self, peer_id: &str) {
        self.mounts.lock().await.remove(peer_id);
    }

    async fn count(&self, peer_id: &str) -> usize {
        self.mounts
            .lock()
            .await
            .get(peer_id)
            .map(|s| s.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: &[u8]) -> Multihash {
        Multihash::blake3_of(b)
    }

    #[tokio::test]
    async fn record_and_list_round_trip() {
        let m = InMemoryPeerMount::new();
        m.record("alice", h(b"a")).await;
        m.record("alice", h(b"b")).await;
        m.record("bob", h(b"x")).await;
        assert_eq!(m.count("alice").await, 2);
        assert_eq!(m.count("bob").await, 1);
        assert_eq!(m.count("carol").await, 0);
    }

    #[tokio::test]
    async fn record_is_idempotent_per_hash() {
        let m = InMemoryPeerMount::new();
        m.record("alice", h(b"a")).await;
        m.record("alice", h(b"a")).await;
        m.record("alice", h(b"a")).await;
        assert_eq!(m.count("alice").await, 1);
    }

    #[tokio::test]
    async fn unmount_clears_only_target_peer() {
        let m = InMemoryPeerMount::new();
        m.record("alice", h(b"a")).await;
        m.record("bob", h(b"x")).await;
        m.unmount("alice").await;
        assert_eq!(m.count("alice").await, 0);
        assert_eq!(m.count("bob").await, 1);
    }
}
