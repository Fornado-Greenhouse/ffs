//! Integration tests covering the full atom-envelope lifecycle: sign,
//! serialize, deserialize, verify, content-address. Plus property tests
//! across many random envelopes — the success criterion specifies "1000
//! random envelopes without false positives or negatives."

use ed25519_dalek::SigningKey;
use ffs_core::{
    AtomEnvelope, AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, Provenance, PublicKey,
    SourceKind, Tier, VerifyError,
};
use proptest::prelude::*;
use rand::rngs::OsRng;

fn sample_template_with(claim: serde_json::Value, classification: &str) -> AtomTemplate {
    AtomTemplate {
        v: 1,
        entity: EntityId::new("entity-001"),
        predicate: PredicateName::new("contact.person"),
        claim,
        valid_from: Iso8601::new("2026-05-05T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new("2026-05-05T14:23:11.421Z").unwrap(),
        classification: Tier::new(classification),
        supersedes: None,
        provenance: vec![],
    }
}

#[test]
fn cross_process_serialization_yields_identical_hash() {
    // The "produced in one process and verified in another" success criterion
    // is satisfied by serializing to bytes, deserializing fresh, and confirming
    // the recomputed content hash is byte-identical.
    let key = SigningKey::generate(&mut OsRng);
    let env1 = sample_template_with(serde_json::json!({"display_name": "Sara"}), "existence")
        .sign(&key)
        .unwrap();
    let bytes = env1.canonical_bytes().unwrap();

    // Simulate the second "process": deserialize from the bytes alone.
    let env2: AtomEnvelope = serde_json::from_slice(&bytes).unwrap();
    env2.verify().unwrap();

    let h1 = env1.content_hash().unwrap();
    let h2 = env2.content_hash().unwrap();
    assert_eq!(h1, h2, "hash must be identical across deserialize");
}

#[test]
fn provenance_with_multihash_roundtrips_through_json() {
    let key = SigningKey::generate(&mut OsRng);
    let source_hash = Multihash::blake3_of(b"source content");
    let mut tmpl = sample_template_with(serde_json::json!({"display_name": "Sara"}), "existence");
    tmpl.provenance = vec![Provenance {
        kind: SourceKind::IngestFile,
        uri: "file:///home/alice/.ffs/ingest/note-123.md".into(),
        hash: source_hash.clone(),
    }];
    let env = tmpl.sign(&key).unwrap();
    env.verify().unwrap();

    let bytes = env.canonical_bytes().unwrap();
    let env2: AtomEnvelope = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env2.provenance.len(), 1);
    assert_eq!(env2.provenance[0].hash, source_hash);
    env2.verify().unwrap();
}

#[test]
fn supersedes_chain_field_roundtrips() {
    let key = SigningKey::generate(&mut OsRng);
    let parent = Multihash::blake3_of(b"parent atom canonical bytes");
    let mut tmpl = sample_template_with(serde_json::json!({"display_name": "Sara"}), "existence");
    tmpl.supersedes = Some(parent.clone());
    let env = tmpl.sign(&key).unwrap();
    env.verify().unwrap();

    let bytes = env.canonical_bytes().unwrap();
    let env2: AtomEnvelope = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env2.supersedes, Some(parent));
}

#[test]
fn signature_field_serializes_as_multibase_string() {
    let key = SigningKey::generate(&mut OsRng);
    let env = sample_template_with(serde_json::json!({"x": 1}), "existence")
        .sign(&key)
        .unwrap();
    let bytes = env.canonical_bytes().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let sig_str = value.get("signature").unwrap().as_str().unwrap();
    assert!(
        sig_str.starts_with('z'),
        "signature must be base58btc multibase"
    );
}

