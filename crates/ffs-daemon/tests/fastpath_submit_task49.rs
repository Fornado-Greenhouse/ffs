//! task_49 / ADR-036: `fastpath.submit` on the dispatcher. A direct
//! submission of a projection edit runs the same `process_edit` the
//! watcher runs on a filesystem event, so the plugin's edit routing and
//! tests get the classifier's answer without touching the filesystem
//! watcher. The result is the fast path's `EditOutcome`, tagged by
//! `outcome`.

use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, PathIndex, PredicateName, PublicKey,
    SuppressionRegistry, Tier,
};
use ffs_daemon::Dispatcher;
use ffs_daemon::api::{ApiPayload, ApiRequest};
use ffs_daemon::notify::EventPublisher;

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[49u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

struct Harness {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    dispatcher: Arc<Dispatcher>,
    store: Arc<dyn AtomStore>,
    index: Arc<dyn PathIndex>,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    let registry = Arc::new(SpecRegistry::new());
    registry
        .load_dir(&repo_root().join("starter").join("predicates"))
        .unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![Action::Read, Action::Write, Action::Supersede],
        CapabilityScope::default(),
        ts("2026-01-01T00:00:00Z"),
        None,
        ts("2026-01-01T00:00:01Z"),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
    let index: Arc<dyn PathIndex> = InMemoryPathIndex::new_arc();
    let renderer = Arc::new(
        ProjectionRenderer::new(
            store.clone(),
            registry.clone(),
            &repo_root().join("starter").join("templates"),
        )
        .unwrap()
        .with_path_index(index.clone()),
    );
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier: Arc::new(EventPublisher::new()),
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(owner_key())),
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: Some(data_dir.clone()),
        skill_invoker: None,
        ingest_agent_identity: None,
        suppression: Some(Arc::new(SuppressionRegistry::new())),
    });
    Harness {
        _dir: dir,
        data_dir,
        dispatcher,
        store,
        index,
    }
}

fn insert_contact(h: &Harness, entity: &EntityId, claim: serde_json::Value) {
    let tmpl = AtomTemplate {
        v: 1,
        entity: entity.clone(),
        predicate: PredicateName::new("contact.person"),
        claim,
        valid_from: ts("2026-09-01T00:00:00Z"),
        valid_to: None,
        tx_time: ts("2026-09-01T00:00:00Z"),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    };
    h.store.insert(&tmpl.sign(&owner_key()).unwrap()).unwrap();
}

async fn submit(h: &Harness, path: &str, content: &str) -> ApiPayload {
    let req = ApiRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: "fastpath.submit".into(),
        params: serde_json::json!({"projection_path": path, "new_content": content}),
    };
    h.dispatcher.handle(req).await.payload
}

#[tokio::test]
async fn submit_applies_an_additive_edit_as_a_supersession() {
    let h = setup();
    let entity = EntityId::mint();
    insert_contact(
        &h,
        &entity,
        serde_json::json!({"display_name": "Sara Chen", "notes": ["met at picnic"]}),
    );
    let basename = h
        .index
        .assign("contacts", &entity, "Sara Chen", &[])
        .unwrap();
    let path = format!("contacts/by-name/S/{basename}.md");
    let edited =
        "---\ndisplay_name: Sara Chen\n---\n\n## Notes\n- met at picnic\n- brought cookies\n";

    let payload = submit(&h, &path, edited).await;
    let ApiPayload::Success { result } = payload else {
        panic!("expected success, got {payload:?}");
    };
    assert_eq!(
        result.get("outcome").and_then(|v| v.as_str()),
        Some("applied")
    );
    let hash = result
        .get("atom_hash")
        .and_then(|v| v.as_str())
        .expect("applied outcome carries the atom hash");
    let atoms = h
        .store
        .list_by_entity(&entity, Some(&PredicateName::new("contact.person")), None)
        .unwrap();
    assert_eq!(atoms.len(), 2, "head plus one supersession");
    let newest = atoms
        .iter()
        .find(|a| a.content_hash().unwrap().to_multibase() == hash)
        .expect("the reported hash is in the store");
    assert!(newest.supersedes.is_some());
    assert_eq!(
        newest.claim.get("notes"),
        Some(&serde_json::json!(["met at picnic", "brought cookies"]))
    );
}

#[tokio::test]
async fn submit_routes_an_ambiguous_edit_to_ingest() {
    let h = setup();
    let entity = EntityId::mint();
    insert_contact(
        &h,
        &entity,
        serde_json::json!({"display_name": "Sara Chen", "notes": ["met at picnic"]}),
    );
    let basename = h
        .index
        .assign("contacts", &entity, "Sara Chen", &[])
        .unwrap();
    let path = format!("contacts/by-name/S/{basename}.md");
    // Two changes at once (a renamed contact and a new bullet) is not
    // one reverse-map rule; the fast path writes it to ingest instead.
    let edited =
        "---\ndisplay_name: Sara C. Chen\n---\n\n## Notes\n- met at picnic\n- brought cookies\n";

    let ApiPayload::Success { result } = submit(&h, &path, edited).await else {
        panic!("expected success");
    };
    assert_eq!(
        result.get("outcome").and_then(|v| v.as_str()),
        Some("routed_to_ingest"),
        "{result}"
    );
    let submission = result
        .get("submission_path")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(
        h.data_dir.join("ingest").join(submission).exists() || PathBuf::from(submission).exists(),
        "the correction landed under ingest/: {submission}"
    );
    let atoms = h
        .store
        .list_by_entity(&entity, Some(&PredicateName::new("contact.person")), None)
        .unwrap();
    assert_eq!(atoms.len(), 1, "no atom authored on the slow path");
}

#[tokio::test]
async fn submit_ignores_paths_outside_the_watched_roots() {
    let h = setup();
    let ApiPayload::Success { result } = submit(&h, "ingest/dropped.md", "# hello\n").await else {
        panic!("expected success");
    };
    assert_eq!(
        result.get("outcome").and_then(|v| v.as_str()),
        Some("ignored"),
        "{result}"
    );
}

#[tokio::test]
async fn submit_rejects_parent_traversal() {
    let h = setup();
    let payload = submit(&h, "contacts/../../etc/passwd", "x").await;
    let ApiPayload::Error { error } = payload else {
        panic!("expected an error, got {payload:?}");
    };
    assert_eq!(error.code, -32602, "invalid params: {}", error.message);
}
