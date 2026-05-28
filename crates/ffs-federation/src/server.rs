//! Server-side handshake/rotate handlers. Pure functions that take a
//! `FederationContext` (substrate state + responder identity) and a
//! request, perform capability + signature validation, and return
//! the response. The axum binding that exposes them over HTTPS is
//! wired in the daemon binary by task_22's onboarding scripts —
//! deferred from MVP per TechSpec § Unit Tests, which calls for
//! trait-mocked transport.
//!
//! Keeping the handlers as plain async functions has two benefits:
//! (1) tests can drive them directly without standing up a server,
//! and (2) the same handler is reusable across multiple transports
//! (HTTPS, in-process for tests, possibly a future Unix-socket
//! federation peer).

use std::sync::Arc;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use ffs_core::capability::{self, CapabilityClaim, Decision, Target};
use ffs_core::federation_peers::FederationPeerStore;
use ffs_core::store::AtomStore;
use ffs_core::{AtomEnvelope, EntityId, Iso8601, Multihash, PredicateName, PublicKey};

use crate::handshake::{
    HANDSHAKE_PROTOCOL_VERSION, HandshakeError, HandshakeRequest, HandshakeResponse, RotateRequest,
    RotateResponse, rotation_signing_bytes, validate_inbound,
};

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("handshake: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("unregistered peer: cert fingerprint {0} is not pinned")]
    UnregisteredPeer(String),
    #[error("rotation signature verification failed: {0}")]
    RotationBadSignature(String),
    #[error("peer store: {0}")]
    PeerStore(#[from] ffs_core::federation_peers::FederationPeerError),
    #[error("capability {0} not found in store")]
    CapabilityUnknown(String),
    #[error("capability grantee does not match the inbound peer")]
    CapabilityMismatch,
    #[error("io: {0}")]
    Io(String),
}

/// Context the handlers need to do their job. The daemon constructs
/// one of these at startup; both the HTTPS server and the in-process
/// `InMemoryFederationClient` use it.
#[derive(Clone)]
pub struct FederationContext {
    /// This substrate's public key (matches its TLS cert CN).
    pub responder_pubkey: PublicKey,
    /// The capability atom this substrate has authored granting peers
    /// scoped read access. Sent back in the handshake response so the
    /// initiator can pull from us.
    pub responder_capability: Multihash,
    /// The predicate vocabulary this substrate can render and serve.
    pub responder_vocab: Vec<String>,
    /// Current substrate anchor (latest atom's tx_time, or
    /// substrate-start time if empty). Sent so the peer can seed its
    /// inbound watermark.
    pub responder_anchor: Iso8601,
    /// Peer store; handlers consult it for fingerprint pinning and
    /// update it when handshake succeeds.
    pub peers: Arc<dyn FederationPeerStore>,
    /// Local atom store. The pull / get-atom / intersection handlers
    /// read from it; capability filtering happens at the source per
    /// ADR-020 so out-of-scope atoms never leave the substrate.
    pub store: Arc<dyn AtomStore>,
}

/// Handler for `POST /federation/v1/handshake`.
///
/// `client_cert_fingerprint` is the BLAKE3 hash of the TLS client
/// cert as observed by the server's rustls layer. The handler maps
/// it to a registered peer (created out-of-band via the CLI's
/// `ffs federation peer add`) before accepting the in-band exchange.
/// Unregistered fingerprints get `UnregisteredPeer` — the production
/// rustls verifier rejects them earlier, but this defense-in-depth
/// check protects the in-process test path.
pub async fn handle_handshake(
    ctx: &FederationContext,
    client_cert_fingerprint: &Multihash,
    req: HandshakeRequest,
) -> Result<HandshakeResponse, ServerError> {
    // Pinned-fingerprint check.
    let peer = ctx
        .peers
        .find_by_fingerprint(client_cert_fingerprint)
        .await
        .ok_or_else(|| ServerError::UnregisteredPeer(client_cert_fingerprint.to_multibase()))?;

    // The cert's subject CN must match the announced initiator_pubkey
    // — defends against a stranger using a registered peer's cert
    // CN with their own substrate identity. (In production this is
    // also bound at the TLS layer; here it's belt-and-suspenders.)
    if peer.peer_pubkey != req.initiator_pubkey {
        return Err(HandshakeError::PeerIdMismatch {
            cert: peer.peer_pubkey.to_multibase(),
            announced: req.initiator_pubkey.to_multibase(),
        }
        .into());
    }

    // Validate version + vocab intersection (returns the intersection
    // but the responder echoes its own full vocab back; the initiator
    // computes its half of the intersection on receipt).
    let _intersection = validate_inbound(&req, &ctx.responder_vocab)?;

    // Persist the bridge contract on our side: stamp the initiator's
    // capability hash + vocab onto the peer record.
    let mut updated = peer.clone();
    updated.their_capability = Some(req.initiator_capability.clone());
    updated.vocab = req.initiator_vocab.clone();
    ctx.peers.upsert(updated).await?;

    Ok(HandshakeResponse {
        protocol_version: HANDSHAKE_PROTOCOL_VERSION,
        responder_pubkey: ctx.responder_pubkey.clone(),
        responder_capability: ctx.responder_capability.clone(),
        responder_vocab: ctx.responder_vocab.clone(),
        responder_anchor: ctx.responder_anchor.clone(),
    })
}

/// Handler for `POST /federation/v1/bridge.rotate`.
///
/// Verifies the OLD signing key's signature over the new fingerprint
/// (domain-separated per `rotation_signing_bytes`). If valid,
/// updates the pinned fingerprint atomically via the peer store's
/// `rotate_fingerprint` method, which itself re-checks the old pin.
pub async fn handle_rotate(
    ctx: &FederationContext,
    client_cert_fingerprint: &Multihash,
    req: RotateRequest,
) -> Result<RotateResponse, ServerError> {
    let peer = ctx
        .peers
        .find_by_fingerprint(client_cert_fingerprint)
        .await
        .ok_or_else(|| ServerError::UnregisteredPeer(client_cert_fingerprint.to_multibase()))?;

    // Verify signature using the peer's CURRENT (old) pubkey over
    // (old_fingerprint, new_fingerprint). Both sides know the old
    // fingerprint (peer's pinned cert) so neither needs to name the
    // other.
    let verifying = peer
        .peer_pubkey
        .to_verifying()
        .map_err(|e| ServerError::RotationBadSignature(e.to_string()))?;
    let sig_bytes: [u8; 64] = req
        .old_signature
        .as_slice()
        .try_into()
        .map_err(|_| ServerError::RotationBadSignature("signature must be 64 bytes".into()))?;
    let signature = Signature::from_bytes(&sig_bytes);
    let signed_bytes = rotation_signing_bytes(&peer.cert_fingerprint, &req.new_fingerprint);
    verify_sig(&verifying, &signed_bytes, &signature)
        .map_err(|e| ServerError::RotationBadSignature(e.to_string()))?;

    // Atomic swap with old-pin re-check.
    ctx.peers
        .rotate_fingerprint(&peer.peer_id, &peer.cert_fingerprint, req.new_fingerprint)
        .await?;
    Ok(RotateResponse { accepted: true })
}

fn verify_sig(
    key: &VerifyingKey,
    msg: &[u8],
    sig: &Signature,
) -> Result<(), ed25519_dalek::SignatureError> {
    key.verify(msg, sig)
}

// ---- pull / get_atom / intersection / revocation handlers ----

/// Per-pull limit. Protects the responder from a runaway requester
/// pulling the entire substrate in one request; the client paginates
/// by advancing its watermark.
pub const MAX_PULL_PAGE: usize = 1_000;

/// Handler for `GET /federation/v1/atoms?since=<tx_time>&capability=<hash>`.
///
/// Returns atoms whose `tx_time > since` (or all when `since` is
/// None) that the peer's pinned capability authorizes them to read.
/// Capability filtering happens AT THE SOURCE — atoms outside the
/// peer's tier or predicate scope never cross the wire.
///
/// The result is sorted oldest-first so the caller can advance its
/// watermark monotonically: `new_watermark = last_returned.tx_time`.
pub async fn handle_pull_atoms(
    ctx: &FederationContext,
    client_cert_fingerprint: &Multihash,
    since: Option<&Iso8601>,
    capability_hash: &Multihash,
) -> Result<Vec<AtomEnvelope>, ServerError> {
    let peer = ctx
        .peers
        .find_by_fingerprint(client_cert_fingerprint)
        .await
        .ok_or_else(|| ServerError::UnregisteredPeer(client_cert_fingerprint.to_multibase()))?;

    // Look up the capability atom the peer announced. The pin on its
    // hash makes sure a peer can't substitute a different capability
    // mid-stream; if we cannot find the atom locally, the request is
    // rejected (the peer's view is out of sync with ours).
    let cap_env = ctx
        .store
        .get(capability_hash)
        .map_err(|e| ServerError::Io(e.to_string()))?
        .ok_or_else(|| ServerError::CapabilityUnknown(capability_hash.to_multibase()))?;
    let cap = CapabilityClaim::from_envelope(&cap_env)
        .map_err(|e| ServerError::Io(format!("malformed capability: {e}")))?;
    if cap.grantee != peer.peer_pubkey {
        return Err(ServerError::CapabilityMismatch);
    }

    // Walk every predicate in the capability's scope (or the
    // responder's whole vocab when scope.predicates is None). For
    // each, list atoms after `since` and filter by capability cover.
    let predicates: Vec<PredicateName> = match cap.scope.predicates.clone() {
        Some(ps) => ps,
        None => ctx
            .responder_vocab
            .iter()
            .map(|s| PredicateName::new(s.clone()))
            .collect(),
    };
    let now = current_iso8601();
    let mut out: Vec<AtomEnvelope> = Vec::new();
    for pred in predicates {
        let atoms = ctx
            .store
            .list_by_predicate(&pred, since, MAX_PULL_PAGE)
            .map_err(|e| ServerError::Io(e.to_string()))?;
        for env in atoms {
            // Re-evaluate against the substrate's capability evaluator
            // so this serves the source-of-truth check (not just a
            // hash-equality match). Out-of-tier atoms get dropped here.
            let target = Target {
                predicate: env.predicate.clone(),
                entity: env.entity.clone(),
                classification: Some(env.classification.clone()),
                tier: None,
            };
            let decision = capability::evaluate(
                &*ctx.store,
                &peer.peer_pubkey,
                capability::Action::Read,
                &target,
                &now,
            )
            .map_err(|e| ServerError::Io(e.to_string()))?;
            if matches!(decision, Decision::Allow { .. }) {
                out.push(env);
            }
        }
    }

    // Sort oldest-first by tx_time so the receiver can advance its
    // watermark monotonically; break ties on content hash for stable
    // ordering.
    out.sort_by(|a, b| {
        a.tx_time.as_str().cmp(b.tx_time.as_str()).then_with(|| {
            match (a.content_hash(), b.content_hash()) {
                (Ok(ha), Ok(hb)) => ha.to_multibase().cmp(&hb.to_multibase()),
                _ => std::cmp::Ordering::Equal,
            }
        })
    });
    out.truncate(MAX_PULL_PAGE);
    Ok(out)
}

/// Handler for `GET /federation/v1/atom/<hash>`. Returns a single
/// atom by content hash if the peer's pinned capability covers it.
pub async fn handle_get_atom(
    ctx: &FederationContext,
    client_cert_fingerprint: &Multihash,
    hash: &Multihash,
) -> Result<Option<AtomEnvelope>, ServerError> {
    let peer = ctx
        .peers
        .find_by_fingerprint(client_cert_fingerprint)
        .await
        .ok_or_else(|| ServerError::UnregisteredPeer(client_cert_fingerprint.to_multibase()))?;
    let Some(env) = ctx
        .store
        .get(hash)
        .map_err(|e| ServerError::Io(e.to_string()))?
    else {
        return Ok(None);
    };
    let target = Target {
        predicate: env.predicate.clone(),
        entity: env.entity.clone(),
        classification: Some(env.classification.clone()),
        tier: None,
    };
    let now = current_iso8601();
    let decision = capability::evaluate(
        &*ctx.store,
        &peer.peer_pubkey,
        capability::Action::Read,
        &target,
        &now,
    )
    .map_err(|e| ServerError::Io(e.to_string()))?;
    Ok(match decision {
        Decision::Allow { .. } => Some(env),
        Decision::Deny { .. } => None,
    })
}

/// Handler for `GET /federation/v1/intersection/<entity>`. Returns
/// true iff the substrate has atoms for the entity at any classification
/// the peer's capability authorizes. Symmetrically, when both sides
/// return true for the same entity, the entity is in the
/// intersection.
///
/// The returned shape carries the responder's pubkey so a downstream
/// aggregator can fuse responses without losing peer attribution.
pub async fn handle_intersection(
    ctx: &FederationContext,
    client_cert_fingerprint: &Multihash,
    entity: &EntityId,
) -> Result<IntersectionResponse, ServerError> {
    let peer = ctx
        .peers
        .find_by_fingerprint(client_cert_fingerprint)
        .await
        .ok_or_else(|| ServerError::UnregisteredPeer(client_cert_fingerprint.to_multibase()))?;
    let atoms = ctx
        .store
        .list_by_entity(entity, None, None)
        .map_err(|e| ServerError::Io(e.to_string()))?;
    let now = current_iso8601();
    let mut visible = false;
    for env in &atoms {
        let target = Target {
            predicate: env.predicate.clone(),
            entity: env.entity.clone(),
            classification: Some(env.classification.clone()),
            tier: None,
        };
        let decision = capability::evaluate(
            &*ctx.store,
            &peer.peer_pubkey,
            capability::Action::Read,
            &target,
            &now,
        )
        .map_err(|e| ServerError::Io(e.to_string()))?;
        if matches!(decision, Decision::Allow { .. }) {
            visible = true;
            break;
        }
    }
    Ok(IntersectionResponse {
        present: visible,
        responder_pubkey: ctx.responder_pubkey.clone(),
    })
}

/// Handler for `POST /federation/v1/revocation-notice`. The opt-in
/// immediate-revocation push from ADR-020 — peers don't have to
/// honor it, but accepting it cuts revocation propagation latency
/// from heartbeat to ~seconds. MVP just records the notice via
/// tracing; the puller already detects revocation on the next pull
/// when the supersession lands.
pub async fn handle_revocation_notice(
    ctx: &FederationContext,
    client_cert_fingerprint: &Multihash,
    capability_hash: &Multihash,
) -> Result<RevocationNoticeAck, ServerError> {
    let peer = ctx
        .peers
        .find_by_fingerprint(client_cert_fingerprint)
        .await
        .ok_or_else(|| ServerError::UnregisteredPeer(client_cert_fingerprint.to_multibase()))?;
    tracing::info!(
        peer_id = %peer.peer_id,
        capability = %capability_hash.to_multibase(),
        "revocation_notice_received"
    );
    Ok(RevocationNoticeAck { received: true })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct IntersectionResponse {
    pub present: bool,
    pub responder_pubkey: PublicKey,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RevocationNoticeAck {
    pub received: bool,
}

fn current_iso8601() -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let now = time::OffsetDateTime::now_utc();
    let s = now
        .format(&Fmt::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    Iso8601::new(s).expect("formatted ISO8601 must parse")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use ed25519_dalek::SigningKey;
    use ffs_core::federation_peers::{FederationPeer, InMemoryFederationPeerStore};
    use std::collections::HashMap;

    fn pubkey_from_key(k: &SigningKey) -> PublicKey {
        PublicKey::from_verifying(&k.verifying_key())
    }

    fn ts(s: &str) -> Iso8601 {
        Iso8601::new(s).unwrap()
    }

    fn fp(b: &[u8]) -> Multihash {
        Multihash::blake3_of(b)
    }

    async fn setup(
        responder_vocab: Vec<&str>,
        peer_seed: u8,
    ) -> (
        FederationContext,
        Multihash,
        SigningKey,
        Arc<InMemoryFederationPeerStore>,
    ) {
        let peers = Arc::new(InMemoryFederationPeerStore::new());
        let initiator_key = SigningKey::from_bytes(&[peer_seed; 32]);
        let initiator_fp = fp(&[peer_seed; 32]);
        let peer = FederationPeer {
            peer_id: "alice".into(),
            peer_pubkey: pubkey_from_key(&initiator_key),
            endpoint: "https://alice.example/federation/v1".into(),
            cert_fingerprint: initiator_fp.clone(),
            our_capability: Some(fp(b"our-cap")),
            their_capability: None,
            vocab: vec![],
            watermarks: HashMap::new(),
            established_at: ts("2026-05-27T08:00:00Z"),
            last_seen_at: None,
        };
        peers.upsert(peer).await.unwrap();

        let responder_key = SigningKey::from_bytes(&[99u8; 32]);
        let store: Arc<dyn AtomStore> = Arc::new(ffs_core::store::MemAtomStore::new());
        let ctx = FederationContext {
            responder_pubkey: pubkey_from_key(&responder_key),
            responder_capability: fp(b"responder-cap"),
            responder_vocab: responder_vocab.into_iter().map(String::from).collect(),
            responder_anchor: ts("2026-05-27T08:00:00Z"),
            peers: peers.clone(),
            store,
        };
        (ctx, initiator_fp, initiator_key, peers)
    }

    #[tokio::test]
    async fn handshake_succeeds_and_updates_peer_record() {
        let (ctx, fp_, initiator_key, peers) = setup(vec!["contact.person", "note"], 5).await;
        let req = HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: pubkey_from_key(&initiator_key),
            initiator_capability: fp(b"their-cap"),
            initiator_vocab: vec!["contact.person".into(), "decision".into()],
            initiator_anchor: ts("2026-05-27T08:00:00Z"),
        };
        let resp = handle_handshake(&ctx, &fp_, req).await.unwrap();
        assert_eq!(resp.protocol_version, HANDSHAKE_PROTOCOL_VERSION);
        // Peer record updated.
        let updated = peers.get("alice").await.unwrap();
        assert_eq!(updated.their_capability, Some(fp(b"their-cap")));
        assert_eq!(updated.vocab, vec!["contact.person", "decision"]);
    }

    #[tokio::test]
    async fn handshake_with_unregistered_cert_is_rejected() {
        let (ctx, _fp, initiator_key, _peers) = setup(vec!["contact.person"], 5).await;
        let req = HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: pubkey_from_key(&initiator_key),
            initiator_capability: fp(b"x"),
            initiator_vocab: vec!["contact.person".into()],
            initiator_anchor: ts("2026-05-27T08:00:00Z"),
        };
        let unregistered = fp(b"stranger");
        let err = handle_handshake(&ctx, &unregistered, req)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerError::UnregisteredPeer(_)));
    }

    #[tokio::test]
    async fn handshake_with_pubkey_mismatch_is_rejected() {
        let (ctx, fp_, _initiator_key, _peers) = setup(vec!["contact.person"], 5).await;
        // Use a *different* pubkey in the body than the cert maps to.
        let other_key = SigningKey::from_bytes(&[77u8; 32]);
        let req = HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: pubkey_from_key(&other_key),
            initiator_capability: fp(b"x"),
            initiator_vocab: vec!["contact.person".into()],
            initiator_anchor: ts("2026-05-27T08:00:00Z"),
        };
        let err = handle_handshake(&ctx, &fp_, req).await.unwrap_err();
        assert!(matches!(
            err,
            ServerError::Handshake(HandshakeError::PeerIdMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn rotate_accepts_valid_old_key_signature_and_updates_pin() {
        let (ctx, fp_, initiator_key, peers) = setup(vec!["contact.person"], 5).await;
        let new_fp = fp(b"new-cert");
        let sig: Signature = initiator_key.sign(&rotation_signing_bytes(&fp_, &new_fp));
        let req = RotateRequest {
            new_fingerprint: new_fp.clone(),
            old_signature: sig.to_bytes().to_vec(),
        };
        let resp = handle_rotate(&ctx, &fp_, req).await.unwrap();
        assert!(resp.accepted);
        // Pin advanced.
        let updated = peers.get("alice").await.unwrap();
        assert_eq!(updated.cert_fingerprint, new_fp);
    }

    #[tokio::test]
    async fn rotate_rejects_signature_from_wrong_key() {
        let (ctx, fp_, _initiator_key, _peers) = setup(vec!["contact.person"], 5).await;
        let other_key = SigningKey::from_bytes(&[77u8; 32]);
        let new_fp = fp(b"new-cert");
        let sig: Signature = other_key.sign(&rotation_signing_bytes(&fp_, &new_fp));
        let req = RotateRequest {
            new_fingerprint: new_fp,
            old_signature: sig.to_bytes().to_vec(),
        };
        let err = handle_rotate(&ctx, &fp_, req).await.unwrap_err();
        assert!(matches!(err, ServerError::RotationBadSignature(_)));
    }
}
