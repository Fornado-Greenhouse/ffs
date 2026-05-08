//! The signed, content-addressed atom envelope. This is the substrate's
//! single interoperability contract: any tool that can canonicalize JSON
//! per RFC 8785, verify Ed25519 signatures, and compute BLAKE3 multihashes
//! can read and verify FFS atoms (per ADR-017 and ADR-018).
//!
//! Signing protocol (ADR-017):
//!   1. Construct the envelope without the `signature` field.
//!   2. JCS-canonicalize.
//!   3. Sign canonical bytes with the author's Ed25519 key.
//!   4. Insert the signature into the envelope.
//!   5. JCS-canonicalize the full envelope.
//!   6. `multihash(blake3(jcs_bytes))` is the content address.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use ed25519_dalek::Signer;

use crate::error::{BadTimestampError, SignError, VerifyError};
use crate::multibase::{decode_base58btc, encode_base58btc};
use crate::multihash::Multihash;

/// Envelope schema version. Persisted as the `v` field. Unknown future
/// versions are refused at the boundary by callers.
pub const ENVELOPE_VERSION: u32 = 1;

/// Opaque entity identifier. Validation that the value is a multibase
/// string is performed by the entity-creation pathway (introduced by a
/// later task); at the envelope layer it is treated as a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub String);

impl EntityId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Predicate name. Convention: dotted snake-case (e.g., `contact.person`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PredicateName(pub String);

impl PredicateName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Classification tier for sub-record tier-based selective sharing
/// (e.g., "existence", "work_email", "personal_email", "notes").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tier(pub String);

impl Tier {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Source of an atom for provenance traceback. Serialized as snake_case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    IngestFile,
    McpAgent,
    FederationPull,
    FastPath,
}

/// Provenance entry pointing back to the source material that produced the atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub kind: SourceKind,
    pub uri: String,
    pub hash: Multihash,
}

/// ISO 8601 timestamp validated to be UTC at construction and on
/// deserialization. Stored as the original string so JCS canonicalization
/// over the envelope produces stable bytes regardless of Rust's preferred
/// formatting choices.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Iso8601(String);

