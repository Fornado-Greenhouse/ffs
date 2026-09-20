//! Predicate-spec types, loader, registry, and JSON Schema validator.
//!
//! Specs live in `~/.ffs/config/predicates/` as TOML files (one spec per
//! file, ADR-021). They are loaded into a `SpecRegistry`, optionally with
//! filesystem hot-reload via `watch_dir`. Each spec carries a JSON Schema
//! for claim payload validation, a rendering convention, reverse-map
//! rules driving the fast-path edit classifier (task 09), and an optional
//! pagination strategy.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

mod registry;
mod reverse_map;

pub use registry::{FamilyEntry, SpecError, SpecRegistry, ValidationError, WatchHandle};
pub use reverse_map::{EditKind, OutputRef, ReverseMapRule};

/// In-memory representation of a parsed predicate spec.
#[derive(Debug, Clone, PartialEq)]
pub struct PredicateSpec {
    pub name: String,
    pub version: u32,
    pub parent_predicate: Option<String>,
    /// JSON Schema (Draft 2020-12) for validating claim payloads.
    pub claim_schema: serde_json::Value,
    pub rendering: RenderingConvention,
    pub reverse_map: Vec<ReverseMapRule>,
    pub pagination: Option<Pagination>,
    /// Projection-path family declaration (ADR-028). `None` means the
    /// predicate has no folder: its atoms are inspectable via `atom.get`
    /// but never materialize as files.
    pub path: Option<PathSpec>,
    /// Informative ontology alignment (ADR-031). Never validated beyond
    /// shape; nothing in the daemon branches on it.
    pub ontology: Option<OntologySpec>,
    /// `[quarantine]` table (ADR-029): `append_only = true` declares that
    /// this predicate's records are events, not state, so a proposal for
    /// it is always additive and may auto-file under an `Accept` grant.
    pub quarantine: Option<QuarantineSpec>,
    /// `[attestation]` table (ADR-034): how many independent
    /// confirmations a fact of this predicate needs to count as
    /// current, and for how long one lasts. Absent means k = 1 and no
    /// window.
    pub attestation: Option<AttestationSpec>,
}

/// `[attestation]` table (ADR-034 § Decision (2)). A per-substrate
/// override in `config/attestation.toml` may raise `k`, never lower it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttestationSpec {
    /// Distinct independent confirmations needed for `current`.
    #[serde(default = "default_k")]
    pub k: u32,
    /// Days after the newest confirmation before the fact is `stale`;
    /// absent means the fact never goes stale on its own.
    #[serde(default)]
    pub window_days: Option<u32>,
    /// Count distinct `(basis, source)` pairs rather than signers.
    #[serde(default = "default_true")]
    pub independent: bool,
}

fn default_k() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

/// `[quarantine]` table (ADR-029, ADR-021 extension).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuarantineSpec {
    #[serde(default)]
    pub append_only: bool,
}

/// `[path]` table: which projection folder a predicate's entities live
/// under and which claim field carries the human-readable name that
/// becomes the file basename (ADR-028, ADR-030).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PathSpec {
    /// Folder root, e.g. `contacts`, `orgs`, `articles`.
    pub family: String,
    /// Claim field holding the display name (`display_name`, `title`).
    pub name_field: String,
    /// Folder layout (task_41): `by_name` (default) files entities
    /// under `<family>/by-name/<letter>/<basename>.md`; `flat` files
    /// them directly as `<family>/<basename>.md`, for families whose
    /// name field is not a name (a briefing's date).
    #[serde(default)]
    pub layout: PathLayout,
}

/// How a path family lays its files out on disk (ADR-028, task_41).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PathLayout {
    /// `<family>/by-name/<letter>/<basename>.md` plus `recent/`.
    #[default]
    ByName,
    /// `<family>/<basename>.md` plus `recent/`.
    Flat,
}

