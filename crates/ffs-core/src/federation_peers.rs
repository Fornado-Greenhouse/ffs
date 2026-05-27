//! Federation peer state: the bridge contracts the daemon has
//! established with other substrates. Each peer record carries the
//! peer's public key (their substrate identity), the pinned
//! certificate fingerprint (used to authenticate inbound mTLS), the
//! exchanged capability atom hashes (in both directions), and pull
//! watermarks per capability.
//!
//! Backs the `federation_peers` SQLite table from ADR-016. For MVP
//! this is in-memory only; cross-restart persistence lands when the
//! transport stops being trait-mocked in tests.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{Iso8601, Multihash, PublicKey};

#[derive(Debug, thiserror::Error)]
pub enum FederationPeerError {
    #[error("peer not found: {0}")]
    NotFound(String),
    #[error("fingerprint mismatch for peer {peer_id}: expected {expected}, got {got}")]
    FingerprintMismatch {
        peer_id: String,
        expected: String,
        got: String,
    },
}

/// A single registered federation peer.
///
/// `peer_id` is the multibase-encoded public key (matches the
/// subject CN of the peer's TLS certificate per ADR-020). The
/// fingerprint is the BLAKE3 hash of the peer's certificate DER —
/// pinned out-of-band before the first handshake and rotated via
/// `bridge.rotate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationPeer {
    pub peer_id: String,
    pub peer_pubkey: PublicKey,
    pub endpoint: String,
    pub cert_fingerprint: Multihash,
    /// Capability atom that this side has granted the peer (allows
    /// the peer to read scoped atoms from us). `None` until the
    /// handshake completes.
    pub our_capability: Option<Multihash>,
    /// Capability atom the peer has granted us (allows us to read
    /// scoped atoms from them).
    pub their_capability: Option<Multihash>,
    /// Predicate vocabularies the peer advertised during handshake.
    /// Informs which `from/<peer>/` paths are renderable.
    pub vocab: Vec<String>,
    /// Watermark per capability hash → tx_time of the last pulled
    /// atom. The next pull asks for atoms above this watermark.
    pub watermarks: HashMap<String, Iso8601>,
    pub established_at: Iso8601,
    pub last_seen_at: Option<Iso8601>,
}

#[async_trait]
pub trait FederationPeerStore: Send + Sync {
    async fn upsert(&self, peer: FederationPeer) -> Result<(), FederationPeerError>;
    async fn get(&self, peer_id: &str) -> Option<FederationPeer>;
    async fn list(&self) -> Vec<FederationPeer>;
    /// Look a peer up by certificate fingerprint — the inbound mTLS
    /// path uses this to map a client cert to its peer record.
    async fn find_by_fingerprint(&self, fingerprint: &Multihash) -> Option<FederationPeer>;
    /// Rotate the pinned fingerprint, requiring the old fingerprint
    /// to match the current pin so a stranger can't forge a rotation.
    async fn rotate_fingerprint(
        &self,
        peer_id: &str,
        expected_old: &Multihash,
        new_fingerprint: Multihash,
    ) -> Result<(), FederationPeerError>;
    async fn remove(&self, peer_id: &str) -> Result<(), FederationPeerError>;
}

#[derive(Debug, Default)]
pub struct InMemoryFederationPeerStore {
    peers: Mutex<HashMap<String, FederationPeer>>,
}

impl InMemoryFederationPeerStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl FederationPeerStore for InMemoryFederationPeerStore {
    async fn upsert(&self, peer: FederationPeer) -> Result<(), FederationPeerError> {
        let mut guard = self.peers.lock().await;
        guard.insert(peer.peer_id.clone(), peer);
        Ok(())
    }

    async fn get(&self, peer_id: &str) -> Option<FederationPeer> {
        self.peers.lock().await.get(peer_id).cloned()
    }

    async fn list(&self) -> Vec<FederationPeer> {
        let guard = self.peers.lock().await;
        let mut out: Vec<FederationPeer> = guard.values().cloned().collect();
        out.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        out
    }

    async fn find_by_fingerprint(&self, fingerprint: &Multihash) -> Option<FederationPeer> {
        self.peers
            .lock()
            .await
            .values()
            .find(|p| &p.cert_fingerprint == fingerprint)
            .cloned()
    }

