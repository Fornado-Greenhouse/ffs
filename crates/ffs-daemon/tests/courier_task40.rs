//! task_40 daemon slice: `entity.search` v2 (schema-declared fields,
//! FTS tier, four-tier ordering, `same_as` following, `different_from`
//! exclusion, `path` on hits), the courier RPCs and `health.summary.courier`,
//! and article dedup by normalized URL and `content_hash` through the
//! resolver and the accept path (ADR-030, ADR-031, ADR-035).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::{InMemoryQuarantine, IngestQuarantine, SubmissionStatus};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomEnvelope, AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, Multihash, PathIndex,
    PredicateName, Proposal, PublicKey, Tier,
};
use ffs_daemon::api::{ApiPayload, ApiRequest, ApiResponse};
use ffs_daemon::dispatch::{ScribeExtractError, ScribeExtractor, SkillInvoker};
use ffs_daemon::notify::EventPublisher;
use ffs_daemon::{Dispatcher, ResolvingExtractor};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[40u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

/// Canned scribe output, swapped per submission by the test.
struct StubScribe {
    next: Mutex<Vec<Proposal>>,
}

#[async_trait]
impl ScribeExtractor for StubScribe {
    async fn extract(
        &self,
        _uri: &str,
        _content: &[u8],
    ) -> Result<Vec<Proposal>, ScribeExtractError> {
        Ok(self.next.lock().unwrap().clone())
    }
}

struct StubInvoker {
    calls: Mutex<Vec<(String, Value)>>,
    reply: Value,
}

#[async_trait]
impl SkillInvoker for StubInvoker {
    async fn invoke(&self, skill: &str, input: Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push((skill.to_string(), input));
        if skill == "courier" {
            Ok(self.reply.clone())
        } else {
            Err(format!("skill `{skill}` is not installed"))
        }
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    dispatcher: Arc<Dispatcher>,
    store: Arc<dyn AtomStore>,
    quarantine: Arc<InMemoryQuarantine>,
    index: Arc<dyn PathIndex>,
    scribe: Arc<StubScribe>,
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
    let quarantine = Arc::new(InMemoryQuarantine::new());
    let scribe = Arc::new(StubScribe {
        next: Mutex::new(Vec::new()),
    });
    let inner: Arc<dyn ScribeExtractor> = scribe.clone();
    let resolving: Arc<dyn ScribeExtractor> = Arc::new(ResolvingExtractor::new(
        inner,
        store.clone(),
        registry.clone(),
        None,
    ));
    let invoker = Arc::new(StubInvoker {
        calls: Mutex::new(Vec::new()),
        reply: json!({
            "would_submit": ["ingest/2026-09-21-example-ledger-a.md", "ingest/2026-09-21-example-ledger-b.md"],
            "items_seen": 2, "files_written": 0, "fetch_failures": 0, "last_error": null, "dry_run": true
        }),
    });
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier: Arc::new(EventPublisher::new()),
        owner: owner_pk(),
        quarantine: quarantine.clone(),
        scribe: Some(resolving),
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(owner_key())),
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: Some(data_dir.clone()),
        skill_invoker: Some(invoker.clone()),
    });
    Harness {
        _dir: dir,
        data_dir,
        dispatcher,
        store,
        quarantine,
        index,
        scribe,
        invoker,
    }
}

fn req(method: &str, params: Value) -> ApiRequest {
    ApiRequest {
        jsonrpc: "2.0".into(),
        id: json!(1),
        method: method.into(),
        params,
    }
}

fn unwrap_ok(resp: ApiResponse) -> Value {
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("expected success; got error: {error:?}"),
    }
}

/// Parse a multibase hash string the way the wire does.
fn mh(s: &str) -> Multihash {
    serde_json::from_value(json!(s)).expect("multihash string")
}

