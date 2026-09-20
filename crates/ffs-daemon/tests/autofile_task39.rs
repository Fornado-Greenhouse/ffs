//! task_39 end to end (ADR-029, ADR-030): auto-filing under an `Accept`
//! grant through the real `ingest.submit` path, the additive rule, the
//! daily cap, default-off, retraction, the reconciliation shorthand,
//! identity assertions, merge and unmerge with rendering, and the
//! capability admin RPCs.
//!
//! Runs against the real starter specs, templates, and
//! `resolution.toml`, with the daemon acting as `mcp:agent/courier` for
//! `file://` drops (the courier's identity, `FFS_INGEST_AGENT_IDENTITY`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::{CrossRef, InMemoryQuarantine, IngestQuarantine, SubmissionStatus};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    EntityId, InMemoryPathIndex, Iso8601, Multihash, PathIndex, PredicateName, Proposal, PublicKey,
    SourceKind, SuppressionRegistry,
};
use ffs_daemon::api::{ApiPayload, ApiRequest, ApiResponse};
use ffs_daemon::dispatch::{ScribeExtractError, ScribeExtractor};
use ffs_daemon::materializer::WorkingSetMaterializer;
use ffs_daemon::notify::EventPublisher;
use ffs_daemon::{Dispatcher, ResolvingExtractor};

