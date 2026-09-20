//! task_41 daemon slice: `audit.query` `kind`, `audit.publish_summary`
//! `predicate`, the entity-less windowed `atom.list`, `audit.run`, the
//! dispatcher-backed skill proxy, and the materializer filing a briefing
//! at `briefings/<date>.md`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::{FamilyTable, ProjectionRenderer};
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, PathIndex, PredicateName, PublicKey,
    SuppressionRegistry, Tier,
};
use ffs_daemon::api::{ApiPayload, ApiRequest};
use ffs_daemon::dispatch::SkillInvoker;
use ffs_daemon::notify::EventPublisher;
use ffs_daemon::{Dispatcher, DispatcherProxy, WorkingSetMaterializer};
use ffs_skills_host::SubstrateAccess;

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[41u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

struct StubInvoker {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl SkillInvoker for StubInvoker {
    async fn invoke(&self, skill: &str, input: Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push((skill.to_string(), input));
        Ok(json!({"atom_hash": "zStub", "reason": null}))
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    dispatcher: Arc<Dispatcher>,
    store: Arc<dyn AtomStore>,
    registry: Arc<SpecRegistry>,
    renderer: Arc<ProjectionRenderer>,
    invoker: Arc<StubInvoker>,
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
    let invoker = Arc::new(StubInvoker {
        calls: Mutex::new(Vec::new()),
    });
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry: registry.clone(),
        renderer: renderer.clone(),
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
        skill_invoker: Some(invoker.clone()),
        ingest_agent_identity: None,
        suppression: Some(Arc::new(SuppressionRegistry::new())),
    });
    Harness {
        _dir: dir,
        data_dir,
        dispatcher,
        store,
        registry,
        renderer,
        invoker,
    }
}

async fn call(h: &Harness, method: &str, params: Value) -> ApiPayload {
    h.dispatcher
        .handle(ApiRequest {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: method.into(),
            params,
        })
        .await
        .payload
}

fn ok(p: ApiPayload) -> Value {
    match p {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success, got {error:?}"),
    }
}

fn err(p: ApiPayload) -> ffs_daemon::ApiError {
    match p {
        ApiPayload::Error { error } => error,
        ApiPayload::Success { result } => panic!("expected error, got {result}"),
    }
}

fn briefing_claim(date: &str) -> Value {
    json!({
        "date": date,
        "window": {"from": "2026-09-13T00:00:00Z", "to": format!("{date}T07:00:00Z")},
        "narrative": "quiet week",
        "new_people": [], "changes": [], "trending_orgs": [], "events": [],
        "promotion_candidates": [], "follow_ups": [], "needs_your_eye": [],
        "possible_duplicates": [], "recent_merges": [],
        "filing": {"auto_filed_count": 0, "reviewed_count": 0}
    })
}

#[tokio::test]
async fn audit_query_kind_filters_briefing_from_daily_summary() {
    let h = setup();
    ok(call(
        &h,
        "audit.publish_summary",
        json!({"claim": {"narrative": "all quiet", "panel": []}}),
    )
    .await);
    ok(call(
        &h,
        "audit.publish_summary",
        json!({"claim": briefing_claim("2026-09-20"), "predicate": "auditor.briefing"}),
    )
    .await);

    let daily = ok(call(&h, "audit.query", json!({})).await);
    let daily = daily.as_array().unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0]["predicate"], "auditor.daily_summary");
    assert_eq!(daily[0]["entity"], "auditor");
    assert!(daily[0]["hash"].is_string(), "rows carry their hash");

    let briefings = ok(call(&h, "audit.query", json!({"kind": "briefing"})).await);
    let briefings = briefings.as_array().unwrap();
    assert_eq!(briefings.len(), 1);
    assert_eq!(briefings[0]["predicate"], "auditor.briefing");
    assert_ne!(
        briefings[0]["entity"], "auditor",
        "a briefing is its own entity"
    );
    assert_eq!(briefings[0]["claim"]["date"], "2026-09-20");

    let explicit = ok(call(&h, "audit.query", json!({"kind": "daily_summary"})).await);
    assert_eq!(explicit.as_array().unwrap().len(), 1);
    let bad = err(call(&h, "audit.query", json!({"kind": "gossip"})).await);
    assert_eq!(bad.code, -32602);
}

#[tokio::test]
async fn audit_publish_summary_rejects_non_auditor_predicate_and_invalid_briefings() {
    let h = setup();
    let e = err(call(
        &h,
        "audit.publish_summary",
        json!({"claim": {"display_name": "x"}, "predicate": "contact.person"}),
    )
    .await);
    assert_eq!(e.code, -32602, "{}", e.message);
    assert!(e.message.contains("auditor.briefing"), "{}", e.message);

    // A briefing must satisfy the registered spec (date, window,
    // narrative are required).
    let e = err(call(
        &h,
        "audit.publish_summary",
        json!({"claim": {"narrative": "no window"}, "predicate": "auditor.briefing"}),
    )
    .await);
    assert_eq!(e.code, -32602, "{}", e.message);
    assert!(
        e.message.contains("briefing claim rejected"),
        "{}",
        e.message
    );
}

#[tokio::test]
async fn two_briefings_are_two_entities_and_never_supersede() {
    let h = setup();
    let a = ok(call(
        &h,
        "audit.publish_summary",
        json!({"claim": briefing_claim("2026-09-13"), "predicate": "auditor.briefing"}),
    )
    .await);
    let b = ok(call(
        &h,
        "audit.publish_summary",
        json!({"claim": briefing_claim("2026-09-20"), "predicate": "auditor.briefing"}),
    )
    .await);
    assert_ne!(a["atom_hash"], b["atom_hash"]);
    let rows = ok(call(&h, "audit.query", json!({"kind": "briefing"})).await);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["entity"], rows[1]["entity"]);
    assert!(rows.iter().all(|r| r["supersedes"].is_null()));
}

