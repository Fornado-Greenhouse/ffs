//! Bridge handshake state machine.
//!
//! The bilateral handshake is two-step in MVP:
//!
//! 1. Out-of-band: peers paste each other's endpoint URL + cert
//!    fingerprint into the CLI / plugin. This seeds the local
//!    `FederationPeerStore` with a peer record whose
//!    `their_capability` is `None`.
//! 2. In-band: the initiator calls `POST /federation/v1/handshake`
//!    on the peer. The endpoint exchanges:
//!      - both substrates' grant-capability atom hashes (each side
//!        authored a capability locally before the handshake),
//!      - the supported predicate vocabulary,
//!      - a starting `tx_time` anchor for pulls.
//!
//!    On success, both sides update their peer record with the
//!    counterpart's capability hash and vocab.
//!
//! This module contains only the *wire types and the pure state-
//! machine logic*. Transport is in `client.rs`; the HTTPS server-
//! side handler is in `server.rs`.

use serde::{Deserialize, Serialize};

use ffs_core::{Iso8601, Multihash, PublicKey};

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("handshake version mismatch: peer announced {0}, this substrate supports {1}")]
    VersionMismatch(u32, u32),
    #[error("peer announced no capability atom")]
    MissingCapability,
    #[error("vocabulary intersection is empty: peer={peer:?}, ours={ours:?}")]
    NoVocabIntersection {
        peer: Vec<String>,
        ours: Vec<String>,
    },
    #[error("peer-id mismatch: cert subject {cert} disagrees with announced key {announced}")]
    PeerIdMismatch { cert: String, announced: String },
}

/// Wire shape sent to `POST /federation/v1/handshake`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeRequest {
    pub protocol_version: u32,
    /// The initiator's substrate public key (matches the TLS cert
    /// CN per ADR-020). The receiver verifies this against the
    /// client's TLS cert before accepting the handshake.
    pub initiator_pubkey: PublicKey,
    /// Capability atom hash the initiator already authored locally
    /// granting the receiver scoped read access to the initiator's
    /// substrate. The receiver pulls this on subsequent fetches.
    pub initiator_capability: Multihash,
    /// Predicate vocabulary the initiator can render and serve.
    pub initiator_vocab: Vec<String>,
    /// Anchor: the initiator's last-known atom tx_time. Used by the
    /// receiver to seed its inbound watermark.
    pub initiator_anchor: Iso8601,
}

/// Wire shape returned by `POST /federation/v1/handshake`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub responder_pubkey: PublicKey,
    pub responder_capability: Multihash,
    pub responder_vocab: Vec<String>,
    pub responder_anchor: Iso8601,
}

/// Wire shape sent to `POST /federation/v1/bridge.rotate`.
///
/// `old_signature` is the OLD signing key's signature over a domain-
/// separated message containing both the old and new cert
/// fingerprints. The receiver verifies the signature against the
/// peer's currently-pinned `peer_pubkey` (the old key) and, on
/// success, swaps the pinned fingerprint to `new_fingerprint`.
///
/// No peer-id on the wire: peer-ids are sender-local strings; the
/// receiver identifies the initiator from the client cert
/// fingerprint at the TLS layer (or its in-process equivalent in
/// tests). Both sides know both fingerprints, so binding the
/// signature to the (old, new) pair prevents cross-bridge replay
/// without leaking either party's local naming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RotateRequest {
    pub new_fingerprint: Multihash,
    pub old_signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RotateResponse {
    pub accepted: bool,
}

/// Protocol version this build of FFS speaks. Incompatible bumps
/// are major. Minor changes (new vocab kinds, optional fields)
/// stay at this version.
pub const HANDSHAKE_PROTOCOL_VERSION: u32 = 1;

/// Domain-separation tag for the rotation signature. Mixed into the
/// signed bytes so a signature over an unrelated payload (atom
/// envelope, capability claim) can't be replayed as a rotation
/// approval.
pub const ROTATION_SIGNING_DOMAIN: &[u8] = b"ffs.federation.bridge.rotate.v1";

