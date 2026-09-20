//! task_39 inbox e2e (ADR-032 as accepted 2026-09-20): a submission
//! renders into `inbox/<date>.md`; a tick parsed back through the fast
//! path's inbox parser and applied through a sink backed by the
//! in-process dispatcher commits the atom and the re-rendered file shows
//! the section under Decided; a candidate tick accepts with
//! `resolved_entity`; "accept all under this article" over an untouched
//! ambiguous child is refused with a parse warning.
//!
//! Lives in `ffs-daemon` since task_49 (ADR-036): the daemon depends on
//! the fast path and owns the `DispatcherSink`.

use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::{
    Candidate, InMemoryQuarantine, IngestQuarantine, Proposal, Resolution, SubmissionStatus,
};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, Multihash, PathIndex, PredicateName,
    Provenance, PublicKey, SourceKind, SuppressionRegistry, Tier,
};
use ffs_daemon::Dispatcher;
use ffs_daemon::DispatcherSink;
use ffs_daemon::inbox::{InboxMaterializer, today_utc};
use ffs_daemon::notify::EventPublisher;
use ffs_fastpath::inbox::{DecisionAction, apply_decisions, parse_inbox};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[57u8; 32])
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
    quarantine: Arc<InMemoryQuarantine>,
    inbox: Arc<InboxMaterializer>,
    publisher: Arc<EventPublisher>,
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
        .with_path_index(index),
    );
    let quarantine = Arc::new(InMemoryQuarantine::new());
    let publisher = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier: publisher.clone(),
        owner: owner_pk(),
        quarantine: quarantine.clone(),
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
        suppression: None,
    });
    let inbox = Arc::new(InboxMaterializer::new(
        quarantine.clone(),
        Arc::new(SuppressionRegistry::new()),
        data_dir.clone(),
    ));
    Harness {
        _dir: dir,
        data_dir,
        dispatcher,
        store,
        quarantine,
        inbox,
        publisher,
    }
}

fn prov() -> Vec<Provenance> {
    vec![Provenance {
        kind: SourceKind::IngestFile,
        uri: "file:///ingest/a.md".into(),
        hash: Multihash::blake3_of(b"a"),
    }]
}

fn proposal(predicate: &str, claim: serde_json::Value, lref: &str) -> Proposal {
    let mut p = Proposal::new(PredicateName::new(predicate), claim, prov(), "test");
    p.local_ref = Some(lref.into());
    p.resolution = Some(Resolution::New);
    p
}

fn tick(text: &str, needle: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?}"));
    assert!(line.contains("- [ ]"), "line is a checkbox: {line}");
    text.replacen(line, &line.replacen("- [ ]", "- [x]", 1), 1)
}

#[tokio::test]
async fn ticking_accept_commits_the_atom_and_the_section_moves_to_decided() {
    let h = setup();
    let id = h
        .quarantine
        .submit(
            "file:///ingest/example-2026-09-21-widget.md".into(),
            b"x".to_vec(),
        )
        .await
        .unwrap();
    h.quarantine
        .complete(
            &id,
            vec![
                proposal(
                    "org.company",
                    serde_json::json!({"display_name": "Acme Widgets", "industry": "manufacturing"}),
                    "org-1",
                ),
                proposal("note", serde_json::json!({"title": "Plain note"}), "note-1"),
            ],
        )
        .await
        .unwrap();

    let path = h.inbox.refresh().await.unwrap().expect("inbox written");
    assert_eq!(
        path,
        h.data_dir.join("inbox").join(format!("{}.md", today_utc()))
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(&format!("<!-- source sub:{id} -->")));
    assert!(text.contains(&format!("- [ ] accept <!-- sub:{id} ref:org-1 -->")));

    // The owner ticks accept on the org block and saves.
    let edited = tick(&text, &format!("accept <!-- sub:{id} ref:org-1 -->"));
    let parsed = parse_inbox(&edited);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.decisions.len(), 1);
    assert!(matches!(
        parsed.decisions[0].action,
        DecisionAction::Accept {
            resolved_entity: None
        }
    ));

    let sink = DispatcherSink::new(
        h.dispatcher.clone(),
        h.publisher.clone(),
        Some(h.inbox.clone()),
    );
    let (applied, errors) = apply_decisions(&sink, &parsed).await;
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(applied, 1);

    let sub = h.quarantine.get(&id).await.unwrap();
    assert_eq!(sub.status, SubmissionStatus::Accepted);
    assert!(!sub.accepted_atom_hashes.is_empty());
    let atom = h.store.get(&sub.accepted_atom_hashes[0]).unwrap().unwrap();
    assert!(
        atom.predicate.as_str() == "org.company" || atom.predicate.as_str() == "note",
        "an atom from the submission was committed"
    );

    // The sink re-rendered the file: the source now sits under Decided.
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        !after.contains(&format!("<!-- source sub:{id} -->")),
        "no longer a pending section"
    );
    assert!(after.contains(&format!("<!-- decided sub:{id} -->")));
    assert!(after.contains("accepted ("));
}

