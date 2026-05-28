//! `FederationClient` — the outbound transport abstraction.
//!
//! Per TechSpec § Unit Tests, federation is mocked at the trait
//! level in unit / integration tests: a `FederationClient` makes the
//! same `handshake` / `rotate` calls regardless of whether the
//! transport underneath is reqwest+rustls (production) or a direct
//! in-process pair (tests). The production reqwest binding is wired
//! by task_22 of the onboarding scripts; this module ships the
//! trait + an in-memory implementation that pairs two
//! `FederationContext`s directly.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use ffs_core::{AtomEnvelope, EntityId, Iso8601, Multihash};

use crate::handshake::{HandshakeRequest, HandshakeResponse, RotateRequest, RotateResponse};
use crate::server::{
    FederationContext, IntersectionResponse, RevocationNoticeAck, ServerError, handle_get_atom,
    handle_handshake, handle_intersection, handle_pull_atoms, handle_revocation_notice,
    handle_rotate,
};

#[derive(Debug, thiserror::Error)]
pub enum FederationClientError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("server: {0}")]
    Server(#[from] ServerError),
    #[error("peer not registered: {0}")]
    UnregisteredPeer(String),
}

#[async_trait]
pub trait FederationClient: Send + Sync {
    /// Send a handshake to a peer.
    ///
    /// `endpoint` is the peer's federation URL (`https://...`),
    /// `our_cert_fingerprint` is the fingerprint of *our* TLS cert
    /// that the peer will see at the TLS layer — passed explicitly
    /// so the in-memory client can simulate mTLS without standing
    /// up a real TLS stack.
    async fn handshake(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        req: HandshakeRequest,
    ) -> Result<HandshakeResponse, FederationClientError>;

    /// Send a bridge.rotate to a peer.
    async fn rotate(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        req: RotateRequest,
    ) -> Result<RotateResponse, FederationClientError>;

    /// Pull atoms from a peer after `since`, filtered by the
    /// receiver-pinned `capability_hash`. Per ADR-020 the source
    /// applies capability filtering; the receiver re-verifies each
    /// returned envelope's signature before insert. Returns
    /// oldest-first so the caller advances its watermark
    /// monotonically.
    async fn pull_atoms(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        since: Option<&Iso8601>,
        capability_hash: &Multihash,
    ) -> Result<Vec<AtomEnvelope>, FederationClientError>;

    /// Fetch a single atom by content hash.
    async fn get_atom(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        hash: &Multihash,
    ) -> Result<Option<AtomEnvelope>, FederationClientError>;

    /// Ask whether the peer holds any capability-visible atoms about
    /// `entity`. Pairs with the local check to compute the
    /// intersection set.
    async fn intersection(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        entity: &EntityId,
    ) -> Result<IntersectionResponse, FederationClientError>;

    /// Best-effort revocation push. The peer is not required to
    /// honor it; if they do, propagation latency drops from
    /// heartbeat to seconds.
    async fn post_revocation_notice(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        capability_hash: &Multihash,
    ) -> Result<RevocationNoticeAck, FederationClientError>;
}

/// In-process federation transport. The registry maps each peer's
/// endpoint to a `FederationContext` — the test harness uses this
/// to pair two daemons in the same process without sockets.
///
/// In production, swap this for a reqwest-based client. The trait
/// surface is identical so the daemon code stays unchanged.
#[derive(Default, Clone)]
pub struct InMemoryFederationClient {
    /// endpoint → (peer's context).
    routes: Arc<Mutex<std::collections::HashMap<String, FederationContext>>>,
}

impl InMemoryFederationClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a peer's context against the URL the client will use
    /// to reach them. Tests call this after standing up both daemons.
    pub async fn route(&self, endpoint: impl Into<String>, ctx: FederationContext) {
        self.routes.lock().await.insert(endpoint.into(), ctx);
    }
}

#[async_trait]
impl FederationClient for InMemoryFederationClient {
    async fn handshake(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        req: HandshakeRequest,
    ) -> Result<HandshakeResponse, FederationClientError> {
        let routes = self.routes.lock().await;
        let ctx = routes
            .get(endpoint)
            .ok_or_else(|| FederationClientError::Transport(format!("no route for {endpoint}")))?
            .clone();
        drop(routes);
        let resp = handle_handshake(&ctx, our_cert_fingerprint, req).await?;
        Ok(resp)
    }

    async fn rotate(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        req: RotateRequest,
    ) -> Result<RotateResponse, FederationClientError> {
        let routes = self.routes.lock().await;
        let ctx = routes
            .get(endpoint)
            .ok_or_else(|| FederationClientError::Transport(format!("no route for {endpoint}")))?
            .clone();
        drop(routes);
        let resp = handle_rotate(&ctx, our_cert_fingerprint, req).await?;
        Ok(resp)
    }