/// Build the bytes the OLD signing key signs to authorize a rotation.
/// The message is
/// `ROTATION_SIGNING_DOMAIN || old_fingerprint_bytes || new_fingerprint_bytes`.
/// Domain tag prevents cross-protocol replay; the
/// (old, new) fingerprint pair binds the signature to a specific
/// rotation event between two parties who both know both fingerprints.
pub fn rotation_signing_bytes(old_fingerprint: &Multihash, new_fingerprint: &Multihash) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        ROTATION_SIGNING_DOMAIN.len()
            + old_fingerprint.digest().len()
            + new_fingerprint.digest().len()
            + 2,
    );
    out.extend_from_slice(ROTATION_SIGNING_DOMAIN);
    out.push(b'|');
    out.extend_from_slice(old_fingerprint.digest());
    out.push(b'|');
    out.extend_from_slice(new_fingerprint.digest());
    out
}

/// Validate the inbound side of a handshake against this substrate's
/// supported version + vocabulary. Returns the vocab intersection
/// (predicates both sides can render) on success.
///
/// Pure function; tests can call it without standing up a server.
pub fn validate_inbound(
    req: &HandshakeRequest,
    our_vocab: &[String],
) -> Result<Vec<String>, HandshakeError> {
    if req.protocol_version != HANDSHAKE_PROTOCOL_VERSION {
        return Err(HandshakeError::VersionMismatch(
            req.protocol_version,
            HANDSHAKE_PROTOCOL_VERSION,
        ));
    }
    let intersection: Vec<String> = req
        .initiator_vocab
        .iter()
        .filter(|v| our_vocab.iter().any(|o| o == *v))
        .cloned()
        .collect();
    if intersection.is_empty() {
        return Err(HandshakeError::NoVocabIntersection {
            peer: req.initiator_vocab.clone(),
            ours: our_vocab.to_vec(),
        });
    }
    Ok(intersection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn pk(seed: u8) -> PublicKey {
        PublicKey::from_verifying(&SigningKey::from_bytes(&[seed; 32]).verifying_key())
    }

    fn hash(b: &[u8]) -> Multihash {
        Multihash::blake3_of(b)
    }

    fn ts(s: &str) -> Iso8601 {
        Iso8601::new(s).unwrap()
    }

    fn req() -> HandshakeRequest {
        HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: pk(1),
            initiator_capability: hash(b"cap-1"),
            initiator_vocab: vec!["contact.person".into(), "note".into()],
            initiator_anchor: ts("2026-05-27T08:00:00Z"),
        }
    }

    #[test]
    fn handshake_request_round_trips_via_serde() {
        let r = req();
        let s = serde_json::to_string(&r).unwrap();
        let r2: HandshakeRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn validate_inbound_returns_vocab_intersection() {
        let r = req();
        let ours = vec!["contact.person".to_string(), "person.generic".to_string()];
        let inter = validate_inbound(&r, &ours).unwrap();
        assert_eq!(inter, vec!["contact.person"]);
    }

    #[test]
    fn validate_inbound_rejects_version_mismatch() {
        let mut r = req();
        r.protocol_version = HANDSHAKE_PROTOCOL_VERSION + 1;
        let err = validate_inbound(&r, &["contact.person".into()]).unwrap_err();
        assert!(matches!(err, HandshakeError::VersionMismatch(_, _)));
    }

    #[test]
    fn validate_inbound_rejects_empty_intersection() {
        let r = req();
        let err = validate_inbound(&r, &["decision".into()]).unwrap_err();
        assert!(matches!(err, HandshakeError::NoVocabIntersection { .. }));
    }

    #[test]
    fn rotation_signing_bytes_starts_with_domain_tag() {
        let bytes = rotation_signing_bytes(&hash(b"old-cert"), &hash(b"new-cert"));
        let s = std::str::from_utf8(&bytes[..ROTATION_SIGNING_DOMAIN.len()]).unwrap();
        assert_eq!(s, "ffs.federation.bridge.rotate.v1");
    }

    #[test]
    fn rotation_signing_bytes_distinguishes_old_fingerprints() {
        let a = rotation_signing_bytes(&hash(b"old-a"), &hash(b"same-new"));
        let b = rotation_signing_bytes(&hash(b"old-b"), &hash(b"same-new"));
        assert_ne!(a, b);
    }

    #[test]
    fn rotation_signing_bytes_distinguishes_new_fingerprints() {
        let a = rotation_signing_bytes(&hash(b"same-old"), &hash(b"new-a"));
        let b = rotation_signing_bytes(&hash(b"same-old"), &hash(b"new-b"));
        assert_ne!(a, b);
    }
}
