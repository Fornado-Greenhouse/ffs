//! task_38 (ADR-028, ADR-030, ADR-031): registry-backed families, the
//! path-to-entity index, link context for wikilinks, affiliation reverse
//! lookup, the merged-entity stub, and the SQLite-backed index.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::store::{AtomStore, MemAtomStore, SqliteAtomStore};
use ffs_core::{
    AtomTemplate, EntityId, InMemoryPathIndex, Iso8601, PathIndex, PredicateName, PublicKey, Tier,
};

const PEOPLE_TOML: &str = r#"
name = "person.generic"
version = 2
[claim_schema]
type = "object"
required = ["display_name"]
[claim_schema.properties]
display_name = { type = "string" }
organization = { type = "string" }
mentions = { type = "array" }
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

const ORGS_TOML: &str = r#"
name = "org.company"
version = 1
[claim_schema]
type = "object"
required = ["display_name"]
[claim_schema.properties]
display_name = { type = "string" }
[rendering]
template = "org.md.tera"
frontmatter_fields = ["display_name"]
[path]
family = "orgs"
name_field = "display_name"
"#;

const ARTICLES_TOML: &str = r#"
name = "source.article"
version = 1
[claim_schema]
type = "object"
required = ["title"]
[claim_schema.properties]
title = { type = "string" }
mentions = { type = "array" }
[rendering]
template = "article.md.tera"
frontmatter_fields = ["title"]
[path]
family = "articles"
name_field = "title"
"#;

const AFFILIATION_TOML: &str = r#"
name = "affiliation"
version = 1
[claim_schema]
type = "object"
required = ["person", "organization"]
[claim_schema.properties]
person = { type = "string" }
organization = { type = "string" }
title = { type = "string" }
kind = { type = "string" }
[rendering]
template = "affiliation.md.tera"
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

const WIDGETS_TOML: &str = r#"
name = "widget.thing"
version = 1
[claim_schema]
type = "object"
required = ["display_name"]
[claim_schema.properties]
display_name = { type = "string" }
[rendering]
template = "widget.md.tera"
frontmatter_fields = ["display_name"]
[path]
family = "widgets"
name_field = "display_name"
"#;

const PERSON_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
---
{% if organization_link %}org: [[{{ organization_link.basename }}|{{ organization_link.display }}]]{% elif claim.organization %}org: {{ claim.organization }}{% endif %}
{% if affiliations %}
## Affiliations
{% for a in affiliations %}- {% if a.basename %}[[{{ a.basename }}|{{ a.display }}]]{% else %}{{ a.display }}{% endif %}: {{ a.title }} ({{ a.valid_from }} to {% if a.valid_to %}{{ a.valid_to }}{% else %}present{% endif %})
{% endfor %}{% endif %}"#;

const ORG_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
---
{% if affiliations %}
## People
{% for a in affiliations %}- {% if a.basename %}[[{{ a.basename }}|{{ a.display }}]]{% else %}{{ a.display }}{% endif %}: {{ a.title }}
{% endfor %}{% endif %}"#;

const ARTICLE_TEMPLATE: &str = r#"---
title: {{ claim.title }}
---
{% for m in mentions_resolved %}- {% if m.basename %}[[{{ m.basename }}|{{ m.display }}]]{% else %}{{ m.display }}{% endif %}
{% endfor %}"#;

const WIDGET_TEMPLATE: &str = "widget: {{ claim.display_name }} at {{ basename }}\n";

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}
fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}
fn grant(store: &dyn AtomStore) {
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

fn insert(
    store: &dyn AtomStore,
    entity: &str,
    predicate: &str,
    claim: serde_json::Value,
    tx: &str,
    valid_to: Option<&str>,
) -> ffs_core::Multihash {
    let atom = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new(predicate),
        claim,
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: valid_to.map(|v| Iso8601::new(v).unwrap()),
        tx_time: Iso8601::new(tx).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&atom).unwrap()
}

struct H {
    store: Arc<dyn AtomStore>,
    renderer: ProjectionRenderer,
    index: Arc<InMemoryPathIndex>,
}

