//! task_45 integration: the daemon-side resolver end to end through
//! `ResolvingExtractor`, the accept path (choices, cross-reference
//! rewriting, role endings, alias growth, priors, opaque ids), and
//! the identity scoring over the corpus fixtures (pairwise precision
//! and recall plus B-cubed), per ADR-030 and ADR-031.
//!
//! Runs against the real starter specs, templates, and
//! `resolution.toml` so the thresholds under test are the shipped ones.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::multibase::decode_base58btc;
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::{CrossRef, InMemoryQuarantine, IngestQuarantine};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, PathIndex, PredicateName, Proposal,
    PublicKey, SuppressionRegistry, Tier,
};
use ffs_daemon::api::{ApiPayload, ApiRequest, ApiResponse};
use ffs_daemon::dispatch::{ScribeExtractError, ScribeExtractor};
use ffs_daemon::materializer::WorkingSetMaterializer;
use ffs_daemon::notify::EventPublisher;
use ffs_daemon::{Dispatcher, ResolvingExtractor};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[45u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

/// A scribe stub that returns whatever wire set it was given.
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
    registry: Arc<SpecRegistry>,
    quarantine: Arc<InMemoryQuarantine>,
    _resolver: Arc<ResolvingExtractor>,
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
    let scribe: Arc<dyn ScribeExtractor> = resolver.clone();
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
        store,
        registry,
        quarantine,
        _resolver: resolver,
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
/// event, all cross-referenced by local_ref.
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

async fn submit_and_wait(h: &Harness, uri: &str, set: Vec<Proposal>) -> serde_json::Value {
    h.canned.sets.lock().unwrap().push(set);
    let r = unwrap_ok(
        h.dispatcher
            .handle(req(
                "ingest.submit",
                serde_json::json!({"source_uri": uri, "content": "# synthetic\n"}),
            ))
            .await,
    );
    let id = r["submission_id"].as_str().unwrap().to_string();
    for _ in 0..200 {
        if let Some(s) = h.quarantine.get(&id).await
            && s.status == ffs_core::quarantine::SubmissionStatus::Extracted
        {
            return serde_json::to_value(s).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("submission {id} never reached Extracted");
}

async fn accept(h: &Harness, id: &str, choices: serde_json::Value) -> ApiResponse {
    h.dispatcher
        .handle(req(
            "ingest.accept",
            serde_json::json!({"submission_id": id, "choices": choices}),
        ))
        .await
}

fn head(h: &Harness, entity: &EntityId, predicate: &str) -> ffs_core::AtomEnvelope {
    h.store
        .head_of_chain(entity, &PredicateName::new(predicate), None)
        .unwrap()
        .unwrap_or_else(|| panic!("no head for {} {predicate}", entity.as_str()))
}

fn is_opaque(id: &EntityId) -> bool {
    decode_base58btc(id.as_str()).is_ok_and(|b| b.len() == 16)
}

// ---- multi-entity end to end ----

#[tokio::test]
async fn first_article_mints_everything_rewrites_refs_and_materializes_wikilinks() {
    let h = setup();
    let sub = submit_and_wait(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    let props = sub["proposals"].as_array().unwrap();
    assert_eq!(props.len(), 5);
    for p in props {
        assert_eq!(
            p["resolution"], "new",
            "fresh substrate: {}",
            p["predicate"]
        );
    }
    // list_pending exposes resolution on every proposal.
    let pending = unwrap_ok(
        h.dispatcher
            .handle(req("ingest.list_pending", serde_json::json!({})))
            .await,
    );
    assert!(
        pending[0]["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["resolution"] == "new")
    );

    let id = sub["id"].as_str().unwrap();
    let r = unwrap_ok(accept(&h, id, serde_json::json!({})).await);
    let hashes = r["accepted_atom_hashes"].as_array().unwrap();
    assert_eq!(hashes.len(), 5);

    // Every id is opaque; refs are rewritten to ids.
    let mut ids: HashMap<String, EntityId> = HashMap::new();
    for hsh in hashes {
        let atom = h
            .store
            .get(&serde_json::from_value(hsh.clone()).unwrap())
            .unwrap()
            .unwrap();
        assert!(is_opaque(&atom.entity), "{}", atom.entity.as_str());
        ids.insert(atom.predicate.as_str().to_string(), atom.entity.clone());
        h.materializer
            .handle_commit(&atom.entity, &atom.predicate)
            .await
            .unwrap();
    }
    // Second pass: reverse-lookup sections (an org's People) link by
    // basename, and the person's basename only exists once the person
    // has materialized; in production the org re-renders on the next
    // commit event touching it. Re-materialize so the org file is current.
    for hsh in hashes {
        let atom = h
            .store
            .get(&serde_json::from_value(hsh.clone()).unwrap())
            .unwrap()
            .unwrap();
        h.materializer
            .handle_commit(&atom.entity, &atom.predicate)
            .await
            .unwrap();
    }
    let person = head(&h, &ids["person.generic"], "person.generic");
    assert_eq!(person.claim["organization"], ids["org.company"].as_str());
    let article = head(&h, &ids["source.article"], "source.article");
    assert_eq!(
        article.claim["mentions"][0]["entity"],
        ids["person.generic"].as_str()
    );
    assert_eq!(
        article.claim["mentions"][1]["entity"],
        ids["org.company"].as_str()
    );
    let aff = head(&h, &ids["affiliation"], "affiliation");
    assert_eq!(aff.claim["person"], ids["person.generic"].as_str());
    assert_eq!(aff.claim["organization"], ids["org.company"].as_str());
    assert_eq!(
        aff.valid_from.as_str(),
        "2026-09-10T00:00:00Z",
        "affiliation dated from the article's published_at"
    );
    let event = head(&h, &ids["event.business"], "event.business");
    assert_eq!(
        event.claim["participants"][0]["entity"],
        ids["person.generic"].as_str()
    );

    // Files land under people/, orgs/, articles/, events/ with wikilinks.
    let person_file =
        std::fs::read_to_string(h.data_dir.join("people/by-name/S/Sara_Chen.md")).unwrap();
    assert!(
        person_file.contains("[[Acme_Widgets|Acme Widgets]]"),
        "{person_file}"
    );
    assert!(person_file.contains("## Affiliations"), "{person_file}");
    let org_file =
        std::fs::read_to_string(h.data_dir.join("orgs/by-name/A/Acme_Widgets.md")).unwrap();
    assert!(org_file.contains("[[Sara_Chen|Sara Chen]]"), "{org_file}");
    assert!(h.data_dir.join("articles").exists() && h.data_dir.join("events").exists());
}

#[tokio::test]
async fn second_article_resolves_to_existing_grows_aliases_and_records_a_prior() {
    let h = setup();
    let first = submit_and_wait(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    unwrap_ok(accept(&h, first["id"].as_str().unwrap(), serde_json::json!({})).await);
    let sara = h
        .store
        .list_by_predicate(&PredicateName::new("person.generic"), None, 10)
        .unwrap()[0]
        .entity
        .clone();

    // "S. Chen" at Acme: surname block + organization agreement.
    let second = submit_and_wait(
        &h,
        "file:///ingest/b.md",
        hire_set("https://example.test/b", "S. Chen", "Acme Widgets", "CEO"),
    )
    .await;
    let props = second["proposals"].as_array().unwrap();
    let person = props
        .iter()
        .find(|p| p["predicate"] == "person.generic")
        .unwrap();
    let org = props
        .iter()
        .find(|p| p["predicate"] == "org.company")
        .unwrap();
    assert_eq!(org["resolution"], "existing");
    assert_eq!(person["resolution"], "existing", "{person}");
    assert_eq!(person["entity"], sara.as_str());
    assert_eq!(
        props
            .iter()
            .find(|p| p["predicate"] == "source.article")
            .unwrap()["resolution"],
        "new"
    );

    unwrap_ok(accept(&h, second["id"].as_str().unwrap(), serde_json::json!({})).await);
    let sara_head = head(&h, &sara, "person.generic");
    assert_eq!(
        sara_head.claim["display_name"], "Sara Chen",
        "head display kept"
    );
    let aliases: Vec<String> = sara_head.claim["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(aliases.contains(&"S. Chen".to_string()), "{aliases:?}");
    assert!(
        sara_head.supersedes.is_some(),
        "existing binds supersede the head"
    );
    let priors = h.store.prior_counts("s chen").unwrap();
    assert_eq!(priors, vec![(sara.clone(), 1)]);
    // Only one person entity exists.
    let people: HashSet<String> = h
        .store
        .list_by_predicate(&PredicateName::new("person.generic"), None, 100)
        .unwrap()
        .into_iter()
        .map(|a| a.entity.as_str().to_string())
        .collect();
    assert_eq!(people.len(), 1);
}

#[tokio::test]
async fn ambiguous_needs_a_choice_and_the_choice_binds_or_mints() {
    let h = setup();
    // Two Sara Chens at different organizations. The second is an
    // exact name match with a disagreeing organization: the resolver
    // must not auto-link it (ambiguous), and the owner says "new".
    let s = submit_and_wait(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    unwrap_ok(accept(&h, s["id"].as_str().unwrap(), serde_json::json!({})).await);
    let s = submit_and_wait(
        &h,
        "file:///ingest/b.md",
        hire_set("https://example.test/b", "Sara Chen", "City Council", "CEO"),
    )
    .await;
    let second_person = s["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["predicate"] == "person.generic")
        .unwrap();
    assert_eq!(second_person["resolution"], "ambiguous", "{second_person}");
    unwrap_ok(
        accept(
            &h,
            s["id"].as_str().unwrap(),
            serde_json::json!({"person-1": "new"}),
        )
        .await,
    );
    let people: Vec<EntityId> = h
        .store
        .list_by_predicate(&PredicateName::new("person.generic"), None, 100)
        .unwrap()
        .into_iter()
        .map(|a| a.entity)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(people.len(), 2, "same name, different orgs stay separate");

    // A bare "Sara Chen" with a third org: both candidates tie on name, so ambiguous.
    let third = submit_and_wait(
        &h,
        "file:///ingest/c.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Riverside Bank"}),
            "person-1",
        )],
    )
    .await;
    let p = &third["proposals"][0];
    assert_eq!(p["resolution"], "ambiguous", "{p}");
    assert_eq!(p["candidates"].as_array().unwrap().len(), 2);

    let id = third["id"].as_str().unwrap();
    let resp = accept(&h, id, serde_json::json!({})).await;
    match resp.payload {
        ApiPayload::Error { error } => {
            assert!(error.message.contains("person-1"), "{}", error.message);
            assert_eq!(error.data.unwrap()["ambiguous"][0], "person-1");
        }
        ApiPayload::Success { .. } => panic!("ambiguous accept without choices must fail"),
    }
    // Choose "new": mints a third entity.
    unwrap_ok(accept(&h, id, serde_json::json!({"person-1": "new"})).await);
    let count = h
        .store
        .list_by_predicate(&PredicateName::new("person.generic"), None, 100)
        .unwrap()
        .into_iter()
        .map(|a| a.entity.as_str().to_string())
        .collect::<HashSet<_>>()
        .len();
    assert_eq!(count, 3);

    // Choose a candidate: binds to it and supersedes its head.
    let fourth = submit_and_wait(
        &h,
        "file:///ingest/d.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Somewhere Else"}),
            "person-1",
        )],
    )
    .await;
    assert_eq!(fourth["proposals"][0]["resolution"], "ambiguous");
    let pick = people[0].clone();
    unwrap_ok(
        accept(
            &h,
            fourth["id"].as_str().unwrap(),
            serde_json::json!({"person-1": pick.as_str()}),
        )
        .await,
    );
    let bound = head(&h, &pick, "person.generic");
    assert!(bound.supersedes.is_some());
    assert_eq!(
        h.store
            .prior_counts("sara chen")
            .unwrap()
            .iter()
            .find(|(e, _)| e == &pick)
            .map(|(_, c)| *c),
        Some(1)
    );
}

#[tokio::test]
async fn ends_role_supersedes_the_existing_affiliation_with_valid_to() {
    let h = setup();
    let first = submit_and_wait(
        &h,
        "file:///ingest/a.md",
        hire_set("https://example.test/a", "Sara Chen", "Acme Widgets", "CEO"),
    )
    .await;
    unwrap_ok(accept(&h, first["id"].as_str().unwrap(), serde_json::json!({})).await);
    let aff_before = h
        .store
        .list_by_predicate(&PredicateName::new("affiliation"), None, 10)
        .unwrap();
    assert_eq!(aff_before.len(), 1);

    // "Sara Chen steps down": an ends_role affiliation proposal.
    let mut ending = with_refs(
        proposal(
            "affiliation",
            serde_json::json!({"person": "Sara Chen", "organization": "Acme Widgets"}),
            "aff-end",
        ),
        &[("person", "person-1"), ("organization", "org-1")],
    );
    ending.ends_role = true;
    ending.valid_to = Some(ts("2026-09-30T00:00:00Z"));
    let set = vec![
        proposal(
            "org.company",
            serde_json::json!({"display_name": "Acme Widgets"}),
            "org-1",
        ),
        with_refs(
            proposal(
                "person.generic",
                serde_json::json!({"display_name": "Sara Chen", "organization": "Acme Widgets"}),
                "person-1",
            ),
            &[("organization", "org-1")],
        ),
        ending,
    ];
    let second = submit_and_wait(&h, "file:///ingest/b.md", set).await;
    let props = second["proposals"].as_array().unwrap();
    let e = props.iter().find(|p| p["ends_role"] == true).unwrap();
    assert_eq!(e["resolution"], "existing");
    unwrap_ok(accept(&h, second["id"].as_str().unwrap(), serde_json::json!({})).await);

    let affs = h
        .store
        .list_by_predicate(&PredicateName::new("affiliation"), None, 10)
        .unwrap();
    let entities: HashSet<String> = affs.iter().map(|a| a.entity.as_str().to_string()).collect();
    assert_eq!(
        entities.len(),
        1,
        "no new affiliation entity; the ending superseded the head"
    );
    let head_aff = head(&h, &aff_before[0].entity, "affiliation");
    assert_eq!(
        head_aff.valid_to.as_ref().map(|t| t.as_str()),
        Some("2026-09-30T00:00:00Z")
    );
    assert!(head_aff.supersedes.is_some());
}

// ---- identity fixtures from the corpus ----

#[derive(serde::Deserialize)]
struct ModelEnvelope {
    proposals: Vec<ModelProposal>,
}

#[derive(serde::Deserialize)]
struct ModelProposal {
    predicate: String,
    #[serde(default)]
    local_ref: Option<String>,
    #[serde(default)]
    rationale: String,
    claim: serde_json::Value,
    #[serde(default)]
    ends_role: bool,
    #[serde(default)]
    valid_to: Option<String>,
    #[serde(default)]
    valid_from: Option<String>,
}

#[derive(serde::Deserialize)]
struct ExpectedIdentity {
    clusters: BTreeMap<String, Vec<ExpectedMention>>,
    #[serde(default)]
    distinct: Vec<Vec<String>>,
    #[serde(default)]
    aliases_expected: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    wrong_merge: Option<serde_json::Value>,
}

#[derive(serde::Deserialize, Clone)]
struct ExpectedMention {
    article: String,
    predicate: String,
    display: String,
}

/// Bind display cross-references the way the Python scribe does:
/// a display string in `organization`, `person`, `mentions[].display`,
/// `participants[].display`, `target`, `other` that equals another
/// proposal's name field (case and whitespace insensitive) becomes a
/// ref to that proposal.
fn bind_refs(set: &mut [Proposal]) {
    let key = |s: &str| ffs_core::resolve::normalized_name_key(s);
    let names: Vec<(String, String)> = set
        .iter()
        .filter_map(|p| {
            let name = p
                .claim
                .get("display_name")
                .or_else(|| p.claim.get("title"))
                .and_then(|v| v.as_str())?;
            Some((key(name), p.local_ref.clone()?))
        })
        .collect();
    let lookup = |display: &str, own: &Option<String>| -> Option<String> {
        let k = key(display);
        names
            .iter()
            .find(|(n, r)| *n == k && Some(r) != own.as_ref())
            .map(|(_, r)| r.clone())
    };
    for p in set.iter_mut() {
        let own = p.local_ref.clone();
        let mut refs = Vec::new();
        for field in ["organization", "person", "target", "other"] {
            if let Some(d) = p.claim.get(field).and_then(|v| v.as_str())
                && let Some(r) = lookup(d, &own)
            {
                refs.push(CrossRef {
                    field: field.into(),
                    local_ref: r,
                });
            }
        }
        for arr in ["mentions", "participants"] {
            if let Some(items) = p.claim.get(arr).and_then(|v| v.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    if let Some(d) = item.get("display").and_then(|v| v.as_str())
                        && let Some(r) = lookup(d, &own)
                    {
                        refs.push(CrossRef {
                            field: format!("{arr}/{i}/entity"),
                            local_ref: r,
                        });
                    }
                }
            }
        }
        p.refs = refs;
    }
}

