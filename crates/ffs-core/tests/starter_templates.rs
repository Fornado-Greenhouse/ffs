//! Integration tests for the starter Tera template library
//! (task_21). Loads `starter/predicates/` + `starter/templates/` via
//! the real `ProjectionRenderer` and asserts canonical atoms render
//! to the expected markdown.
//!
//! Each test exercises one of the four required-by-spec properties:
//!
//! - canonical-atom rendering matches a golden string
//! - empty optional fields don't bleed into output
//! - rendering is deterministic (byte-identical across runs)
//! - templates align with their predicate spec's reverse-map (so
//!   the fast-path classifier round-trips a frontmatter or
//!   additive-section edit cleanly)

use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[131u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn grant_owner_full(store: &dyn AtomStore) {
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

fn setup() -> (Arc<dyn AtomStore>, ProjectionRenderer) {
    let predicates_dir = repo_root().join("starter").join("predicates");
    let templates_dir = repo_root().join("starter").join("templates");
    let registry = Arc::new(SpecRegistry::new());
    registry
        .load_dir(&predicates_dir)
        .expect("starter predicates must load");
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant_owner_full(&*store);
    let renderer =
        ProjectionRenderer::new(store.clone(), registry, &templates_dir).expect("renderer");
    (store, renderer)
}

fn insert(
    store: &dyn AtomStore,
    entity: &str,
    predicate: &str,
    classification: &str,
    claim: serde_json::Value,
    tx_time: &str,
) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new(classification),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap()
}

fn render(renderer: &ProjectionRenderer, path: &str) -> String {
    let req = ProjectionRequest {
        path: path.to_string(),
        as_of: None,
        agent: owner_pk(),
    };
    renderer.render(&req).expect("render").markdown
}

// ---------------------------------------------------------------------
// contact.person
// ---------------------------------------------------------------------

#[test]
fn contact_person_renders_canonical_atom_into_expected_markdown() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "Sara_Chen",
        "contact.person",
        "existence",
        serde_json::json!({
            "display_name": "Sara Chen",
            "work_email": "sara@example.com",
            "tier": "introducible",
            "notes": ["met at gardening conference", "passionate about heirlooms"],
            "tags": ["plants", "open-source"],
        }),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "contacts/by-name/S/Sara_Chen.md");
    let expected = "---\n\
                    display_name: Sara Chen\n\
                    work_email: sara@example.com\n\
                    tier: introducible\n\
                    ---\n\
                    \n\
                    ## Notes\n\
                    - met at gardening conference\n\
                    - passionate about heirlooms\n\
                    \n\
                    ## Tags\n\
                    - plants\n\
                    - open-source\n";
    assert_eq!(md, expected, "rendered:\n---\n{md}\n---");
}

#[test]
fn contact_person_with_no_optional_fields_omits_empty_lines_and_headers() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "Bob",
        "contact.person",
        "existence",
        serde_json::json!({"display_name": "Bob"}),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "contacts/by-name/B/Bob.md");
    let expected = "---\n\
                    display_name: Bob\n\
                    ---\n";
    assert_eq!(md, expected, "rendered:\n---\n{md}\n---");
    // No empty `## Notes` / `## Tags` headers.
    assert!(!md.contains("## Notes"));
    assert!(!md.contains("## Tags"));
    // No empty frontmatter lines like `work_email: `.
    assert!(!md.contains("work_email"));
}

#[test]
fn contact_person_render_is_deterministic() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "Alex",
        "contact.person",
        "existence",
        serde_json::json!({
            "display_name": "Alex Kim",
            "work_email": "alex@example.com",
            "notes": ["a", "b", "c"],
        }),
        "2026-05-27T08:00:00Z",
    );
    let a = render(&renderer, "contacts/by-name/A/Alex.md");
    let b = render(&renderer, "contacts/by-name/A/Alex.md");
    assert_eq!(a.as_bytes(), b.as_bytes(), "byte-identical render expected");
}

// ---------------------------------------------------------------------
// person.generic
// ---------------------------------------------------------------------