fn harness(specs: &[&str]) -> H {
    let dir = tempfile::tempdir().unwrap();
    let preds = dir.path().join("predicates");
    std::fs::create_dir_all(&preds).unwrap();
    for (i, spec) in specs.iter().enumerate() {
        std::fs::write(preds.join(format!("{i}.toml")), spec).unwrap();
    }
    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&preds).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant(&*store);
    let index = InMemoryPathIndex::new_arc();
    let templates = dir.path().join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    let mut renderer = ProjectionRenderer::new(store.clone(), registry, &templates)
        .unwrap()
        .with_path_index(index.clone());
    renderer
        .add_raw_template("person.md.tera", PERSON_TEMPLATE)
        .unwrap();
    renderer
        .add_raw_template("org.md.tera", ORG_TEMPLATE)
        .unwrap();
    renderer
        .add_raw_template("article.md.tera", ARTICLE_TEMPLATE)
        .unwrap();
    renderer
        .add_raw_template("widget.md.tera", WIDGET_TEMPLATE)
        .unwrap();
    std::mem::forget(dir);
    H {
        store,
        renderer,
        index,
    }
}

fn render(h: &H, path: &str) -> ffs_core::projection::ProjectionResponse {
    h.renderer
        .render(&ProjectionRequest {
            path: path.into(),
            as_of: Some(Iso8601::new("2026-12-31T00:00:00Z").unwrap()),
            agent: owner_pk(),
        })
        .unwrap()
}

#[test]
fn organization_link_resolves_through_the_index_and_falls_back_to_plain_text() {
    let h = harness(&[PEOPLE_TOML, ORGS_TOML]);
    let org = EntityId::mint();
    insert(
        &*h.store,
        org.as_str(),
        "org.company",
        serde_json::json!({"display_name": "Acme Inc"}),
        "2026-02-01T00:00:00Z",
        None,
    );
    h.index.assign("orgs", &org, "Acme Inc", &[]).unwrap();
    let person = EntityId::mint();
    insert(
        &*h.store,
        person.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "Sara Chen", "organization": org.as_str()}),
        "2026-02-02T00:00:00Z",
        None,
    );
    h.index.assign("people", &person, "Sara Chen", &[]).unwrap();

    let out = render(&h, "people/by-name/S/Sara_Chen.md");
    assert!(
        out.markdown.contains("org: [[Acme_Inc|Acme Inc]]"),
        "{}",
        out.markdown
    );

    // An organization value that is not a known entity prints as text.
    let other = EntityId::mint();
    insert(
        &*h.store,
        other.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "Bo Lee", "organization": "Somewhere Else"}),
        "2026-02-03T00:00:00Z",
        None,
    );
    h.index.assign("people", &other, "Bo Lee", &[]).unwrap();
    let out = render(&h, "people/by-name/B/Bo_Lee.md");
    assert!(
        out.markdown.contains("org: Somewhere Else"),
        "{}",
        out.markdown
    );
    assert!(!out.markdown.contains("[["), "{}", out.markdown);
}

#[test]
fn article_mentions_resolve_or_print_display() {
    let h = harness(&[PEOPLE_TOML, ARTICLES_TOML]);
    let person = EntityId::mint();
    insert(
        &*h.store,
        person.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "Sara Chen"}),
        "2026-02-02T00:00:00Z",
        None,
    );
    h.index.assign("people", &person, "Sara Chen", &[]).unwrap();
    let article = EntityId::mint();
    insert(
        &*h.store,
        article.as_str(),
        "source.article",
        serde_json::json!({"title": "Big news", "mentions": [
            {"entity": person.as_str(), "display": "S. Chen", "context": "quoted"},
            {"display": "Unknown Person", "context": "passing"}
        ]}),
        "2026-02-05T00:00:00Z",
        None,
    );
    h.index
        .assign("articles", &article, "Big news", &[])
        .unwrap();
    let out = render(&h, "articles/by-name/B/Big_news.md");
    assert!(
        out.markdown.contains("- [[Sara_Chen|S. Chen]]"),
        "{}",
        out.markdown
    );
    assert!(
        out.markdown.contains("- Unknown Person\n"),
        "{}",
        out.markdown
    );
}