fn atom(entity: &EntityId, predicate: &str, claim: Value, tx: &str) -> AtomEnvelope {
    AtomTemplate {
        v: 1,
        entity: entity.clone(),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: ts("2026-05-31T00:00:00Z"),
        valid_to: None,
        tx_time: ts(tx),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap()
}

async fn search(h: &Harness, params: Value) -> Vec<Value> {
    let r = unwrap_ok(h.dispatcher.handle(req("entity.search", params)).await);
    r["results"].as_array().cloned().unwrap()
}

// ---- entity.search v2 ----

#[tokio::test]
async fn entity_search_matches_alias_tags_and_url_fields_from_the_schema() {
    let h = setup();
    let acme = EntityId::mint();
    h.store
        .insert(&atom(
            &acme,
            "org.company",
            json!({"display_name": "Acme Widgets", "aliases": ["Acme Corp"], "tags": ["manufacturing"]}),
            "2026-06-01T00:00:00Z",
        ))
        .unwrap();
    let art = EntityId::mint();
    h.store
        .insert(&atom(
            &art,
            "source.article",
            json!({"title": "Plant opens", "url": "https://example.com/news/plant-opens"}),
            "2026-06-01T00:00:01Z",
        ))
        .unwrap();
    h.index.assign("orgs", &acme, "Acme Widgets", &[]).unwrap();

    let hits = search(&h, json!({"query": "acme corp"})).await;
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0]["entity"], acme.as_str());
    assert_eq!(hits[0]["matched_on"][0], "alias");
    assert_eq!(hits[0]["path"], "orgs/by-name/A/Acme_Widgets.md");
    assert_eq!(hits[0]["basename"], "Acme_Widgets");
    assert!(hits[0]["score"].as_f64().unwrap() >= 80.0);
    assert!(hits[0]["tx_time"].is_string());

    let hits = search(&h, json!({"query": "manufacturing"})).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["matched_on"][0], "other");

    let hits = search(&h, json!({"query": "plant-opens"})).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["entity"], art.as_str());
    assert_eq!(hits[0]["matched_on"][0], "other");
}

#[tokio::test]
async fn entity_search_filters_by_predicate_and_family() {
    let h = setup();
    let org = EntityId::mint();
    let person = EntityId::mint();
    h.store
        .insert(&atom(
            &org,
            "org.company",
            json!({"display_name": "Harbor Lane"}),
            "2026-06-01T00:00:00Z",
        ))
        .unwrap();
    h.store
        .insert(&atom(
            &person,
            "person.generic",
            json!({"display_name": "Harbor Lane"}),
            "2026-06-01T00:00:01Z",
        ))
        .unwrap();
    assert_eq!(search(&h, json!({"query": "harbor lane"})).await.len(), 2);
    let only_org = search(
        &h,
        json!({"query": "harbor lane", "predicate": "org.company"}),
    )
    .await;
    assert_eq!(only_org.len(), 1);
    assert_eq!(only_org[0]["entity"], org.as_str());
    let only_people = search(&h, json!({"query": "harbor lane", "family": "people"})).await;
    assert_eq!(only_people.len(), 1);
    assert_eq!(only_people[0]["entity"], person.as_str());
}

#[tokio::test]
async fn entity_search_orders_exact_name_then_alias_then_fts_and_priors_within_a_tier() {
    let h = setup();
    let exact = EntityId::mint();
    let alias_low = EntityId::mint();
    let alias_high = EntityId::mint();
    let body_only = EntityId::mint();
    h.store
        .insert(&atom(&body_only, "org.company", json!({"display_name": "Zed Holdings", "extra_prose": "Formerly traded as Northgate in the Charlotte market."}), "2026-06-01T00:00:00Z"))
        .unwrap();
    h.store
        .insert(&atom(
            &alias_low,
            "org.company",
            json!({"display_name": "NG Partners", "aliases": ["Northgate"]}),
            "2026-06-01T00:00:01Z",
        ))
        .unwrap();
    h.store
        .insert(&atom(
            &alias_high,
            "org.company",
            json!({"display_name": "North Gate Realty", "aliases": ["Northgate"]}),
            "2026-06-01T00:00:02Z",
        ))
        .unwrap();
    h.store
        .insert(&atom(
            &exact,
            "org.company",
            json!({"display_name": "Northgate"}),
            "2026-06-01T00:00:03Z",
        ))
        .unwrap();
    // Two accepted resolutions of "northgate" to alias_high, none to alias_low.
    h.store.record_resolution("northgate", &alias_high).unwrap();
    h.store.record_resolution("northgate", &alias_high).unwrap();

    let hits = search(&h, json!({"query": "Northgate"})).await;
    let ids: Vec<&str> = hits.iter().map(|x| x["entity"].as_str().unwrap()).collect();
    assert_eq!(
        ids[0],
        exact.as_str(),
        "exact canonical name first: {hits:?}"
    );
    assert_eq!(hits[0]["matched_on"][0], "display_name");
    assert_eq!(
        ids[1],
        alias_high.as_str(),
        "prior count orders within the alias tier"
    );
    assert_eq!(ids[2], alias_low.as_str());
    assert_eq!(hits[1]["matched_on"][0], "alias");
    assert_eq!(ids[3], body_only.as_str(), "full-text hit last");
    assert_eq!(hits[3]["matched_on"][0], "fts");
    assert!(hits[1]["score"].as_f64().unwrap() > hits[2]["score"].as_f64().unwrap());
    assert!(hits[2]["score"].as_f64().unwrap() > hits[3]["score"].as_f64().unwrap());
}

