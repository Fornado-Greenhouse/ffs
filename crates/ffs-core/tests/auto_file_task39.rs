//! task_39 (ADR-029, ADR-030): the additive/conflict classifier, the
//! store-backed `max_per_day` count, merge-loser lookup, and the
//! winner render that folds a merged loser's atoms in.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::{PredicateSpec, SpecRegistry};
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::quarantine::{Candidate, Resolution};
use ffs_core::store::{AtomStore, MemAtomStore, SqliteAtomStore};
use ffs_core::{
    AtomEnvelope, AtomTemplate, EntityId, Filing, InMemoryPathIndex, Iso8601, Multihash, PathIndex,
    PredicateName, Proposal, Provenance, PublicKey, SourceKind, Tier, classify,
};
use proptest::prelude::*;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}
fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn atom(
    entity: &str,
    predicate: &str,
    claim: serde_json::Value,
    tx: &str,
    provenance: Vec<Provenance>,
) -> AtomEnvelope {
    AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance,
    }
    .sign(&owner_key())
    .unwrap()
}

fn spec(toml: &str) -> PredicateSpec {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("s.toml"), toml).unwrap();
    let reg = SpecRegistry::new();
    reg.load_dir(dir.path()).unwrap();
    let name = reg.names().remove(0);
    reg.get(&name).unwrap()
}

const PERSON_SPEC: &str = r#"
name = "person.generic"
version = 2
[claim_schema]
type = "object"
required = ["display_name"]
[claim_schema.properties]
display_name = { type = "string" }
role = { type = "string" }
organization = { type = "string" }
notes = { type = "array", items = { type = "string" } }
[rendering]
template = "person.md.tera"
frontmatter_fields = ["display_name", "role", "organization"]
body_sections = ["Notes"]
additive_sections = ["Notes"]
[[reverse_map]]
output = "frontmatter.role"
atom_field = "claim.role"
edit_kind = "single_line_text"
[[reverse_map]]
output = "section.Notes.list_item"
atom_field = "claim.notes[]"
edit_kind = "additive_section"
"#;

const ARTICLE_SPEC: &str = r#"
name = "source.article"
version = 1
[claim_schema]
type = "object"
required = ["title"]
[claim_schema.properties]
title = { type = "string" }
[rendering]
template = "article.md.tera"
frontmatter_fields = ["title"]
[quarantine]
append_only = true
"#;

fn proposal(predicate: &str, claim: serde_json::Value) -> Proposal {
    Proposal::new(PredicateName::new(predicate), claim, vec![], "test")
}

fn scalar_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        "[a-z]{1,8}".prop_map(serde_json::Value::from),
        any::<i32>().prop_map(serde_json::Value::from),
        any::<bool>().prop_map(serde_json::Value::from),
    ]
}

