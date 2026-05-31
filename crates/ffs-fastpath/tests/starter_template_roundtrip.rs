//! Round-trip: render a canonical atom through the starter Tera
//! template, edit the rendered markdown, and verify the fast-path
//! classifier maps the edit back to the right reverse-map rule
//! (per ADR-014). This is the "templates align with reverse-map"
//! integration check the task_21 spec calls out.

use std::path::PathBuf;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::{EditKind, SpecRegistry};
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{AtomTemplate, EntityId, Iso8601, PredicateName, PublicKey, Tier};
use ffs_fastpath::classifier::{Classification, classify};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[151u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn setup() -> (Arc<dyn AtomStore>, Arc<SpecRegistry>, ProjectionRenderer) {
    let predicates_dir = repo_root().join("starter").join("predicates");
    let templates_dir = repo_root().join("starter").join("templates");
    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
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
    let renderer =
        ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap();
    (store, registry, renderer)
}

fn insert_contact(store: &dyn AtomStore, entity: &str, claim: serde_json::Value) {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim,
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new("2026-05-27T08:00:00Z").unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap();
}

fn render(renderer: &ProjectionRenderer, path: &str) -> String {
    renderer
        .render(&ProjectionRequest {
            path: path.into(),
            as_of: None,
            agent: owner_pk(),
        })
        .unwrap()
        .markdown
}

#[test]
fn frontmatter_edit_on_rendered_contact_classifies_as_single_line_text() {
    let (store, registry, renderer) = setup();
    let claim = serde_json::json!({
        "display_name": "Sara Chen",
        "work_email": "sara@example.com",
    });
    insert_contact(&*store, "Sara_Chen", claim.clone());
    let old_md = render(&renderer, "contacts/by-name/S/Sara_Chen.md");
    // User edits the work_email frontmatter value.
    let new_md = old_md.replace(
        "work_email: sara@example.com",
        "work_email: sara@newjob.example",
    );
    assert_ne!(old_md, new_md, "edit should change the markdown");
    let spec = registry.get("contact.person").unwrap();
    let result = classify(&spec, &claim, &old_md, &new_md);
    match result {
        Classification::Applied {
            edit_kind,
            rule_output,
            modified_claim,
        } => {
            assert_eq!(edit_kind, EditKind::SingleLineText);
            assert_eq!(rule_output, "frontmatter.work_email");
            assert_eq!(modified_claim["work_email"], "sara@newjob.example");
        }
        other => panic!("expected Applied; got {other:?}"),
    }
}

#[test]
fn appended_notes_bullet_classifies_as_additive_section() {
    let (store, registry, renderer) = setup();
    let claim = serde_json::json!({
        "display_name": "Sara Chen",
        "work_email": "sara@example.com",
        "notes": ["met at conference"],
    });
    insert_contact(&*store, "Sara_Chen", claim.clone());
    let old_md = render(&renderer, "contacts/by-name/S/Sara_Chen.md");
    // User appends a new bullet to the Notes section.
    let new_md = old_md.replace(
        "- met at conference\n",
        "- met at conference\n- followed up by email\n",
    );
    assert_ne!(old_md, new_md);
    let spec = registry.get("contact.person").unwrap();
    let result = classify(&spec, &claim, &old_md, &new_md);
    match result {
        Classification::Applied {
            edit_kind,
            rule_output,
            modified_claim,
        } => {
            assert_eq!(edit_kind, EditKind::AdditiveSection);
            assert_eq!(rule_output, "section.Notes.list_item");
            let notes = modified_claim["notes"].as_array().unwrap();
            assert_eq!(notes.len(), 2);
            assert_eq!(notes[1], "followed up by email");
        }
        other => panic!("expected Applied; got {other:?}"),
    }
}

#[test]
fn tier_frontmatter_change_classifies_as_frontmatter_value() {
    let (store, registry, renderer) = setup();
    let claim = serde_json::json!({
        "display_name": "Sara",
        "tier": "introducible",
    });
    insert_contact(&*store, "Sara", claim.clone());
    let old_md = render(&renderer, "contacts/by-name/S/Sara.md");
    let new_md = old_md.replace("tier: introducible", "tier: discreet");
    let spec = registry.get("contact.person").unwrap();
    let result = classify(&spec, &claim, &old_md, &new_md);
    match result {
        Classification::Applied {
            edit_kind,
            rule_output,
            modified_claim,
        } => {
            // Per starter/predicates/contact.person.toml, the tier
            // reverse-map declares edit_kind = "frontmatter_value".
            assert_eq!(edit_kind, EditKind::FrontmatterValue);
            assert_eq!(rule_output, "frontmatter.tier");
            assert_eq!(modified_claim["tier"], "discreet");
        }
        other => panic!("expected Applied; got {other:?}"),
    }
}
