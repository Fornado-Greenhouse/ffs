//! Integration test for the starter predicate-spec library (task_20).
//!
//! Loads `starter/predicates/` via the real `SpecRegistry::load_dir`
//! and verifies the substrate's MVP vocabulary (per ADR-011) is
//! discoverable, validatable, and reverse-map-complete for the
//! fast-path classifier's three edit categories (per ADR-014).

use std::collections::HashSet;
use std::path::PathBuf;

use ffs_core::predicate::{EditKind, SpecRegistry};

/// The original three MVP predicates (ADR-011). Every one covers all
/// three fast-path edit kinds and their rules sum inside the ADR-014
/// envelope.
const MVP_THREE: &[&str] = &["contact.person", "person.generic", "note"];

/// The full starter set after task_38 (ADR-028, ADR-030, ADR-031).
const EXPECTED_PREDICATES: &[&str] = &[
    "affiliation",
    "contact.person",
    "entity.different_from",
    "entity.same_as",
    "event.business",
    "note",
    "org.company",
    "person.generic",
    "source.article",
];

fn starter_dir() -> PathBuf {
    // Tests live at crates/ffs-core/tests; starter/predicates is two
    // levels up + into starter/predicates.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p.push("starter");
    p.push("predicates");
    p
}

fn load_starter() -> SpecRegistry {
    let registry = SpecRegistry::new();
    registry
        .load_dir(&starter_dir())
        .expect("starter predicate specs must load without error");
    registry
}

#[test]
fn all_starter_specs_load_cleanly() {
    let registry = load_starter();
    let mut names = registry.names();
    names.sort();
    let mut expected: Vec<String> = EXPECTED_PREDICATES.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(names, expected, "expected the nine starter predicates");
}

#[test]
fn family_table_declares_the_folders_and_only_the_folders() {
    let registry = load_starter();
    let table: Vec<(String, String, String)> = registry
        .families()
        .into_iter()
        .map(|f| (f.family, f.predicate, f.name_field))
        .collect();
    assert_eq!(
        table,
        vec![
            ("articles".into(), "source.article".into(), "title".into()),
            (
                "contacts".into(),
                "contact.person".into(),
                "display_name".into()
            ),
            ("events".into(), "event.business".into(), "title".into()),
            ("notes".into(), "note".into(), "title".into()),
            ("orgs".into(), "org.company".into(), "display_name".into()),
            (
                "people".into(),
                "person.generic".into(),
                "display_name".into()
            ),
        ]
    );
    for folderless in ["affiliation", "entity.same_as", "entity.different_from"] {
        assert!(
            registry.get(folderless).unwrap().path.is_none(),
            "{folderless} has no folder"
        );
    }
}

#[test]
fn every_starter_spec_carries_an_ontology_row() {
    let registry = load_starter();
    for name in EXPECTED_PREDICATES {
        let spec = registry.get(name).unwrap();
        let ont = spec
            .ontology
            .as_ref()
            .unwrap_or_else(|| panic!("{name} lacks [ontology]"));
        assert!(!ont.bfo.trim().is_empty(), "{name} has an empty bfo value");
    }
}

#[test]
fn person_generic_v1_claim_validates_under_v2_spec() {
    let registry = load_starter();
    assert_eq!(registry.get("person.generic").unwrap().version, 2);
    let v1_claim = serde_json::json!({
        "display_name": "Alex Kim",
        "role": "engineer",
        "team": "platform",
        "bio": ["joined 2024"],
    });
    registry
        .validate_claim("person.generic", &v1_claim)
        .expect("a v1 person.generic claim must validate under v2 (additive change)");
}

#[test]
fn each_mvp_spec_covers_all_three_edit_kinds() {
    let registry = load_starter();
    for predicate in MVP_THREE {
        let spec = registry
            .get(predicate)
            .unwrap_or_else(|| panic!("predicate {predicate} should load"));
        let kinds: HashSet<EditKind> = spec.reverse_map.iter().map(|r| r.edit_kind).collect();
        // ADR-014: the fast-path classifier supports three categories.
        // The MVP starter library must cover all three on every
        // predicate so any edit category is fast-path-eligible
        // out-of-the-box.
        assert!(
            kinds.contains(&EditKind::SingleLineText),
            "{predicate} missing single_line_text rule; got {kinds:?}"
        );
        assert!(
            kinds.contains(&EditKind::FrontmatterValue),
            "{predicate} missing frontmatter_value rule; got {kinds:?}"
        );
        assert!(
            kinds.contains(&EditKind::AdditiveSection),
            "{predicate} missing additive_section rule; got {kinds:?}"
        );
    }
}