fn notes_list() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z]{1,6}", 0..4)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A proposal that sets any field already holding a different scalar
    /// on the head is never additive.
    #[test]
    fn proposal_touching_existing_scalar_is_never_additive(
        head_role in scalar_value(),
        new_role in scalar_value(),
    ) {
        prop_assume!(head_role != new_role);
        let s = spec(PERSON_SPEC);
        let head = atom(
            "e1",
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "role": head_role}),
            "2026-02-01T00:00:00Z",
            vec![],
        );
        let mut p = proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "role": new_role}),
        );
        p.entity = Some(EntityId::new("e1"));
        p.resolution = Some(Resolution::Existing);
        prop_assert!(matches!(classify(&p, &[head], Some(&s), false), Filing::Conflicting(_)));
    }

    /// Appending to a declared additive section is additive for every
    /// head state, as long as the proposal keeps what the head had.
    #[test]
    fn additive_section_append_is_always_additive(
        existing in notes_list(),
        added in notes_list(),
        role in "[a-z]{1,8}",
    ) {
        let s = spec(PERSON_SPEC);
        let head = atom(
            "e1",
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "role": role, "notes": existing}),
            "2026-02-01T00:00:00Z",
            vec![],
        );
        let mut notes = existing.clone();
        notes.extend(added);
        let mut p = proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "role": role, "notes": notes}),
        );
        p.entity = Some(EntityId::new("e1"));
        p.resolution = Some(Resolution::Existing);
        prop_assert_eq!(classify(&p, &[head], Some(&s), false), Filing::Additive);
    }

    /// An append-only predicate's proposal is additive regardless of
    /// existing records, even when the claims differ.
    #[test]
    fn append_only_predicate_is_always_additive(
        old_title in "[a-z ]{1,12}",
        new_title in "[a-z ]{1,12}",
    ) {
        let s = spec(ARTICLE_SPEC);
        let head = atom(
            "a1",
            "source.article",
            serde_json::json!({"title": old_title}),
            "2026-02-01T00:00:00Z",
            vec![],
        );
        let mut p = proposal("source.article", serde_json::json!({"title": new_title}));
        p.entity = Some(EntityId::new("a1"));
        p.resolution = Some(Resolution::Existing);
        prop_assert_eq!(classify(&p, &[head], Some(&s), false), Filing::Additive);
    }

    /// A same-name candidate whose organization does not agree makes the
    /// proposal conflicting even when the resolver said existing.
    #[test]
    fn same_name_different_org_candidate_conflicts(
        score in 0.0f64..20.0,
        org in "[A-Z][a-z]{2,8}",
    ) {
        let s = spec(PERSON_SPEC);
        let mut p = proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": org}),
        );
        p.entity = Some(EntityId::new("e1"));
        p.resolution = Some(Resolution::Existing);
        p.candidates = vec![
            Candidate {
                entity: EntityId::new("e1"),
                score: score + 1.0,
                matched_on: vec!["display_name".into(), "organization".into()],
                display: "Sara Chen".into(),
            },
            Candidate {
                entity: EntityId::new("e2"),
                score,
                matched_on: vec!["display_name".into()],
                display: "Sara Chen".into(),
            },
        ];
        prop_assert!(matches!(classify(&p, &[], Some(&s), false), Filing::Conflicting(_)));
    }

    /// A multi-leaf head is conflicting whatever the proposal says.
    #[test]
    fn multi_leaf_head_is_conflicting(notes in notes_list()) {
        let s = spec(PERSON_SPEC);
        let mut p = proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "notes": notes}),
        );
        p.resolution = Some(Resolution::New);
        prop_assert!(matches!(classify(&p, &[], Some(&s), true), Filing::Conflicting(_)));
    }

    /// Ambiguous resolution is conflicting for every predicate, including
    /// append-only ones, so no grant can auto-file it.
    #[test]
    fn ambiguous_resolution_is_always_conflicting(title in "[a-z ]{1,12}") {
        let s = spec(ARTICLE_SPEC);
        let mut p = proposal("source.article", serde_json::json!({"title": title}));
        p.resolution = Some(Resolution::Ambiguous);
        prop_assert!(matches!(classify(&p, &[], Some(&s), false), Filing::Conflicting(_)));
        let mut q = proposal("person.generic", serde_json::json!({"display_name": title}));
        q.resolution = Some(Resolution::Ambiguous);
        prop_assert!(matches!(classify(&q, &[], Some(&spec(PERSON_SPEC)), false), Filing::Conflicting(_)));
    }
}

#[test]
fn new_entity_and_blank_fill_are_additive_and_role_ending_is_not() {
    let s = spec(PERSON_SPEC);
    // New entity: additive.
    let mut p = proposal("person.generic", serde_json::json!({"display_name": "Pat"}));
    p.resolution = Some(Resolution::New);
    assert_eq!(classify(&p, &[], Some(&s), false), Filing::Additive);
    // Filling a blank scalar on an existing head: additive.
    let head = atom(
        "e1",
        "person.generic",
        serde_json::json!({"display_name": "Pat"}),
        "2026-02-01T00:00:00Z",
        vec![],
    );
    let mut fill = proposal(
        "person.generic",
        serde_json::json!({"display_name": "Pat", "role": "CEO"}),
    );
    fill.entity = Some(EntityId::new("e1"));
    fill.resolution = Some(Resolution::Existing);
    assert_eq!(
        classify(&fill, std::slice::from_ref(&head), Some(&s), false),
        Filing::Additive
    );
    // A role ending on an existing head supersedes: conflicting.
    let mut end = proposal("person.generic", serde_json::json!({"display_name": "Pat"}));
    end.entity = Some(EntityId::new("e1"));
    end.resolution = Some(Resolution::Existing);
    end.ends_role = true;
    assert!(matches!(
        classify(&end, &[head], Some(&s), false),
        Filing::Conflicting(_)
    ));
}

