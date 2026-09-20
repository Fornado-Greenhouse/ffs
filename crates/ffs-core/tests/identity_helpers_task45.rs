//! task_45 (ADR-030): store-level identity helpers shared by the
//! in-memory and SQLite backends: `same_as` redirects with cycle
//! guard and undo, symmetric `different_from`, store-backed priors,
//! and NIL sightings.

use ffs_core::{
    AtomStore, AtomTemplate, EntityId, Iso8601, MemAtomStore, Multihash, PredicateName,
    SqliteAtomStore, Tier,
};

fn key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
}

fn t(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

fn insert(
    store: &dyn AtomStore,
    entity: &str,
    predicate: &str,
    claim: serde_json::Value,
    tx: &str,
    valid_to: Option<&str>,
    supersedes: Option<Multihash>,
) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: t("2026-01-01T00:00:00Z"),
        valid_to: valid_to.map(t),
        tx_time: t(tx),
        classification: Tier::new("existence"),
        supersedes,
        provenance: vec![],
    }
    .sign(&key())
    .unwrap();
    store.insert(&env).unwrap()
}

fn backends() -> Vec<(&'static str, Box<dyn AtomStore>)> {
    vec![
        ("mem", Box::new(MemAtomStore::new())),
        (
            "sqlite",
            Box::new(SqliteAtomStore::open_in_memory(&[0x11u8; 32]).unwrap()),
        ),
    ]
}

#[test]
fn same_as_follows_chains_with_a_cycle_guard_and_respects_undo() {
    for (name, store) in backends() {
        let s: &dyn AtomStore = &*store;
        assert_eq!(
            s.follow_same_as(&EntityId::new("a"), None)
                .unwrap()
                .as_str(),
            "a",
            "{name}"
        );
        let h1 = insert(
            s,
            "a",
            "entity.same_as",
            serde_json::json!({"target": "b"}),
            "2026-02-01T00:00:00Z",
            None,
            None,
        );
        insert(
            s,
            "b",
            "entity.same_as",
            serde_json::json!({"target": "c"}),
            "2026-02-02T00:00:00Z",
            None,
            None,
        );
        assert_eq!(
            s.same_as_target(&EntityId::new("a"), None)
                .unwrap()
                .unwrap()
                .as_str(),
            "b",
            "{name}"
        );
        assert_eq!(
            s.follow_same_as(&EntityId::new("a"), None)
                .unwrap()
                .as_str(),
            "c",
            "{name}: chain a->b->c"
        );
        // A cycle must terminate.
        insert(
            s,
            "c",
            "entity.same_as",
            serde_json::json!({"target": "a"}),
            "2026-02-03T00:00:00Z",
            None,
            None,
        );
        let _ = s.follow_same_as(&EntityId::new("a"), None).unwrap();
        // Undo: supersede a's same_as with valid_to set; a is its own entity again.
        insert(
            s,
            "a",
            "entity.same_as",
            serde_json::json!({"target": "b"}),
            "2026-02-04T00:00:00Z",
            Some("2026-02-04T00:00:00Z"),
            Some(h1),
        );
        assert!(
            s.same_as_target(&EntityId::new("a"), None)
                .unwrap()
                .is_none(),
            "{name}: undone merge"
        );
        assert_eq!(
            s.follow_same_as(&EntityId::new("a"), None)
                .unwrap()
                .as_str(),
            "a",
            "{name}"
        );
    }
}

#[test]
fn different_from_is_symmetric() {
    for (name, store) in backends() {
        let s: &dyn AtomStore = &*store;
        insert(
            s,
            "sara1",
            "entity.different_from",
            serde_json::json!({"other": "sara2", "criterion": "different orgs"}),
            "2026-03-01T00:00:00Z",
            None,
            None,
        );
        let from_one = s.different_from(&EntityId::new("sara1"), None).unwrap();
        let from_two = s.different_from(&EntityId::new("sara2"), None).unwrap();
        assert_eq!(from_one, vec![EntityId::new("sara2")], "{name}");
        assert_eq!(
            from_two,
            vec![EntityId::new("sara1")],
            "{name}: reverse direction"
        );
        assert!(
            s.different_from(&EntityId::new("nobody"), None)
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}

#[test]
fn priors_count_accepted_resolutions_per_form_highest_first() {
    for (name, store) in backends() {
        let s: &dyn AtomStore = &*store;
        assert!(s.prior_counts("s. chen").unwrap().is_empty(), "{name}");
        s.record_resolution("s. chen", &EntityId::new("z1"))
            .unwrap();
        s.record_resolution("s. chen", &EntityId::new("z2"))
            .unwrap();
        s.record_resolution("s. chen", &EntityId::new("z2"))
            .unwrap();
        let counts = s.prior_counts("s. chen").unwrap();
        assert_eq!(
            counts,
            vec![(EntityId::new("z2"), 2), (EntityId::new("z1"), 1)],
            "{name}"
        );
        assert!(s.prior_counts("other").unwrap().is_empty(), "{name}");
    }
}

#[test]
fn nil_sightings_return_the_prior_on_second_sighting_and_clear() {
    for (name, store) in backends() {
        let s: &dyn AtomStore = &*store;
        let when = t("2026-04-01T00:00:00Z");
        assert!(
            s.record_sighting("pat example|acme", "sub-1", "Pat Example", &when)
                .unwrap()
                .is_none(),
            "{name}"
        );
        let prior = s
            .record_sighting(
                "pat example|acme",
                "sub-2",
                "P. Example",
                &t("2026-04-02T00:00:00Z"),
            )
            .unwrap()
            .expect("second sighting sees the first");
        assert_eq!(prior.submission_id, "sub-1", "{name}");
        assert_eq!(prior.display, "Pat Example", "{name}");
        assert_eq!(prior.first_seen, when, "{name}");
        s.clear_sighting("pat example|acme").unwrap();
        assert!(
            s.record_sighting("pat example|acme", "sub-3", "Pat", &when)
                .unwrap()
                .is_none(),
            "{name}: cleared"
        );
    }
}
