//! Integration tests for the predicate-spec loader: end-to-end TOML
//! parsing through registry, hot-reload via filesystem watcher, and
//! parent-predicate inheritance.

use std::fs;
use std::time::{Duration, Instant};

use ffs_core::predicate::{SpecError, SpecRegistry, ValidationError};

const PERSON_GENERIC_TOML: &str = r#"
name = "person.generic"
version = 1

[claim_schema]
"$schema" = "https://json-schema.org/draft/2020-12/schema"
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }
team = { type = "string" }

[rendering]
template = "person-generic.md.tera"
frontmatter_fields = ["display_name", "team"]
"#;

const CONTACT_PERSON_INHERITS_TOML: &str = r#"
name = "contact.person"
version = 1
parent_predicate = "person.generic"

[claim_schema]
"$schema" = "https://json-schema.org/draft/2020-12/schema"
type = "object"

[claim_schema.properties]
work_email = { type = "string" }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name", "work_email"]
body_sections = ["Notes"]
additive_sections = ["Notes"]

[[reverse_map]]
output = "frontmatter.work_email"
atom_field = "claim.work_email"
edit_kind = "single_line_text"
"#;

fn poll_until<F>(deadline: Duration, mut f: F) -> bool
where
    F: FnMut() -> bool,
{
    let stop = Instant::now() + deadline;
    while Instant::now() < stop {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

#[test]
fn hot_reload_picks_up_new_spec_within_2s() {
    let dir = tempfile::tempdir().unwrap();
    let registry = SpecRegistry::new();
    let _watch = registry.watch_dir(dir.path()).expect("watch_dir");

    assert!(registry.get("person.generic").is_none());

    fs::write(dir.path().join("person.generic.toml"), PERSON_GENERIC_TOML).unwrap();

    let registered = poll_until(Duration::from_secs(2), || {
        registry.get("person.generic").is_some()
    });
    assert!(
        registered,
        "expected registry to pick up new spec within 2s; current names: {:?}",
        registry.names()
    );

    // The spec is functional: a valid claim validates.
    let claim = serde_json::json!({"display_name": "Sara"});
    registry.validate_claim("person.generic", &claim).unwrap();
}

#[test]
fn hot_reload_picks_up_modifications() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("person.generic.toml"), PERSON_GENERIC_TOML).unwrap();

    let registry = SpecRegistry::new();
    registry.load_dir(dir.path()).unwrap();
    assert!(registry.get("person.generic").is_some());
    let _watch = registry.watch_dir(dir.path()).expect("watch_dir");

    // Modify: bump the version field. Ensures the file modification path triggers reload.
    let modified = PERSON_GENERIC_TOML.replace("version = 1", "version = 2");
    fs::write(dir.path().join("person.generic.toml"), modified).unwrap();

    let updated = poll_until(Duration::from_secs(2), || {
        registry
            .get("person.generic")
            .map(|s| s.version == 2)
            .unwrap_or(false)
    });
    assert!(
        updated,
        "expected version=2 within 2s; got {:?}",
        registry.get("person.generic").map(|s| s.version)
    );
}

#[test]
fn parent_inheritance_allows_parent_fields() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("person.generic.toml"), PERSON_GENERIC_TOML).unwrap();
    fs::write(
        dir.path().join("contact.person.toml"),
        CONTACT_PERSON_INHERITS_TOML,
    )
    .unwrap();

    let registry = SpecRegistry::new();
    registry.load_dir(dir.path()).unwrap();

    // A claim using a parent-defined field must validate via the child.
    let claim = serde_json::json!({"display_name": "Sara", "team": "engineering"});
    registry.validate_claim("contact.person", &claim).unwrap();

    // A claim using a child-defined field must also validate.
    let claim2 = serde_json::json!({"display_name": "Sara", "work_email": "sara@example.com"});
    registry.validate_claim("contact.person", &claim2).unwrap();

    // Parent's required field is enforced through the child.
    let bad = serde_json::json!({"work_email": "x@example.com"});
    let err = registry.validate_claim("contact.person", &bad).unwrap_err();
    assert!(matches!(err, ValidationError::SchemaValidation(_)));
}

#[test]
fn parent_added_after_child_recovers_via_hot_reload() {
    // Drop the child file in first; it should fail to register because
    // the parent is unknown. Then drop the parent in; the child should
    // become valid via the watcher's reload pass.
    let dir = tempfile::tempdir().unwrap();
    let registry = SpecRegistry::new();
    let _watch = registry.watch_dir(dir.path()).expect("watch_dir");

    fs::write(
        dir.path().join("contact.person.toml"),
        CONTACT_PERSON_INHERITS_TOML,
    )
    .unwrap();
    // child arrives first; not registered (warning logged to stderr).
    std::thread::sleep(Duration::from_millis(150));
    assert!(registry.get("contact.person").is_none());

    // Now add the parent; child should NOT auto-recover because the
    // watcher only sees the parent file create, not the child. The user
    // must touch the child to trigger a reload. This is the documented
    // MVP semantics.
    fs::write(dir.path().join("person.generic.toml"), PERSON_GENERIC_TOML).unwrap();
    let parent_loaded = poll_until(Duration::from_secs(2), || {
        registry.get("person.generic").is_some()
    });
    assert!(parent_loaded, "parent must register within 2s");

    // Touch the child so the watcher reloads it now that the parent exists.
    fs::write(
        dir.path().join("contact.person.toml"),
        CONTACT_PERSON_INHERITS_TOML,
    )
    .unwrap();
    let child_loaded = poll_until(Duration::from_secs(2), || {
        registry.get("contact.person").is_some()
    });
    assert!(
        child_loaded,
        "child must register within 2s after touch; current names: {:?}",
        registry.names()
    );
}

#[test]
fn load_dir_resolves_load_order_two_pass() {
    // Children listed before parents in directory listing must still
    // resolve. This exercises the topological-insert pass in load_dir.
    let dir = tempfile::tempdir().unwrap();
    // Use names that sort children-first under the default lexicographic order.
    fs::write(
        dir.path().join("child.contact.person.toml"),
        CONTACT_PERSON_INHERITS_TOML,
    )
    .unwrap();
    fs::write(
        dir.path().join("parent.person.generic.toml"),
        PERSON_GENERIC_TOML,
    )
    .unwrap();
    let registry = SpecRegistry::new();
    registry.load_dir(dir.path()).unwrap();
    assert!(registry.get("person.generic").is_some());
    assert!(registry.get("contact.person").is_some());
}

#[test]
fn load_dir_unresolvable_parent_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("contact.person.toml"),
        CONTACT_PERSON_INHERITS_TOML,
    )
    .unwrap();
    let registry = SpecRegistry::new();
    let err = registry.load_dir(dir.path()).unwrap_err();
    match err {
        SpecError::UnknownParent { predicate, parent } => {
            assert_eq!(predicate, "contact.person");
            assert_eq!(parent, "person.generic");
        }
        other => panic!("expected UnknownParent, got {other:?}"),
    }
}