#[test]
fn mvp_three_reverse_map_rule_count_is_within_adr_014_envelope() {
    let registry = load_starter();
    let total: usize = MVP_THREE
        .iter()
        .map(|name| registry.get(name).unwrap().reverse_map.len())
        .sum();
    assert!(
        (15..=25).contains(&total),
        "the original three specs have {total} reverse-map rules; ADR-014 envelopes the count at 15-25"
    );
}

#[test]
fn full_starter_reverse_map_rule_count_is_recorded() {
    // task_38 added six specs; the total is tracked here so a spec
    // edit that silently drops rules is noticed in review.
    let registry = load_starter();
    let total: usize = EXPECTED_PREDICATES
        .iter()
        .map(|name| registry.get(name).unwrap().reverse_map.len())
        .sum();
    assert_eq!(
        total, 55,
        "expected 55 reverse-map rules across the nine starter specs"
    );
}

#[test]
fn contact_person_validates_canonical_claim() {
    let registry = load_starter();
    let claim = serde_json::json!({
        "display_name": "Sara Chen",
        "work_email": "sara@example.com",
        "phone": "+1-555-0101",
        "organization": "Foley Greenhouse",
        "tier": "introducible",
        "notes": ["met at the gardening conference", "passionate about heirloom tomatoes"],
        "tags": ["plants", "open-source"],
    });
    registry
        .validate_claim("contact.person", &claim)
        .expect("canonical contact.person claim must validate");
}

#[test]
fn contact_person_rejects_unknown_tier_enum_value() {
    let registry = load_starter();
    let claim = serde_json::json!({
        "display_name": "Sara Chen",
        "tier": "rogue-classification",
    });
    let err = registry
        .validate_claim("contact.person", &claim)
        .expect_err("rogue tier should fail enum check");
    assert!(
        err.to_string().contains("rogue-classification") || err.to_string().contains("enum"),
        "got: {err}"
    );
}

#[test]
fn person_generic_requires_display_name() {
    let registry = load_starter();
    let no_name = serde_json::json!({"role": "engineer"});
    assert!(
        registry.validate_claim("person.generic", &no_name).is_err(),
        "person.generic without display_name should fail required check"
    );
    let with_name = serde_json::json!({"display_name": "Alex Kim", "role": "engineer"});
    registry
        .validate_claim("person.generic", &with_name)
        .expect("person.generic with display_name should validate");
}

#[test]
fn note_validates_with_status_enum_and_rejects_invalid_status() {
    let registry = load_starter();
    let ok = serde_json::json!({"title": "tuesday standup", "status": "draft"});
    registry
        .validate_claim("note", &ok)
        .expect("status=draft is valid");

    let bad = serde_json::json!({"title": "tuesday standup", "status": "wip"});
    let err = registry
        .validate_claim("note", &bad)
        .expect_err("status=wip should fail enum check");
    assert!(err.to_string().contains("wip") || err.to_string().contains("enum"));
}

#[test]
fn every_reverse_map_output_resolves_to_a_defined_rendering_element() {
    // The loader runs this check internally and would have errored
    // on `load_dir` above if a rule pointed at an undefined output.
    // Re-asserting it here keeps the property visible in the test
    // suite and guards against a future change to `load_dir`'s
    // validation strictness.
    let registry = load_starter();
    for predicate in EXPECTED_PREDICATES {
        let spec = registry.get(predicate).unwrap();
        for rule in &spec.reverse_map {
            let resolves = if let Some(field) = rule.output.strip_prefix("frontmatter.") {
                spec.rendering.frontmatter_fields.iter().any(|f| f == field)
            } else if let Some(rest) = rule.output.strip_prefix("section.") {
                // `section.X.list_item` must be an additive section;
                // `section.X` (a whole-section edit) may be any body
                // section, matching the loader's own rule.
                if let Some(section) = rest.strip_suffix(".list_item") {
                    spec.rendering
                        .additive_sections
                        .iter()
                        .any(|s| s == section)
                } else {
                    spec.rendering.body_sections.iter().any(|s| s == rest)
                        || spec.rendering.additive_sections.iter().any(|s| s == rest)
                }
            } else {
                false
            };
            assert!(
                resolves,
                "{predicate}: reverse-map output {} does not resolve",
                rule.output
            );
        }
    }
}

#[test]
fn pagination_is_set_for_every_starter_predicate() {
    // The Obsidian plugin's listing UX depends on each predicate
    // exposing a pagination strategy — missing it falls back to
    // "no listing" which is a regression.
    // Only predicates with a projection folder have listings; the
    // folderless identity and role predicates (affiliation,
    // entity.same_as, entity.different_from) render inside other files.
    let registry = load_starter();
    for predicate in EXPECTED_PREDICATES {
        let spec = registry.get(predicate).unwrap();
        if spec.path.is_none() {
            continue;
        }
        assert!(
            spec.pagination.is_some(),
            "{predicate} should declare a pagination strategy"
        );
    }
}