fn insert_person(h: &Harness, name: &str, tx: &str) -> EntityId {
    let entity = EntityId::mint();
    let atom = AtomTemplate {
        v: 1,
        entity: entity.clone(),
        predicate: PredicateName::new("person.generic"),
        claim: json!({"display_name": name}),
        valid_from: ts("2026-09-01T00:00:00Z"),
        valid_to: None,
        tx_time: ts(tx),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    h.store.insert(&atom).unwrap();
    entity
}

#[tokio::test]
async fn atom_list_windowed_form_lists_by_predicate_since_with_hashes() {
    let h = setup();
    let old = insert_person(&h, "Old Timer", "2026-09-01T00:00:00Z");
    let new = insert_person(&h, "New Face", "2026-09-15T00:00:00Z");

    let all = ok(call(&h, "atom.list", json!({"predicate": "person.generic"})).await);
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0]["entity"], new.as_str(), "newest first");
    assert!(all[0]["hash"].is_string());

    let since = ok(call(
        &h,
        "atom.list",
        json!({"predicate": "person.generic", "since": "2026-09-10T00:00:00Z"}),
    )
    .await);
    let since = since.as_array().unwrap();
    assert_eq!(since.len(), 1);
    assert_eq!(since[0]["entity"], new.as_str());

    let limited = ok(call(
        &h,
        "atom.list",
        json!({"predicate": "person.generic", "limit": 1}),
    )
    .await);
    assert_eq!(limited.as_array().unwrap().len(), 1);

    // The entity-scoped form is unchanged and also carries hashes.
    let by_entity = ok(call(&h, "atom.list", json!({"entity": old.as_str()})).await);
    let by_entity = by_entity.as_array().unwrap();
    assert_eq!(by_entity.len(), 1);
    assert!(by_entity[0]["hash"].is_string());

    let e = err(call(&h, "atom.list", json!({})).await);
    assert_eq!(e.code, -32602);
    let e = err(call(
        &h,
        "atom.list",
        json!({"predicate": "person.generic", "limit": 999999}),
    )
    .await);
    assert_eq!(e.code, -32602);
}

#[tokio::test]
async fn audit_run_invokes_the_auditor_with_the_requested_op() {
    let h = setup();
    ok(call(&h, "audit.run", json!({"op": "briefing", "window_days": 3})).await);
    ok(call(&h, "audit.run", json!({})).await);
    let calls = h.invoker.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "auditor");
    assert_eq!(calls[0].1, json!({"op": "briefing", "window_days": 3}));
    assert_eq!(calls[1].1, json!({"op": "tick"}));
    let e = err(call(&h, "audit.run", json!({"op": "gossip"})).await);
    assert_eq!(e.code, -32602);
}

#[tokio::test]
async fn skill_proxy_routes_allowed_queries_and_refuses_the_rest() {
    let h = setup();
    let proxy = DispatcherProxy::new();
    assert!(proxy.install(h.dispatcher.clone()));
    assert!(!proxy.install(h.dispatcher.clone()), "install is once");

    let summary = proxy
        .handle_query("auditor", "health.summary", json!({}))
        .await
        .unwrap();
    assert!(summary.get("proposals").is_some(), "{summary}");

    let published = proxy
        .handle_query(
            "auditor",
            "audit.publish_summary",
            json!({"claim": briefing_claim("2026-09-20"), "predicate": "auditor.briefing"}),
        )
        .await
        .unwrap();
    assert!(published["atom_hash"].is_string());

    for denied in [
        "ingest.accept",
        "ingest.reject",
        "entity.merge",
        "entity.unmerge",
        "entity.assert_different",
        "capability.grant",
        "ingest.retract",
        "fastpath.submit",
    ] {
        let e = proxy
            .handle_query("auditor", denied, json!({}))
            .await
            .unwrap_err();
        assert!(e.contains("allow-list"), "{denied}: {e}");
    }
    // A dispatcher error comes back as an Err with the message.
    let e = proxy
        .handle_query("auditor", "atom.list", json!({}))
        .await
        .unwrap_err();
    assert!(e.contains("atom.list requires"), "{e}");
}

#[tokio::test]
async fn materializer_files_a_briefing_at_its_date_under_briefings() {
    let h = setup();
    let published = ok(call(
        &h,
        "audit.publish_summary",
        json!({"claim": briefing_claim("2026-09-20"), "predicate": "auditor.briefing"}),
    )
    .await);
    let rows = ok(call(&h, "audit.query", json!({"kind": "briefing"})).await);
    let entity = EntityId::new(rows[0]["entity"].as_str().unwrap());
    assert!(published["atom_hash"].is_string());

    let materializer = WorkingSetMaterializer::new(
        h.renderer.clone(),
        h.store.clone(),
        Arc::new(InMemoryWorkingSet::new()),
        Arc::new(SuppressionRegistry::new()),
        h.data_dir.clone(),
        owner_pk(),
    );
    let family = FamilyTable::from_registry(&h.registry)
        .for_folder("briefings")
        .unwrap();
    let written = materializer
        .materialize_entity(&family, &entity)
        .await
        .unwrap()
        .expect("briefing written");
    assert_eq!(
        written.path,
        h.data_dir.join("briefings").join("2026-09-20.md")
    );
    let text = std::fs::read_to_string(&written.path).unwrap();
    assert!(text.contains("## Narrative"), "{text}");
    assert!(text.contains("quiet week"), "{text}");
}