fn load_fixture_articles(dir: &std::path::Path) -> Vec<(String, Vec<Proposal>)> {
    // input.md <-> model_output.json, input-b.md <-> model_output-b.json, ...
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.starts_with("model_output") && n.ends_with(".json"))
        .collect();
    // `model_output.json` is the first article; `-b`, `-c` follow.
    // A plain sort would put `-b` before `.json`.
    names.sort_by_key(|n| {
        let suffix = n
            .trim_start_matches("model_output")
            .trim_end_matches(".json")
            .to_string();
        (!suffix.is_empty(), suffix)
    });
    names
        .into_iter()
        .map(|n| {
            let suffix = n
                .trim_start_matches("model_output")
                .trim_end_matches(".json")
                .to_string();
            let article = format!("input{suffix}.md");
            let env: ModelEnvelope =
                serde_json::from_str(&std::fs::read_to_string(dir.join(&n)).unwrap()).unwrap();
            let mut set: Vec<Proposal> = env
                .proposals
                .into_iter()
                .enumerate()
                .map(|(i, mp)| {
                    let mut p = Proposal::new(
                        PredicateName::new(&mp.predicate),
                        mp.claim,
                        vec![],
                        mp.rationale,
                    );
                    p.local_ref = Some(mp.local_ref.unwrap_or_else(|| format!("p{i}")));
                    p.ends_role = mp.ends_role;
                    p.valid_to = mp
                        .valid_to
                        .as_deref()
                        .and_then(ffs_daemon::scribe::parse_scribe_date);
                    p.valid_from = mp
                        .valid_from
                        .as_deref()
                        .and_then(ffs_daemon::scribe::parse_scribe_date);
                    p
                })
                .collect();
            bind_refs(&mut set);
            (article, set)
        })
        .collect()
}