    async fn pull_atoms(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        since: Option<&Iso8601>,
        capability_hash: &Multihash,
    ) -> Result<Vec<AtomEnvelope>, FederationClientError> {
        let ctx = self.route_to(endpoint).await?;
        let atoms = handle_pull_atoms(&ctx, our_cert_fingerprint, since, capability_hash).await?;
        Ok(atoms)
    }

    async fn get_atom(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        hash: &Multihash,
    ) -> Result<Option<AtomEnvelope>, FederationClientError> {
        let ctx = self.route_to(endpoint).await?;
        let env = handle_get_atom(&ctx, our_cert_fingerprint, hash).await?;
        Ok(env)
    }

    async fn intersection(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        entity: &EntityId,
    ) -> Result<IntersectionResponse, FederationClientError> {
        let ctx = self.route_to(endpoint).await?;
        let resp = handle_intersection(&ctx, our_cert_fingerprint, entity).await?;
        Ok(resp)
    }

    async fn post_revocation_notice(
        &self,
        endpoint: &str,
        our_cert_fingerprint: &Multihash,
        capability_hash: &Multihash,
    ) -> Result<RevocationNoticeAck, FederationClientError> {
        let ctx = self.route_to(endpoint).await?;
        let ack = handle_revocation_notice(&ctx, our_cert_fingerprint, capability_hash).await?;
        Ok(ack)
    }
}

impl InMemoryFederationClient {
    async fn route_to(&self, endpoint: &str) -> Result<FederationContext, FederationClientError> {
        let routes = self.routes.lock().await;
        routes
            .get(endpoint)
            .cloned()
            .ok_or_else(|| FederationClientError::Transport(format!("no route for {endpoint}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handshake::HANDSHAKE_PROTOCOL_VERSION;
    use ed25519_dalek::SigningKey;
    use ffs_core::federation_peers::{
        FederationPeer, FederationPeerStore, InMemoryFederationPeerStore,
    };
    use ffs_core::{Iso8601, PublicKey};
    use std::collections::HashMap;

    fn pubkey_from_key(k: &SigningKey) -> PublicKey {
        PublicKey::from_verifying(&k.verifying_key())
    }
    fn ts(s: &str) -> Iso8601 {
        Iso8601::new(s).unwrap()
    }

    #[tokio::test]
    async fn handshake_round_trips_via_in_memory_client() {
        let initiator_key = SigningKey::from_bytes(&[5u8; 32]);
        let initiator_fp = Multihash::blake3_of(&[5u8; 32]);
        let responder_key = SigningKey::from_bytes(&[6u8; 32]);

        let peers = Arc::new(InMemoryFederationPeerStore::new());
        peers
            .upsert(FederationPeer {
                peer_id: "alice".into(),
                peer_pubkey: pubkey_from_key(&initiator_key),
                endpoint: "https://alice/".into(),
                cert_fingerprint: initiator_fp.clone(),
                our_capability: Some(Multihash::blake3_of(b"our-cap")),
                their_capability: None,
                vocab: vec![],
                watermarks: HashMap::new(),
                established_at: ts("2026-05-27T08:00:00Z"),
                last_seen_at: None,
            })
            .await
            .unwrap();
        let store: Arc<dyn ffs_core::store::AtomStore> =
            Arc::new(ffs_core::store::MemAtomStore::new());
        let ctx = FederationContext {
            responder_pubkey: pubkey_from_key(&responder_key),
            responder_capability: Multihash::blake3_of(b"r-cap"),
            responder_vocab: vec!["contact.person".into()],
            responder_anchor: ts("2026-05-27T08:00:00Z"),
            peers,
            store,
        };
        let client = InMemoryFederationClient::new();
        client.route("https://bob/", ctx).await;

        let req = HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: pubkey_from_key(&initiator_key),
            initiator_capability: Multihash::blake3_of(b"i-cap"),
            initiator_vocab: vec!["contact.person".into()],
            initiator_anchor: ts("2026-05-27T08:00:00Z"),
        };
        let resp = client
            .handshake("https://bob/", &initiator_fp, req)
            .await
            .unwrap();
        assert_eq!(resp.protocol_version, HANDSHAKE_PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn unrouted_endpoint_yields_transport_error() {
        let client = InMemoryFederationClient::new();
        let req = HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: pubkey_from_key(&SigningKey::from_bytes(&[1u8; 32])),
            initiator_capability: Multihash::blake3_of(b"x"),
            initiator_vocab: vec![],
            initiator_anchor: ts("2026-05-27T08:00:00Z"),
        };
        let err = client
            .handshake("https://nope/", &Multihash::blake3_of(b"f"), req)
            .await
            .unwrap_err();
        assert!(matches!(err, FederationClientError::Transport(_)));
    }
}
