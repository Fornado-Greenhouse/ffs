//! End-to-end test: daemon dispatcher + real Python scribe subprocess
//! wired through `ffs-skills-host`. The dispatcher's `ingest.submit`
//! handler accepts markdown, spawns the scribe via the
//! `ScribeExtractor` trait, and stores extracted proposals in the
//! `InMemoryQuarantine`. The test verifies:
//!
//! - the submission lands with the expected provenance (source_uri +
//!   content_hash on every proposal),
//! - the scribe is actually invoked and produces the contact.person
//!   proposal we expect from the test markdown,
//! - a caller without `Write` capability gets a structured
//!   capability-denial error from `ingest.submit`.
//!
//! Skips itself with a stderr note if `python3` is not on `PATH`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde_json::Value;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::{InMemoryQuarantine, IngestQuarantine, Proposal, SubmissionStatus};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{Iso8601, PredicateName, PublicKey};
use ffs_daemon::dispatch::{ScribeExtractError, ScribeExtractor};
use ffs_daemon::{ApiRequest, ApiResponse, Dispatcher, EventPublisher};
use ffs_skills_host::{RefuseAllProxy, SkillKind, SkillManifest, SkillProcess};

const CONTACT_PERSON_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }
email = { type = "string" }
notes = { type = "array", items = { type = "string" } }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name"]
body_sections = ["Notes"]
additive_sections = ["Notes"]

[[reverse_map]]
output = "frontmatter.display_name"
atom_field = "claim.display_name"
edit_kind = "single_line_text"
"#;

const NOTE_TOML: &str = r#"
name = "note"
version = 1

[claim_schema]
type = "object"
required = ["title"]

[claim_schema.properties]
title = { type = "string" }
body = { type = "string" }
tags = { type = "array", items = { type = "string" } }

[rendering]
template = "note.md.tera"
frontmatter_fields = ["title", "tags"]
body_sections = ["Body"]
additive_sections = []
"#;