#[test]
fn affiliations_render_by_reverse_lookup_on_both_sides_and_count_as_sources() {
    let h = harness(&[PEOPLE_TOML, ORGS_TOML, AFFILIATION_TOML]);
    let acme = EntityId::mint();
    let city = EntityId::mint();
    insert(
        &*h.store,
        acme.as_str(),
        "org.company",
        serde_json::json!({"display_name": "Acme Inc"}),
        "2026-02-01T00:00:00Z",
        None,
    );
    insert(
        &*h.store,
        city.as_str(),
        "org.company",
        serde_json::json!({"display_name": "City Council"}),
        "2026-02-01T00:00:01Z",
        None,
    );
    h.index.assign("orgs", &acme, "Acme Inc", &[]).unwrap();
    h.index.assign("orgs", &city, "City Council", &[]).unwrap();
    let sara = EntityId::mint();
    insert(
        &*h.store,
        sara.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "Sara Chen"}),
        "2026-02-02T00:00:00Z",
        None,
    );
    h.index.assign("people", &sara, "Sara Chen", &[]).unwrap();
    // Two current roles and one ended one.
    let a1 = insert(
        &*h.store,
        EntityId::mint().as_str(),
        "affiliation",
        serde_json::json!({"person": sara.as_str(), "organization": acme.as_str(), "title": "CEO", "kind": "executive"}),
        "2026-02-03T00:00:00Z",
        None,
    );
    let a2 = insert(
        &*h.store,
        EntityId::mint().as_str(),
        "affiliation",
        serde_json::json!({"person": sara.as_str(), "organization": city.as_str(), "title": "Board member", "kind": "board"}),
        "2026-02-04T00:00:00Z",
        None,
    );
    let a3 = insert(
        &*h.store,
        EntityId::mint().as_str(),
        "affiliation",
        serde_json::json!({"person": sara.as_str(), "organization": acme.as_str(), "title": "CFO", "kind": "executive"}),
        "2026-02-05T00:00:00Z",
        Some("2025-12-31T00:00:00Z"),
    );

    let out = render(&h, "people/by-name/S/Sara_Chen.md");
    assert!(out.markdown.contains("## Affiliations"), "{}", out.markdown);
    assert!(
        out.markdown
            .contains("- [[Acme_Inc|Acme Inc]]: CEO (2026-01-01T00:00:00Z to present)"),
        "{}",
        out.markdown
    );
    assert!(
        out.markdown.contains(
            "- [[City_Council|City Council]]: Board member (2026-01-01T00:00:00Z to present)"
        ),
        "{}",
        out.markdown
    );
    assert!(
        out.markdown.contains(
            "- [[Acme_Inc|Acme Inc]]: CFO (2026-01-01T00:00:00Z to 2025-12-31T00:00:00Z)"
        ),
        "{}",
        out.markdown
    );
    // Ended roles sort after current ones.
    assert!(out.markdown.find("CFO").unwrap() > out.markdown.find("Board member").unwrap());
    for hash in [&a1, &a2, &a3] {
        assert!(
            out.source_atoms.contains(hash),
            "affiliation hash in source_atoms"
        );
    }

    let org_out = render(&h, "orgs/by-name/A/Acme_Inc.md");
    assert!(
        org_out.markdown.contains("## People"),
        "{}",
        org_out.markdown
    );
    assert!(
        org_out.markdown.contains("- [[Sara_Chen|Sara Chen]]: CEO"),
        "{}",
        org_out.markdown
    );
    assert!(
        org_out.markdown.contains("- [[Sara_Chen|Sara Chen]]: CFO"),
        "{}",
        org_out.markdown
    );
    assert!(
        !org_out.markdown.contains("Board member"),
        "{}",
        org_out.markdown
    );
}

#[test]
fn a_person_without_affiliation_atoms_renders_no_affiliations_section() {
    let h = harness(&[PEOPLE_TOML, ORGS_TOML, AFFILIATION_TOML]);
    let sara = EntityId::mint();
    insert(
        &*h.store,
        sara.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "Sara Chen"}),
        "2026-02-02T00:00:00Z",
        None,
    );
    h.index.assign("people", &sara, "Sara Chen", &[]).unwrap();
    let out = render(&h, "people/by-name/S/Sara_Chen.md");
    assert!(
        !out.markdown.contains("## Affiliations"),
        "{}",
        out.markdown
    );
    assert_eq!(out.source_atoms.len(), 1);
}

