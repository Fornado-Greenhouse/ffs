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

use ffs_core::federation_peers::FederationPeerStore;
use ffs_core::{Iso8601, Multihash, PublicKey};

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
        let ctx = FederationContext {
            responder_pubkey: pubkey_from_key(&responder_key),
            responder_capability: fp(b"responder-cap"),
            responder_vocab: responder_vocab.into_iter().map(String::from).collect(),
            responder_anchor: ts("2026-05-27T08:00:00Z"),
            peers: peers.clone(),
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