#[test]
fn person_generic_renders_canonical_atom_into_expected_markdown() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "Alex_Kim",
        "person.generic",
        "existence",
        serde_json::json!({
            "display_name": "Alex Kim",
            "role": "staff engineer",
            "team": "platform",
            "location": "Brooklyn",
            "pronouns": "they/them",
            "bio": ["joined 2023", "leads ingest reliability"],
        }),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "people/by-name/A/Alex_Kim.md");
    let expected = "---\n\
                    display_name: Alex Kim\n\
                    role: staff engineer\n\
                    team: platform\n\
                    location: Brooklyn\n\
                    pronouns: they/them\n\
                    ---\n\
                    \n\
                    ## Bio\n\
                    - joined 2023\n\
                    - leads ingest reliability\n";
    assert_eq!(md, expected, "rendered:\n---\n{md}\n---");
}

#[test]
fn person_generic_with_only_required_field_renders_minimal() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "Pat",
        "person.generic",
        "existence",
        serde_json::json!({"display_name": "Pat"}),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "people/by-name/P/Pat.md");
    let expected = "---\n\
                    display_name: Pat\n\
                    ---\n";
    assert_eq!(md, expected, "rendered:\n---\n{md}\n---");
    assert!(!md.contains("## Bio"));
}

// ---------------------------------------------------------------------
// note
// ---------------------------------------------------------------------

#[test]
fn note_renders_canonical_atom_into_expected_markdown() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "tuesday_standup",
        "note",
        "existence",
        serde_json::json!({
            "title": "tuesday standup",
            "author": "wes",
            "status": "draft",
            "body": "discussed federation rollout; need to circle back on revocation latency.",
            "tags": ["meeting", "federation"],
            "references": ["adr-020.md"],
        }),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "notes/by-name/T/tuesday_standup.md");
    let expected = "---\n\
                    title: tuesday standup\n\
                    author: wes\n\
                    status: draft\n\
                    ---\n\
                    \n\
                    ## Body\n\
                    discussed federation rollout; need to circle back on revocation latency.\n\
                    \n\
                    ## Tags\n\
                    - meeting\n\
                    - federation\n\
                    \n\
                    ## References\n\
                    - adr-020.md\n";
    assert_eq!(md, expected, "rendered:\n---\n{md}\n---");
}

#[test]
fn note_with_only_title_renders_minimal() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "stub",
        "note",
        "existence",
        serde_json::json!({"title": "stub"}),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "notes/by-name/S/stub.md");
    let expected = "---\n\
                    title: stub\n\
                    ---\n";
    assert_eq!(md, expected, "rendered:\n---\n{md}\n---");
}

// ---------------------------------------------------------------------
// Fastpath roundtrip — templates must produce shapes the classifier
// recognizes as `frontmatter_value` / `single_line_text` /
// `additive_section`. The classifier itself lives in ffs-fastpath;
// here we just assert the structural shape (frontmatter present,
// additive sections emit `- ` bullets) so the reverse-map references
// the right rendering elements.
// ---------------------------------------------------------------------

#[test]
fn templates_align_with_reverse_map_outputs() {
    let (store, renderer) = setup();
    insert(
        &*store,
        "Sara",
        "contact.person",
        "existence",
        serde_json::json!({
            "display_name": "Sara",
            "tier": "introducible",
            "notes": ["one", "two"],
        }),
        "2026-05-27T08:00:00Z",
    );
    let md = render(&renderer, "contacts/by-name/S/Sara.md");
    // Frontmatter shape (the classifier's frontmatter parser
    // walks `---\nkey: value\n---`).
    assert!(md.starts_with("---\n"), "starts with frontmatter fence");
    assert!(
        md.contains("display_name: Sara"),
        "frontmatter line for display_name present"
    );
    assert!(
        md.contains("tier: introducible"),
        "frontmatter line for tier (constrained vocab) present"
    );
    // Additive-section shape (the classifier requires `## Name\n`
    // followed by `- item` bullets).
    assert!(md.contains("\n## Notes\n"), "Notes section header present");
    assert!(md.contains("\n- one\n"), "first bullet present");
    assert!(md.contains("\n- two\n"), "second bullet present");
}
