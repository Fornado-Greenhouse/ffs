//! Integration tests for the capability evaluator: scope coverage,
//! bitemporal windows, supersession-as-revocation, multi-capability union,
//! parity between MemAtomStore and SqliteAtomStore, property tests for
//! supersession-narrowing monotonicity and bitemporal-window correctness,
//! and a perf check confirming 10K evaluations against a 1K-capability
//! store complete well under 1 second.

use ed25519_dalek::SigningKey;
use ffs_core::capability::{
    Action, CAPABILITY_PREDICATE, CapabilityClaim, CapabilityScope, Decision, DenyReason, Target,
    build_capability_atom, evaluate, validate_supersession_narrows,
};
use ffs_core::store::{AtomStore, MemAtomStore, SqliteAtomStore};
use ffs_core::{EntityId, Iso8601, PredicateName, PublicKey, Tier};
use proptest::prelude::*;

fn grantor_key() -> SigningKey {
    SigningKey::from_bytes(&[1u8; 32])
}

fn grantee_key() -> SigningKey {
    SigningKey::from_bytes(&[2u8; 32])
}

fn grantee_pk() -> PublicKey {
    PublicKey::from_verifying(&grantee_key().verifying_key())
}

fn dek() -> [u8; 32] {
    [99u8; 32]
}

fn target_existence() -> Target {
    Target::new(PredicateName::new("contact.person"), EntityId::new("alice"))
        .with_classification(Tier::new("existence"))
}

fn target_personal_email() -> Target {
    Target::new(PredicateName::new("contact.person"), EntityId::new("alice"))
        .with_classification(Tier::new("personal_email"))
}

// ----- Each test runs once per backend via a closure -----

fn each_backend<F: Fn(&dyn AtomStore)>(f: F) {
    f(&MemAtomStore::new());
    f(&SqliteAtomStore::open_in_memory(&dek()).unwrap());
}

// ----- Spec-required test cases -----

#[test]
fn grant_for_existence_allows_read_on_existence_atom() {
    each_backend(|store| {
        let cap = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-05-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        let cap_hash = store.insert(&cap).unwrap();

        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Read,
            &target_existence(),
            &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
        )
        .unwrap();
        match d {
            Decision::Allow { capability } => assert_eq!(capability, cap_hash),
            other => panic!("expected Allow, got {other:?}"),
        }
    });
}

#[test]
fn grant_for_existence_denies_personal_email() {
    each_backend(|store| {
        let cap = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-05-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap).unwrap();

        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Read,
            &target_personal_email(),
            &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            d,
            Decision::Deny {
                reason: DenyReason::NotInScope
            }
        ));
    });
}

#[test]
fn capability_not_yet_valid_denies_with_not_yet_valid() {
    each_backend(|store| {
        let cap = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-06-01T00:00:00Z").unwrap(), // future
            None,
            Iso8601::new("2026-05-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap).unwrap();
        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Read,
            &target_existence(),
            &Iso8601::new("2026-05-15T12:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            d,
            Decision::Deny {
                reason: DenyReason::NotYetValid
            }
        ));
    });
}

#[test]
fn capability_expired_denies_with_expired() {
    each_backend(|store| {
        let cap = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-04-01T00:00:00Z").unwrap(),
            Some(Iso8601::new("2026-04-30T00:00:00Z").unwrap()), // past
            Iso8601::new("2026-04-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap).unwrap();
        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Read,
            &target_existence(),
            &Iso8601::new("2026-05-15T12:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            d,
            Decision::Deny {
                reason: DenyReason::Expired
            }
        ));
    });
}

#[test]
fn superseded_capability_no_longer_grants() {
    each_backend(|store| {
        let cap_v1 = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read, Action::Write],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-05-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        let h_v1 = store.insert(&cap_v1).unwrap();

        // Narrowing supersession that strips the Write action.
        let cap_v2 = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-05-10T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-05-10T00:00:01Z").unwrap(),
            Some(h_v1),
        )
        .unwrap();
        store.insert(&cap_v2).unwrap();

        // Read still allowed via v2.
        assert!(
            evaluate(
                store,
                &grantee_pk(),
                Action::Read,
                &target_existence(),
                &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
            )
            .unwrap()
            .is_allow()
        );

        // Write denied — v1 (which granted Write) is superseded by v2; v2 doesn't grant Write.
        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Write,
            &target_existence(),
            &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(d.is_deny(), "expected Deny for Write, got {d:?}");
    });
}

