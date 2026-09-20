//! task_38: the fast-path classifier recognizes the new additive
//! sections through the starter specs' reverse-map rules with no
//! classifier code change, and edits inside reverse-lookup sections
//! (`## Affiliations`, `## People`, `## Participants`) route to ingest.

use std::path::PathBuf;

use ffs_core::predicate::{EditKind, SpecRegistry};
use ffs_fastpath::{Classification, classify};

fn starter_registry() -> SpecRegistry {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    let registry = SpecRegistry::new();
    registry.load_dir(&p.join("starter/predicates")).unwrap();
    registry
}

fn assert_additive(predicate: &str, head: serde_json::Value, old: &str, new: &str, field: &str) {
    let spec = starter_registry().get(predicate).unwrap();
    match classify(&spec, &head, old, new) {
        Classification::Applied {
            edit_kind,
            modified_claim,
            ..
        } => {
            assert_eq!(edit_kind, EditKind::AdditiveSection, "{predicate}");
            assert_eq!(
                modified_claim[field].as_array().unwrap().len(),
                2,
                "{predicate}"
            );
        }
        other => panic!("{predicate}: expected Applied additive_section, got {other:?}"),
    }
}

#[test]
fn org_notes_bullet_is_additive() {
    assert_additive(
        "org.company",
        serde_json::json!({"display_name": "Acme", "notes": ["one"]}),
        "---\ndisplay_name: Acme\n---\n\n## Notes\n- one\n",
        "---\ndisplay_name: Acme\n---\n\n## Notes\n- one\n- two\n",
        "notes",
    );
}

#[test]
fn person_mentions_bullet_is_additive() {
    assert_additive(
        "person.generic",
        serde_json::json!({"display_name": "Sara Chen", "mentions": ["2026-09-01: a"]}),
        "---\ndisplay_name: Sara Chen\n---\n\n## Mentions\n- 2026-09-01: a\n",
        "---\ndisplay_name: Sara Chen\n---\n\n## Mentions\n- 2026-09-01: a\n- 2026-09-02: b\n",
        "mentions",
    );
}

#[test]
fn article_tags_bullet_is_additive() {
    assert_additive(
        "source.article",
        serde_json::json!({"title": "T", "url": "https://example.com/t", "tags": ["a"]}),
        "---\ntitle: T\n---\n\n## Tags\n- a\n",
        "---\ntitle: T\n---\n\n## Tags\n- a\n- b\n",
        "tags",
    );
}

fn assert_routes_to_ingest(predicate: &str, section: &str) {
    let spec = starter_registry().get(predicate).unwrap();
    let head = serde_json::json!({"display_name": "X", "title": "X", "url": "https://example.com/x", "kind": "funding"});
    let old =
        format!("---\ndisplay_name: X\n---\n\n## {section}\n- [[A|A]]: CEO (2024 to present)\n");
    let new = format!(
        "---\ndisplay_name: X\n---\n\n## {section}\n- [[A|A]]: CEO (2024 to present)\n- [[B|B]]: CFO (2025 to present)\n"
    );
    match classify(&spec, &head, &old, &new) {
        Classification::RoutedToIngest { .. } => {}
        other => panic!("{predicate} / {section}: expected route to ingest, got {other:?}"),
    }
}

#[test]
fn reverse_lookup_sections_route_to_ingest() {
    assert_routes_to_ingest("person.generic", "Affiliations");
    assert_routes_to_ingest("contact.person", "Affiliations");
    assert_routes_to_ingest("org.company", "People");
    assert_routes_to_ingest("event.business", "Participants");
}