/// Pairwise precision/recall over same-entity pairs and B-cubed over
/// clusters, comparing predicted clusters (entity ids) to expected
/// labels (cluster names) for the mentions listed in the fixture.
fn identity_scores(
    predicted: &BTreeMap<(String, String, String), String>, // (article, predicate, display) -> entity id
    expected: &ExpectedIdentity,
) -> (f64, f64, f64, f64) {
    let mut gold: BTreeMap<(String, String, String), String> = BTreeMap::new();
    for (label, mentions) in &expected.clusters {
        for m in mentions {
            gold.insert(
                (m.article.clone(), m.predicate.clone(), m.display.clone()),
                label.clone(),
            );
        }
    }
    let keys: Vec<_> = gold.keys().cloned().collect();
    let (mut tp, mut fp, mut fn_) = (0usize, 0usize, 0usize);
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            let same_gold = gold[&keys[i]] == gold[&keys[j]];
            let same_pred = match (predicted.get(&keys[i]), predicted.get(&keys[j])) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            match (same_gold, same_pred) {
                (true, true) => tp += 1,
                (false, true) => fp += 1,
                (true, false) => fn_ += 1,
                _ => {}
            }
        }
    }
    let pp = if tp + fp == 0 {
        1.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let pr = if tp + fn_ == 0 {
        1.0
    } else {
        tp as f64 / (tp + fn_) as f64
    };
    // B-cubed: per mention, |pred cluster ∩ gold cluster| / |pred| and / |gold|.
    let (mut bp, mut br) = (0.0, 0.0);
    for k in &keys {
        let g = &gold[k];
        let p = predicted.get(k);
        let gold_members: Vec<_> = keys.iter().filter(|x| &gold[*x] == g).collect();
        let pred_members: Vec<_> = keys
            .iter()
            .filter(|x| predicted.get(*x).is_some() && predicted.get(*x) == p)
            .collect();
        let inter = gold_members
            .iter()
            .filter(|x| pred_members.contains(x))
            .count() as f64;
        bp += if pred_members.is_empty() {
            0.0
        } else {
            inter / pred_members.len() as f64
        };
        br += inter / gold_members.len() as f64;
    }
    let n = keys.len().max(1) as f64;
    (pp, pr, bp / n, br / n)
}

