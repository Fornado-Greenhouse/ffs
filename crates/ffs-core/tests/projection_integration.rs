//! Integration tests for the projection renderer: single-entity render
//! matches spec, capability filtering hides entities from listings,
//! render_hash stability + change-detection, reverse-map annotations
//! point at the correct atom field, alphabetical and recency listing
//! semantics, end-to-end golden output, and the 500ms-per-100-atoms
//! perf budget.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest, RenderError};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{
    AtomEnvelope, AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier,
};
use tempfile::TempDir;

const CONTACT_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
"$schema" = "https://json-schema.org/draft/2020-12/schema"
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }
work_email = { type = "string" }
notes = { type = "array", items = { type = "string" } }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name", "work_email"]
body_sections = ["Notes"]
additive_sections = ["Notes"]

[[reverse_map]]
output = "frontmatter.display_name"
atom_field = "claim.display_name"
edit_kind = "single_line_text"

[[reverse_map]]
output = "frontmatter.work_email"
atom_field = "claim.work_email"
edit_kind = "single_line_text"

[[reverse_map]]
output = "section.Notes.list_item"
atom_field = "claim.notes[]"
edit_kind = "additive_section"
"#;

const NOTE_TOML: &str = r#"
name = "note"
version = 1

[claim_schema]
type = "object"
required = ["body"]

[claim_schema.properties]
title = { type = "string" }
body = { type = "string" }

[rendering]
template = "note.md.tera"
frontmatter_fields = ["title"]
"#;

const CONTACT_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
{% if claim.work_email %}work_email: {{ claim.work_email }}
{% endif %}---
{% if claim.notes %}
## Notes
{% for note in claim.notes %}- {{ note }}
{% endfor %}{% endif %}
"#;

const NOTE_TEMPLATE: &str = r#"---
{% if claim.title %}title: {{ claim.title }}
{% endif %}---

{{ claim.body }}
"#;

fn agent_key() -> SigningKey {
    SigningKey::from_bytes(&[11u8; 32])
}

fn agent_pk() -> PublicKey {
    PublicKey::from_verifying(&agent_key().verifying_key())
}

fn grantor_key() -> SigningKey {
    SigningKey::from_bytes(&[22u8; 32])
}