#[tokio::test]
async fn entity_search_follows_same_as_chain_to_winner_and_excludes_different_from() {
    let h = setup();
    let loser = EntityId::mint();
    let middle = EntityId::mint();
    let winner = EntityId::mint();
    for (e, name, tx) in [
        (&loser, "Riverside Mill Co", "2026-06-01T00:00:00Z"),
        (&middle, "Riverside Mill Company", "2026-06-01T00:00:01Z"),
        (&winner, "Riverside Mill", "2026-06-01T00:00:02Z"),
    ] {
        h.store
            .insert(&atom(e, "org.company", json!({"display_name": name}), tx))
            .unwrap();
    }
    h.index
        .assign("orgs", &winner, "Riverside Mill", &[])
        .unwrap();
    h.store
        .insert(&atom(
            &loser,
            "entity.same_as",
            json!({"target": middle.as_str()}),
            "2026-06-02T00:00:00Z",
        ))
        .unwrap();
    h.store
        .insert(&atom(
            &middle,
            "entity.same_as",
            json!({"target": winner.as_str()}),
            "2026-06-02T00:00:01Z",
        ))
        .unwrap();

    let hits = search(&h, json!({"query": "Riverside Mill Co"})).await;
    assert_eq!(hits.len(), 1, "losers fold into the winner: {hits:?}");
    assert_eq!(hits[0]["entity"], winner.as_str());
    assert_eq!(hits[0]["display_name"], "Riverside Mill");
    assert_eq!(hits[0]["path"], "orgs/by-name/R/Riverside_Mill.md");

    // The owner separated a same-name org from a context entity.
    let other = EntityId::mint();
    h.store
        .insert(&atom(
            &other,
            "org.company",
            json!({"display_name": "Riverside Mill Bakery"}),
            "2026-06-03T00:00:00Z",
        ))
        .unwrap();
    let ctx = EntityId::mint();
    h.store
        .insert(&atom(
            &ctx,
            "entity.different_from",
            json!({"other": other.as_str()}),
            "2026-06-03T00:00:01Z",
        ))
        .unwrap();
    let all = search(&h, json!({"query": "riverside mill"})).await;
    assert_eq!(all.len(), 2);
    let filtered = search(
        &h,
        json!({"query": "riverside mill", "context_entity": ctx.as_str()}),
    )
    .await;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["entity"], winner.as_str());
}

// ---- courier RPCs and health ----

#[tokio::test]
async fn courier_status_is_null_until_the_skill_writes_last_run_json() {
    let h = setup();
    let status = unwrap_ok(
        h.dispatcher
            .handle(req("courier.status", Value::Null))
            .await,
    );
    assert!(status.is_null());
    let health = unwrap_ok(
        h.dispatcher
            .handle(req("health.summary", Value::Null))
            .await,
    );
    assert!(
        health.get("courier").is_some_and(Value::is_null),
        "{health}"
    );

    let dir = h.data_dir.join("ingest").join(".courier");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("last_run.json"),
        r#"{"last_run":"2026-09-21T06:10:00Z","items_seen":14,"files_written":9,"fetch_failures":1,"last_error":null}"#,
    )
    .unwrap();
    let status = unwrap_ok(
        h.dispatcher
            .handle(req("courier.status", Value::Null))
            .await,
    );
    assert_eq!(status["last_run"], "2026-09-21T06:10:00Z");
    assert_eq!(status["items_seen"], 14);
    assert_eq!(status["fetch_failures"], 1);
    let health = unwrap_ok(
        h.dispatcher
            .handle(req("health.summary", Value::Null))
            .await,
    );
    assert_eq!(health["courier"]["files_written"], 9);
}

#[tokio::test]
async fn courier_run_invokes_the_courier_skill_and_returns_its_result_verbatim() {
    let h = setup();
    let out = unwrap_ok(
        h.dispatcher
            .handle(req("courier.run", json!({"dry_run": true})))
            .await,
    );
    assert_eq!(out["would_submit"].as_array().unwrap().len(), 2);
    assert_eq!(out["items_seen"], 2);
    let calls = h.invoker.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "courier");
    assert_eq!(calls[0].1["dry_run"], true);
    assert_eq!(calls[0].1["op"], "tick");
}