impl Iso8601 {
    pub fn new(s: impl Into<String>) -> Result<Self, BadTimestampError> {
        let s = s.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    pub fn validate(s: &str) -> Result<(), BadTimestampError> {
        let dt =
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
                .map_err(|e| BadTimestampError::Parse(e.to_string()))?;
        if dt.offset() != time::UtcOffset::UTC {
            return Err(BadTimestampError::NonUtc);
        }
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Iso8601 {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Iso8601 {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(de)?;
        Self::new(s).map_err(D::Error::custom)
    }
}

/// Ed25519 public key (32 bytes). Serialized as a base58btc multibase string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_verifying(k: &ed25519_dalek::VerifyingKey) -> Self {
        Self(k.to_bytes())
    }

    pub fn to_verifying(
        &self,
    ) -> Result<ed25519_dalek::VerifyingKey, ed25519_dalek::SignatureError> {
        ed25519_dalek::VerifyingKey::from_bytes(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_multibase(&self) -> String {
        encode_base58btc(&self.0)
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_multibase())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(de)?;
        let bytes = decode_base58btc(&s).map_err(D::Error::custom)?;
        if bytes.len() != 32 {
            return Err(D::Error::custom(format!(
                "expected 32-byte public key, got {} bytes",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

/// Ed25519 signature (64 bytes). Serialized as a base58btc multibase string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Signature([u8; 64]);

impl Signature {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn from_dalek(sig: &ed25519_dalek::Signature) -> Self {
        Self(sig.to_bytes())
    }

    pub fn to_dalek(&self) -> ed25519_dalek::Signature {
        ed25519_dalek::Signature::from_bytes(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    pub fn to_multibase(&self) -> String {
        encode_base58btc(&self.0)
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_multibase())
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(de)?;
        let bytes = decode_base58btc(&s).map_err(D::Error::custom)?;
        if bytes.len() != 64 {
            return Err(D::Error::custom(format!(
                "expected 64-byte signature, got {} bytes",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

/// The signed, content-addressed atom envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AtomEnvelope {
    pub v: u32,
    pub entity: EntityId,
    pub predicate: PredicateName,
    pub claim: serde_json::Value,
    pub author: PublicKey,
    pub valid_from: Iso8601,
    pub valid_to: Option<Iso8601>,
    pub tx_time: Iso8601,
    pub classification: Tier,
    pub supersedes: Option<Multihash>,
    pub provenance: Vec<Provenance>,
    pub signature: Signature,
}

/// Borrowed view of an envelope's signable fields (everything except
/// `signature`). Serializing this view via `serde_jcs` produces the bytes
/// that the Ed25519 signature covers.
#[derive(Serialize)]
struct UnsignedView<'a> {
    v: u32,
    entity: &'a EntityId,
    predicate: &'a PredicateName,
    claim: &'a serde_json::Value,
    author: &'a PublicKey,
    valid_from: &'a Iso8601,
    valid_to: &'a Option<Iso8601>,
    tx_time: &'a Iso8601,
    classification: &'a Tier,
    supersedes: &'a Option<Multihash>,
    provenance: &'a [Provenance],
}

impl<'a> From<&'a AtomEnvelope> for UnsignedView<'a> {
    fn from(e: &'a AtomEnvelope) -> Self {
        Self {
            v: e.v,
            entity: &e.entity,
            predicate: &e.predicate,
            claim: &e.claim,
            author: &e.author,
            valid_from: &e.valid_from,
            valid_to: &e.valid_to,
            tx_time: &e.tx_time,
            classification: &e.classification,
            supersedes: &e.supersedes,
            provenance: &e.provenance,
        }
    }
}

/// Unsigned atom skeleton. Build this then call `sign(&signing_key)` to
/// produce a fully-formed `AtomEnvelope`.
#[derive(Debug, Clone)]
pub struct AtomTemplate {
    pub v: u32,
    pub entity: EntityId,
    pub predicate: PredicateName,
    pub claim: serde_json::Value,
    pub valid_from: Iso8601,
    pub valid_to: Option<Iso8601>,
    pub tx_time: Iso8601,
    pub classification: Tier,
    pub supersedes: Option<Multihash>,
    pub provenance: Vec<Provenance>,
}

impl AtomTemplate {
    /// Sign the template with the author's signing key, producing a
    /// fully-formed `AtomEnvelope`.
    pub fn sign(self, key: &ed25519_dalek::SigningKey) -> Result<AtomEnvelope, SignError> {
        let author = PublicKey::from_verifying(&key.verifying_key());

        let view = UnsignedView {
            v: self.v,
            entity: &self.entity,
            predicate: &self.predicate,
            claim: &self.claim,
            author: &author,
            valid_from: &self.valid_from,
            valid_to: &self.valid_to,
            tx_time: &self.tx_time,
            classification: &self.classification,
            supersedes: &self.supersedes,
            provenance: &self.provenance,
        };
        let canonical =
            serde_jcs::to_vec(&view).map_err(|e| SignError::Serialization(e.to_string()))?;

        let dalek_sig = key.sign(&canonical);
        let signature = Signature::from_dalek(&dalek_sig);

        Ok(AtomEnvelope {
            v: self.v,
            entity: self.entity,
            predicate: self.predicate,
            claim: self.claim,
            author,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            tx_time: self.tx_time,
            classification: self.classification,
            supersedes: self.supersedes,
            provenance: self.provenance,
            signature,
        })
    }
}

impl AtomEnvelope {
    /// JCS-canonical bytes of the full envelope (including signature).
    /// Used for content addressing and on-disk persistence.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SignError> {
        serde_jcs::to_vec(self).map_err(|e| SignError::Serialization(e.to_string()))
    }

    /// JCS-canonical bytes of the envelope WITHOUT the signature field.
    /// These are the bytes the Ed25519 signature covers.
    pub fn canonical_bytes_for_signing(&self) -> Result<Vec<u8>, SignError> {
        let view = UnsignedView::from(self);
        serde_jcs::to_vec(&view).map_err(|e| SignError::Serialization(e.to_string()))
    }

    /// Content address of the envelope: `multihash(blake3(canonical_bytes))`.
    pub fn content_hash(&self) -> Result<Multihash, SignError> {
        let bytes = self.canonical_bytes()?;
        Ok(Multihash::blake3_of(&bytes))
    }

    /// Verify the envelope's signature against its declared author.
    pub fn verify(&self) -> Result<(), VerifyError> {
        // Validate timestamps (defensive — Deserialize already validates,
        // but constructed-in-Rust envelopes also pass through this gate).
        Iso8601::validate(self.valid_from.as_str())
            .map_err(|e| VerifyError::Malformed(format!("valid_from: {e}")))?;
        if let Some(vt) = &self.valid_to {
            Iso8601::validate(vt.as_str())
                .map_err(|e| VerifyError::Malformed(format!("valid_to: {e}")))?;
        }
        Iso8601::validate(self.tx_time.as_str())
            .map_err(|e| VerifyError::Malformed(format!("tx_time: {e}")))?;

        let unsigned = self
            .canonical_bytes_for_signing()
            .map_err(|e| VerifyError::Malformed(format!("canonicalization: {e}")))?;
        let vk = self
            .author
            .to_verifying()
            .map_err(|e| VerifyError::Malformed(format!("author public key: {e}")))?;
        let sig = self.signature.to_dalek();
        vk.verify_strict(&unsigned, &sig)
            .map_err(|_| VerifyError::Signature)?;
        Ok(())
    }

    /// Verify both the signature and that `expected` matches the envelope's
    /// content hash. Use when receiving an envelope by reference (e.g., from
    /// federation pulls or `ffs://atom/<hash>` resolution).
    pub fn verify_with_hash(&self, expected: &Multihash) -> Result<(), VerifyError> {
        self.verify()?;
        let actual = self
            .content_hash()
            .map_err(|e| VerifyError::Malformed(format!("hash recompute: {e}")))?;
        if &actual != expected {
            return Err(VerifyError::HashMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_signing_key() -> ed25519_dalek::SigningKey {
        // Deterministic test key. Production keys live in the OS keychain.
        ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
    }

    fn sample_template() -> AtomTemplate {
        AtomTemplate {
            v: ENVELOPE_VERSION,
            entity: EntityId::new("entity-001"),
            predicate: PredicateName::new("contact.person"),
            claim: serde_json::json!({"display_name": "Sara Chen"}),
            valid_from: Iso8601::new("2026-05-05T00:00:00Z").unwrap(),
            valid_to: None,
            tx_time: Iso8601::new("2026-05-05T14:23:11.421Z").unwrap(),
            classification: Tier::new("existence"),
            supersedes: None,
            provenance: vec![],
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let key = fixed_signing_key();
        let env = sample_template().sign(&key).unwrap();
        env.verify().expect("freshly-signed envelope must verify");
    }

    #[test]
    fn verify_fails_with_mismatched_key() {
        let key1 = fixed_signing_key();
        let key2 = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let mut env = sample_template().sign(&key1).unwrap();
        // Replace declared author with the other key's public key.
        env.author = PublicKey::from_verifying(&key2.verifying_key());
        let err = env.verify().unwrap_err();
        assert!(matches!(err, VerifyError::Signature));
    }

    #[test]
    fn tampering_with_claim_breaks_signature() {
        let key = fixed_signing_key();
        let mut env = sample_template().sign(&key).unwrap();
        env.claim = serde_json::json!({"display_name": "Sarah Chen"}); // changed
        let err = env.verify().unwrap_err();
        assert!(matches!(err, VerifyError::Signature));
    }

    #[test]
    fn tampering_with_classification_breaks_signature() {
        let key = fixed_signing_key();
        let mut env = sample_template().sign(&key).unwrap();
        env.classification = Tier::new("personal_email");
        let err = env.verify().unwrap_err();
        assert!(matches!(err, VerifyError::Signature));
    }

    #[test]
    fn content_hash_changes_when_envelope_changes() {
        let key = fixed_signing_key();
        let env1 = sample_template().sign(&key).unwrap();
        let mut env2 = env1.clone();
        env2.classification = Tier::new("personal_email");
        let h1 = env1.content_hash().unwrap();
        let h2 = env2.content_hash().unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_is_stable_across_calls() {
        let key = fixed_signing_key();
        let env = sample_template().sign(&key).unwrap();
        let h1 = env.content_hash().unwrap();
        let h2 = env.content_hash().unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn verify_with_hash_detects_mismatch() {
        let key = fixed_signing_key();
        let env = sample_template().sign(&key).unwrap();
        let mut wrong_digest = [0u8; 32];
        wrong_digest[0] = 0xab;
        let wrong = Multihash::from_blake3(&wrong_digest);
        let err = env.verify_with_hash(&wrong).unwrap_err();
        assert!(matches!(err, VerifyError::HashMismatch));
    }

    #[test]
    fn verify_with_hash_accepts_correct_hash() {
        let key = fixed_signing_key();
        let env = sample_template().sign(&key).unwrap();
        let correct = env.content_hash().unwrap();
        env.verify_with_hash(&correct).unwrap();
    }

    #[test]
    fn iso8601_rejects_non_utc() {
        let err = Iso8601::new("2026-05-05T14:23:00+01:00").unwrap_err();
        assert!(matches!(err, BadTimestampError::NonUtc));
    }

    #[test]
    fn iso8601_rejects_malformed() {
        let err = Iso8601::new("not-a-date").unwrap_err();
        assert!(matches!(err, BadTimestampError::Parse(_)));
    }

    #[test]
    fn iso8601_accepts_z_suffix() {
        Iso8601::new("2026-05-05T14:23:11.421Z").unwrap();
    }

    #[test]
    fn iso8601_accepts_explicit_utc_offset() {
        // RFC 3339 / ISO 8601 allow "+00:00" as an explicit UTC offset,
        // semantically equivalent to "Z".
        Iso8601::new("2026-05-05T14:23:11.421+00:00").unwrap();
    }

    #[test]
    fn deserialize_rejects_non_utc_timestamp() {
        // Construct envelope JSON with a non-UTC timestamp. Deserialize
        // must fail (the Iso8601 Deserialize impl runs validation).
        let key = fixed_signing_key();
        let env = sample_template().sign(&key).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&env.canonical_bytes().unwrap()).unwrap();
        value["valid_from"] = serde_json::Value::String("2026-05-05T00:00:00+01:00".into());
        let result: Result<AtomEnvelope, _> = serde_json::from_value(value);
        assert!(result.is_err());
    }

    #[test]
    fn permuting_input_field_order_yields_identical_canonical_bytes() {
        // Two input JSON values that differ only in key order must produce
        // identical canonical bytes.
        let a: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":2,"c":3}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"c":3,"a":2,"b":1}"#).unwrap();
        let ja = serde_jcs::to_vec(&a).unwrap();
        let jb = serde_jcs::to_vec(&b).unwrap();
        assert_eq!(ja, jb);
    }

    #[test]
    fn json_roundtrip_preserves_signature() {
        let key = fixed_signing_key();
        let env1 = sample_template().sign(&key).unwrap();
        let bytes = env1.canonical_bytes().unwrap();
        let env2: AtomEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(env1, env2);
        env2.verify().unwrap();
    }
}