#[tokio::test]
async fn ticking_a_candidate_accepts_with_resolved_entity() {
    let h = setup();
    // An existing person the candidate points at.
    let existing = EntityId::mint();
    let tmpl = AtomTemplate {
        v: 1,
        entity: existing.clone(),
        predicate: PredicateName::new("person.generic"),
        claim: serde_json::json!({"display_name": "Pat Example", "role": "CEO"}),
        valid_from: ts("2026-09-01T00:00:00Z"),
        valid_to: None,
        tx_time: ts("2026-09-01T00:00:00Z"),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: prov(),
    };
    h.store.insert(&tmpl.sign(&owner_key()).unwrap()).unwrap();

    let id = h
        .quarantine
        .submit("file:///ingest/b.md".into(), b"y".to_vec())
        .await
        .unwrap();
    let mut person = proposal(
        "person.generic",
        serde_json::json!({"display_name": "Pat Example", "role": "CEO", "aliases": ["P. Example"]}),
        "person-1",
    );
    person.resolution = Some(Resolution::Ambiguous);
    person.candidates = vec![
        Candidate {
            entity: existing.clone(),
            score: 7.2,
            matched_on: vec!["display_name".into()],
            display: "Pat Example (CEO)".into(),
        },
        Candidate {
            entity: EntityId::new("zOther"),
            score: 6.8,
            matched_on: vec!["display_name".into()],
            display: "Pat Example (other)".into(),
        },
    ];
    h.quarantine.complete(&id, vec![person]).await.unwrap();

    let path = h.inbox.refresh().await.unwrap().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let needle = format!("entity:{} -->", existing.as_str());
    let edited = tick(&text, &needle);
    let parsed = parse_inbox(&edited);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.decisions.len(), 1);
    assert_eq!(
        parsed.decisions[0].action,
        DecisionAction::Accept {
            resolved_entity: Some(existing.as_str().to_string())
        }
    );

    let sink = DispatcherSink::new(
        h.dispatcher.clone(),
        h.publisher.clone(),
        Some(h.inbox.clone()),
    );
    let (applied, errors) = apply_decisions(&sink, &parsed).await;
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(applied, 1);
    let sub = h.quarantine.get(&id).await.unwrap();
    assert_eq!(sub.status, SubmissionStatus::Accepted);
    let atom = h.store.get(&sub.accepted_atom_hashes[0]).unwrap().unwrap();
    assert_eq!(atom.entity, existing, "the accept bound the owner's pick");
}

#[tokio::test]
async fn accept_all_over_an_untouched_ambiguous_child_is_refused_with_a_warning() {
    let h = setup();
    let id = h
        .quarantine
        .submit("file:///ingest/c.md".into(), b"z".to_vec())
        .await
        .unwrap();
    let mut person = proposal(
        "person.generic",
        serde_json::json!({"display_name": "Q"}),
        "p",
    );
    person.resolution = Some(Resolution::Ambiguous);
    person.candidates = vec![Candidate {
        entity: EntityId::new("zQ1"),
        score: 5.0,
        matched_on: vec!["display_name".into()],
        display: "Q (one)".into(),
    }];
    h.quarantine
        .complete(
            &id,
            vec![
                proposal("note", serde_json::json!({"title": "N"}), "n"),
                person,
            ],
        )
        .await
        .unwrap();
    let path = h.inbox.refresh().await.unwrap().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let edited = tick(
        &text,
        &format!("accept all under this article <!-- sub:{id} all -->"),
    );
    let parsed = parse_inbox(&edited);
    assert!(parsed.decisions.is_empty(), "{:?}", parsed.decisions);
    assert_eq!(parsed.warnings.len(), 1);
    assert!(parsed.warnings[0].message.contains("refused"));
    assert_eq!(parsed.warnings[0].section, id);

    // The warning renders under the section on the next refresh.
    h.inbox.set_warnings(parsed.warnings.clone());
    h.inbox.refresh().await.unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("- parse warning: accept all under this article refused"));
    assert_eq!(
        h.quarantine.get(&id).await.unwrap().status,
        SubmissionStatus::Extracted,
        "nothing was accepted"
    );
}
