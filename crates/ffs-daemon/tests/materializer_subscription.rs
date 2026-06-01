//! End-to-end test that the working-set materializer (task_25)
//! fires through the live `EventPublisher` broadcast channel:
//! publish an `event.atom.committed` notification → wait briefly
//! → assert the projection file lands on disk with the expected
//! content.
//!
//! This is the "broadcast wiring is correct" smoke. The unit tests
//! under `src/materializer.rs` cover the resolve-and-write logic
//! directly; this test covers the subscribe/parse/dispatch path
//! that lives only at runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{
    AtomTemplate, EntityId, Iso8601, PredicateName, PublicKey, SuppressionRegistry, Tier,
};

use ffs_daemon::{Event, EventPublisher, WorkingSetMaterializer};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}
fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn grant_full_caps(store: &dyn AtomStore) {
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

#[tokio::test]
async fn published_atom_committed_event_materializes_a_projection_file() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let predicates_dir = repo_root().join("starter").join("predicates");
    let templates_dir = repo_root().join("starter").join("templates");

    let store = Arc::new(MemAtomStore::new());
    grant_full_caps(&*store);

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let renderer = Arc::new(
        ProjectionRenderer::new(
            store.clone() as Arc<dyn AtomStore>,
            registry,
            &templates_dir,
        )
        .unwrap(),
    );

    let publisher = Arc::new(EventPublisher::new());
    let working_set = Arc::new(InMemoryWorkingSet::new());
    let suppression = Arc::new(SuppressionRegistry::new());

    let materializer = Arc::new(WorkingSetMaterializer::new(
        renderer,
        working_set,
        suppression,
        data_dir.clone(),
        owner_pk(),
    ));
    let handle = materializer.spawn(publisher.clone());

    // Insert the atom into the store first (the materializer will
    // re-render from the store on commit, so the atom has to be
    // present before the notification fires).
    let envelope = AtomTemplate {
        v: 1,
        entity: EntityId::new("Sara_Chen"),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({
            "display_name": "Sara Chen",
            "work_email": "sara@example.com",
        }),
        valid_from: Iso8601::new("2026-05-31T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new("2026-05-31T00:00:00Z").unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    let hash = store.insert(&envelope).unwrap();

    // Fire the notification.
    publisher.publish(Event::AtomCommitted {
        hash: hash.clone(),
        entity: EntityId::new("Sara_Chen"),
        predicate: PredicateName::new("contact.person"),
    });

    // Wait for the materializer's tokio task to handle it.
    let expected = data_dir.join("contacts/by-name/S/Sara_Chen.md");
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if expected.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        expected.exists(),
        "projection file should appear at {}",
        expected.display()
    );
    let on_disk = std::fs::read_to_string(&expected).unwrap();
    assert!(on_disk.contains("display_name: Sara Chen"));
    assert!(on_disk.contains("work_email: sara@example.com"));

    // Clean up the spawned task.
    handle.abort();
}