fn python_available() -> bool {
    Command::new("python3").arg("--version").output().is_ok()
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[3u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn stranger_pk() -> PublicKey {
    let k = SigningKey::from_bytes(&[7u8; 32]);
    PublicKey::from_verifying(&k.verifying_key())
}

fn grant_write_for_owner(store: &dyn AtomStore) {
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

fn scribe_dir() -> PathBuf {
    // tests/ lives at crates/ffs-daemon/tests; the scribe at
    // skills/scribe is two levels up + into skills/scribe.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p.push("skills");
    p.push("scribe");
    p
}

/// Wraps a `SkillProcess` to satisfy the daemon's `ScribeExtractor`
/// trait. Translates between Rust types and the wire shape the
/// Python helper expects (`source_uri`, `content` strings in;
/// `proposals` list out).
struct SkillScribe {
    process: SkillProcess,
}

#[async_trait]
impl ScribeExtractor for SkillScribe {
    async fn extract(
        &self,
        source_uri: &str,
        content: &[u8],
    ) -> Result<Vec<Proposal>, ScribeExtractError> {
        let input = serde_json::json!({
            "source_uri": source_uri,
            "content": String::from_utf8_lossy(content),
        });
        let result = self
            .process
            .invoke(input)
            .await
            .map_err(|e| ScribeExtractError::Failed(e.to_string()))?;
        let raw = result
            .get("proposals")
            .ok_or_else(|| ScribeExtractError::Failed("missing proposals".into()))?;
        let proposals: Vec<ScribeProposalWire> = serde_json::from_value(raw.clone())
            .map_err(|e| ScribeExtractError::Failed(format!("decode proposals: {e}")))?;
        Ok(proposals.into_iter().map(Into::into).collect())
    }
}

#[derive(serde::Deserialize)]
struct ScribeProposalWire {
    predicate: String,
    claim: serde_json::Value,
    provenance: Vec<ScribeProvenanceWire>,
    rationale: String,
}

#[derive(serde::Deserialize)]
struct ScribeProvenanceWire {
    kind: String,
    uri: String,
    hash_hex: String,
}

impl From<ScribeProposalWire> for Proposal {
    fn from(w: ScribeProposalWire) -> Self {
        use ffs_core::{Multihash, Provenance, SourceKind};
        let provenance = w
            .provenance
            .into_iter()
            .map(|p| {
                // Recompute a Multihash from the BLAKE3 hex the scribe
                // sent. We could also recompute server-side from the
                // raw content for verification — the production
                // wiring (task 22) does that.
                let mh = match hex_to_bytes(&p.hash_hex) {
                    Some(bytes) if bytes.len() == 32 => {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        Multihash::from_blake3(&arr)
                    }
                    _ => Multihash::blake3_of(p.hash_hex.as_bytes()),
                };
                let kind = match p.kind.as_str() {
                    "ingest" => SourceKind::IngestFile,
                    "federation_pull" => SourceKind::FederationPull,
                    "fast_path" => SourceKind::FastPath,
                    _ => SourceKind::IngestFile,
                };
                Provenance {
                    kind,
                    uri: p.uri,
                    hash: mh,
                }
            })
            .collect();
        Proposal {
            predicate: PredicateName::new(w.predicate),
            claim: w.claim,
            provenance,
            rationale: w.rationale,
        }
    }
}

fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks(2) {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

struct Harness {
    _dir: tempfile::TempDir,
    _store: Arc<dyn AtomStore>,
    quarantine: Arc<InMemoryQuarantine>,
    dispatcher: Dispatcher,
}

fn setup_with_scribe(connect_scribe: bool) -> Option<Harness> {
    if connect_scribe && !python_available() {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::set_permissions(&predicates_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        predicates_dir.join("contact.person.toml"),
        CONTACT_PERSON_TOML,
    )
    .unwrap();
    std::fs::write(predicates_dir.join("note.toml"), NOTE_TOML).unwrap();
    // Minimal templates — only used by the projection renderer, which
    // this test doesn't exercise; they just need to exist for
    // `ProjectionRenderer::new` to succeed.
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        "---\ndisplay_name: {{ atom.claim.display_name }}\n---\n",
    )
    .unwrap();
    std::fs::write(
        templates_dir.join("note.md.tera"),
        "---\ntitle: {{ atom.claim.title }}\n---\n",
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant_write_for_owner(&*store);
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let quarantine = Arc::new(InMemoryQuarantine::new());

    let scribe: Option<Arc<dyn ScribeExtractor>> = if connect_scribe {
        let manifest = SkillManifest {
            name: "scribe".to_string(),
            kind: SkillKind::Scribe,
            entry_point: Path::new("extraction.py").into(),
            python: "python3".to_string(),
            timeout: Duration::from_millis(15_000),
            dir: scribe_dir(),
        };
        let proc = SkillProcess::spawn(manifest, Arc::new(RefuseAllProxy));
        Some(Arc::new(SkillScribe { process: proc }))
    } else {
        None
    };

    let dispatcher = Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: quarantine.clone(),
        scribe,
    };
    Some(Harness {
        _dir: dir,
        _store: store,
        quarantine,
        dispatcher,
    })
}

fn ingest_submit_req(source_uri: &str, content: &str) -> ApiRequest {
    ApiRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: "ingest.submit".into(),
        params: serde_json::json!({
            "source_uri": source_uri,
            "content": content,
        }),
    }
}

fn unwrap_success(resp: ApiResponse) -> Value {
    match resp.payload {
        ffs_daemon::ApiPayload::Success { result } => result,
        ffs_daemon::ApiPayload::Error { error } => {
            panic!("expected success, got error: {error:?}");
        }
    }
}

#[tokio::test]
async fn ingest_submit_returns_submission_id_and_stores_pending_entry() {
    let h = setup_with_scribe(false).expect("setup");
    let resp = h
        .dispatcher
        .handle(ingest_submit_req("file:///a.md", "hello"))
        .await;
    let result = unwrap_success(resp);
    let id = result["submission_id"].as_str().expect("submission_id");
    let sub = h.quarantine.get(id).await.expect("submission stored");
    assert_eq!(sub.source_uri, "file:///a.md");
    assert_eq!(sub.status, SubmissionStatus::Pending);
    assert_eq!(sub.proposals.len(), 0);
    assert_eq!(sub.content, b"hello");
}

#[tokio::test]
async fn ingest_submit_with_scribe_lands_contact_person_proposal_with_provenance() {
    let Some(h) = setup_with_scribe(true) else {
        eprintln!("skipping: python3 not on PATH");
        return;
    };
    let markdown =
        "---\nname: Sara\nemail: sara@example.com\n---\n\n## Notes\n- Met at conference\n";
    let resp = h
        .dispatcher
        .handle(ingest_submit_req("file:///sara.md", markdown))
        .await;
    let result = unwrap_success(resp);
    let id = result["submission_id"]
        .as_str()
        .expect("submission_id")
        .to_string();

    // Scribe runs asynchronously; poll the quarantine until the
    // submission transitions to Extracted (with a generous budget for
    // the python subprocess + skills-host pipeline).
    let mut sub = None;
    for _ in 0..50 {
        let s = h.quarantine.get(&id).await.unwrap();
        if s.status != SubmissionStatus::Pending {
            sub = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let sub = sub.expect("scribe extraction completed within budget");
    assert_eq!(sub.status, SubmissionStatus::Extracted);
    let contact = sub
        .proposals
        .iter()
        .find(|p| p.predicate.as_str() == "contact.person")
        .expect("contact.person proposal");
    assert_eq!(contact.claim["display_name"], "Sara");
    assert_eq!(contact.claim["email"], "sara@example.com");
    // Provenance points back at the source.
    let prov = contact.provenance.first().expect("provenance entry");
    assert_eq!(prov.uri, "file:///sara.md");
    // And every proposal in the submission shares one stable hash.
    let hashes: std::collections::HashSet<_> = sub
        .proposals
        .iter()
        .flat_map(|p| p.provenance.iter().map(|p| p.hash.clone()))
        .collect();
    assert_eq!(hashes.len(), 1, "all proposals share one content hash");
}

#[tokio::test]
async fn ingest_submit_denied_when_caller_lacks_write_capability() {
    // Setup but override the dispatcher's identity to a key with no
    // capability atom in the store.
    let mut h = setup_with_scribe(false).expect("setup");
    h.dispatcher.owner = stranger_pk();
    let resp = h
        .dispatcher
        .handle(ingest_submit_req("file:///x.md", "anything"))
        .await;
    match resp.payload {
        ffs_daemon::ApiPayload::Error { error } => {
            assert_eq!(error.code, ffs_daemon::ERR_CAPABILITY_DENIED);
        }
        ffs_daemon::ApiPayload::Success { result } => {
            panic!("expected capability-denied error; got success: {result}");
        }
    }
    // And nothing should have landed in the quarantine.
    let all = h.quarantine.list(None).await;
    assert!(all.is_empty(), "no submission should have been stored");
}
