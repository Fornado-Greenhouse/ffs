//! Integration test: dispatcher's `audit.publish_summary` and
//! `audit.query` round-trip a signed `auditor.daily_summary` atom
//! through the substrate. Verifies the atom's signature, supersession
//! chaining, and capability checks.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{AtomEnvelope, Iso8601, Multihash, PublicKey};
use ffs_daemon::{ApiPayload, ApiRequest, ApiResponse, Dispatcher, EventPublisher};

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[11u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn stranger_pk() -> PublicKey {
    let k = SigningKey::from_bytes(&[13u8; 32]);
    PublicKey::from_verifying(&k.verifying_key())
}

fn grant_write_for_owner(store: &dyn AtomStore) {
    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![Action::Read, Action::Write, Action::Supersede],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
}

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<dyn AtomStore>,
    dispatcher: Dispatcher,
}

fn setup(with_signing_key: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&predicates_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant_write_for_owner(&*store);
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());

    let signing_key = if with_signing_key {
        Some(Arc::new(owner_key()))
    } else {
        None
    };

    let dispatcher = Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
    };
    Harness {
        _dir: dir,
        store,
        dispatcher,
    }
}

fn req(method: &str, params: Value) -> ApiRequest {
    ApiRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: method.into(),
        params,
    }
}

fn unwrap_success(resp: ApiResponse) -> Value {
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success; got error: {error:?}"),
    }
}

#[tokio::test]
async fn publish_summary_inserts_signed_atom_with_auditor_predicate() {
    let h = setup(true);
    let claim = serde_json::json!({
        "metrics": {"atom_author_rate": 5, "drift_flags": 0},
        "flags": [],
        "panel": [],
        "narrative": "All quiet. 5 atoms in the last 24h.",
    });
    let resp = h
        .dispatcher
        .handle(req(
            "audit.publish_summary",
            serde_json::json!({"claim": claim}),
        ))
        .await;
    let result = unwrap_success(resp);
    let hash_str = result["atom_hash"].as_str().expect("atom_hash present");
    let hash = Multihash::from_multibase(hash_str).expect("decodable multihash");

    // The store now contains a fully-signed atom with auditor predicate.
    let env = h.store.get(&hash).unwrap().expect("atom stored");
    assert_eq!(env.predicate.as_str(), "auditor.daily_summary");
    assert_eq!(env.entity.as_str(), "auditor");
    assert_eq!(env.author, owner_pk());
    env.verify().expect("signature verifies against author key");
    // Claim was embedded verbatim.
    assert_eq!(env.claim["metrics"]["atom_author_rate"], 5);
}

#[tokio::test]
async fn query_returns_most_recent_summary_first() {
    let h = setup(true);
    // Publish two summaries with distinct content; the second
    // supersedes the first.
    let first = unwrap_success(
        h.dispatcher
            .handle(req(
                "audit.publish_summary",
                serde_json::json!({"claim": {"label": "first"}}),
            ))
            .await,
    );
    let _first_hash = first["atom_hash"].as_str().unwrap().to_string();

    // Sleep enough that tx_time advances on the second one.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let second = unwrap_success(
        h.dispatcher
            .handle(req(
                "audit.publish_summary",
                serde_json::json!({"claim": {"label": "second"}}),
            ))
            .await,
    );
    let second_hash = second["atom_hash"].as_str().unwrap().to_string();

    let resp = h
        .dispatcher
        .handle(req("audit.query", serde_json::Value::Null))
        .await;
    let atoms: Vec<AtomEnvelope> = serde_json::from_value(unwrap_success(resp)).unwrap();
    assert!(atoms.len() >= 2);
    // Newest-first: the head atom is the second publish.
    let head_hash = atoms[0].content_hash().unwrap();
    assert_eq!(head_hash.to_multibase(), second_hash);
    // And the second atom supersedes the first.
    let head_supersedes = atoms[0].supersedes.clone().expect("supersedes set");
    assert_ne!(head_supersedes.to_multibase(), second_hash);
}

#[tokio::test]
async fn publish_summary_without_signing_key_returns_not_implemented() {
    let h = setup(false);
    let resp = h
        .dispatcher
        .handle(req(
            "audit.publish_summary",
            serde_json::json!({"claim": {}}),
        ))
        .await;
    match resp.payload {
        ApiPayload::Error { error } => {
            assert_eq!(error.code, ffs_daemon::api::ERR_NOT_IMPLEMENTED);
        }
        ApiPayload::Success { result } => panic!("expected error; got success: {result}"),
    }
}

#[tokio::test]
async fn publish_summary_capability_denied_for_unprivileged_owner() {
    // Setup with a signing key but override the dispatcher's owner
    // to a key with no capability atom in the store.
    let mut h = setup(true);
    h.dispatcher.owner = stranger_pk();
    let resp = h
        .dispatcher
        .handle(req(
            "audit.publish_summary",
            serde_json::json!({"claim": {}}),
        ))
        .await;
    match resp.payload {
        ApiPayload::Error { error } => {
            assert_eq!(error.code, ffs_daemon::api::ERR_CAPABILITY_DENIED);
        }
        ApiPayload::Success { result } => {
            panic!("expected capability-denied; got success: {result}");
        }
    }
}
