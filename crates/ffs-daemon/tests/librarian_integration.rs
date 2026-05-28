//! End-to-end test for the working-set / librarian pipeline.
//!
//! Exercises the dispatcher's `working_set.*` methods that the
//! librarian skill (task 12) drives. The test:
//!
//! - Materializes a projection, capturing its render hash.
//! - Inserts a superseding atom that changes the projection's
//!   contents; verifies that `working_set.detect_drift` flags it.
//! - Calls `working_set.refresh_drifted` and verifies the stored
//!   render hash advances to the new value.
//! - Pins one entry, evicts to a cap below the count, verifies the
//!   pinned entry survives.
//! - Verifies `health.summary` reports the right `drift_flags` and
//!   `proposals` counts from the working set + quarantine state.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::{InMemoryWorkingSet, WorkingSetStore};
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::{ApiPayload, ApiRequest, ApiResponse, Dispatcher, EventPublisher};

const CONTACT_TOML: &str = r#"
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

const CONTACT_TEMPLATE: &str = "---\ndisplay_name: {{ claim.display_name }}\n---\n";

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[5u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn grant_full_capability(store: &dyn AtomStore) {
    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![
            Action::Read,
            Action::Write,
            Action::Supersede,
            Action::Erase,
            Action::Classify,
            Action::Federate,
        ],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
}

fn insert_contact(
    store: &dyn AtomStore,
    entity: &str,
    name: &str,
    tx_time: &str,
    supersedes: Option<Multihash>,
) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({"display_name": name}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("existence"),
        supersedes,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap()
}

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<dyn AtomStore>,
    working_set: Arc<InMemoryWorkingSet>,
    dispatcher: Dispatcher,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::set_permissions(&predicates_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant_full_capability(&*store);
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let working_set = Arc::new(InMemoryWorkingSet::new());
    let dispatcher = Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: working_set.clone(),
        signing_key: None,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
    };
    Harness {
        _dir: dir,
        store,
        working_set,
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
async fn materialize_records_render_hash_in_working_set() {
    let h = setup();
    insert_contact(&*h.store, "Sara_Chen", "Sara", "2026-05-25T08:00:00Z", None);

    let resp = h
        .dispatcher
        .handle(req(
            "working_set.materialize",
            serde_json::json!({"path": "contacts/by-name/S/Sara_Chen.md"}),
        ))
        .await;
    let result = unwrap_success(resp);
    assert!(result["render_hash"].is_string());
    let entry = h
        .working_set
        .get("contacts/by-name/S/Sara_Chen.md")
        .await
        .expect("entry stored");
    let h1 = result["render_hash"].as_str().unwrap();
    assert_eq!(entry.last_render_hash.to_multibase(), h1);
}

#[tokio::test]
async fn drift_detection_fires_when_underlying_atom_changes() {
    let h = setup();
    let head = insert_contact(&*h.store, "Sara_Chen", "Sara", "2026-05-25T08:00:00Z", None);
    // Initial materialization captures the current render hash.
    let _ = h
        .dispatcher
        .handle(req(
            "working_set.materialize",
            serde_json::json!({"path": "contacts/by-name/S/Sara_Chen.md"}),
        ))
        .await;
    let initial = h
        .working_set
        .get("contacts/by-name/S/Sara_Chen.md")
        .await
        .unwrap()
        .last_render_hash
        .clone();

    // Now an atom changes — Sara → Sarah, superseding the head.
    insert_contact(
        &*h.store,
        "Sara_Chen",
        "Sarah",
        "2026-05-25T09:00:00Z",
        Some(head),
    );

    // Drift detection should flag the projection.
    let resp = h
        .dispatcher
        .handle(req("working_set.detect_drift", serde_json::Value::Null))
        .await;
    let drifted = unwrap_success(resp);
    let paths: Vec<&str> = drifted["drifted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"contacts/by-name/S/Sara_Chen.md"),
        "expected drift; got {paths:?}"
    );

    // Refresh should advance the stored hash and report what it
    // re-rendered.
    let resp = h
        .dispatcher
        .handle(req("working_set.refresh_drifted", serde_json::Value::Null))
        .await;
    let refreshed = unwrap_success(resp);
    let refreshed_arr = refreshed["refreshed"].as_array().unwrap();
    assert_eq!(refreshed_arr.len(), 1);
    let updated = h
        .working_set
        .get("contacts/by-name/S/Sara_Chen.md")
        .await
        .unwrap();
    assert_ne!(
        updated.last_render_hash, initial,
        "render hash should have advanced after refresh"
    );
}

#[tokio::test]
async fn evict_to_cap_preserves_pinned_entries() {
    let h = setup();
    // Seed three contacts.
    for (entity, name, t) in [
        ("Alice", "Alice", "2026-05-25T08:00:00Z"),
        ("Bob", "Bob", "2026-05-25T08:00:01Z"),
        ("Carol", "Carol", "2026-05-25T08:00:02Z"),
    ] {
        insert_contact(&*h.store, entity, name, t, None);
    }
    // Materialize all three; each materialize uses `now`, so the
    // last-touched timestamps order Alice < Bob < Carol.
    for entity in ["Alice", "Bob", "Carol"] {
        let path = format!("contacts/by-name/{}/{}.md", &entity[..1], entity);
        let _ = h
            .dispatcher
            .handle(req(
                "working_set.materialize",
                serde_json::json!({"path": path}),
            ))
            .await;
        // Tiny sleep so wall-clock ISO timestamps differ between
        // materializations on fast machines.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    // Pin Alice — she's oldest, so without pinning she'd be evicted first.
    let _ = h
        .dispatcher
        .handle(req(
            "working_set.pin",
            serde_json::json!({"path": "contacts/by-name/A/Alice.md", "pinned": true}),
        ))
        .await;

    // Evict to cap of 2.
    let resp = h
        .dispatcher
        .handle(req(
            "working_set.evict_to_cap",
            serde_json::json!({"cap": 2}),
        ))
        .await;
    let result = unwrap_success(resp);
    let evicted: Vec<&str> = result["evicted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // Bob (next-oldest, unpinned) goes; Alice survives.
    assert_eq!(evicted, vec!["contacts/by-name/B/Bob.md"]);
    let remaining: Vec<String> = h
        .working_set
        .list_oldest_first()
        .await
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(
        remaining,
        vec!["contacts/by-name/A/Alice.md", "contacts/by-name/C/Carol.md"]
    );
}

#[tokio::test]
async fn health_summary_reports_drift_flags_from_working_set() {
    let h = setup();
    let head = insert_contact(&*h.store, "Sara_Chen", "Sara", "2026-05-25T08:00:00Z", None);
    let _ = h
        .dispatcher
        .handle(req(
            "working_set.materialize",
            serde_json::json!({"path": "contacts/by-name/S/Sara_Chen.md"}),
        ))
        .await;
    // Pre-drift: summary reports 0 drift flags.
    let resp = h
        .dispatcher
        .handle(req("health.summary", serde_json::Value::Null))
        .await;
    let summary = unwrap_success(resp);
    assert_eq!(summary["drift_flags"], 0);

    // Cause drift.
    insert_contact(
        &*h.store,
        "Sara_Chen",
        "Sarah",
        "2026-05-25T09:00:00Z",
        Some(head),
    );

    let resp = h
        .dispatcher
        .handle(req("health.summary", serde_json::Value::Null))
        .await;
    let summary = unwrap_success(resp);
    assert_eq!(
        summary["drift_flags"], 1,
        "expected drift count to reflect the staled projection; got {summary}"
    );
}