#[test]
fn no_capability_at_all_denies_with_no_capability_found() {
    each_backend(|store| {
        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Read,
            &target_existence(),
            &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            d,
            Decision::Deny {
                reason: DenyReason::NoCapabilityFound
            }
        ));
    });
}

#[test]
fn multiple_capabilities_union_their_grants() {
    each_backend(|store| {
        // Cap A: Read on contact.person/existence
        let cap_a = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-05-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap_a).unwrap();

        // Cap B: Write on note (different predicate entirely)
        let cap_b = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Write],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("note")]),
                ..Default::default()
            },
            Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-05-01T00:00:02Z").unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap_b).unwrap();

        // Read on existence — Cap A allows.
        assert!(
            evaluate(
                store,
                &grantee_pk(),
                Action::Read,
                &target_existence(),
                &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
            )
            .unwrap()
            .is_allow()
        );

        // Write on note — Cap B allows.
        let note_target = Target::new(PredicateName::new("note"), EntityId::new("note-001"));
        assert!(
            evaluate(
                store,
                &grantee_pk(),
                Action::Write,
                &note_target,
                &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
            )
            .unwrap()
            .is_allow()
        );

        // Read on decisions — neither matches.
        let dec_target = Target::new(PredicateName::new("decision"), EntityId::new("dec-001"));
        let d = evaluate(
            store,
            &grantee_pk(),
            Action::Read,
            &dec_target,
            &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            d,
            Decision::Deny {
                reason: DenyReason::NotInScope
            }
        ));
    });
}