#[test]
fn max_per_day_counts_from_store_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("atoms.db");
    let key = [0x42u8; 32];
    let grant_hash = Multihash::blake3_of(b"grant-1");
    let other_grant = Multihash::blake3_of(b"grant-2");
    let auto = |hash: &Multihash| Provenance {
        kind: SourceKind::AutoAccept,
        uri: format!("ffs://local/atom/{}", hash.to_multibase()),
        hash: hash.clone(),
    };
    {
        let store = SqliteAtomStore::open_with_key(&path, &key).unwrap();
        store
            .insert(&atom(
                "e1",
                "note",
                serde_json::json!({"title": "a"}),
                "2026-09-20T08:00:00Z",
                vec![auto(&grant_hash)],
            ))
            .unwrap();
        store
            .insert(&atom(
                "e2",
                "note",
                serde_json::json!({"title": "b"}),
                "2026-09-20T09:00:00Z",
                vec![auto(&grant_hash)],
            ))
            .unwrap();
        // Yesterday: outside the window.
        store
            .insert(&atom(
                "e3",
                "note",
                serde_json::json!({"title": "c"}),
                "2026-09-19T23:00:00Z",
                vec![auto(&grant_hash)],
            ))
            .unwrap();
        // Another grant, and an owner accept with no auto_accept entry.
        store
            .insert(&atom(
                "e4",
                "note",
                serde_json::json!({"title": "d"}),
                "2026-09-20T10:00:00Z",
                vec![auto(&other_grant)],
            ))
            .unwrap();
        store
            .insert(&atom(
                "e5",
                "note",
                serde_json::json!({"title": "e"}),
                "2026-09-20T11:00:00Z",
                vec![],
            ))
            .unwrap();
        let since = Iso8601::new("2026-09-20T00:00:00Z").unwrap();
        assert_eq!(
            store
                .count_auto_accepted_since(&grant_hash, &since)
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .count_auto_accepted_since(&other_grant, &since)
                .unwrap(),
            1
        );
    }
    let reopened = SqliteAtomStore::open_with_key(&path, &key).unwrap();
    let since = Iso8601::new("2026-09-20T00:00:00Z").unwrap();
    assert_eq!(
        reopened
            .count_auto_accepted_since(&grant_hash, &since)
            .unwrap(),
        2
    );
    // The in-memory backend agrees.
    let mem = MemAtomStore::new();
    mem.insert(&atom(
        "e1",
        "note",
        serde_json::json!({"title": "a"}),
        "2026-09-20T08:00:00Z",
        vec![auto(&grant_hash)],
    ))
    .unwrap();
    mem.insert(&atom(
        "e3",
        "note",
        serde_json::json!({"title": "c"}),
        "2026-09-19T23:00:00Z",
        vec![auto(&grant_hash)],
    ))
    .unwrap();
    assert_eq!(
        mem.count_auto_accepted_since(&grant_hash, &since).unwrap(),
        1
    );
}

#[test]
fn same_as_losers_lists_entities_merged_into_a_winner_and_resolve_same_as_follows_chain() {
    let store = MemAtomStore::new();
    let winner = EntityId::new("w");
    store
        .insert(&atom(
            "l1",
            "entity.same_as",
            serde_json::json!({"target": "w"}),
            "2026-02-01T00:00:00Z",
            vec![],
        ))
        .unwrap();
    store
        .insert(&atom(
            "l2",
            "entity.same_as",
            serde_json::json!({"target": "l1"}),
            "2026-02-02T00:00:00Z",
            vec![],
        ))
        .unwrap();
    let mut losers = store.same_as_losers(&winner, None).unwrap();
    losers.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(losers, vec![EntityId::new("l1")], "direct losers only");
    assert_eq!(
        store.resolve_same_as(&EntityId::new("l2"), None).unwrap(),
        winner
    );
    assert_eq!(store.resolve_same_as(&winner, None).unwrap(), winner);
}

const PEOPLE_TOML: &str = r#"
name = "person.generic"
version = 2
[claim_schema]
type = "object"
required = ["display_name"]
[claim_schema.properties]
display_name = { type = "string" }
notes = { type = "array" }
[rendering]
template = "person.md.tera"
frontmatter_fields = ["display_name"]
[pagination]
strategy = "alphabetical_first_letter"
group_field = "display_name"
[path]
family = "people"
name_field = "display_name"
"#;

