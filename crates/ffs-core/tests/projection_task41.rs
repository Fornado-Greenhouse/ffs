//! task_41: the `briefings/` flat path family and the briefing
//! template's `[[target|display]]` wikilinks, rendered through the real
//! starter specs and templates with deep entity-reference resolution.

use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::{PathLayout, SpecRegistry};
use ffs_core::projection::path::parse;
use ffs_core::projection::path_for_basename;
use ffs_core::projection::{FamilyTable, ParsedPath, ProjectionRenderer, ProjectionRequest};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, PathIndex, PredicateName, PublicKey, Tier,
};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[41u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn starter_registry() -> Arc<SpecRegistry> {
    let registry = Arc::new(SpecRegistry::new());
    registry
        .load_dir(&repo_root().join("starter").join("predicates"))
        .unwrap();
    registry
}

#[test]
fn the_briefings_family_is_flat_and_files_by_date() {
    let table = FamilyTable::from_registry(&starter_registry());
    let briefings = table.for_folder("briefings").expect("briefings family");
    assert_eq!(briefings.layout, PathLayout::Flat);
    assert_eq!(briefings.name_field, "date");
    assert_eq!(
        path_for_basename(&briefings, "2026-09-20").as_deref(),
        Some("briefings/2026-09-20.md"),
        "a date basename has a flat destination"
    );
    let contacts = table.for_folder("contacts").unwrap();
    assert_eq!(contacts.layout, PathLayout::ByName);
    assert_eq!(
        path_for_basename(&contacts, "2026-09-20"),
        None,
        "a by-name family still has no home for a leading digit"
    );
    assert_eq!(
        parse("briefings/2026-09-20.md", &table).unwrap(),
        ParsedPath::SingleEntity {
            family: briefings.clone(),
            basename: "2026-09-20".into()
        }
    );
    assert_eq!(
        parse("briefings/recent/", &table).unwrap(),
        ParsedPath::Recent { family: briefings }
    );
    assert!(
        matches!(
            parse("contacts/Sara_Chen.md", &table).unwrap(),
            ParsedPath::Unsupported { .. }
        ),
        "a by-name family does not accept the flat shape"
    );
}

#[test]
fn briefing_template_renders_all_sections_with_target_display_wikilinks() {
    let registry = starter_registry();
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
    let index = InMemoryPathIndex::new_arc();
    let renderer = ProjectionRenderer::new(
        store.clone(),
        registry,
        &repo_root().join("starter").join("templates"),
    )
    .unwrap()
    .with_path_index(index.clone());

    let insert = |entity: &EntityId, predicate: &str, claim: serde_json::Value| {
        let atom = AtomTemplate {
            v: 1,
            entity: entity.clone(),
            predicate: PredicateName::new(predicate),
            claim,
            valid_from: Iso8601::new("2026-09-01T00:00:00Z").unwrap(),
            valid_to: None,
            tx_time: Iso8601::new("2026-09-15T00:00:00Z").unwrap(),
            classification: Tier::new("existence"),
            supersedes: None,
            provenance: vec![],
        }
        .sign(&owner_key())
        .unwrap();
        store.insert(&atom).unwrap();
    };

    let pat = EntityId::mint();
    let acme = EntityId::mint();
    let article = EntityId::mint();
    insert(
        &pat,
        "person.generic",
        serde_json::json!({"display_name": "Pat Example"}),
    );
    insert(
        &acme,
        "org.company",
        serde_json::json!({"display_name": "Acme Widgets"}),
    );
    insert(
        &article,
        "source.article",
        serde_json::json!({"title": "Widget maker breaks ground", "url": "https://example.com/a"}),
    );
    index.assign("people", &pat, "Pat Example", &[]).unwrap();
    index.assign("orgs", &acme, "Acme Widgets", &[]).unwrap();
    index
        .assign("articles", &article, "Widget maker breaks ground", &[])
        .unwrap();

    let briefing = EntityId::mint();
    let acme_ref = serde_json::json!({"entity": acme.as_str(), "display": "Acme Widgets"});
    let article_ref =
        serde_json::json!({"entity": article.as_str(), "display": "Widget maker breaks ground"});
    insert(
        &briefing,
        "auditor.briefing",
        serde_json::json!({
            "date": "2026-09-20",
            "cadence": "7d",
            "window": {"from": "2026-09-13T00:00:00Z", "to": "2026-09-20T07:00:00Z"},
            "narrative": "One new person, one join, one trending organization.",
            "truncated": false,
            "ceiling": 500,
            "new_people": [{"entity": pat.as_str(), "display": "Pat Example",
                            "organization": acme_ref, "first_seen_article": article_ref}],
            "changes": [{"entity": pat.as_str(), "display": "Pat Example", "kind": "joined",
                         "organization": acme_ref, "from": null, "to": "CEO",
                         "as_reported": "2026-09-14", "source_article": article_ref}],
            "trending_orgs": [{"entity": acme.as_str(), "display": "Acme Widgets",
                               "mentions_this_window": 3, "mentions_prior_window": 1}],
            "events": [{"kind": "expansion", "items": [{"entity": "zNoSuchEvent", "display": "Groundbreaking",
                        "date": "2026-09-14", "participants": [{"entity": acme.as_str(), "display": "Acme Widgets", "role": "developer"}]}]}],
            "promotion_candidates": [{"entity": pat.as_str(), "display": "Pat Example",
                                      "organization": acme_ref, "mention_count": 4, "article_count": 3,
                                      "reason": "mentioned in 3 articles"}],
            "follow_ups": [{"entity": "zNoSuchContact", "display": "Sam Contact",
                            "organization": acme_ref, "triggering_article": article_ref}],
            "needs_your_eye": [{"submission_id": "sub-1", "local_ref": "p-1", "predicate": "person.generic",
                                "display": "Pat Example", "candidates": [{"entity": pat.as_str(), "display": "Pat Example", "score": 7.5}]}],
            "possible_duplicates": [{"family": "people",
                                     "entity_a": {"entity": pat.as_str(), "display": "Pat Example"},
                                     "entity_b": {"entity": "zOther", "display": "P. Example"},
                                     "shared_aliases": ["pat example"]}],
            "recent_merges": [],
            "filing": {"auto_filed_count": 2, "reviewed_count": 5}
        }),
    );
    index
        .assign("briefings", &briefing, "2026-09-20", &[])
        .unwrap();

    let rendered = renderer
        .render(&ProjectionRequest {
            path: "briefings/2026-09-20.md".into(),
            as_of: None,
            agent: owner_pk(),
        })
        .unwrap();
    let md = rendered.markdown;

    for header in [
        "## Narrative",
        "## Needs your eye",
        "## New people",
        "## Changes",
        "## Trending organizations",
        "## Events",
        "## Promotion candidates",
        "## Follow-ups",
        "## Possible duplicates",
        "## Filing stats",
    ] {
        assert!(md.contains(header), "missing {header} in:\n{md}");
    }
    // Section order is the task's order.
    let positions: Vec<usize> = [
        "## Narrative",
        "## Needs your eye",
        "## New people",
        "## Changes",
        "## Trending organizations",
        "## Events",
        "## Promotion candidates",
        "## Follow-ups",
        "## Possible duplicates",
        "## Filing stats",
    ]
    .iter()
    .map(|h| md.find(h).unwrap())
    .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");

    // Every known entity renders as [[basename|display]], keyed by id.
    assert!(md.contains("[[Pat_Example|Pat Example]]"), "{md}");
    assert!(md.contains("[[Acme_Widgets|Acme Widgets]]"), "{md}");
    assert!(
        md.contains("[[Widget_maker_breaks_ground|Widget maker breaks ground]]"),
        "{md}"
    );
    // Unknown ids print their display as plain text, never a broken link.
    assert!(md.contains("Sam Contact"), "{md}");
    assert!(!md.contains("[[zNoSuchContact"), "{md}");
    assert!(md.contains("Groundbreaking (2026-09-14)"), "{md}");
    // The affiliation date is labelled as reported.
    assert!(md.contains("(as reported 2026-09-14)"), "{md}");
    assert!(
        md.contains("joined [[Acme_Widgets|Acme Widgets]]: CEO"),
        "{md}"
    );
    assert!(md.contains("auto-filed: 2"), "{md}");
    assert!(md.contains("reviewed: 5"), "{md}");
    assert!(
        md.starts_with("---\ndate: 2026-09-20\ncadence: 7d\n"),
        "{md}"
    );

    // A rename keeps the link alive: the index moves Pat's basename and
    // the same briefing atom re-renders with the new target.
    index
        .rename("people", &pat, "Patricia Example", &[])
        .unwrap();
    let again = renderer
        .render(&ProjectionRequest {
            path: "briefings/2026-09-20.md".into(),
            as_of: None,
            agent: owner_pk(),
        })
        .unwrap();
    assert!(
        again.markdown.contains("[[Patricia_Example|Pat Example]]"),
        "{}",
        again.markdown
    );
}