#[test]
fn merged_entity_renders_the_redirect_stub_only() {
    let h = harness(&[PEOPLE_TOML, SAME_AS_TOML]);
    let winner = EntityId::mint();
    let loser = EntityId::mint();
    insert(
        &*h.store,
        winner.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "Sara Chen"}),
        "2026-02-02T00:00:00Z",
        None,
    );
    insert(
        &*h.store,
        loser.as_str(),
        "person.generic",
        serde_json::json!({"display_name": "S. Chen"}),
        "2026-02-03T00:00:00Z",
        None,
    );
    h.index.assign("people", &winner, "Sara Chen", &[]).unwrap();
    h.index.assign("people", &loser, "S. Chen", &[]).unwrap();
    let same_as = insert(
        &*h.store,
        loser.as_str(),
        "entity.same_as",
        serde_json::json!({"target": winner.as_str(), "reason": "alias"}),
        "2026-02-04T00:00:00Z",
        None,
    );

    let out = render(&h, "people/by-name/S/S._Chen.md");
    assert_eq!(out.markdown, "Merged into [[Sara_Chen|Sara Chen]]\n");
    assert_eq!(out.source_atoms, vec![same_as]);
    assert!(out.reverse_map.is_empty());
    // The winner is unchanged by the merge.
    let w = render(&h, "people/by-name/S/Sara_Chen.md");
    assert!(w.markdown.contains("display_name: Sara Chen"));
}

#[test]
fn a_widgets_family_renders_with_no_code_changes_and_listings_use_basenames() {
    let h = harness(&[WIDGETS_TOML]);
    let w = EntityId::mint();
    insert(
        &*h.store,
        w.as_str(),
        "widget.thing",
        serde_json::json!({"display_name": "Blue Widget"}),
        "2026-02-02T00:00:00Z",
        None,
    );
    h.index.assign("widgets", &w, "Blue Widget", &[]).unwrap();
    let out = render(&h, "widgets/by-name/B/Blue_Widget.md");
    assert_eq!(out.markdown, "widget: Blue Widget at Blue_Widget\n");
    let listing = render(&h, "widgets/by-name/B/");
    assert!(
        listing.markdown.contains("- [Blue_Widget](Blue_Widget.md)"),
        "{}",
        listing.markdown
    );
    let recent = render(&h, "widgets/recent/");
    assert!(
        recent.markdown.contains("- [Blue_Widget](Blue_Widget.md)"),
        "{}",
        recent.markdown
    );
}

#[test]
fn a_basename_with_no_index_row_resolves_to_the_slug_form_entity_id() {
    let h = harness(&[WIDGETS_TOML]);
    insert(
        &*h.store,
        "Legacy_Widget",
        "widget.thing",
        serde_json::json!({"display_name": "Legacy Widget"}),
        "2026-02-02T00:00:00Z",
        None,
    );
    let out = render(&h, "widgets/by-name/L/Legacy_Widget.md");
    assert_eq!(out.markdown, "widget: Legacy Widget at Legacy_Widget\n");
}

#[test]
fn sqlite_path_index_persists_assignments_qualifiers_and_renames() {
    let store = SqliteAtomStore::open_in_memory(&[7u8; 32]).unwrap();
    let a = EntityId::mint();
    let b = EntityId::mint();
    assert_eq!(
        store
            .assign("people", &a, "Sara Chen", &["Acme".into()])
            .unwrap(),
        "Sara_Chen"
    );
    assert_eq!(
        store
            .assign("people", &b, "Sara Chen", &["City Council".into()])
            .unwrap(),
        "Sara_Chen_(City_Council)"
    );
    // Idempotent per entity.
    assert_eq!(
        store.assign("people", &a, "Sara Chen", &[]).unwrap(),
        "Sara_Chen"
    );
    assert_eq!(store.resolve("people", "Sara_Chen").unwrap().unwrap(), a);
    assert_eq!(
        store.basename_for("people", &b).unwrap().unwrap(),
        "Sara_Chen_(City_Council)"
    );
    // Rename moves the row and reports the old basename.
    let old = store.rename("people", &a, "Sara Chen-Lopez", &[]).unwrap();
    assert_eq!(old.as_deref(), Some("Sara_Chen"));
    assert!(store.resolve("people", "Sara_Chen").unwrap().is_none());
    assert_eq!(
        store.resolve("people", "Sara_Chen-Lopez").unwrap().unwrap(),
        a
    );
    // A third Sara Chen now takes the bare name that was freed.
    let c = EntityId::mint();
    assert_eq!(
        store.assign("people", &c, "Sara Chen", &[]).unwrap(),
        "Sara_Chen"
    );
    // Slug-form ids are ordinary keys, and families are separate.
    let slug = EntityId::new("Sara_Chen");
    assert_eq!(
        store.assign("contacts", &slug, "Sara Chen", &[]).unwrap(),
        "Sara_Chen"
    );
    PathIndex::remove(&store, "contacts", &slug).unwrap();
    assert!(store.resolve("contacts", "Sara_Chen").unwrap().is_none());
}