#[test]
fn flipping_one_byte_in_envelope_breaks_signature_or_parse() {
    let key = SigningKey::generate(&mut OsRng);
    let env = sample_template_with(serde_json::json!({"x": 1}), "existence")
        .sign(&key)
        .unwrap();
    let mut bytes = env.canonical_bytes().unwrap();
    // Flip a byte well inside the JSON object (avoid leading '{' / trailing '}').
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    // Either the JSON re-parses but signature fails, or parse fails outright.
    match serde_json::from_slice::<AtomEnvelope>(&bytes) {
        Ok(env2) => {
            let err = env2.verify().unwrap_err();
            assert!(
                matches!(err, VerifyError::Signature | VerifyError::Malformed(_)),
                "expected Signature or Malformed, got {err:?}"
            );
        }
        Err(_) => {
            // Parse-level failure is also acceptable: tampering broke the
            // structural invariant before signature check could run.
        }
    }
}

// Property tests. The success criterion calls for "1000 random envelopes
// without false positives or negatives"; proptest's default of 256 cases
// is below that, so we explicitly bump cases to 1024 here.

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    })]

    #[test]
    fn signing_roundtrip_property(
        entity_id in "[A-Za-z0-9_-]{8,32}",
        predicate in "[a-z]{3,12}\\.[a-z]{3,12}",
        display_name in ".*",
        classification in prop_oneof![
            Just("existence"),
            Just("work_email"),
            Just("personal_email"),
            Just("notes"),
        ],
    ) {
        let key = SigningKey::from_bytes(&[3u8; 32]); // deterministic per case
        let claim = serde_json::json!({"display_name": display_name});
        let tmpl = AtomTemplate {
            v: 1,
            entity: EntityId::new(entity_id),
            predicate: PredicateName::new(predicate),
            claim,
            valid_from: Iso8601::new("2026-05-05T00:00:00Z").unwrap(),
            valid_to: None,
            tx_time: Iso8601::new("2026-05-05T14:23:11.421Z").unwrap(),
            classification: Tier::new(classification),
            supersedes: None,
            provenance: vec![],
        };
        let env = tmpl.sign(&key).expect("signing must not fail for valid input");
        prop_assert!(env.verify().is_ok(), "freshly signed envelope must verify");

        // Roundtrip through JSON.
        let bytes = env.canonical_bytes().unwrap();
        let env2: AtomEnvelope = serde_json::from_slice(&bytes).unwrap();
        prop_assert_eq!(env, env2);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    })]

    #[test]
    fn canonical_bytes_stable_across_calls(
        entries in prop::collection::vec(("[a-z]{1,8}", any::<i32>()), 0..20),
    ) {
        // Build a JSON object out of the proptest-generated entries.
        // Duplicate keys collapse to the last value (the natural JSON map
        // behavior), which is fine: we only need stability across calls.
        let mut obj = serde_json::Map::new();
        for (k, v) in entries {
            obj.insert(k, serde_json::Value::Number(v.into()));
        }
        let value = serde_json::Value::Object(obj);
        let a = serde_jcs::to_vec(&value).unwrap();
        let b = serde_jcs::to_vec(&value).unwrap();
        prop_assert_eq!(a, b);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    })]

    #[test]
    fn multihash_multibase_roundtrip(payload in prop::collection::vec(any::<u8>(), 0..256)) {
        let mh = Multihash::blake3_of(&payload);
        let s = mh.to_multibase();
        let back = Multihash::from_multibase(&s).unwrap();
        prop_assert_eq!(mh, back);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    #[test]
    fn public_key_serde_roundtrip(key_bytes in any::<[u8; 32]>()) {
        // Note: not every 32-byte sequence is a valid Ed25519 point, but
        // PublicKey just stores bytes; verification loads them via
        // VerifyingKey::from_bytes which fails on invalid points. Here we
        // only test the byte-level serialize/deserialize path, which must
        // round-trip regardless.
        let pk = PublicKey::from_bytes(key_bytes);
        let json = serde_json::to_string(&pk).unwrap();
        let pk2: PublicKey = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(pk, pk2);
    }
}