const COURIER: &str = "mcp:agent/courier";
const FILING_PREDICATES: &str =
    "source.article,event.business,org.company,person.generic,affiliation";

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[46u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

struct CannedScribe {
    sets: std::sync::Mutex<Vec<Vec<Proposal>>>,
}

#[async_trait::async_trait]
impl ScribeExtractor for CannedScribe {
    async fn extract(&self, _: &str, _: &[u8]) -> Result<Vec<Proposal>, ScribeExtractError> {
        let mut g = self.sets.lock().unwrap();
        if g.is_empty() {
            return Ok(vec![]);
        }
        Ok(g.remove(0))
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    dispatcher: Arc<Dispatcher>,
    materializer: Arc<WorkingSetMaterializer>,
    store: Arc<dyn AtomStore>,
    quarantine: Arc<InMemoryQuarantine>,
    canned: Arc<CannedScribe>,
    _index: Arc<dyn PathIndex>,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    std::fs::create_dir_all(data_dir.join("config")).unwrap();
    std::fs::copy(
        repo_root().join("starter/config/resolution.toml"),
        data_dir.join("config/resolution.toml"),
    )
    .unwrap();
    let registry = Arc::new(SpecRegistry::new());
    registry
        .load_dir(&repo_root().join("starter").join("predicates"))
        .unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    // The owner's bootstrap self-grant: never `accept` (ADR-029).
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
    let canned = Arc::new(CannedScribe {
        sets: std::sync::Mutex::new(vec![]),
    });
    let inner: Arc<dyn ScribeExtractor> = canned.clone();
    let resolver = Arc::new(ResolvingExtractor::new(
        inner,
        store.clone(),
        registry.clone(),
        Some(&data_dir),
    ));
    let scribe: Arc<dyn ScribeExtractor> = resolver;
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry: registry.clone(),
        renderer: renderer.clone(),
        notifier,
        owner: owner_pk(),
        quarantine: quarantine.clone(),
        scribe: Some(scribe),
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(owner_key())),
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: None,
        skill_invoker: None,
        ingest_agent_identity: Some(COURIER.into()),
        suppression: None,
    });
    let materializer = Arc::new(WorkingSetMaterializer::new(
        renderer,
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
        store,
        quarantine,
        canned,
        _index: index,
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

fn unwrap_err(resp: ApiResponse) -> ffs_daemon::api::ApiError {
    match resp.payload {
        ApiPayload::Success { result } => panic!("expected an error; got {result}"),
        ApiPayload::Error { error } => error,
    }
}

async fn call(h: &Harness, method: &str, params: serde_json::Value) -> ApiResponse {
    h.dispatcher.handle(req(method, params)).await
}

fn proposal(predicate: &str, claim: serde_json::Value, local_ref: &str) -> Proposal {
    let mut p = Proposal::new(PredicateName::new(predicate), claim, vec![], "canned");
    p.local_ref = Some(local_ref.into());
    p
}

fn with_refs(mut p: Proposal, refs: &[(&str, &str)]) -> Proposal {
    p.refs = refs
        .iter()
        .map(|(f, r)| CrossRef {
            field: CrossRef::from_wire_field(f),
            local_ref: (*r).to_string(),
        })
        .collect();
    p
}

/// The canonical one-article set: article, org, person, affiliation,
/// event, cross-referenced by local_ref.
fn hire_set(article_url: &str, person: &str, org: &str, role: &str) -> Vec<Proposal> {
    vec![
        with_refs(
            proposal(
                "source.article",
                serde_json::json!({
                    "title": format!("{org} names {person} as {role}"),
                    "url": article_url,
                    "publication": "Example Ledger",
                    "published_at": "2026-09-10",
                    "summary": "synthetic",
                    "mentions": [
                        {"display": person, "context": role},
                        {"display": org, "context": "employer"}
                    ]
                }),
                "article",
            ),
            &[
                ("mentions[0].entity", "person-1"),
                ("mentions[1].entity", "org-1"),
            ],
        ),
        proposal(
            "org.company",
            serde_json::json!({"display_name": org}),
            "org-1",
        ),
        with_refs(
            proposal(
                "person.generic",
                serde_json::json!({"display_name": person, "organization": org, "role": role}),
                "person-1",
            ),
            &[("organization", "org-1")],
        ),
        with_refs(
            proposal(
                "affiliation",
                serde_json::json!({"person": person, "organization": org, "title": role, "kind": "executive", "source": article_url}),
                "aff-1",
            ),
            &[("person", "person-1"), ("organization", "org-1")],
        ),
        with_refs(
            proposal(
                "event.business",
                serde_json::json!({
                    "title": format!("{person} hired"),
                    "kind": "hire",
                    "date": "2026-09-10",
                    "participants": [
                        {"display": person, "role": "hire"},
                        {"display": org, "role": "employer"}
                    ],
                    "source": article_url
                }),
                "event-1",
            ),
            &[
                ("participants[0].entity", "person-1"),
                ("participants[1].entity", "org-1"),
            ],
        ),
    ]
}

/// Submit through `ingest.submit` and wait for extraction plus the
/// auto-file pass that follows it. Returns the settled submission.
async fn submit_and_settle(
    h: &Harness,
    uri: &str,
    set: Vec<Proposal>,
) -> ffs_core::quarantine::Submission {
    h.canned.sets.lock().unwrap().push(set);
    let r = unwrap_ok(
        call(
            h,
            "ingest.submit",
            serde_json::json!({"source_uri": uri, "content": "# synthetic\n"}),
        )
        .await,
    );
    let id = r["submission_id"].as_str().unwrap().to_string();
    let mut extracted_at: Option<std::time::Instant> = None;
    for _ in 0..400 {
        if let Some(s) = h.quarantine.get(&id).await {
            match s.status {
                SubmissionStatus::AutoAccepted | SubmissionStatus::PartiallyAccepted => {
                    return s;
                }
                SubmissionStatus::Extracted => {
                    // Auto-filing runs right after extraction in the
                    // same task; give it a moment to say nothing.
                    let t = *extracted_at.get_or_insert_with(std::time::Instant::now);
                    if t.elapsed() > std::time::Duration::from_millis(300) {
                        return s;
                    }
                }
                SubmissionStatus::Failed => panic!("submission {id} failed"),
                _ => {}
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("submission {id} never settled");
}

async fn grant_accept(h: &Harness, predicates: &str, cap: serde_json::Value) -> String {
    let mut params = serde_json::json!({
        "action": "accept",
        "grantee": COURIER,
        "predicates": predicates.split(',').collect::<Vec<_>>(),
        "classifications": [],
    });
    if let Some(n) = cap.as_u64() {
        params["max_per_day"] = serde_json::json!(n);
    } else {
        params["unlimited"] = serde_json::json!(true);
    }
    unwrap_ok(call(h, "capability.grant", params).await)["grant_hash"]
        .as_str()
        .unwrap()
        .to_string()
}

fn head(h: &Harness, entity: &EntityId, predicate: &str) -> ffs_core::AtomEnvelope {
    h.store
        .head_of_chain(entity, &PredicateName::new(predicate), None)
        .unwrap()
        .unwrap_or_else(|| panic!("no head for {} {predicate}", entity.as_str()))
}

fn entities_of(h: &Harness, predicate: &str) -> HashSet<EntityId> {
    h.store
        .list_by_predicate(&PredicateName::new(predicate), None, 1000)
        .unwrap()
        .into_iter()
        .map(|a| a.entity)
        .collect()
}

fn only_entity(h: &Harness, predicate: &str) -> EntityId {
    let e = entities_of(h, predicate);
    assert_eq!(e.len(), 1, "expected exactly one {predicate} entity");
    e.into_iter().next().unwrap()
}

fn atoms_by_predicate(
    hashes: &[Multihash],
    h: &Harness,
) -> HashMap<String, ffs_core::AtomEnvelope> {
    hashes
        .iter()
        .map(|hsh| {
            let a = h.store.get(hsh).unwrap().unwrap();
            (a.predicate.as_str().to_string(), a)
        })
        .collect()
}

/// Render every filed atom twice (reverse-lookup sections link by
/// basename, which only exists once the other side has rendered).
async fn materialize(h: &Harness, hashes: &[Multihash]) {
    for _ in 0..2 {
        for hsh in hashes {
            let a = h.store.get(hsh).unwrap().unwrap();
            h.materializer
                .handle_commit(&a.entity, &a.predicate)
                .await
                .unwrap();
        }
    }
}

fn read(h: &Harness, rel: &str) -> String {
    std::fs::read_to_string(h.data_dir.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

async fn pending_ids(h: &Harness) -> Vec<String> {
    unwrap_ok(call(h, "ingest.list_pending", serde_json::Value::Null).await)
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

// ---- default off, additive filing, provenance ----

#[tokio::test]
async fn no_grant_on_a_fresh_substrate_leaves_every_proposal_pending() {
    let h = setup();
    let sub = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::Extracted);
    assert!(sub.auto_accepted_atom_hashes.is_empty());
    assert_eq!(pending_ids(&h).await, vec![sub.id.clone()]);
    assert!(entities_of(&h, "person.generic").is_empty());
    let health = unwrap_ok(call(&h, "health.summary", serde_json::Value::Null).await);
    assert_eq!(health["auto_filed"]["count"], 0, "{health}");
}

#[tokio::test]
async fn additive_set_files_under_the_grant_with_auto_accept_provenance_and_renders() {
    let h = setup();
    let grant = grant_accept(&h, FILING_PREDICATES, serde_json::json!(50)).await;
    let sub = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::AutoAccepted, "{sub:?}");
    assert_eq!(sub.auto_accepted_atom_hashes.len(), 5);
    assert!(
        pending_ids(&h).await.is_empty(),
        "auto-filed is not pending"
    );

    let atoms = atoms_by_predicate(&sub.auto_accepted_atom_hashes, &h);
    for (pred, a) in &atoms {
        let auto = a
            .provenance
            .iter()
            .find(|p| p.kind == SourceKind::AutoAccept)
            .unwrap_or_else(|| panic!("{pred}: no auto_accept provenance"));
        assert_eq!(auto.hash.to_multibase(), grant);
        assert_eq!(auto.uri, format!("ffs://local/atom/{grant}"));
    }
    // Cross-references bound within the filed set.
    let person = &atoms["person.generic"];
    let org = &atoms["org.company"];
    assert_eq!(person.claim["organization"], org.entity.as_str());
    assert_eq!(atoms["affiliation"].claim["person"], person.entity.as_str());
    assert_eq!(
        atoms["source.article"].claim["mentions"][0]["entity"],
        person.entity.as_str()
    );

    materialize(&h, &sub.auto_accepted_atom_hashes).await;
    let person_file = read(&h, "people/by-name/S/Sara_Chen.md");
    assert!(
        person_file.contains("[[Acme_Widgets|Acme Widgets]]"),
        "{person_file}"
    );

    let listed = unwrap_ok(call(&h, "ingest.list_auto_filed", serde_json::Value::Null).await);
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 5, "{listed:?}");
    assert!(listed.iter().all(|i| i["kind"] == "auto_accept"
        && i["submission_id"] == sub.id.as_str()
        && i["source_uri"] == "file:///ingest/a.md"));
    let health = unwrap_ok(call(&h, "health.summary", serde_json::Value::Null).await);
    assert_eq!(health["auto_filed"]["count"], 5, "{health}");
    assert_eq!(health["auto_filed"]["by_predicate"]["person.generic"], 1);

    let list = unwrap_ok(call(&h, "capability.list", serde_json::Value::Null).await);
    let mine = list
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["grant_hash"] == grant.as_str())
        .unwrap();
    assert_eq!(mine["used_today"], 5, "{mine}");
    assert_eq!(mine["max_per_day"], 50);
}

#[tokio::test]
async fn mcp_agent_source_names_its_own_grantee() {
    let h = setup();
    grant_accept(&h, "org.company", serde_json::json!(50)).await;
    let sub = submit_and_settle(
        &h,
        COURIER,
        vec![proposal(
            "org.company",
            serde_json::json!({"display_name": "Beta Corp"}),
            "org-1",
        )],
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::AutoAccepted);
    // Another agent's submission has no grant.
    let sub = submit_and_settle(
        &h,
        "mcp:agent/stranger",
        vec![proposal(
            "org.company",
            serde_json::json!({"display_name": "Gamma LLC"}),
            "org-1",
        )],
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::Extracted);
}

// ---- the additive rule: conflicts stay, mixed sets split ----

#[tokio::test]
async fn role_change_on_an_existing_person_stays_pending_and_the_remainder_accepts_without_duplicates()
 {
    let h = setup();
    grant_accept(&h, FILING_PREDICATES, serde_json::json!(50)).await;
    let first = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    assert_eq!(first.status, SubmissionStatus::AutoAccepted);
    let sara = only_entity(&h, "person.generic");
    let acme = only_entity(&h, "org.company");
    let sara_head_before = head(&h, &sara, "person.generic");

    // Same person, new role: the article and event are append-only
    // records, the org repeats an equal scalar, the affiliation is a
    // fresh record; the person's `role` would be overwritten.
    let second = submit_and_settle(
        &h,
        "file:///ingest/b.md",
        hire_set("https://example.test/b", "Sara Chen", "Acme Widgets", "CTO"),
    )
    .await;
    assert_eq!(
        second.status,
        SubmissionStatus::PartiallyAccepted,
        "{second:?}"
    );
    let filed = atoms_by_predicate(&second.auto_accepted_atom_hashes, &h);
    assert!(filed.contains_key("source.article"), "{:?}", filed.keys());
    assert!(filed.contains_key("event.business"), "{:?}", filed.keys());
    assert!(
        !filed.contains_key("person.generic"),
        "role change never auto-files"
    );
    let sara_head = head(&h, &sara, "person.generic");
    assert_eq!(
        sara_head.content_hash().unwrap(),
        sara_head_before.content_hash().unwrap()
    );
    assert_eq!(sara_head.claim["role"], "CEO");
    assert_eq!(
        pending_ids(&h).await,
        vec![second.id.clone()],
        "the remainder waits"
    );

    // The owner accepts the remainder: only the pending proposals file,
    // and the filed ones bind by reference instead of filing again.
    let before_orgs = entities_of(&h, "org.company").len();
    let before_articles = entities_of(&h, "source.article").len();
    let r = unwrap_ok(
        call(
            &h,
            "ingest.accept",
            serde_json::json!({"submission_id": second.id, "choices": {}}),
        )
        .await,
    );
    let accepted = r["accepted_atom_hashes"].as_array().unwrap();
    let accepted_preds: HashSet<String> = accepted
        .iter()
        .map(|hsh| {
            h.store
                .get(&serde_json::from_value(hsh.clone()).unwrap())
                .unwrap()
                .unwrap()
                .predicate
                .as_str()
                .to_string()
        })
        .collect();
    assert!(
        accepted_preds.contains("person.generic"),
        "{accepted_preds:?}"
    );
    assert!(
        !accepted_preds.contains("source.article"),
        "{accepted_preds:?}"
    );
    assert_eq!(
        entities_of(&h, "org.company").len(),
        before_orgs,
        "no duplicate org"
    );
    assert_eq!(entities_of(&h, "source.article").len(), before_articles);
    assert_eq!(
        entities_of(&h, "person.generic").len(),
        1,
        "no duplicate person"
    );
    // The owner's accept merges additively (ADR-030 § 6.2): the head's
    // scalars stay, the role change lives in the new affiliation.
    let sara_head = head(&h, &sara, "person.generic");
    assert_ne!(
        sara_head.content_hash().unwrap(),
        sara_head_before.content_hash().unwrap()
    );
    assert_eq!(sara_head.claim["role"], "CEO");
    assert_eq!(sara_head.claim["organization"], acme.as_str());
    let cto = h
        .store
        .list_by_predicate(&PredicateName::new("affiliation"), None, 100)
        .unwrap()
        .into_iter()
        .find(|a| a.claim["title"] == "CTO")
        .expect("the CTO affiliation auto-filed");
    assert_eq!(cto.claim["person"], sara.as_str());
    assert_eq!(
        h.quarantine.get(&second.id).await.unwrap().status,
        SubmissionStatus::Accepted
    );
    assert!(pending_ids(&h).await.is_empty());
}

#[tokio::test]
async fn max_per_day_one_routes_the_second_additive_proposal_to_review() {
    let h = setup();
    let grant = grant_accept(&h, "org.company", serde_json::json!(1)).await;
    let sub = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        vec![
            proposal(
                "org.company",
                serde_json::json!({"display_name": "Beta Corp"}),
                "org-1",
            ),
            proposal(
                "org.company",
                serde_json::json!({"display_name": "Gamma LLC"}),
                "org-2",
            ),
        ],
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::PartiallyAccepted, "{sub:?}");
    assert_eq!(sub.auto_accepted_atom_hashes.len(), 1);
    assert_eq!(
        h.store
            .count_auto_accepted_since(
                &Multihash::from_multibase(&grant).unwrap(),
                &ts("2000-01-01T00:00:00Z")
            )
            .unwrap(),
        1
    );

    // The cap is spent for the day: the next drop is pending only.
    let sub = submit_and_settle(
        &h,
        "file:///ingest/b.md",
        vec![proposal(
            "org.company",
            serde_json::json!({"display_name": "Delta Inc"}),
            "org-1",
        )],
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::Extracted);
    let list = unwrap_ok(call(&h, "capability.list", serde_json::Value::Null).await);
    let mine = list
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["grant_hash"] == grant.as_str())
        .unwrap();
    assert_eq!(mine["used_today"], 1);
    assert_eq!(mine["max_per_day"], 1);
}

#[tokio::test]
async fn grant_outside_the_predicate_scope_does_not_file() {
    let h = setup();
    grant_accept(&h, "org.company", serde_json::json!(50)).await;
    let sub = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::PartiallyAccepted);
    let filed = atoms_by_predicate(&sub.auto_accepted_atom_hashes, &h);
    assert_eq!(filed.keys().collect::<Vec<_>>(), vec!["org.company"]);
    assert!(entities_of(&h, "person.generic").is_empty());
}

// ---- retraction ----

#[tokio::test]
async fn retract_moves_the_head_back_rerenders_and_refuses_non_heads() {
    let h = setup();
    grant_accept(&h, FILING_PREDICATES, serde_json::json!(50)).await;
    let first = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    assert_eq!(first.status, SubmissionStatus::AutoAccepted);
    materialize(&h, &first.auto_accepted_atom_hashes).await;
    let sara = only_entity(&h, "person.generic");
    let original = head(&h, &sara, "person.generic");
    let original_hash = original.content_hash().unwrap();

    // A blank fill (`team`) on the existing person is additive and
    // supersedes her head. The organization travels as a reference,
    // as the scribe emits it.
    let second = submit_and_settle(
        &h,
        "file:///ingest/b.md",
        vec![
            proposal(
                "org.company",
                serde_json::json!({"display_name": "Acme Widgets"}),
                "org-1",
            ),
            with_refs(
                proposal(
                    "person.generic",
                    serde_json::json!({"display_name": "Sara Chen", "organization": "Acme Widgets", "role": "CEO", "team": "Executive"}),
                    "person-1",
                ),
                &[("organization", "org-1")],
            ),
        ],
    )
    .await;
    assert_eq!(second.status, SubmissionStatus::AutoAccepted, "{second:?}");
    let filled = head(&h, &sara, "person.generic");
    assert_eq!(filled.claim["team"], "Executive");
    assert_eq!(filled.supersedes.as_ref(), Some(&original_hash));
    materialize(&h, &second.auto_accepted_atom_hashes).await;
    assert!(read(&h, "people/by-name/S/Sara_Chen.md").contains("Executive"));

    // Not the head any more: refused.
    let err = unwrap_err(
        call(
            &h,
            "ingest.retract",
            serde_json::json!({"atom_hash": original_hash.to_multibase()}),
        )
        .await,
    );
    assert!(err.message.contains("not the head"), "{}", err.message);

    // Retract the auto-filed fill: the head is the original claim again.
    let filled_hash = filled.content_hash().unwrap();
    let r = unwrap_ok(
        call(
            &h,
            "ingest.retract",
            serde_json::json!({"atom_hash": filled_hash.to_multibase()}),
        )
        .await,
    );
    assert_eq!(r["retracted"], filled_hash.to_multibase());
    let restored = head(&h, &sara, "person.generic");
    assert_eq!(restored.claim, original.claim);
    assert_eq!(restored.supersedes.as_ref(), Some(&filled_hash));
    assert!(restored.valid_to.is_none());
    let retraction = restored
        .provenance
        .iter()
        .find(|p| p.kind == SourceKind::Retraction)
        .expect("retraction provenance");
    assert_eq!(retraction.hash, filled_hash);
    h.materializer
        .handle_commit(&sara, &PredicateName::new("person.generic"))
        .await
        .unwrap();
    let file = read(&h, "people/by-name/S/Sara_Chen.md");
    assert!(!file.contains("Executive"), "file reverted:\n{file}");

    // A first atom retracted closes it (valid_to) rather than restoring.
    let event = only_entity(&h, "event.business");
    let event_head = head(&h, &event, "event.business");
    assert!(event_head.supersedes.is_none());
    let event_hash = event_head.content_hash().unwrap();
    unwrap_ok(
        call(
            &h,
            "ingest.retract",
            serde_json::json!({"atom_hash": event_hash.to_multibase()}),
        )
        .await,
    );
    let closed = head(&h, &event, "event.business");
    assert!(closed.valid_to.is_some());
    assert_eq!(closed.supersedes.as_ref(), Some(&event_hash));
    assert!(
        closed
            .provenance
            .iter()
            .any(|p| p.kind == SourceKind::Retraction)
    );
}

// ---- the reconciliation band ----

#[tokio::test]
async fn ambiguous_stays_pending_under_unlimited_grant_and_resolved_entity_files_with_alias() {
    let h = setup();
    grant_accept(&h, FILING_PREDICATES, serde_json::Value::Null).await;
    let first = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    assert_eq!(first.status, SubmissionStatus::AutoAccepted);
    let sara = only_entity(&h, "person.generic");

    // Same name, a disagreeing organization: the resolver's doubt.
    let second = submit_and_settle(
        &h,
        "file:///ingest/b.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "City Council"}),
            "person-1",
        )],
    )
    .await;
    assert_eq!(second.status, SubmissionStatus::Extracted, "{second:?}");
    assert_eq!(
        second.proposals[0].resolution,
        Some(ffs_core::quarantine::Resolution::Ambiguous)
    );
    assert!(pending_ids(&h).await.contains(&second.id));

    // The owner picks the candidate: the atom lands on her entity, the
    // mention joins her aliases, and nothing says "different".
    let r = unwrap_ok(
        call(
            &h,
            "ingest.accept",
            serde_json::json!({"submission_id": second.id, "choices": {}, "resolved_entity": sara.as_str()}),
        )
        .await,
    );
    assert_eq!(r["accepted_atom_hashes"].as_array().unwrap().len(), 1);
    assert_eq!(entities_of(&h, "person.generic").len(), 1);
    let sara_head = head(&h, &sara, "person.generic");
    assert_eq!(sara_head.claim["display_name"], "Sara Chen");
    assert!(
        h.store
            .list_by_predicate(
                &PredicateName::new(ffs_core::DIFFERENT_FROM_PREDICATE),
                None,
                10
            )
            .unwrap()
            .is_empty(),
        "picking a candidate writes no different_from"
    );

    // "Someone new" mints; "these are different people" is explicit.
    let third = submit_and_settle(
        &h,
        "file:///ingest/c.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Riverside Bank"}),
            "person-1",
        )],
    )
    .await;
    assert_eq!(third.status, SubmissionStatus::Extracted);
    let r = unwrap_ok(
        call(
            &h,
            "ingest.accept",
            serde_json::json!({"submission_id": third.id, "choices": {}, "resolved_entity": "new"}),
        )
        .await,
    );
    let minted = h
        .store
        .get(&serde_json::from_value(r["accepted_atom_hashes"][0].clone()).unwrap())
        .unwrap()
        .unwrap()
        .entity;
    assert_ne!(minted, sara);
    let r = unwrap_ok(
        call(
            &h,
            "entity.assert_different",
            serde_json::json!({"a": minted.as_str(), "b": sara.as_str(), "criterion": "different employers"}),
        )
        .await,
    );
    let d = h
        .store
        .get(&Multihash::from_multibase(r["atom_hash"].as_str().unwrap()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(d.predicate.as_str(), ffs_core::DIFFERENT_FROM_PREDICATE);
    assert_eq!(d.entity, minted);
    assert_eq!(d.claim["other"], sara.as_str());
    assert_eq!(d.claim["criterion"], "different employers");
    assert!(d.verify().is_ok(), "owner-signed");
    assert!(
        h.store
            .different_from(&minted, None)
            .unwrap()
            .contains(&sara)
    );
}

// ---- merge and unmerge ----

#[tokio::test]
async fn merge_renders_stub_and_folds_atoms_and_unmerge_restores_both() {
    let h = setup();
    grant_accept(&h, FILING_PREDICATES, serde_json::json!(50)).await;
    let a = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    let b = submit_and_settle(
        &h,
        "file:///ingest/b.md",
        hire_set("https://example.test/b", "Maria Lopez", "Beta Corp", "CFO"),
    )
    .await;
    assert_eq!(a.status, SubmissionStatus::AutoAccepted);
    assert_eq!(b.status, SubmissionStatus::AutoAccepted);
    materialize(&h, &a.auto_accepted_atom_hashes).await;
    materialize(&h, &b.auto_accepted_atom_hashes).await;
    let people = entities_of(&h, "person.generic");
    assert_eq!(people.len(), 2);
    let sara = head_named(&h, "Sara Chen");
    let maria = head_named(&h, "Maria Lopez");

    let r = unwrap_ok(
        call(
            &h,
            "entity.merge",
            serde_json::json!({"source": maria.as_str(), "target": sara.as_str(), "reason": "same person, maiden name"}),
        )
        .await,
    );
    let same_as = r["same_as_hash"].as_str().unwrap().to_string();
    assert_eq!(r["target"], sara.as_str());
    // The losing atoms stay in place.
    assert_eq!(
        h.store
            .list_by_entity(&maria, Some(&PredicateName::new("person.generic")), None)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(h.store.follow_same_as(&maria, None).unwrap(), sara);

    let pg = PredicateName::new("person.generic");
    h.materializer.handle_commit(&maria, &pg).await.unwrap();
    h.materializer.handle_commit(&sara, &pg).await.unwrap();
    let loser = read(&h, "people/by-name/M/Maria_Lopez.md");
    assert!(loser.contains("Merged into [[Sara_Chen"), "{loser}");
    let winner = read(&h, "people/by-name/S/Sara_Chen.md");
    assert!(
        winner.contains("merged_from: [\"[[Maria_Lopez|Maria Lopez]]\"]"),
        "winner names the loser:\n{winner}"
    );
    assert!(
        winner.contains("[[Beta_Corp|Beta Corp]]: CFO"),
        "winner folds the loser's affiliations:\n{winner}"
    );

    // Merges show up in the auto-filed list for the daily summary.
    let listed = unwrap_ok(call(&h, "ingest.list_auto_filed", serde_json::Value::Null).await);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "merge" && i["hash"] == same_as.as_str())
    );

    // A later mention of the losing name resolves to the winner while
    // the merge is active (the classifier routes the differing display
    // name to review; the owner's accept lands on the winner).
    let c = submit_and_settle(
        &h,
        "file:///ingest/c.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Maria Lopez", "organization": "Beta Corp"}),
            "person-1",
        )],
    )
    .await;
    assert_ne!(c.status, SubmissionStatus::AutoAccepted, "{c:?}");
    let r = unwrap_ok(
        call(
            &h,
            "ingest.accept",
            serde_json::json!({"submission_id": c.id, "choices": {}}),
        )
        .await,
    );
    let landed = h
        .store
        .get(&serde_json::from_value(r["accepted_atom_hashes"][0].clone()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        landed.entity, sara,
        "resolved through same_as to the winner"
    );
    assert_eq!(entities_of(&h, "person.generic").len(), 2, "nothing minted");
    let aliases = head(&h, &sara, "person.generic").claim["aliases"].clone();
    assert!(
        aliases
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "Maria Lopez"),
        "{aliases}"
    );

    // Unmerge: both files come back; the same mention now resolves to
    // Maria's own entity.
    let r = unwrap_ok(
        call(
            &h,
            "entity.unmerge",
            serde_json::json!({"same_as_hash": same_as}),
        )
        .await,
    );
    assert_eq!(r["unmerged"], same_as.as_str());
    assert_eq!(h.store.follow_same_as(&maria, None).unwrap(), maria);
    let err = unwrap_err(
        call(
            &h,
            "entity.unmerge",
            serde_json::json!({"same_as_hash": same_as}),
        )
        .await,
    );
    assert!(err.message.contains("already undone"), "{}", err.message);
    h.materializer.handle_commit(&maria, &pg).await.unwrap();
    h.materializer.handle_commit(&sara, &pg).await.unwrap();
    let loser = read(&h, "people/by-name/M/Maria_Lopez.md");
    assert!(!loser.contains("Merged into"), "{loser}");
    assert!(loser.contains("Beta_Corp"), "{loser}");
    let winner = read(&h, "people/by-name/S/Sara_Chen.md");
    assert!(!winner.contains("merged_from"), "{winner}");
    assert!(!winner.contains("Beta_Corp"), "{winner}");
    let listed = unwrap_ok(call(&h, "ingest.list_auto_filed", serde_json::Value::Null).await);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "unmerge")
    );

    let d = submit_and_settle(
        &h,
        "file:///ingest/d.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Maria Lopez", "organization": "Beta Corp", "role": "CFO"}),
            "person-1",
        )],
    )
    .await;
    let landed_on: EntityId = match d.status {
        SubmissionStatus::AutoAccepted => {
            h.store
                .get(&d.auto_accepted_atom_hashes[0])
                .unwrap()
                .unwrap()
                .entity
        }
        _ => {
            let r = unwrap_ok(
                call(
                    &h,
                    "ingest.accept",
                    serde_json::json!({"submission_id": d.id, "choices": {}}),
                )
                .await,
            );
            h.store
                .get(&serde_json::from_value(r["accepted_atom_hashes"][0].clone()).unwrap())
                .unwrap()
                .unwrap()
                .entity
        }
    };
    assert_eq!(landed_on, maria, "after unmerge the mention is hers again");
}