struct Harness {
    _dir: TempDir, // kept to extend lifetime of tempfiles
    store: Arc<dyn AtomStore>,
    renderer: ProjectionRenderer,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();

    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(predicates_dir.join("note.toml"), NOTE_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();
    std::fs::write(templates_dir.join("note.md.tera"), NOTE_TEMPLATE).unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();

    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let renderer = ProjectionRenderer::new(store.clone(), registry, &templates_dir).unwrap();

    Harness {
        _dir: dir,
        store,
        renderer,
    }
}

fn grant_full_capability(store: &dyn AtomStore, grantee: &PublicKey) -> Multihash {
    let cap = build_capability_atom(
        &grantor_key(),
        grantee.clone(),
        vec![
            Action::Read,
            Action::Write,
            Action::Supersede,
            Action::Erase,
            Action::Classify,
            Action::Federate,
        ],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap()
}

fn insert_contact(
    store: &dyn AtomStore,
    entity: &str,
    display_name: &str,
    work_email: Option<&str>,
    notes: &[&str],
    tx_time: &str,
) -> Multihash {
    let mut claim = serde_json::json!({"display_name": display_name});
    if let Some(email) = work_email {
        claim["work_email"] = serde_json::Value::String(email.into());
    }
    if !notes.is_empty() {
        claim["notes"] = serde_json::Value::Array(
            notes
                .iter()
                .map(|n| serde_json::Value::String((*n).into()))
                .collect(),
        );
    }
    let env: AtomEnvelope = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim,
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&grantor_key())
    .unwrap();
    store.insert(&env).unwrap()
}

fn insert_note(store: &dyn AtomStore, entity: &str, body: &str, tx_time: &str) -> Multihash {
    let env: AtomEnvelope = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("note"),
        claim: serde_json::json!({"body": body}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("notes"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&grantor_key())
    .unwrap();
    store.insert(&env).unwrap()
}

fn req(path: &str) -> ProjectionRequest {
    ProjectionRequest {
        path: path.into(),
        as_of: Some(Iso8601::new("2026-12-31T23:59:59Z").unwrap()),
        agent: agent_pk(),
    }
}

// ----- spec tests -----

#[test]
fn single_entity_render_matches_spec_frontmatter_and_body() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    let atom_hash = insert_contact(
        &*h.store,
        "Sarah_Chen",
        "Sarah Chen",
        Some("sarah@acme.com"),
        &["Met at conference", "Distributed-systems person"],
        "2026-05-25T10:00:00Z",
    );

    let resp = h
        .renderer
        .render(&req("contacts/by-name/S/Sarah_Chen.md"))
        .unwrap();

    assert!(
        resp.markdown.contains("display_name: Sarah Chen"),
        "missing display_name; got:\n{}",
        resp.markdown
    );
    assert!(
        resp.markdown.contains("work_email: sarah@acme.com"),
        "missing work_email; got:\n{}",
        resp.markdown
    );
    assert!(
        resp.markdown.contains("## Notes"),
        "missing Notes section; got:\n{}",
        resp.markdown
    );
    assert!(
        resp.markdown.contains("- Met at conference"),
        "missing note bullet; got:\n{}",
        resp.markdown
    );
    assert_eq!(resp.source_atoms, vec![atom_hash]);
}

#[test]
fn entity_hidden_by_capability_filtering_does_not_appear_in_listing() {
    let h = setup();
    // Grant access only to entity "Alice" (not Bob).
    let cap = build_capability_atom(
        &grantor_key(),
        agent_pk(),
        vec![Action::Read],
        CapabilityScope {
            entities: Some(vec![EntityId::new("Alice")]),
            ..Default::default()
        },
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    h.store.insert(&cap).unwrap();

    insert_contact(
        &*h.store,
        "Alice",
        "Alice",
        None,
        &[],
        "2026-05-25T09:00:00Z",
    );
    insert_contact(&*h.store, "Bob", "Bob", None, &[], "2026-05-25T10:00:00Z");

    let resp = h.renderer.render(&req("contacts/recent/")).unwrap();
    assert!(resp.markdown.contains("Alice"), "expected Alice in listing");
    assert!(
        !resp.markdown.contains("Bob"),
        "Bob should be hidden by capability scope; got:\n{}",
        resp.markdown
    );
}

#[test]
fn capability_denial_on_single_entity_returns_typed_error() {
    let h = setup();
    // No capability granted at all.
    insert_contact(
        &*h.store,
        "Sarah_Chen",
        "Sarah Chen",
        None,
        &[],
        "2026-05-25T10:00:00Z",
    );
    let err = h
        .renderer
        .render(&req("contacts/by-name/S/Sarah_Chen.md"))
        .unwrap_err();
    assert!(
        matches!(err, RenderError::CapabilityDenied(_)),
        "expected CapabilityDenied, got {err:?}"
    );
}

#[test]
fn two_consecutive_renders_produce_identical_render_hash() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    insert_contact(
        &*h.store,
        "Sarah_Chen",
        "Sarah Chen",
        None,
        &[],
        "2026-05-25T10:00:00Z",
    );

    let r1 = h
        .renderer
        .render(&req("contacts/by-name/S/Sarah_Chen.md"))
        .unwrap();
    let r2 = h
        .renderer
        .render(&req("contacts/by-name/S/Sarah_Chen.md"))
        .unwrap();
    assert_eq!(r1.render_hash, r2.render_hash);
    assert_eq!(r1.markdown, r2.markdown);
}

#[test]
fn atom_insert_between_renders_changes_render_hash_for_listing() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    insert_contact(
        &*h.store,
        "Alice",
        "Alice",
        None,
        &[],
        "2026-05-25T09:00:00Z",
    );
    let r1 = h.renderer.render(&req("contacts/recent/")).unwrap();

    insert_contact(&*h.store, "Bob", "Bob", None, &[], "2026-05-25T10:00:00Z");
    let r2 = h.renderer.render(&req("contacts/recent/")).unwrap();

    assert_ne!(
        r1.render_hash, r2.render_hash,
        "render_hash should change after a new contact is inserted"
    );
}

#[test]
fn reverse_map_annotations_match_spec_rules() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    let atom_hash = insert_contact(
        &*h.store,
        "Sarah_Chen",
        "Sarah Chen",
        Some("sarah@acme.com"),
        &["a note"],
        "2026-05-25T10:00:00Z",
    );

    let resp = h
        .renderer
        .render(&req("contacts/by-name/S/Sarah_Chen.md"))
        .unwrap();
    assert_eq!(
        resp.reverse_map.len(),
        3,
        "expected 3 reverse-map annotations (per the spec)"
    );
    let outputs: Vec<&str> = resp
        .reverse_map
        .iter()
        .map(|a| a.output_element.as_str())
        .collect();
    assert!(outputs.contains(&"frontmatter.display_name"));
    assert!(outputs.contains(&"frontmatter.work_email"));
    assert!(outputs.contains(&"section.Notes.list_item"));
    for ann in &resp.reverse_map {
        assert_eq!(
            ann.source_atom, atom_hash,
            "all annotations should point at the source atom"
        );
        assert!(
            ann.source_field.starts_with("claim."),
            "source_field should be claim-rooted; got {}",
            ann.source_field
        );
    }
}

