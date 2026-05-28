//! Integration test for the starter predicate-spec library (task_20).
//!
//! Loads `starter/predicates/` via the real `SpecRegistry::load_dir`
//! and verifies the substrate's MVP vocabulary (per ADR-011) is
//! discoverable, validatable, and reverse-map-complete for the
//! fast-path classifier's three edit categories (per ADR-014).

use std::collections::HashSet;
use std::path::PathBuf;

use ffs_core::predicate::{EditKind, SpecRegistry};

const EXPECTED_PREDICATES: &[&str] = &["contact.person", "person.generic", "note"];

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
fn all_three_starter_specs_load_cleanly() {
    let registry = load_starter();
    let mut names = registry.names();
    names.sort();
    let mut expected: Vec<String> = EXPECTED_PREDICATES.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(names, expected, "expected the three MVP predicates");
}

#[test]
fn each_starter_spec_covers_all_three_edit_kinds() {
    let registry = load_starter();
    for predicate in EXPECTED_PREDICATES {
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
fn total_reverse_map_rule_count_is_within_adr_014_envelope() {
    let registry = load_starter();
    let total: usize = EXPECTED_PREDICATES
        .iter()
        .map(|name| registry.get(name).unwrap().reverse_map.len())
        .sum();
    assert!(
        (15..=25).contains(&total),
        "starter library has {total} reverse-map rules; ADR-014 envelopes the count at 15-25"
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
                let section = rest.trim_end_matches(".list_item");
                spec.rendering
                    .additive_sections
                    .iter()
                    .any(|s| s == section)
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
    let registry = load_starter();
    for predicate in EXPECTED_PREDICATES {
        let spec = registry.get(predicate).unwrap();
        assert!(
            spec.pagination.is_some(),
            "{predicate} should declare a pagination strategy"
        );
    }
}