/// `[ontology]` table: BFO / CCO / IAO alignment annotations (ADR-031).
/// Every field is a free string; unknown or empty values load fine.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct OntologySpec {
    #[serde(default)]
    pub bfo: String,
    #[serde(default)]
    pub cco: String,
    #[serde(default)]
    pub iao: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RenderingConvention {
    /// Tera template filename (lives in `~/.ffs/config/templates/`).
    pub template: String,
    #[serde(default)]
    pub frontmatter_fields: Vec<String>,
    #[serde(default)]
    pub body_sections: Vec<String>,
    /// Subset of body sections that accept additive bullet edits via fast-path.
    #[serde(default)]
    pub additive_sections: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    pub strategy: PaginationStrategy,
    #[serde(default)]
    pub group_field: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaginationStrategy {
    AlphabeticalFirstLetter,
    Recency,
    ByOrg,
}

/// Helpers shared between this module and `registry.rs`.
pub(crate) mod mod_helpers {
    use super::*;

    /// Internal raw TOML shape. `claim_schema` is read as a generic
    /// TOML value and converted to JSON downstream.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct RawSpec {
        pub name: String,
        pub version: u32,
        #[serde(default)]
        pub parent_predicate: Option<String>,
        pub claim_schema: toml::Value,
        pub rendering: RenderingConvention,
        #[serde(default)]
        pub reverse_map: Vec<ReverseMapRule>,
        #[serde(default)]
        pub pagination: Option<Pagination>,
        #[serde(default)]
        pub path: Option<PathSpec>,
        #[serde(default)]
        pub ontology: Option<OntologySpec>,
        #[serde(default)]
        pub quarantine: Option<QuarantineSpec>,
        #[serde(default)]
        pub attestation: Option<AttestationSpec>,
    }

    /// Parse a TOML predicate-spec string, converting the claim_schema
    /// table to JSON and rejecting unknown top-level fields.
    pub fn parse_spec_str(content: &str, path: &Path) -> Result<PredicateSpec, SpecError> {
        let raw: RawSpec = toml::from_str(content).map_err(|e| SpecError::Toml {
            path: path.to_path_buf(),
            source: e,
        })?;
        let claim_schema = toml_to_json(raw.claim_schema);
        if let (Some(path), Some(pagination)) = (&raw.path, &raw.pagination)
            && let Some(group_field) = &pagination.group_field
            && group_field != &path.name_field
        {
            return Err(SpecError::PathNameFieldMismatch {
                predicate: raw.name.clone(),
                name_field: path.name_field.clone(),
                group_field: group_field.clone(),
            });
        }
        Ok(PredicateSpec {
            name: raw.name,
            version: raw.version,
            parent_predicate: raw.parent_predicate,
            claim_schema,
            rendering: raw.rendering,
            reverse_map: raw.reverse_map,
            pagination: raw.pagination,
            path: raw.path,
            ontology: raw.ontology,
            quarantine: raw.quarantine,
            attestation: raw.attestation,
        })
    }

    /// Validate that every reverse-map rule's `output` references a
    /// rendering element actually defined on the spec.
    pub fn validate_reverse_map(spec: &PredicateSpec) -> Result<(), SpecError> {
        for rule in &spec.reverse_map {
            let parsed = OutputRef::parse(&rule.output);
            let ok = match parsed {
                Some(OutputRef::Frontmatter(field)) => {
                    spec.rendering.frontmatter_fields.iter().any(|f| f == field)
                }
                Some(OutputRef::Section(name)) => {
                    spec.rendering.body_sections.iter().any(|s| s == name)
                        || spec.rendering.additive_sections.iter().any(|s| s == name)
                }
                Some(OutputRef::SectionListItem(name)) => {
                    spec.rendering.additive_sections.iter().any(|s| s == name)
                }
                None => false,
            };
            if !ok {
                return Err(SpecError::UndefinedReverseMapOutput {
                    predicate: spec.name.clone(),
                    output: rule.output.clone(),
                });
            }
        }
        Ok(())
    }

    /// Compute the spec's effective JSON Schema, composing parent schemas
    /// via JSON Schema `allOf`. A claim must validate against both the
    /// parent and the child schema (parent first, recursively).
    pub fn effective_schema(
        spec: &PredicateSpec,
        registry: &HashMap<String, PredicateSpec>,
    ) -> serde_json::Value {
        match &spec.parent_predicate {
            Some(parent_name) => match registry.get(parent_name) {
                Some(parent) => {
                    let parent_eff = effective_schema(parent, registry);
                    serde_json::json!({
                        "allOf": [parent_eff, spec.claim_schema.clone()]
                    })
                }
                None => spec.claim_schema.clone(),
            },
            None => spec.claim_schema.clone(),
        }
    }

    /// Compile a JSON Schema (Draft 2020-12) into a reusable validator.
    pub fn compile_validator(schema: &serde_json::Value) -> Result<jsonschema::Validator, String> {
        jsonschema::draft202012::new(schema).map_err(|e| e.to_string())
    }

    /// Convert a `toml::Value` to a `serde_json::Value` for JSON Schema use.
    pub fn toml_to_json(v: toml::Value) -> serde_json::Value {
        match v {
            toml::Value::String(s) => serde_json::Value::String(s),
            toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
            toml::Value::Float(f) => serde_json::Number::from_f64(f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            toml::Value::Boolean(b) => serde_json::Value::Bool(b),
            toml::Value::Array(arr) => {
                serde_json::Value::Array(arr.into_iter().map(toml_to_json).collect())
            }
            toml::Value::Table(t) => {
                let map: serde_json::Map<String, serde_json::Value> =
                    t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect();
                serde_json::Value::Object(map)
            }
            toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mod_helpers::*;
    use super::*;
    use std::path::PathBuf;

    const CONTACT_PERSON_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
"$schema" = "https://json-schema.org/draft/2020-12/schema"
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }
work_email = { type = "string" }
phone = { type = "string" }

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
output = "section.Notes.list_item"
atom_field = "claim.notes[]"
edit_kind = "additive_section"
"#;

    #[test]
    fn canonical_contact_person_parses() {
        let spec = parse_spec_str(CONTACT_PERSON_TOML, &PathBuf::from("contact.person.toml"))
            .expect("canonical TOML must parse");
        assert_eq!(spec.name, "contact.person");
        assert_eq!(spec.version, 1);
        assert_eq!(spec.rendering.template, "contact-person.md.tera");
        assert_eq!(spec.reverse_map.len(), 2);
        assert_eq!(spec.reverse_map[0].edit_kind, EditKind::SingleLineText);
        assert_eq!(spec.reverse_map[1].edit_kind, EditKind::AdditiveSection);
    }

    #[test]
    fn unknown_top_level_field_rejected() {
        let bad = format!("{CONTACT_PERSON_TOML}\nrogue_field = 42\n");
        let err = parse_spec_str(&bad, &PathBuf::from("bad.toml")).unwrap_err();
        // toml's deny-unknown-fields surfaces as a Toml error.
        match err {
            SpecError::Toml { source, .. } => {
                let msg = source.to_string();
                assert!(
                    msg.contains("rogue_field") || msg.contains("unknown"),
                    "{msg}"
                );
            }
            other => panic!("expected SpecError::Toml, got {other:?}"),
        }
    }

    #[test]
    fn reverse_map_referencing_undefined_output_rejected() {
        let bad = r#"
name = "person.bad"
version = 1

[claim_schema]
type = "object"

[rendering]
template = "x.md.tera"
frontmatter_fields = []

[[reverse_map]]
output = "frontmatter.does_not_exist"
atom_field = "claim.x"
edit_kind = "frontmatter_value"
"#;
        let spec = parse_spec_str(bad, &PathBuf::from("bad.toml")).unwrap();
        let err = validate_reverse_map(&spec).unwrap_err();
        match err {
            SpecError::UndefinedReverseMapOutput { output, .. } => {
                assert_eq!(output, "frontmatter.does_not_exist");
            }
            other => panic!("expected UndefinedReverseMapOutput, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_schema_rejected_on_compile() {
        // type as wrong shape (must be string or array of strings).
        let bad = r#"
name = "broken"
version = 1

[claim_schema]
type = 42

[rendering]
template = "x.md.tera"
"#;
        let spec = parse_spec_str(bad, &PathBuf::from("broken.toml")).unwrap();
        let err = compile_validator(&spec.claim_schema).unwrap_err();
        // Just check the error string is non-empty; specific wording varies by jsonschema version.
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_claim_happy_path_via_registry() {
        let registry = SpecRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("contact.person.toml"), CONTACT_PERSON_TOML).unwrap();
        registry.load_dir(dir.path()).unwrap();
        let claim = serde_json::json!({"display_name": "Sara"});
        registry.validate_claim("contact.person", &claim).unwrap();
    }

    #[test]
    fn validate_claim_rejects_missing_required() {
        let registry = SpecRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("contact.person.toml"), CONTACT_PERSON_TOML).unwrap();
        registry.load_dir(dir.path()).unwrap();
        let claim = serde_json::json!({});
        let err = registry
            .validate_claim("contact.person", &claim)
            .unwrap_err();
        match err {
            ValidationError::SchemaValidation(_) => {}
            other => panic!("expected SchemaValidation, got {other:?}"),
        }
    }

    #[test]
    fn validate_claim_rejects_type_mismatch() {
        let registry = SpecRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("contact.person.toml"), CONTACT_PERSON_TOML).unwrap();
        registry.load_dir(dir.path()).unwrap();
        let claim = serde_json::json!({"display_name": 1});
        let err = registry
            .validate_claim("contact.person", &claim)
            .unwrap_err();
        match err {
            ValidationError::SchemaValidation(_) => {}
            other => panic!("expected SchemaValidation, got {other:?}"),
        }
    }

    #[test]
    fn validate_claim_unknown_predicate() {
        let registry = SpecRegistry::new();
        let claim = serde_json::json!({});
        let err = registry.validate_claim("not.loaded", &claim).unwrap_err();
        match err {
            ValidationError::UnknownPredicate(name) => assert_eq!(name, "not.loaded"),
            other => panic!("expected UnknownPredicate, got {other:?}"),
        }
    }

    #[test]
    fn unknown_parent_rejected() {
        let bad = r#"
name = "child.alone"
version = 1
parent_predicate = "ghost.parent"

[claim_schema]
type = "object"

[rendering]
template = "x.md.tera"
"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("child.toml"), bad).unwrap();
        let registry = SpecRegistry::new();
        let err = registry.load_dir(dir.path()).unwrap_err();
        match err {
            SpecError::UnknownParent { predicate, parent } => {
                assert_eq!(predicate, "child.alone");
                assert_eq!(parent, "ghost.parent");
            }
            other => panic!("expected UnknownParent, got {other:?}"),
        }
    }

    #[test]
    fn names_returns_sorted_list() {
        let registry = SpecRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("contact.person.toml"), CONTACT_PERSON_TOML).unwrap();
        registry.load_dir(dir.path()).unwrap();
        let names = registry.names();
        assert_eq!(names, vec!["contact.person".to_string()]);
    }
}

#[cfg(test)]
mod path_and_ontology_tests {
    use super::*;
    use std::path::Path;

    const BASE: &str = r#"
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
"#;

    fn parse(extra: &str) -> Result<PredicateSpec, SpecError> {
        mod_helpers::parse_spec_str(&format!("{BASE}\n{extra}"), Path::new("widget.thing.toml"))
    }

    #[test]
    fn spec_without_path_table_loads_and_has_no_family() {
        let spec = parse("").expect("loads");
        assert!(spec.path.is_none());
        assert!(spec.ontology.is_none());
    }

    #[test]
    fn path_table_is_parsed() {
        let spec =
            parse("[path]\nfamily = \"widgets\"\nname_field = \"display_name\"\n").expect("loads");
        let path = spec.path.expect("path present");
        assert_eq!(path.family, "widgets");
        assert_eq!(path.name_field, "display_name");
    }

    #[test]
    fn path_name_field_must_agree_with_pagination_group_field() {
        let err = parse(
            "[pagination]\nstrategy = \"alphabetical_first_letter\"\ngroup_field = \"title\"\n\n[path]\nfamily = \"widgets\"\nname_field = \"display_name\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, SpecError::PathNameFieldMismatch { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("display_name") && err.to_string().contains("title"));
    }

    #[test]
    fn path_with_agreeing_group_field_loads() {
        let spec = parse(
            "[pagination]\nstrategy = \"alphabetical_first_letter\"\ngroup_field = \"display_name\"\n\n[path]\nfamily = \"widgets\"\nname_field = \"display_name\"\n",
        )
        .expect("loads");
        assert_eq!(spec.path.unwrap().family, "widgets");
    }

    #[test]
    fn quarantine_append_only_table_is_parsed_and_absent_means_false() {
        let spec = parse("[quarantine]\nappend_only = true\n").expect("loads");
        assert!(spec.quarantine.as_ref().is_some_and(|q| q.append_only));
        let plain = parse("").expect("loads");
        assert!(plain.quarantine.is_none());
        let err = parse("[quarantine]\nappend_only = true\nbogus = 1\n").unwrap_err();
        assert!(
            matches!(err, SpecError::Toml { .. }),
            "unknown key in [quarantine] fails loudly"
        );
    }

    #[test]
    fn unknown_or_empty_ontology_values_load_without_error() {
        let spec = parse("[ontology]\nbfo = \"not a real class\"\ncco = \"\"\n").expect("loads");
        let ont = spec.ontology.expect("ontology present");
        assert_eq!(ont.bfo, "not a real class");
        assert_eq!(ont.cco, "");
        assert_eq!(ont.iao, "");
        assert_eq!(ont.note, "");
    }

    #[test]
    fn registry_family_table_lists_declared_families_and_picks_up_a_fixture_spec() {
        let dir = tempfile::tempdir().unwrap();
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../starter/predicates");
        for name in ["contact.person.toml", "person.generic.toml", "note.toml"] {
            std::fs::copy(starter.join(name), dir.path().join(name)).unwrap();
        }
        std::fs::write(
            dir.path().join("widget.thing.toml"),
            format!("{BASE}\n[path]\nfamily = \"widgets\"\nname_field = \"display_name\"\n"),
        )
        .unwrap();
        let reg = SpecRegistry::new();
        reg.load_dir(dir.path()).expect("registry loads");
        let families: Vec<(String, String, String)> = reg
            .families()
            .into_iter()
            .map(|f| (f.family, f.predicate, f.name_field))
            .collect();
        assert_eq!(
            families,
            vec![
                (
                    "contacts".into(),
                    "contact.person".into(),
                    "display_name".into()
                ),
                ("notes".into(), "note".into(), "title".into()),
                (
                    "people".into(),
                    "person.generic".into(),
                    "display_name".into()
                ),
                (
                    "widgets".into(),
                    "widget.thing".into(),
                    "display_name".into()
                ),
            ]
        );
        assert_eq!(
            reg.family_for_folder("widgets").unwrap().predicate,
            "widget.thing"
        );
        assert_eq!(reg.family_for_predicate("note").unwrap().family, "notes");
        assert!(reg.family_for_folder("nope").is_none());
    }
}
