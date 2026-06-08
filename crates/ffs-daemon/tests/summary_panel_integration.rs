//! Integration tests for the daily-summary-panel surface area
//! (task_19): `ingest.list_pending`, `ingest.accept`, `ingest.reject`,
//! `entity.search`. These exercise the new dispatcher RPCs end-to-
//! end through a real `Dispatcher` so the Obsidian plugin's
//! summary + search modules have a verified wire contract.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::{InMemoryQuarantine, IngestQuarantine, Proposal, SubmissionStatus};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::{ApiPayload, ApiRequest, ApiResponse, Dispatcher, EventPublisher};
use ffs_federation::mount::InMemoryPeerMount;

const CONTACT_PERSON_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name"]
"#;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn grant_owner_full(store: &dyn AtomStore) {
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

fn insert_contact(store: &dyn AtomStore, entity: &str, name: &str, tx_time: &str) {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({"display_name": name}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap();
}

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<dyn AtomStore>,
    quarantine: Arc<InMemoryQuarantine>,
    dispatcher: Dispatcher,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&predicates_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        predicates_dir.join("contact.person.toml"),
        CONTACT_PERSON_TOML,
    )
    .unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        "---\ndisplay_name: {{ claim.display_name }}\n---\n",
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant_owner_full(&*store);
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let quarantine = Arc::new(InMemoryQuarantine::new());

    let dispatcher = Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: quarantine.clone(),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(owner_key())),
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(InMemoryPeerMount::new()),
    };
    Harness {
        _dir: dir,
        store,
        quarantine,
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

fn unwrap_ok(resp: ApiResponse) -> Value {
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success; got {error:?}"),
    }
}

#[tokio::test]
async fn ingest_list_pending_returns_only_extracted_submissions() {
    let h = setup();
    // Seed three submissions in three different states.
    let extracted = h
        .quarantine
        .submit("a".into(), b"a".to_vec())
        .await
        .unwrap();
    h.quarantine
        .complete(
            &extracted,
            vec![Proposal {
                predicate: PredicateName::new("contact.person"),
                claim: serde_json::json!({"display_name": "Sara"}),
                provenance: vec![],
                rationale: "test".into(),
            }],
        )
        .await
        .unwrap();
    let _pending = h
        .quarantine
        .submit("b".into(), b"b".to_vec())
        .await
        .unwrap();
    let failed = h
        .quarantine
        .submit("c".into(), b"c".to_vec())
        .await
        .unwrap();
    h.quarantine.fail(&failed, "boom".into()).await.unwrap();

    let resp = h
        .dispatcher
        .handle(req("ingest.list_pending", Value::Null))
        .await;
    let result = unwrap_ok(resp);
    let subs = result.as_array().expect("array of submissions");
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["status"], "extracted");
    assert_eq!(subs[0]["source_uri"], "a");
}

#[tokio::test]
async fn ingest_accept_signs_proposals_into_atoms_and_flips_status() {
    let h = setup();
    let id = h
        .quarantine
        .submit("a".into(), b"a".to_vec())
        .await
        .unwrap();
    h.quarantine
        .complete(
            &id,
            vec![Proposal {
                predicate: PredicateName::new("contact.person"),
                claim: serde_json::json!({"display_name": "Sara"}),
                provenance: vec![],
                rationale: "test".into(),
            }],
        )
        .await
        .unwrap();

    let resp = h
        .dispatcher
        .handle(req(
            "ingest.accept",
            serde_json::json!({"submission_id": id}),
        ))
        .await;
    let result = unwrap_ok(resp);
    let hashes = result["accepted_atom_hashes"]
        .as_array()
        .expect("hashes array");
    assert_eq!(hashes.len(), 1);
    // The accepted atom landed in the store.
    let hash_str = hashes[0].as_str().unwrap();
    let hash = Multihash::from_multibase(hash_str).unwrap();
    let env = h.store.get(&hash).unwrap().expect("atom stored");
    assert_eq!(env.claim["display_name"], "Sara");
    // Quarantine status flipped.
    let sub = h.quarantine.get(&id).await.unwrap();
    assert_eq!(sub.status, SubmissionStatus::Accepted);
}

#[tokio::test]
async fn ingest_reject_marks_submission_rejected_without_authoring_atoms() {
    let h = setup();
    let id = h
        .quarantine
        .submit("a".into(), b"a".to_vec())
        .await
        .unwrap();
    h.quarantine
        .complete(
            &id,
            vec![Proposal {
                predicate: PredicateName::new("contact.person"),
                claim: serde_json::json!({"display_name": "Sara"}),
                provenance: vec![],
                rationale: "test".into(),
            }],
        )
        .await
        .unwrap();
    let resp = h
        .dispatcher
        .handle(req(
            "ingest.reject",
            serde_json::json!({"submission_id": id}),
        ))
        .await;
    let result = unwrap_ok(resp);
    assert_eq!(result["rejected"], id);
    let sub = h.quarantine.get(&id).await.unwrap();
    assert_eq!(sub.status, SubmissionStatus::Rejected);
}

#[tokio::test]
async fn entity_search_matches_display_name_case_insensitively() {
    let h = setup();
    insert_contact(&*h.store, "Sara_Chen", "Sara Chen", "2026-05-27T08:00:00Z");
    insert_contact(
        &*h.store,
        "Sarah_Park",
        "Sarah Park",
        "2026-05-27T08:01:00Z",
    );
    insert_contact(&*h.store, "Bob", "Bob", "2026-05-27T08:02:00Z");

    let resp = h
        .dispatcher
        .handle(req("entity.search", serde_json::json!({"query": "sara"})))
        .await;
    let result = unwrap_ok(resp);
    let hits = result["results"].as_array().expect("results array");
    let names: Vec<&str> = hits
        .iter()
        .map(|h| h["display_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Sara Chen"));
    assert!(names.contains(&"Sarah Park"));
    assert!(!names.contains(&"Bob"));
}

#[tokio::test]
async fn entity_search_with_empty_query_returns_empty_results() {
    let h = setup();
    insert_contact(&*h.store, "Sara", "Sara", "2026-05-27T08:00:00Z");
    let resp = h
        .dispatcher
        .handle(req("entity.search", serde_json::json!({"query": ""})))
        .await;
    let result = unwrap_ok(resp);
    assert_eq!(result["results"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn entity_search_respects_limit() {
    let h = setup();
    for i in 0..20 {
        insert_contact(
            &*h.store,
            &format!("Sara_{i:02}"),
            &format!("Sara {i:02}"),
            "2026-05-27T08:00:00Z",
        );
    }
    let resp = h
        .dispatcher
        .handle(req(
            "entity.search",
            serde_json::json!({"query": "sara", "limit": 5}),
        ))
        .await;
    let result = unwrap_ok(resp);
    assert_eq!(result["results"].as_array().unwrap().len(), 5);
}