    async fn rotate_fingerprint(
        &self,
        peer_id: &str,
        expected_old: &Multihash,
        new_fingerprint: Multihash,
    ) -> Result<(), FederationPeerError> {
        let mut guard = self.peers.lock().await;
        let peer = guard
            .get_mut(peer_id)
            .ok_or_else(|| FederationPeerError::NotFound(peer_id.to_string()))?;
        if &peer.cert_fingerprint != expected_old {
            return Err(FederationPeerError::FingerprintMismatch {
                peer_id: peer_id.to_string(),
                expected: peer.cert_fingerprint.to_multibase(),
                got: expected_old.to_multibase(),
            });
        }
        peer.cert_fingerprint = new_fingerprint;
        Ok(())
    }

    async fn remove(&self, peer_id: &str) -> Result<(), FederationPeerError> {
        let mut guard = self.peers.lock().await;
        if guard.remove(peer_id).is_none() {
            return Err(FederationPeerError::NotFound(peer_id.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(seed: u8) -> PublicKey {
        let bytes = [seed; 32];
        PublicKey::from_bytes(bytes)
    }

    fn fp(seed: u8) -> Multihash {
        Multihash::blake3_of(&[seed; 32])
    }

    fn ts(s: &str) -> Iso8601 {
        Iso8601::new(s).unwrap()
    }

    fn make_peer(id: &str, seed: u8) -> FederationPeer {
        FederationPeer {
            peer_id: id.to_string(),
            peer_pubkey: pk(seed),
            endpoint: format!("https://{id}.example/federation/v1"),
            cert_fingerprint: fp(seed),
            our_capability: None,
            their_capability: None,
            vocab: vec!["contact.person".into(), "note".into()],
            watermarks: HashMap::new(),
            established_at: ts("2026-05-27T08:00:00Z"),
            last_seen_at: None,
        }
    }

    #[tokio::test]
    async fn upsert_and_get_round_trip() {
        let store = InMemoryFederationPeerStore::new();
        store.upsert(make_peer("alice", 1)).await.unwrap();
        let p = store.get("alice").await.unwrap();
        assert_eq!(p.endpoint, "https://alice.example/federation/v1");
    }

    #[tokio::test]
    async fn find_by_fingerprint_matches_inbound_cert() {
        let store = InMemoryFederationPeerStore::new();
        store.upsert(make_peer("alice", 1)).await.unwrap();
        store.upsert(make_peer("bob", 2)).await.unwrap();
        let p = store.find_by_fingerprint(&fp(2)).await.unwrap();
        assert_eq!(p.peer_id, "bob");
    }

    #[tokio::test]
    async fn find_by_fingerprint_misses_unregistered_cert() {
        let store = InMemoryFederationPeerStore::new();
        store.upsert(make_peer("alice", 1)).await.unwrap();
        assert!(store.find_by_fingerprint(&fp(99)).await.is_none());
    }

    #[tokio::test]
    async fn rotate_fingerprint_swaps_on_correct_old_pin() {
        let store = InMemoryFederationPeerStore::new();
        store.upsert(make_peer("alice", 1)).await.unwrap();
        store
            .rotate_fingerprint("alice", &fp(1), fp(7))
            .await
            .unwrap();
        let p = store.get("alice").await.unwrap();
        assert_eq!(p.cert_fingerprint, fp(7));
    }

    #[tokio::test]
    async fn rotate_fingerprint_rejects_wrong_old_pin() {
        let store = InMemoryFederationPeerStore::new();
        store.upsert(make_peer("alice", 1)).await.unwrap();
        let err = store
            .rotate_fingerprint("alice", &fp(99), fp(7))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            FederationPeerError::FingerprintMismatch { .. }
        ));
        // And the pin is unchanged.
        assert_eq!(store.get("alice").await.unwrap().cert_fingerprint, fp(1));
    }

    #[tokio::test]
    async fn list_is_sorted_by_peer_id() {
        let store = InMemoryFederationPeerStore::new();
        for (id, s) in [("charlie", 3), ("alice", 1), ("bob", 2)] {
            store.upsert(make_peer(id, s)).await.unwrap();
        }
        let ids: Vec<_> = store.list().await.into_iter().map(|p| p.peer_id).collect();
        assert_eq!(ids, vec!["alice", "bob", "charlie"]);
    }
}
