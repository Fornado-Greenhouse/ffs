// Python skill subprocesses over stdio; Unix-only like the other skill e2es.
#![cfg(unix)]

//! task_41 end to end, in process: the real auditor and scribe bundles
//! run under a `SkillsHost` whose substrate access is the daemon's
//! `DispatcherProxy`, against an in-memory dispatcher seeded with a
//! week of business atoms. `audit.run {op: briefing}` publishes one
//! `auditor.briefing` atom whose derivations name the right people, the
//! materializer files it at `briefings/<date>.md` with wikilinks, and
//! "Promote to contact" from the briefing lands in the quarantine, not
//! the store. Skips without `python3`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::{FamilyTable, ProjectionRenderer};
use ffs_core::quarantine::{InMemoryQuarantine, IngestQuarantine, SubmissionStatus};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, Multihash, PathIndex, PredicateName,
    PublicKey, SuppressionRegistry, Tier,
};
use ffs_daemon::api::{ApiPayload, ApiRequest};
use ffs_daemon::notify::EventPublisher;
use ffs_daemon::{
    Dispatcher, DispatcherProxy, ResolvingExtractor, SkillsHostInvoker, SkillsHostScribeExtractor,
    WorkingSetMaterializer,
};
use ffs_skills_host::{SkillRegistry, SkillsHost};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn days_ago(days: i64) -> Iso8601 {
    let t = time::OffsetDateTime::now_utc() - time::Duration::days(days);
    let s = t
        .replace_nanosecond(0)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    Iso8601::new(&s).unwrap()
}

struct Harness {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    dispatcher: Arc<Dispatcher>,
    store: Arc<dyn AtomStore>,
    registry: Arc<SpecRegistry>,
    renderer: Arc<ProjectionRenderer>,
    quarantine: Arc<InMemoryQuarantine>,
    skills: Arc<SkillsHost>,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    // The scribe reads specs from $FFS_DATA_DIR/config/predicates.
    let pred = data_dir.join("config").join("predicates");
    std::fs::create_dir_all(&pred).unwrap();
    for e in std::fs::read_dir(repo_root().join("starter").join("predicates")).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), pred.join(e.file_name())).unwrap();
        }
    }
    let skills_dir = data_dir.join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();
    for sub in ["auditor", "scribe", "_lib"] {
        std::os::unix::fs::symlink(repo_root().join("skills").join(sub), skills_dir.join(sub))
            .unwrap();
    }

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&pred).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
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

    let proxy = Arc::new(DispatcherProxy::new());
    let mut host = SkillsHost::new(proxy.clone());
    host.set_child_env(vec![(
        "FFS_DATA_DIR".to_string(),
        data_dir.to_string_lossy().into_owned(),
    )]);
    let mut skill_registry = SkillRegistry::new();
    skill_registry.discover(&skills_dir).unwrap();
    host.spawn_from_registry(&skill_registry);
    let skills = Arc::new(host);
    assert!(skills.get("auditor").is_some(), "auditor bundle discovered");
    assert!(skills.get("scribe").is_some(), "scribe bundle discovered");

    let inner: Arc<dyn ffs_daemon::dispatch::ScribeExtractor> =
        Arc::new(SkillsHostScribeExtractor::new(skills.clone()));
    let scribe: Arc<dyn ffs_daemon::dispatch::ScribeExtractor> = Arc::new(ResolvingExtractor::new(
        inner,
        store.clone(),
        registry.clone(),
        Some(&data_dir),
    ));

    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry: registry.clone(),
        renderer: renderer.clone(),
        notifier: Arc::new(EventPublisher::new()),
        owner: owner_pk(),
        quarantine: quarantine.clone(),
        scribe: Some(scribe),
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: Some(Arc::new(owner_key())),
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
        data_dir: Some(data_dir.clone()),
        skill_invoker: Some(Arc::new(SkillsHostInvoker::new(skills.clone()))),
        ingest_agent_identity: None,
        suppression: Some(Arc::new(SuppressionRegistry::new())),
    });
    assert!(proxy.install(dispatcher.clone()));
    Harness {
        _dir: dir,
        data_dir,
        dispatcher,
        store,
        registry,
        renderer,
        quarantine,
        skills,
    }
}