async fn run_identity_fixture(
    name: &str,
) -> Option<(
    Harness,
    ExpectedIdentity,
    BTreeMap<(String, String, String), String>,
)> {
    let dir = repo_root().join("skills/scribe/tests/corpus").join(name);
    let expected_path = dir.join("expected_identity.json");
    if !expected_path.exists() {
        eprintln!("identity fixture {name} absent; skipping");
        return None;
    }
    let expected: ExpectedIdentity =
        serde_json::from_str(&std::fs::read_to_string(expected_path).unwrap()).unwrap();
    let h = setup();
    let mut predicted: BTreeMap<(String, String, String), String> = BTreeMap::new();
    let mut reviewed = 0usize;
    for (article, set) in load_fixture_articles(&dir) {
        let displays: Vec<(String, String)> = set
            .iter()
            .filter_map(|p| {
                Some((
                    p.predicate.as_str().to_string(),
                    p.claim
                        .get("display_name")
                        .or_else(|| p.claim.get("title"))
                        .and_then(|v| v.as_str())?
                        .to_string(),
                ))
            })
            .collect();
        let sub = submit_and_wait(&h, &format!("file:///ingest/{name}/{article}"), set).await;
        // An ambiguous outcome is a review decision. Simulate the owner
        // deciding correctly from the gold labels: pick the candidate
        // that is the gold cluster's entity when one is already
        // predicted, else "new". An `existing` outcome is the resolver's
        // own call and is scored as such; a wrong auto-link is the real
        // failure this harness catches.
        let mut choices = serde_json::Map::new();
        for p in sub["proposals"].as_array().unwrap() {
            if p["resolution"] != "ambiguous" {
                continue;
            }
            let pred = p["predicate"].as_str().unwrap().to_string();
            let display = p["claim"]["display_name"]
                .as_str()
                .or_else(|| p["claim"]["title"].as_str())
                .unwrap_or("")
                .to_string();
            let label = expected
                .clusters
                .iter()
                .find(|(_, ms)| {
                    ms.iter().any(|m| {
                        m.article == article
                            && m.predicate == pred
                            && ffs_core::resolve::normalized_name_key(&m.display)
                                == ffs_core::resolve::normalized_name_key(&display)
                    })
                })
                .map(|(l, _)| l.clone());
            let gold_entity = label.as_ref().and_then(|l| {
                expected.clusters[l].iter().find_map(|m| {
                    predicted
                        .get(&(m.article.clone(), m.predicate.clone(), m.display.clone()))
                        .cloned()
                })
            });
            let pick = match gold_entity {
                Some(e)
                    if p["candidates"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["entity"] == e) =>
                {
                    e
                }
                _ => "new".to_string(),
            };
            reviewed += 1;
            choices.insert(
                p["local_ref"].as_str().unwrap().to_string(),
                serde_json::Value::String(pick),
            );
        }
        let r = unwrap_ok(
            accept(
                &h,
                sub["id"].as_str().unwrap(),
                serde_json::Value::Object(choices),
            )
            .await,
        );
        for hsh in r["accepted_atom_hashes"].as_array().unwrap() {
            let atom = h
                .store
                .get(&serde_json::from_value(hsh.clone()).unwrap())
                .unwrap()
                .unwrap();
            let winner = h.store.follow_same_as(&atom.entity, None).unwrap();
            let name_field = h
                .registry
                .family_for_predicate(atom.predicate.as_str())
                .map(|f| f.name_field);
            if let Some(nf) = name_field
                && let Some(d) = atom.claim.get(&nf).and_then(|v| v.as_str())
            {
                // The accepted atom's name may be the head's (merged) name;
                // record under the display the article used.
                let used = displays
                    .iter()
                    .find(|(pred, disp)| {
                        pred == atom.predicate.as_str()
                            && (ffs_core::resolve::normalized_name_key(disp)
                                == ffs_core::resolve::normalized_name_key(d)
                                || atom
                                    .claim
                                    .get("aliases")
                                    .and_then(|a| a.as_array())
                                    .is_some_and(|a| {
                                        a.iter().any(|v| {
                                            v.as_str().is_some_and(|s| {
                                                ffs_core::resolve::normalized_name_key(s)
                                                    == ffs_core::resolve::normalized_name_key(disp)
                                            })
                                        })
                                    }))
                    })
                    .map(|(_, disp)| disp.clone())
                    .unwrap_or_else(|| d.to_string());
                predicted.insert(
                    (article.clone(), atom.predicate.as_str().to_string(), used),
                    winner.as_str().to_string(),
                );
            }
        }
    }
    eprintln!(
        "identity fixture {name}: {reviewed} ambiguous proposal(s) went to (simulated) review"
    );
    Some((h, expected, predicted))
}

