//! End-to-end fast-path integration: stand up a FastPathWatcher against a
//! tempdir, write to a projection file, and assert that a supersession
//! atom appears in the store within the latency budget.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::EventPublisher;
use ffs_fastpath::{FastPathContext, PollingFastPathWatcher, SuppressionRegistry};

const CONTACT_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }
tier = { type = "string" }
notes = { type = "array", items = { type = "string" } }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name", "tier"]
body_sections = ["Notes"]
additive_sections = ["Notes"]

[[reverse_map]]
output = "frontmatter.display_name"
atom_field = "claim.display_name"
edit_kind = "single_line_text"

[[reverse_map]]
output = "frontmatter.tier"
atom_field = "claim.tier"
edit_kind = "frontmatter_value"

[[reverse_map]]
output = "section.Notes.list_item"
atom_field = "claim.notes[]"
edit_kind = "additive_section"
"#;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[99u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

struct Harness {
    _dir: tempfile::TempDir,
    working_set_dir: std::path::PathBuf,
    ingest_dir: std::path::PathBuf,
    store: Arc<dyn AtomStore>,
    ctx: FastPathContext,
    notifier: Arc<EventPublisher>,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let working_set_dir = dir.path().join("working_set");
    let ingest_dir = dir.path().join("ingest");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&working_set_dir).unwrap();
    std::fs::create_dir_all(&ingest_dir).unwrap();
    // 0o700 is the safe-default for a substrate working set on Unix.
    // Windows uses ACLs rather than POSIX modes; the equivalent
    // hardening lives in the installer for Windows, not in test
    // setup, so we just skip the mode change there.
    #[cfg(unix)]
    std::fs::set_permissions(&working_set_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());

    // Owner capability so capability checks succeed (although the fastpath
    // dispatch path doesn't actually capability-check at insert; this is
    // here for symmetry with future cap-gated write paths).
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

    let notifier = Arc::new(EventPublisher::new());
    let suppression = Arc::new(SuppressionRegistry::new());
    let ctx = FastPathContext {
        store: store.clone(),
        registry,
        notifier: notifier.clone(),
        signing_key: Arc::new(owner_key()),
        working_set_dir: working_set_dir.clone(),
        ingest_dir: ingest_dir.clone(),
        suppression,
    };

    Harness {
        _dir: dir,
        working_set_dir,
        ingest_dir,
        store,
        ctx,
        notifier,
    }
}

fn insert_contact(
    store: &dyn AtomStore,
    entity: &str,
    display_name: &str,
    tx_time: &str,
) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({"display_name": display_name}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap()
}

/// Render the baseline content the same way `watcher::render_via_template`
/// does so the test's "old" content matches what the classifier will
/// reconstruct from the head atom.
fn render_baseline(claim: &serde_json::Value, fm_keys: &[(&str, &str)]) -> String {
    let mut out = String::from("---\n");
    for (k, _) in fm_keys {
        if let Some(v) = claim.get(*k).and_then(|x| x.as_str()) {
            out.push_str(&format!("{k}: {v}\n"));
        }
    }
    out.push_str("---\n");
    if let Some(notes) = claim.get("notes").and_then(|n| n.as_array()) {
        out.push_str("\n## Notes\n");
        for n in notes {
            if let Some(s) = n.as_str() {
                out.push_str(&format!("- {s}\n"));
            }
        }
    }
    out.push('\n');
    out
}

#[tokio::test]
async fn fastpath_applies_frontmatter_value_edit_within_budget() {
    let h = setup();
    insert_contact(&*h.store, "Sarah_Chen", "Sarah", "2026-05-25T08:00:00Z");

    let projection_dir = h.working_set_dir.join("contacts/by-name/S");
    std::fs::create_dir_all(&projection_dir).unwrap();
    let path = projection_dir.join("Sarah_Chen.md");
    let head_claim = serde_json::json!({"display_name": "Sarah"});
    let baseline = render_baseline(&head_claim, &[("display_name", "Sarah")]);
    std::fs::write(&path, &baseline).unwrap();

    // Start watcher AFTER writing baseline so the initial write isn't classified.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _watcher = PollingFastPathWatcher::start(h.ctx.clone(), Duration::from_millis(50)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Subscribe to events before triggering the edit.
    let mut sub = h.notifier.subscribe();

    let edited = render_baseline(
        &serde_json::json!({"display_name": "Sara"}),
        &[("display_name", "Sara")],
    );
    let start = Instant::now();
    std::fs::write(&path, &edited).unwrap();

    // Wait up to 2s for either an applied event or a routed event.
    let line = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event timeout")
        .expect("event recv");
    let elapsed = start.elapsed();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(
        v["method"], "event.fastpath.applied",
        "expected fastpath.applied; got {v}"
    );
    // Atom hash present.
    let _ = v["params"]["atom_hash"]
        .as_str()
        .expect("atom_hash present");

    // Latency budget: 200ms release, 2s debug (relaxed per CLAUDE.md).
    let budget = if cfg!(debug_assertions) {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(200)
    };
    assert!(
        elapsed < budget,
        "fast-path latency {elapsed:?} exceeds budget {budget:?}"
    );
}