async fn call(h: &Harness, method: &str, params: Value) -> Value {
    let resp = h
        .dispatcher
        .handle(ApiRequest {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: method.into(),
            params,
        })
        .await;
    match resp.payload {
        ApiPayload::Success { result } => result,
        ApiPayload::Error { error } => panic!("{method} failed: {error:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert(
    h: &Harness,
    entity: &EntityId,
    predicate: &str,
    claim: Value,
    tx: Iso8601,
    valid_to: Option<Iso8601>,
    supersedes: Option<Multihash>,
) -> Multihash {
    let atom = AtomTemplate {
        v: 1,
        entity: entity.clone(),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: tx.clone(),
        valid_to,
        tx_time: tx,
        classification: Tier::new("existence"),
        supersedes,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    h.store.insert(&atom).unwrap()
}

async fn materialize(
    h: &Harness,
    materializer: &WorkingSetMaterializer,
    folder: &str,
    e: &EntityId,
) {
    let family = FamilyTable::from_registry(&h.registry)
        .for_folder(folder)
        .unwrap();
    materializer
        .materialize_entity(&family, e)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{folder}/{} did not materialize", e.as_str()));
}

#[tokio::test]
async fn briefing_after_two_articles_yields_atom_file_candidate_and_follow_up() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let h = setup();
    let materializer = WorkingSetMaterializer::new(
        h.renderer.clone(),
        h.store.clone(),
        Arc::new(InMemoryWorkingSet::new()),
        Arc::new(SuppressionRegistry::new()),
        h.data_dir.clone(),
        owner_pk(),
    );

    // An existing contact affiliated with Acme, from well before the window.
    let acme = EntityId::mint();
    let sam = EntityId::mint();
    insert(
        &h,
        &acme,
        "org.company",
        json!({"display_name": "Acme Widgets", "aliases": ["Acme"]}),
        days_ago(40),
        None,
        None,
    );
    insert(
        &h,
        &sam,
        "contact.person",
        json!({"display_name": "Sam Contact", "organization": acme.as_str()}),
        days_ago(40),
        None,
        None,
    );
    insert(
        &h,
        &EntityId::mint(),
        "affiliation",
        json!({"person": sam.as_str(), "organization": acme.as_str(), "title": "CFO", "kind": "executive"}),
        days_ago(40),
        None,
        None,
    );
    // A person who left Acme this week: a root affiliation from long
    // ago, superseded in the window by one that sets valid_to.
    let lee = EntityId::mint();
    insert(
        &h,
        &lee,
        "person.generic",
        json!({"display_name": "Lee Leaver"}),
        days_ago(60),
        None,
        None,
    );
    let lee_role = EntityId::mint();
    let root = insert(
        &h,
        &lee_role,
        "affiliation",
        json!({"person": lee.as_str(), "organization": acme.as_str(), "title": "COO", "kind": "executive"}),
        days_ago(60),
        None,
        None,
    );
    insert(
        &h,
        &lee_role,
        "affiliation",
        json!({"person": lee.as_str(), "organization": acme.as_str(), "title": "COO", "kind": "executive"}),
        days_ago(1),
        Some(days_ago(2)),
        Some(root),
    );
    // A new person this week, joining Acme, mentioned in two articles.
    let pat = EntityId::mint();
    insert(
        &h,
        &pat,
        "person.generic",
        json!({"display_name": "Pat Example", "organization": acme.as_str(), "role": "CEO"}),
        days_ago(2),
        None,
        None,
    );
    let article1 = EntityId::mint();
    let article2 = EntityId::mint();
    insert(
        &h,
        &article1,
        "source.article",
        json!({"title": "Widget maker names new chief", "url": "https://example.com/1",
               "published_at": days_ago(2).as_str(),
               "mentions": [{"entity": pat.as_str(), "display": "Pat Example"},
                            {"entity": acme.as_str(), "display": "Acme Widgets"}]}),
        days_ago(2),
        None,
        None,
    );
    insert(
        &h,
        &article2,
        "source.article",
        json!({"title": "Acme breaks ground on plant", "url": "https://example.com/2",
               "published_at": days_ago(1).as_str(),
               "mentions": [{"entity": pat.as_str(), "display": "Pat Example"},
                            {"entity": acme.as_str(), "display": "Acme Widgets"}]}),
        days_ago(1),
        None,
        None,
    );
    insert(
        &h,
        &EntityId::mint(),
        "affiliation",
        json!({"person": pat.as_str(), "organization": acme.as_str(), "title": "CEO", "kind": "executive",
               "source": article1.as_str()}),
        days_ago(2),
        None,
        None,
    );
    for (folder, e) in [
        ("orgs", &acme),
        ("contacts", &sam),
        ("people", &lee),
        ("people", &pat),
        ("articles", &article1),
        ("articles", &article2),
    ] {
        materialize(&h, &materializer, folder, e).await;
    }

    // Run the real auditor through the dispatcher and the skill proxy.
    let ran = call(&h, "audit.run", json!({"op": "briefing"})).await;
    assert!(ran["atom_hash"].is_string(), "briefing published: {ran}");

    let rows = call(&h, "audit.query", json!({"kind": "briefing"})).await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "one auditor.briefing atom");
    let claim = &rows[0]["claim"];

    let ids = |key: &str| -> Vec<String> {
        claim[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} missing in {claim}"))
            .iter()
            .map(|x| x["entity"].as_str().unwrap_or("").to_string())
            .collect()
    };
    assert_eq!(ids("new_people"), vec![pat.as_str().to_string()], "{claim}");
    assert_eq!(
        claim["new_people"][0]["first_seen_article"]["entity"],
        article1.as_str()
    );
    let candidates = ids("promotion_candidates");
    assert_eq!(candidates, vec![pat.as_str().to_string()], "{claim}");
    let follow_ups = ids("follow_ups");
    assert_eq!(follow_ups, vec![sam.as_str().to_string()], "{claim}");
    assert_eq!(
        claim["follow_ups"][0]["organization"]["entity"],
        acme.as_str()
    );

    // Changes from affiliation activity: Pat joined, Lee left.
    let changes: Vec<(String, String)> = claim["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["entity"].as_str().unwrap().to_string(),
                c["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        changes.contains(&(pat.as_str().to_string(), "joined".into())),
        "{claim}"
    );
    assert!(
        changes.contains(&(lee.as_str().to_string(), "left".into())),
        "{claim}"
    );
    let joined = claim["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "joined")
        .unwrap();
    assert!(joined["as_reported"].is_string(), "{joined}");
    assert_eq!(joined["source_article"]["entity"], article1.as_str());
    assert_eq!(
        claim["trending_orgs"][0]["entity"],
        acme.as_str(),
        "{claim}"
    );

    // The page: filed by date, every name a [[basename|display]] link.
    let briefing_entity = EntityId::new(rows[0]["entity"].as_str().unwrap());
    materialize(&h, &materializer, "briefings", &briefing_entity).await;
    let date = claim["date"].as_str().unwrap();
    let page = h.data_dir.join("briefings").join(format!("{date}.md"));
    let text = std::fs::read_to_string(&page).unwrap();
    assert!(text.contains("[[Pat_Example|Pat Example]]"), "{text}");
    assert!(text.contains("[[Acme_Widgets|Acme Widgets]]"), "{text}");
    assert!(text.contains("[[Sam_Contact|Sam Contact]]"), "{text}");
    assert!(text.contains("[[Lee_Leaver|Lee Leaver]]"), "{text}");
    assert!(text.contains("as reported"), "{text}");

    // Promote to contact, as the plugin does: a quarantined proposal
    // built from the person.generic head, cited to the briefing atom.
    let briefing_hash = rows[0]["hash"].as_str().unwrap();
    let before = h
        .store
        .list_by_predicate(&PredicateName::new("contact.person"), None, 100)
        .unwrap()
        .len();
    let note = "---\npredicate: contact.person\nname: Pat Example\norganization: Acme Widgets\nrole: CEO\n---\n\nPromoted from the briefing.\n";
    let submitted = call(
        &h,
        "ingest.submit",
        json!({"source_uri": format!("ffs://briefing/{briefing_hash}/promote/{}", pat.as_str()),
               "content": note}),
    )
    .await;
    let sub_id = submitted["submission_id"].as_str().unwrap().to_string();
    let start = Instant::now();
    let mut sub = None;
    while start.elapsed() < Duration::from_secs(20) {
        let s = h.quarantine.get(&sub_id).await.unwrap();
        if s.status == SubmissionStatus::Extracted {
            sub = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let sub = sub.expect("scribe extracted the promotion note");
    assert!(
        sub.source_uri.starts_with("ffs://briefing/"),
        "{}",
        sub.source_uri
    );
    assert!(
        sub.proposals
            .iter()
            .any(|p| p.predicate.as_str() == "contact.person"
                && p.claim["display_name"] == "Pat Example"),
        "{:?}",
        sub.proposals
    );
    let after = h
        .store
        .list_by_predicate(&PredicateName::new("contact.person"), None, 100)
        .unwrap()
        .len();
    assert_eq!(before, after, "promotion is a proposal, not an atom");

    h.skills.shutdown_all();
}