#[tokio::test]
async fn identity_nickname_across_three_articles_is_one_entity() {
    let Some((h, expected, predicted)) = run_identity_fixture("31-nickname-three-articles").await
    else {
        return;
    };
    let (pp, pr, bp, br) = identity_scores(&predicted, &expected);
    eprintln!("31 pairwise P/R {pp:.2}/{pr:.2} B3 P/R {bp:.2}/{br:.2}");
    assert_eq!((pp, pr, bp, br), (1.0, 1.0, 1.0, 1.0), "{predicted:?}");
    for (label, aliases) in &expected.aliases_expected {
        let member = &expected.clusters[label][0];
        let entity = EntityId::new(
            &predicted[&(
                member.article.clone(),
                member.predicate.clone(),
                member.display.clone(),
            )],
        );
        let head = head(&h, &entity, &member.predicate);
        let got: Vec<String> = head.claim["aliases"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for a in aliases {
            assert!(
                got.iter().any(|g| ffs_core::resolve::normalized_name_key(g)
                    == ffs_core::resolve::normalized_name_key(a)),
                "alias {a:?} missing from {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn identity_same_name_two_orgs_stay_separate() {
    let Some((_h, expected, predicted)) = run_identity_fixture("32-same-name-two-orgs").await
    else {
        return;
    };
    let (pp, pr, bp, br) = identity_scores(&predicted, &expected);
    eprintln!("32 pairwise P/R {pp:.2}/{pr:.2} B3 P/R {bp:.2}/{br:.2}");
    assert_eq!((pp, pr, bp, br), (1.0, 1.0, 1.0, 1.0), "{predicted:?}");
    for pair in &expected.distinct {
        let ids: Vec<Option<&String>> = pair
            .iter()
            .map(|label| {
                let m = &expected.clusters[label][0];
                predicted.get(&(m.article.clone(), m.predicate.clone(), m.display.clone()))
            })
            .collect();
        assert_ne!(ids[0], ids[1], "distinct pair {pair:?} collapsed");
    }
}

#[tokio::test]
async fn identity_rename_announced_stays_one_entity_with_alias() {
    let Some((h, expected, predicted)) = run_identity_fixture("33-rename-announced").await else {
        return;
    };
    let (pp, pr, bp, br) = identity_scores(&predicted, &expected);
    eprintln!("33 pairwise P/R {pp:.2}/{pr:.2} B3 P/R {bp:.2}/{br:.2}");
    assert_eq!((pp, pr, bp, br), (1.0, 1.0, 1.0, 1.0), "{predicted:?}");
    for (label, aliases) in &expected.aliases_expected {
        let member = &expected.clusters[label][0];
        let entity = EntityId::new(
            &predicted[&(
                member.article.clone(),
                member.predicate.clone(),
                member.display.clone(),
            )],
        );
        let head = head(&h, &entity, &member.predicate);
        let got: Vec<String> = head.claim["aliases"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for a in aliases {
            assert!(
                got.iter().any(|g| ffs_core::resolve::normalized_name_key(g)
                    == ffs_core::resolve::normalized_name_key(a)),
                "alias {a:?} missing from {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn identity_wrong_merge_undo_restores_both_clusters() {
    let Some((h, expected, predicted)) = run_identity_fixture("34-wrong-merge-undo").await else {
        return;
    };
    assert!(
        expected.wrong_merge.is_some(),
        "fixture declares the wrong merge"
    );
    // Baseline: two separate entities, as the fixture expects.
    let (pp, pr, _, _) = identity_scores(&predicted, &expected);
    assert_eq!((pp, pr), (1.0, 1.0), "{predicted:?}");
    let labels: Vec<&String> = expected.clusters.keys().collect();
    let people: Vec<EntityId> = labels
        .iter()
        .filter(|l| l.starts_with('P'))
        .map(|l| {
            let m = &expected.clusters[*l][0];
            EntityId::new(&predicted[&(m.article.clone(), m.predicate.clone(), m.display.clone())])
        })
        .collect();
    assert!(people.len() >= 2);
    let (loser, winner) = (&people[0], &people[1]);

    // Wrong merge: the owner asserts loser same_as winner.
    let merge = AtomTemplate {
        v: 1,
        entity: loser.clone(),
        predicate: PredicateName::new("entity.same_as"),
        claim: serde_json::json!({"target": winner.as_str(), "reason": "owner merge", "criterion": "manual"}),
        valid_from: ts("2026-09-20T00:00:00Z"),
        valid_to: None,
        tx_time: ts("2026-09-20T00:00:00Z"),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    let merge_hash = h.store.insert(&merge).unwrap();
    assert_eq!(
        h.store.follow_same_as(loser, None).unwrap(),
        *winner,
        "merged: loser redirects to winner"
    );
    // A new mention of the loser's name now lands on the winner.
    let loser_head = head(&h, loser, "person.generic");
    let loser_display = loser_head.claim["display_name"]
        .as_str()
        .unwrap()
        .to_string();
    let loser_org = loser_head.claim["organization"]
        .as_str()
        .map(str::to_string);
    let org_display = loser_org
        .as_ref()
        .and_then(|o| {
            h.store
                .head_of_chain(&EntityId::new(o), &PredicateName::new("org.company"), None)
                .ok()
                .flatten()
        })
        .and_then(|a| a.claim["display_name"].as_str().map(str::to_string))
        .unwrap_or_default();
    let sub = submit_and_wait(
        &h,
        "file:///ingest/merged.md",
        vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": loser_display, "organization": org_display}),
            "person-1",
        )],
    )
    .await;
    let p = &sub["proposals"][0];
    assert!(
        p["resolution"] == "existing" || p["resolution"] == "ambiguous",
        "{p}"
    );
    let landed: Vec<&str> = p["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["entity"].as_str().unwrap())
        .collect();
    assert!(
        landed.contains(&winner.as_str()) && !landed.contains(&loser.as_str()),
        "candidates fold the loser into the winner: {landed:?}"
    );

    // Undo: supersede the same_as atom with a valid_to.
    let undo = AtomTemplate {
        v: 1,
        entity: loser.clone(),
        predicate: PredicateName::new("entity.same_as"),
        claim: merge.claim.clone(),
        valid_from: merge.valid_from.clone(),
        valid_to: Some(ts("2026-09-21T00:00:00Z")),
        tx_time: ts("2026-09-21T00:00:00Z"),
        classification: Tier::new("existence"),
        supersedes: Some(merge_hash),
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    h.store.insert(&undo).unwrap();
    assert_eq!(
        h.store.follow_same_as(loser, None).unwrap(),
        *loser,
        "undone: both clusters restored"
    );
    let sub2 = submit_and_wait(&h, "file:///ingest/unmerged.md", vec![
        proposal("person.generic", serde_json::json!({"display_name": loser_head.claim["display_name"].as_str().unwrap(), "organization": org_display}), "person-1"),
    ]).await;
    let p2 = &sub2["proposals"][0];
    let cands: Vec<&str> = p2["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["entity"].as_str().unwrap())
        .collect();
    assert!(
        cands.contains(&loser.as_str()),
        "after undo the loser is a candidate again: {cands:?}"
    );
}