#[tokio::test]
async fn fastpath_routes_ambiguous_diff_to_ingest() {
    let h = setup();
    insert_contact(&*h.store, "Bob", "Bob", "2026-05-25T08:00:00Z");
    let projection_dir = h.working_set_dir.join("contacts/by-name/B");
    std::fs::create_dir_all(&projection_dir).unwrap();
    let path = projection_dir.join("Bob.md");
    let baseline = render_baseline(
        &serde_json::json!({"display_name": "Bob"}),
        &[("display_name", "Bob")],
    );
    std::fs::write(&path, &baseline).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _watcher = PollingFastPathWatcher::start(h.ctx.clone(), Duration::from_millis(50)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut sub = h.notifier.subscribe();
    // Drastic rewrite — multi-line change, no clean single-rule match.
    let edited = "completely\nrewritten\ncontent\nwith\nmultiple\nnew\nlines\n";
    std::fs::write(&path, edited).unwrap();

    let line = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event timeout")
        .expect("event recv");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    // Slow path emits a projection.invalidated event (see dispatch.rs).
    assert_eq!(v["method"], "event.projection.invalidated");
    // An ingest correction file should appear.
    let entries: Vec<_> = std::fs::read_dir(&h.ingest_dir).unwrap().collect();
    assert!(
        !entries.is_empty(),
        "expected at least one ingest correction file"
    );
}

#[tokio::test]
async fn fastpath_refuses_federated_projection() {
    let h = setup();
    let projection_dir = h.working_set_dir.join("contacts/from/alice/by-name/X");
    std::fs::create_dir_all(&projection_dir).unwrap();
    let path = projection_dir.join("Xan.md");
    std::fs::write(&path, "anything").unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _watcher = PollingFastPathWatcher::start(h.ctx.clone(), Duration::from_millis(50)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut sub = h.notifier.subscribe();
    std::fs::write(&path, "anything new").unwrap();
    let line = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event timeout")
        .expect("event recv");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        v["method"], "event.projection.invalidated",
        "federated edits route to ingest"
    );
}

#[tokio::test]
async fn fastpath_reconciles_edit_made_before_startup() {
    let h = setup();
    insert_contact(&*h.store, "Casey", "Casey", "2026-05-25T08:00:00Z");
    let projection_dir = h.working_set_dir.join("contacts/by-name/C");
    std::fs::create_dir_all(&projection_dir).unwrap();
    let path = projection_dir.join("Casey.md");
    // Pre-existing on-disk edit (Casey → Case) made "while the daemon was down".
    let edited = render_baseline(
        &serde_json::json!({"display_name": "Case"}),
        &[("display_name", "Case")],
    );
    std::fs::write(&path, &edited).unwrap();
    let mut sub = h.notifier.subscribe();
    // Subscribe before starting the watcher so we don't miss the reconciliation event.
    let _watcher = PollingFastPathWatcher::start(h.ctx.clone(), Duration::from_millis(50)).unwrap();
    let line = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("reconciliation event timeout")
        .expect("event recv");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        v["method"], "event.fastpath.applied",
        "expected reconciliation to author a supersession; got {v}"
    );
}

#[tokio::test]
async fn fastpath_debounces_rapid_save_burst() {
    let h = setup();
    insert_contact(&*h.store, "Dana", "Dana", "2026-05-25T08:00:00Z");
    let projection_dir = h.working_set_dir.join("contacts/by-name/D");
    std::fs::create_dir_all(&projection_dir).unwrap();
    let path = projection_dir.join("Dana.md");
    let baseline = render_baseline(
        &serde_json::json!({"display_name": "Dana"}),
        &[("display_name", "Dana")],
    );
    std::fs::write(&path, &baseline).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _watcher = PollingFastPathWatcher::start(h.ctx.clone(), Duration::from_millis(50)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut sub = h.notifier.subscribe();

    // Burst of 5 rapid writes — debouncer should collapse them so only the
    // final state ("Dani") is processed; we expect a single fastpath.applied.
    for name in ["Dan", "Dann", "Danni", "Dann", "Dani"] {
        let content = render_baseline(
            &serde_json::json!({"display_name": name}),
            &[("display_name", name)],
        );
        std::fs::write(&path, &content).unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let line = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event timeout")
        .expect("event recv");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["method"], "event.fastpath.applied");

    // No further event should arrive within the debounce window: the rapid
    // burst must collapse into exactly one applied event.
    let extra = tokio::time::timeout(Duration::from_millis(300), sub.recv()).await;
    assert!(
        extra.is_err(),
        "expected debouncer to collapse the burst; got extra event: {extra:?}"
    );
}

#[tokio::test]
async fn suppression_registry_ignores_daemon_self_write() {
    let h = setup();
    let path = h.working_set_dir.join("test.md");
    let content = b"some bytes";
    h.ctx.suppression.record(&path, content);
    assert!(h.ctx.suppression.check(&path, content));
    // Second check should miss (consumed).
    assert!(!h.ctx.suppression.check(&path, content));
}