#[test]
fn capability_filed_via_atomstore_is_honored_by_next_evaluation() {
    // Roundtrip via AtomStore::insert directly (no build_capability_atom shortcut).
    let store = SqliteAtomStore::open_in_memory(&dek()).unwrap();
    let cap = build_capability_atom(
        &grantor_key(),
        grantee_pk(),
        vec![Action::Read],
        CapabilityScope {
            predicates: Some(vec![PredicateName::new("contact.person")]),
            classifications: Some(vec![Tier::new("existence")]),
            ..Default::default()
        },
        Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-05-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    let h = store.insert(&cap).unwrap();

    let d = evaluate(
        &store,
        &grantee_pk(),
        Action::Read,
        &target_existence(),
        &Iso8601::new("2026-05-25T12:00:00Z").unwrap(),
    )
    .unwrap();
    assert_eq!(d, Decision::Allow { capability: h });
}

// ----- Perf budget -----

#[test]
fn ten_thousand_evaluations_against_one_thousand_capabilities_under_one_second() {
    let store = SqliteAtomStore::open_in_memory(&dek()).unwrap();

    // Seed 1000 capabilities. Most won't match; one will.
    for i in 0..1000u32 {
        let other_key = ed25519_dalek::SigningKey::from_bytes(&[(i % 250) as u8; 32]);
        let other_grantee = PublicKey::from_verifying(&other_key.verifying_key());
        let cap = build_capability_atom(
            &grantor_key(),
            other_grantee,
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new(format!("pred.{}", i % 50))]),
                ..Default::default()
            },
            Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
            None,
            // unique tx_time per atom so content hashes don't collide
            Iso8601::new(format!("2026-05-01T00:00:{:02}.{:03}Z", i % 60, i / 60)).unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap).unwrap();
    }
    // Seed the real grantee's capability.
    let cap = build_capability_atom(
        &grantor_key(),
        grantee_pk(),
        vec![Action::Read],
        CapabilityScope {
            predicates: Some(vec![PredicateName::new("contact.person")]),
            classifications: Some(vec![Tier::new("existence")]),
            ..Default::default()
        },
        Iso8601::new("2026-05-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-05-01T00:01:00Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();

    let target = target_existence();
    let as_of = Iso8601::new("2026-05-25T12:00:00Z").unwrap();

    let start = std::time::Instant::now();
    for _ in 0..10_000 {
        let d = evaluate(&store, &grantee_pk(), Action::Read, &target, &as_of).unwrap();
        assert!(d.is_allow());
    }
    let elapsed = start.elapsed();
    // Production budget per the task spec: < 1s. Debug builds are ~10x slower
    // because rusqlite + serde compile without optimization; relax to 10s in
    // debug while keeping the production-relevant assertion tight in release.
    let budget = if cfg!(debug_assertions) {
        std::time::Duration::from_secs(10)
    } else {
        std::time::Duration::from_secs(1)
    };
    assert!(
        elapsed < budget,
        "10K evaluations took {elapsed:?}, expected < {budget:?} (debug_assertions={})",
        cfg!(debug_assertions)
    );
}

// ----- Property tests -----

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// For any chain of capability supersessions where each link narrows
    /// via `validate_supersession_narrows`, the tail's scope is a subset
    /// of the head's scope (transitive narrowing).
    #[test]
    fn supersession_chain_monotonically_narrows(
        keep_predicates in prop::collection::vec(any::<u32>(), 1..6),
        actions in prop::collection::vec(prop_oneof![
            Just(Action::Read), Just(Action::Write), Just(Action::Supersede),
            Just(Action::Erase), Just(Action::Classify), Just(Action::Federate),
        ], 1..7),
    ) {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let grantee = PublicKey::from_verifying(&key.verifying_key());

        // Root capability with the full predicate list + full action set.
        let preds: Vec<PredicateName> = keep_predicates.iter()
            .map(|n| PredicateName::new(format!("p.{n}")))
            .collect();
        let root = CapabilityClaim {
            grantee: grantee.clone(),
            actions: actions.clone(),
            scope: CapabilityScope {
                predicates: Some(preds.clone()),
                ..Default::default()
            },
        };

        // Build a narrowing chain: each successor drops the last predicate
        // (or keeps everything if only one left). All actions retained.
        let mut prev = root.clone();
        for _ in 0..3 {
            let mut next_preds = prev.scope.predicates.clone().unwrap();
            if next_preds.len() > 1 {
                next_preds.pop();
            }
            let next = CapabilityClaim {
                grantee: grantee.clone(),
                actions: prev.actions.clone(),
                scope: CapabilityScope {
                    predicates: Some(next_preds),
                    ..Default::default()
                },
            };
            prop_assert!(validate_supersession_narrows(&next, &prev).is_ok());
            prop_assert!(next.scope.narrows_or_equals(&root.scope));
            prev = next;
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// No combination of valid_from / valid_to / tx_time grants access
    /// outside the declared window.
    #[test]
    fn bitemporal_window_correctness(
        as_of_day in 1u8..28,
        valid_from_day in 1u8..28,
        valid_to_day in 1u8..28,
    ) {
        let store = MemAtomStore::new();
        let from = Iso8601::new(format!("2026-05-{valid_from_day:02}T00:00:00Z")).unwrap();
        let to = Iso8601::new(format!("2026-05-{valid_to_day:02}T00:00:00Z")).unwrap();
        let as_of = Iso8601::new(format!("2026-05-{as_of_day:02}T12:00:00Z")).unwrap();

        // valid_to must be >= valid_from for a sensible capability.
        if valid_to_day < valid_from_day {
            return Ok(());
        }

        let cap = build_capability_atom(
            &grantor_key(),
            grantee_pk(),
            vec![Action::Read],
            CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                classifications: Some(vec![Tier::new("existence")]),
                ..Default::default()
            },
            from.clone(),
            Some(to.clone()),
            Iso8601::new(format!("2026-05-{valid_from_day:02}T00:00:01Z")).unwrap(),
            None,
        ).unwrap();
        store.insert(&cap).unwrap();

        let d = evaluate(&store, &grantee_pk(), Action::Read, &target_existence(), &as_of).unwrap();

        let in_window = as_of.as_str() >= from.as_str() && as_of.as_str() <= to.as_str();
        if in_window {
            prop_assert!(d.is_allow(), "in-window as_of={as_of:?} expected Allow, got {d:?}");
        } else {
            prop_assert!(d.is_deny(), "out-of-window as_of={as_of:?} expected Deny, got {d:?}");
        }
    }
}

// Sanity check: the predicate constant matches what the rest of the substrate expects.
#[test]
fn capability_predicate_name_is_stable() {
    assert_eq!(CAPABILITY_PREDICATE, "capability.grant");
}