const SAME_AS_TOML: &str = r#"
name = "entity.same_as"
version = 1
[claim_schema]
type = "object"
required = ["target"]
[claim_schema.properties]
target = { type = "string" }
[rendering]
template = "same-as.md.tera"
"#;

const PERSON_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
---
{% if merged_from %}merged: {% for m in merged_from %}[[{{ m.basename }}|{{ m.display }}]] {% endfor %}
{% endif %}{% if claim.notes %}## Notes
{% for n in claim.notes %}- {{ n }}
{% endfor %}{% endif %}"#;

#[test]
fn materializer_renders_merged_into_stub_and_folds_atoms_into_winner() {
    let dir = tempfile::tempdir().unwrap();
    let preds = dir.path().join("predicates");
    std::fs::create_dir_all(&preds).unwrap();
    std::fs::write(preds.join("people.toml"), PEOPLE_TOML).unwrap();
    std::fs::write(preds.join("same_as.toml"), SAME_AS_TOML).unwrap();
    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&preds).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![Action::Read],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
    let index = InMemoryPathIndex::new_arc();
    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    let mut renderer = ProjectionRenderer::new(store.clone(), registry, &templates)
        .unwrap()
        .with_path_index(index.clone());
    renderer
        .add_raw_template("person.md.tera", PERSON_TEMPLATE)
        .unwrap();

    let winner = EntityId::mint();
    let loser = EntityId::mint();
    let winner_hash = store
        .insert(&atom(
            winner.as_str(),
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "notes": ["met at picnic"]}),
            "2026-02-02T00:00:00Z",
            vec![],
        ))
        .unwrap();
    let loser_hash = store
        .insert(&atom(
            loser.as_str(),
            "person.generic",
            serde_json::json!({"display_name": "S. Chen", "notes": ["met at picnic", "runs the greenhouse"]}),
            "2026-02-03T00:00:00Z",
            vec![],
        ))
        .unwrap();
    index.assign("people", &winner, "Sara Chen", &[]).unwrap();
    index.assign("people", &loser, "S. Chen", &[]).unwrap();
    let same_as = store
        .insert(&atom(
            loser.as_str(),
            "entity.same_as",
            serde_json::json!({"target": winner.as_str()}),
            "2026-02-04T00:00:00Z",
            vec![],
        ))
        .unwrap();

    let req = |path: &str| ProjectionRequest {
        path: path.into(),
        as_of: Some(Iso8601::new("2026-12-31T00:00:00Z").unwrap()),
        agent: owner_pk(),
    };
    let stub = renderer
        .render(&req("people/by-name/S/S._Chen.md"))
        .unwrap();
    assert_eq!(stub.markdown, "Merged into [[Sara_Chen|Sara Chen]]\n");
    assert_eq!(stub.source_atoms, vec![same_as.clone()]);

    let w = renderer
        .render(&req("people/by-name/S/Sara_Chen.md"))
        .unwrap();
    assert!(
        w.markdown.contains("merged: [[S._Chen|S. Chen]]"),
        "{}",
        w.markdown
    );
    assert!(
        w.markdown
            .contains("- met at picnic\n- runs the greenhouse\n"),
        "winner first, then the loser's new items: {}",
        w.markdown
    );
    assert_eq!(
        w.markdown.matches("met at picnic").count(),
        1,
        "no duplicate items"
    );
    assert!(w.source_atoms.contains(&winner_hash) && w.source_atoms.contains(&loser_hash));

    // Undo: supersede the same_as atom with a valid_to; the fold is gone.
    let undo = AtomTemplate {
        v: 1,
        entity: loser.clone(),
        predicate: PredicateName::new("entity.same_as"),
        claim: serde_json::json!({"target": winner.as_str()}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: Some(Iso8601::new("2026-02-05T00:00:00Z").unwrap()),
        tx_time: Iso8601::new("2026-02-05T00:00:00Z").unwrap(),
        classification: Tier::new("existence"),
        supersedes: Some(same_as),
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&undo).unwrap();
    let w2 = renderer
        .render(&req("people/by-name/S/Sara_Chen.md"))
        .unwrap();
    assert!(!w2.markdown.contains("merged:"));
    assert!(!w2.markdown.contains("runs the greenhouse"));
    let l2 = renderer
        .render(&req("people/by-name/S/S._Chen.md"))
        .unwrap();
    assert!(
        l2.markdown.contains("display_name: S. Chen"),
        "loser renders on its own again"
    );
}