fn head_named(h: &Harness, name: &str) -> EntityId {
    let pg = PredicateName::new("person.generic");
    entities_of(h, "person.generic")
        .into_iter()
        .find(|e| {
            h.store
                .head_of_chain(e, &pg, None)
                .unwrap()
                .is_some_and(|a| a.claim["display_name"] == name)
        })
        .unwrap_or_else(|| panic!("no person named {name}"))
}

// ---- capability admin ----

#[tokio::test]
async fn revoke_turns_auto_filing_off_and_grant_refuses_accept_without_a_cap() {
    let h = setup();
    let err = unwrap_err(
        call(
            &h,
            "capability.grant",
            serde_json::json!({"action": "accept", "grantee": COURIER, "predicates": ["org.company"], "classifications": []}),
        )
        .await,
    );
    assert!(err.message.contains("max_per_day"), "{}", err.message);
    let err = unwrap_err(
        call(
            &h,
            "capability.grant",
            serde_json::json!({"action": "shred", "grantee": COURIER, "predicates": [], "classifications": []}),
        )
        .await,
    );
    assert!(err.message.contains("unknown action"), "{}", err.message);

    let grant = grant_accept(&h, "org.company", serde_json::json!(50)).await;
    let sub = submit_and_settle(
        &h,
        "file:///ingest/a.md",
        vec![proposal(
            "org.company",
            serde_json::json!({"display_name": "Beta Corp"}),
            "org-1",
        )],
    )
    .await;
    assert_eq!(sub.status, SubmissionStatus::AutoAccepted);

    let r = unwrap_ok(
        call(
            &h,
            "capability.revoke",
            serde_json::json!({"grant_hash": grant}),
        )
        .await,
    );
    assert_eq!(r["revoked"], grant.as_str());
    let list = unwrap_ok(call(&h, "capability.list", serde_json::Value::Null).await);
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["grant_hash"] == grant.as_str()),
        "{list}"
    );
    let sub = submit_and_settle(
        &h,
        "file:///ingest/b.md",
        vec![proposal(
            "org.company",
            serde_json::json!({"display_name": "Gamma LLC"}),
            "org-1",
        )],
    )
    .await;
    assert_eq!(
        sub.status,
        SubmissionStatus::Extracted,
        "next drop is pending only"
    );
}