#[test]
fn alphabetical_letter_listing_filters_by_first_letter() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    insert_contact(
        &*h.store,
        "Sarah_Chen",
        "Sarah Chen",
        None,
        &[],
        "2026-05-25T08:00:00Z",
    );
    insert_contact(
        &*h.store,
        "Sam_Smith",
        "Sam Smith",
        None,
        &[],
        "2026-05-25T09:00:00Z",
    );
    insert_contact(
        &*h.store,
        "Bob_Brown",
        "Bob Brown",
        None,
        &[],
        "2026-05-25T10:00:00Z",
    );

    let resp = h.renderer.render(&req("contacts/by-name/S/")).unwrap();
    assert!(
        resp.markdown.contains("Sarah_Chen") && resp.markdown.contains("Sam_Smith"),
        "expected Sarah and Sam in listing; got:\n{}",
        resp.markdown
    );
    assert!(
        !resp.markdown.contains("Bob_Brown"),
        "Bob should not appear under S; got:\n{}",
        resp.markdown
    );
}

#[test]
fn recent_listing_returns_entities_in_tx_time_desc() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    insert_contact(
        &*h.store,
        "Alice",
        "Alice",
        None,
        &[],
        "2026-05-25T08:00:00Z",
    );
    insert_contact(&*h.store, "Bob", "Bob", None, &[], "2026-05-25T09:00:00Z");
    insert_contact(
        &*h.store,
        "Carol",
        "Carol",
        None,
        &[],
        "2026-05-25T10:00:00Z",
    );

    let resp = h.renderer.render(&req("contacts/recent/")).unwrap();
    let alice_pos = resp.markdown.find("Alice").unwrap();
    let bob_pos = resp.markdown.find("Bob").unwrap();
    let carol_pos = resp.markdown.find("Carol").unwrap();
    assert!(
        carol_pos < bob_pos && bob_pos < alice_pos,
        "expected DESC tx_time ordering (Carol < Bob < Alice positions); got:\n{}",
        resp.markdown
    );
}

#[test]
fn end_to_end_golden_render_matches() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    insert_contact(
        &*h.store,
        "Sarah_Chen",
        "Sarah Chen",
        Some("sarah@acme.com"),
        &["distributed systems"],
        "2026-05-25T10:00:00Z",
    );

    let resp = h
        .renderer
        .render(&req("contacts/by-name/S/Sarah_Chen.md"))
        .unwrap();

    // Trim and normalize so minor template-whitespace tweaks don't break golden.
    let lines: Vec<&str> = resp
        .markdown
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        lines,
        vec![
            "---",
            "display_name: Sarah Chen",
            "work_email: sarah@acme.com",
            "---",
            "## Notes",
            "- distributed systems",
        ],
        "golden mismatch; full output:\n{}",
        resp.markdown
    );
}

#[test]
fn notes_recent_with_one_hundred_atoms_renders_under_500ms() {
    let h = setup();
    grant_full_capability(&*h.store, &agent_pk());
    for i in 0..100u32 {
        insert_note(
            &*h.store,
            &format!("note-{i:03}"),
            &format!("entry body {i}"),
            &format!("2026-05-25T{:02}:{:02}:00Z", i / 60, i % 60),
        );
    }
    let start = std::time::Instant::now();
    let resp = h.renderer.render(&req("notes/recent/")).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(resp.source_atoms.len(), 100, "expected 100 source atoms");
    // PRD budget: < 500ms. Debug builds are slower; relax there per CLAUDE.md.
    let budget = if cfg!(debug_assertions) {
        std::time::Duration::from_secs(5)
    } else {
        std::time::Duration::from_millis(500)
    };
    assert!(
        elapsed < budget,
        "100-atom notes/recent/ render took {elapsed:?}, expected < {budget:?} (debug_assertions={})",
        cfg!(debug_assertions)
    );
}

#[test]
fn unsupported_subpath_returns_typed_error() {
    let h = setup();
    let err = h
        .renderer
        .render(&req("contacts/by-org/AcmeCorp/"))
        .unwrap_err();
    assert!(matches!(err, RenderError::UnsupportedSubpath(_)));
}

#[test]
fn missing_predicate_in_registry_returns_unknown_predicate() {
    // Stand up a renderer with NO predicate specs loaded.
    let dir = tempfile::tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();
    let registry = Arc::new(SpecRegistry::new());
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let renderer = ProjectionRenderer::new(store.clone(), registry, &templates_dir).unwrap();
    grant_full_capability(&*store, &agent_pk());
    insert_contact(
        &*store,
        "Sarah_Chen",
        "Sarah Chen",
        None,
        &[],
        "2026-05-25T10:00:00Z",
    );

    let req = ProjectionRequest {
        path: "contacts/by-name/S/Sarah_Chen.md".into(),
        as_of: Some(Iso8601::new("2026-12-31T23:59:59Z").unwrap()),
        agent: agent_pk(),
    };
    let err = renderer.render(&req).unwrap_err();
    assert!(matches!(err, RenderError::UnknownPredicate(_)));
}
