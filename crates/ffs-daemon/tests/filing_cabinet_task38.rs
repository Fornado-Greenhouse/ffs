//! task_38 integration: registry-declared path families, opaque ids,
//! the path-to-entity index with collision qualifiers and redirect
//! stubs, the merged-entity stub, `path.families`, and `entity.search`
//! deriving the name field from the spec (ADR-028, ADR-030, ADR-031).
//!
//! Runs against the real starter specs and templates so the folder
//! set under test is the one the installer ships.

use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::multibase::decode_base58btc;
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::quarantine::{InMemoryQuarantine, IngestQuarantine};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomEnvelope, AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, Multihash, PathIndex,
    PredicateName, Proposal, PublicKey, SuppressionRegistry, Tier,
};
use ffs_daemon::Dispatcher;
use ffs_daemon::api::{ApiPayload, ApiRequest, ApiResponse};
use ffs_daemon::materializer::WorkingSetMaterializer;
use ffs_daemon::notify::EventPublisher;

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[38u8; 32])
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
    materializer: Arc<WorkingSetMaterializer>,
    renderer: Arc<ProjectionRenderer>,
    store: Arc<dyn AtomStore>,
    quarantine: Arc<InMemoryQuarantine>,
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
    let quarantine = Arc::new(InMemoryQuarantine::new());
    let notifier = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer: renderer.clone(),
        notifier,
        owner: owner_pk(),
        quarantine: quarantine.clone(),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(owner_key())),
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: None,
        skill_invoker: None,
    });
    let materializer = Arc::new(WorkingSetMaterializer::new(
        renderer.clone(),
        store.clone(),
        Arc::new(InMemoryWorkingSet::new()),
        Arc::new(SuppressionRegistry::new()),
        data_dir.clone(),
        owner_pk(),
    ));
    Harness {
        _dir: dir,
        data_dir,
        dispatcher,
        materializer,
        renderer,
        store,
        quarantine,
        index,
    }
}

fn req(method: &str, params: serde_json::Value) -> ApiRequest {
    ApiRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: method.into(),
        params,
    }
}

fn unwrap_ok(resp: ApiResponse) -> serde_json::Value {
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success; got error: {error:?}"),
    }
}

