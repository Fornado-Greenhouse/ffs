//! ADR-035 (task_48): the `clip` classification tier at the evaluator.
//! The owner's own self-grant keeps "any classification" meaning for
//! their library copies; a grant to a peer must name `clip`.

use ed25519_dalek::SigningKey;

use ffs_core::capability::{
    Action, CapabilityScope, Decision, Target, build_capability_atom, evaluate,
};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{EntityId, Iso8601, PredicateName, PublicKey, Tier};

fn ts(s: &str) -> Iso8601 {
    Iso8601::new(s).unwrap()
}

fn pk(k: &SigningKey) -> PublicKey {
    PublicKey::from_verifying(&k.verifying_key())
}

fn clip_target() -> Target {
    Target {
        predicate: PredicateName::new("source.article"),
        entity: EntityId::new("zClip"),
        classification: Some(Tier::new("clip")),
        tier: None,
    }
}

fn grant(store: &dyn AtomStore, grantor: &SigningKey, grantee: PublicKey, scope: CapabilityScope) {
    let cap = build_capability_atom(
        grantor,
        grantee,
        vec![Action::Read],
        scope,
        ts("2026-01-01T00:00:00Z"),
        None,
        ts("2026-01-02T00:00:00Z"),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
}

#[test]
fn owner_self_grant_with_any_scope_reads_clip() {
    let store = MemAtomStore::new();
    let owner = SigningKey::from_bytes(&[7u8; 32]);
    grant(&store, &owner, pk(&owner), CapabilityScope::default());
    let d = evaluate(
        &store,
        &pk(&owner),
        Action::Read,
        &clip_target(),
        &ts("2026-06-01T00:00:00Z"),
    )
    .unwrap();
    assert!(matches!(d, Decision::Allow { .. }), "{d:?}");
}

#[test]
fn peer_grant_with_any_scope_does_not_read_clip_until_it_names_the_tier() {
    let store = MemAtomStore::new();
    let owner = SigningKey::from_bytes(&[7u8; 32]);
    let peer = SigningKey::from_bytes(&[9u8; 32]);
    grant(&store, &owner, pk(&peer), CapabilityScope::default());
    let d = evaluate(
        &store,
        &pk(&peer),
        Action::Read,
        &clip_target(),
        &ts("2026-06-01T00:00:00Z"),
    )
    .unwrap();
    assert!(
        matches!(d, Decision::Deny { .. }),
        "any-scope peer grant must not cover clip: {d:?}"
    );

    let store2 = MemAtomStore::new();
    grant(
        &store2,
        &owner,
        pk(&peer),
        CapabilityScope {
            classifications: Some(vec![Tier::new("existence"), Tier::new("clip")]),
            ..Default::default()
        },
    );
    let d = evaluate(
        &store2,
        &pk(&peer),
        Action::Read,
        &clip_target(),
        &ts("2026-06-01T00:00:00Z"),
    )
    .unwrap();
    assert!(matches!(d, Decision::Allow { .. }), "{d:?}");
}