#[tokio::test]
async fn courier_run_errors_when_no_skills_host_is_wired() {
    let mut h = setup();
    let d = Arc::get_mut(&mut h.dispatcher).unwrap();
    d.skill_invoker = None;
    let resp = h.dispatcher.handle(req("courier.run", json!({}))).await;
    match resp.payload {
        ApiPayload::Error { error } => assert!(error.message.contains("no skills host")),
        ApiPayload::Success { result } => panic!("expected error; got {result}"),
    }
}

// ---- article dedup through submit, resolve, accept ----

fn article_proposal(title: &str, url: &str, content_hash: Option<&str>) -> Proposal {
    let mut claim = json!({"title": title, "url": url, "publication": "Example Ledger", "published_at": "2026-09-20"});
    if let Some(h) = content_hash {
        claim["content_hash"] = json!(h);
    }
    let mut p = Proposal::new(
        PredicateName::new("source.article"),
        claim,
        vec![ffs_core::Provenance {
            kind: ffs_core::SourceKind::IngestFile,
            uri: url.to_string(),
            hash: Multihash::blake3_of(url.as_bytes()),
        }],
        "courier pointer",
    );
    p.local_ref = Some("article".into());
    p
}

async fn submit_and_accept(h: &Harness, uri: &str, proposals: Vec<Proposal>) -> Vec<String> {
    *h.scribe.next.lock().unwrap() = proposals;
    let sub = unwrap_ok(
        h.dispatcher
            .handle(req(
                "ingest.submit",
                json!({"source_uri": uri, "content": "x"}),
            ))
            .await,
    );
    let id = sub["submission_id"].as_str().unwrap().to_string();
    // The extraction is spawned; wait for the quarantine to flip.
    for _ in 0..200 {
        let s = h.quarantine.get(&id).await;
        if s.as_ref()
            .is_some_and(|s| s.status == SubmissionStatus::Extracted)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let acc = unwrap_ok(
        h.dispatcher
            .handle(req("ingest.accept", json!({"submission_id": id})))
            .await,
    );
    acc["accepted_atom_hashes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn same_url_submitted_twice_yields_one_article_entity_with_two_provenance_entries() {
    let h = setup();
    let first = submit_and_accept(
        &h,
        "file:///ingest/a.md",
        vec![article_proposal(
            "Mill to reopen",
            "https://Example.com/news/mill/?utm_source=mail",
            None,
        )],
    )
    .await;
    let second = submit_and_accept(
        &h,
        "file:///ingest/b.md",
        vec![article_proposal(
            "Mill to reopen as maker space",
            "https://example.com/news/mill#top",
            None,
        )],
    )
    .await;
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    let a = h.store.get(&mh(&first[0])).unwrap().unwrap();
    let b = h.store.get(&mh(&second[0])).unwrap().unwrap();
    assert_eq!(a.entity, b.entity, "one article entity");
    assert_eq!(b.supersedes.as_ref(), Some(&mh(&first[0])));
    assert_eq!(b.claim["title"], "Mill to reopen as maker space");
    assert_eq!(a.provenance.len(), 1);
    assert_eq!(b.provenance.len(), 1);
    assert_ne!(
        a.provenance[0].uri, b.provenance[0].uri,
        "both provenance entries survive on the chain"
    );
    let heads = h.store.list_by_entity(&a.entity, None, None).unwrap();
    assert_eq!(heads.len(), 2);
}

#[tokio::test]
async fn same_content_hash_different_url_yields_one_article_entity() {
    let h = setup();
    let mut bytes = vec![0x1e, 0x20];
    bytes.extend([5u8; 32]);
    let hash = ffs_core::multibase::encode_base58btc(&bytes);
    let first = submit_and_accept(
        &h,
        "file:///ingest/a.md",
        vec![article_proposal(
            "Syndicated",
            "https://origin.example.com/story",
            Some(&hash),
        )],
    )
    .await;
    let second = submit_and_accept(
        &h,
        "file:///ingest/b.md",
        vec![article_proposal(
            "Syndicated",
            "https://mirror.example.net/copy",
            Some(&hash),
        )],
    )
    .await;
    let a = h.store.get(&mh(&first[0])).unwrap().unwrap();
    let b = h.store.get(&mh(&second[0])).unwrap().unwrap();
    assert_eq!(a.entity, b.entity);
    assert!(b.supersedes.is_some());
}