fn atom(
    entity: &EntityId,
    predicate: &str,
    claim: serde_json::Value,
    tx: &str,
    supersedes: Option<Multihash>,
) -> AtomEnvelope {
    AtomTemplate {
        v: 1,
        entity: entity.clone(),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: ts("2026-05-31T00:00:00Z"),
        valid_to: None,
        tx_time: ts(tx),
        classification: Tier::new("existence"),
        supersedes,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap()
}

#[tokio::test]
async fn path_families_rpc_lists_the_starter_folders() {
    let h = setup();
    let result = unwrap_ok(
        h.dispatcher
            .handle(req("path.families", serde_json::json!({})))
            .await,
    );
    let rows = result.as_array().expect("bare array");
    let folders: Vec<&str> = rows.iter().map(|r| r["family"].as_str().unwrap()).collect();
    assert_eq!(
        folders,
        vec!["articles", "contacts", "events", "notes", "orgs", "people"]
    );
    let notes = rows.iter().find(|r| r["family"] == "notes").unwrap();
    assert_eq!(notes["predicate"], "note");
    assert_eq!(notes["name_field"], "title");
}

#[tokio::test]
async fn entity_search_uses_the_spec_name_field_and_returns_basenames() {
    let h = setup();
    let sara = EntityId::mint();
    h.store
        .insert(&atom(
            &sara,
            "contact.person",
            serde_json::json!({"display_name": "Sara Chen"}),
            "2026-05-31T00:00:01Z",
            None,
        ))
        .unwrap();
    h.index.assign("contacts", &sara, "Sara Chen", &[]).unwrap();
    let plan = EntityId::new("Quarterly_Plan");
    h.store
        .insert(&atom(
            &plan,
            "note",
            serde_json::json!({"title": "Quarterly Plan", "body": "x"}),
            "2026-05-31T00:00:02Z",
            None,
        ))
        .unwrap();

    let by_title = unwrap_ok(
        h.dispatcher
            .handle(req(
                "entity.search",
                serde_json::json!({"query": "quarterly"}),
            ))
            .await,
    );
    let hits = by_title["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["predicate"], "note");
    assert_eq!(hits[0]["display_name"], "Quarterly Plan");
    assert!(
        hits[0].get("basename").is_none(),
        "no index row for a slug-id note"
    );

    let by_name = unwrap_ok(
        h.dispatcher
            .handle(req("entity.search", serde_json::json!({"query": "sara"})))
            .await,
    );
    let hits = by_name["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["entity"], sara.as_str());
    assert_eq!(hits[0]["basename"], "Sara_Chen");
}

#[tokio::test]
async fn ingest_accept_mints_opaque_ids_and_the_file_is_named_from_the_display_name() {
    let h = setup();
    let id = h
        .quarantine
        .submit("file:///ingest/acme-person.md".into(), b"x".to_vec())
        .await
        .unwrap();
    h.quarantine
        .complete(
            &id,
            vec![Proposal {
                predicate: PredicateName::new("contact.person"),
                claim: serde_json::json!({"display_name": "Acme Person"}),
                provenance: vec![],
                rationale: "test".into(),
                engine: None,
                model: None,
                local_ref: None,
                refs: vec![],
                valid_from: None,
                valid_to: None,
                ends_role: false,
                entity: None,
                resolution: None,
                candidates: vec![],
            }],
        )
        .await
        .unwrap();
    let result = unwrap_ok(
        h.dispatcher
            .handle(req(
                "ingest.accept",
                serde_json::json!({"submission_id": id}),
            ))
            .await,
    );
    let hash = result["accepted_atom_hashes"][0]
        .as_str()
        .unwrap()
        .to_string();
    let env = h
        .store
        .get(&Multihash::from_multibase(&hash).unwrap())
        .unwrap()
        .expect("accepted atom stored");
    let entity = env.entity.as_str();
    assert!(entity.starts_with('z'), "opaque multibase id, got {entity}");
    assert_eq!(decode_base58btc(entity).unwrap().len(), 16);
    assert_ne!(
        entity, "Acme_Person",
        "the id must not be the display-name slug"
    );

    let written = h
        .materializer
        .handle_commit(&env.entity, &env.predicate)
        .await
        .unwrap()
        .expect("materialized");
    assert_eq!(
        written.path,
        h.data_dir.join("contacts/by-name/A/Acme_Person.md")
    );
}

#[tokio::test]
async fn org_company_materializes_under_orgs_and_events_list_by_recency() {
    let h = setup();
    let acme = EntityId::mint();
    h.store
        .insert(&atom(
            &acme,
            "org.company",
            serde_json::json!({"display_name": "Acme", "industry": "widgets"}),
            "2026-05-31T00:00:01Z",
            None,
        ))
        .unwrap();
    let written = h
        .materializer
        .handle_commit(&acme, &PredicateName::new("org.company"))
        .await
        .unwrap()
        .expect("org materialized");
    assert_eq!(written.path, h.data_dir.join("orgs/by-name/A/Acme.md"));
    let on_disk = std::fs::read_to_string(&written.path).unwrap();
    assert!(on_disk.contains("display_name: Acme"), "{on_disk}");

    let raise = EntityId::mint();
    h.store
        .insert(&atom(
            &raise,
            "event.business",
            serde_json::json!({
                "title": "Acme raises a round",
                "kind": "funding",
                "date": "2026-05-30",
                "participants": [{"display": "Acme", "role": "investee"}]
            }),
            "2026-05-31T00:00:02Z",
            None,
        ))
        .unwrap();
    h.materializer
        .handle_commit(&raise, &PredicateName::new("event.business"))
        .await
        .unwrap()
        .expect("event materialized");
    let listing = h
        .renderer
        .render(&ProjectionRequest {
            path: "events/recent/".into(),
            as_of: None,
            agent: owner_pk(),
        })
        .unwrap();
    assert!(
        listing.markdown.contains("Acme_raises_a_round"),
        "recency listing should name the event by basename: {}",
        listing.markdown
    );
}

#[tokio::test]
async fn two_people_with_the_same_name_get_qualified_basenames() {
    let h = setup();
    let first = EntityId::mint();
    let second = EntityId::mint();
    h.store
        .insert(&atom(
            &first,
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Acme"}),
            "2026-05-31T00:00:01Z",
            None,
        ))
        .unwrap();
    h.store
        .insert(&atom(
            &second,
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "City Council"}),
            "2026-05-31T00:00:02Z",
            None,
        ))
        .unwrap();
    let a = h
        .materializer
        .handle_commit(&first, &PredicateName::new("person.generic"))
        .await
        .unwrap()
        .unwrap();
    let b = h
        .materializer
        .handle_commit(&second, &PredicateName::new("person.generic"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.path, h.data_dir.join("people/by-name/S/Sara_Chen.md"));
    assert_eq!(
        b.path,
        h.data_dir
            .join("people/by-name/S/Sara_Chen_(City_Council).md")
    );
    assert_eq!(
        h.index
            .resolve("people", "Sara_Chen_(City_Council)")
            .unwrap(),
        Some(second.clone())
    );
}

#[tokio::test]
async fn rename_moves_the_file_and_leaves_a_redirect_stub() {
    let h = setup();
    let sara = EntityId::mint();
    let v1 = atom(
        &sara,
        "contact.person",
        serde_json::json!({"display_name": "Sara Chen"}),
        "2026-05-31T00:00:01Z",
        None,
    );
    let v1_hash = h.store.insert(&v1).unwrap();
    let before = h
        .materializer
        .handle_commit(&sara, &PredicateName::new("contact.person"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.path,
        h.data_dir.join("contacts/by-name/S/Sara_Chen.md")
    );

    h.store
        .insert(&atom(
            &sara,
            "contact.person",
            serde_json::json!({"display_name": "Sara Chen-Lee"}),
            "2026-05-31T00:00:02Z",
            Some(v1_hash),
        ))
        .unwrap();
    let after = h
        .materializer
        .handle_commit(&sara, &PredicateName::new("contact.person"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.path,
        h.data_dir.join("contacts/by-name/S/Sara_Chen-Lee.md")
    );
    let stub = std::fs::read_to_string(&before.path).unwrap();
    assert_eq!(stub, "Moved to [[Sara_Chen-Lee|Sara Chen-Lee]]\n");
}

#[tokio::test]
async fn merged_entity_writes_only_the_stub() {
    let h = setup();
    let winner = EntityId::mint();
    let loser = EntityId::mint();
    h.store
        .insert(&atom(
            &winner,
            "person.generic",
            serde_json::json!({"display_name": "Pat Example"}),
            "2026-05-31T00:00:01Z",
            None,
        ))
        .unwrap();
    h.store
        .insert(&atom(
            &loser,
            "person.generic",
            serde_json::json!({"display_name": "P. Example"}),
            "2026-05-31T00:00:02Z",
            None,
        ))
        .unwrap();
    h.materializer
        .handle_commit(&winner, &PredicateName::new("person.generic"))
        .await
        .unwrap()
        .unwrap();
    h.store
        .insert(&atom(
            &loser,
            "entity.same_as",
            serde_json::json!({"target": winner.as_str(), "reason": "same person"}),
            "2026-05-31T00:00:03Z",
            None,
        ))
        .unwrap();
    let written = h
        .materializer
        .handle_commit(&loser, &PredicateName::new("person.generic"))
        .await
        .unwrap()
        .expect("stub written");
    let on_disk = std::fs::read_to_string(&written.path).unwrap();
    assert!(
        on_disk.starts_with("Merged into [[Pat_Example|Pat Example]]"),
        "{on_disk}"
    );
    assert_eq!(
        on_disk.lines().count(),
        1,
        "stub is a single line: {on_disk:?}"
    );
}
