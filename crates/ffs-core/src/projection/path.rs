//! Projection-path parsing. Translates filesystem-style paths under
//! `~/.ffs/<family>/...` into a structured `ParsedPath` that the renderer
//! turns into store queries.
//!
//! Families are not hardcoded: the predicate registry declares them
//! through each spec's `[path]` table (ADR-028) and callers pass a
//! [`FamilyTable`] snapshot. Shapes supported (per ADR-011, generalized):
//!
//! - `<family>/recent/`                          → recency listing
//! - `<family>/by-name/<letter>/`                → alphabetical listing
//! - `<family>/by-name/<letter>/<entity>.md`     → single-entity render
//!
//! Other ADR-011 sub-paths (`starred/`, `by-org/`, `phone/`, `email/`,
//! `from/<peer>/`, `intersection/with/<peer>/`, `all/`) parse to
//! [`ParsedPath::Unsupported`] for MVP. Adding any of them is a small
//! per-sub-path addition; the renderer just needs a new arm.

use std::borrow::Cow;

use crate::atom::PredicateName;
use crate::predicate::{FamilyEntry, PathLayout, SpecRegistry};

/// Normalize OS-native path separators in a projection-path string to
/// the substrate-canonical forward slash. The substrate's contract is
/// `/`-separated everywhere — atom envelopes, reverse-map rules,
/// projection URLs, event payloads — so anything sourced from a
/// `std::path::Path` on Windows (where `to_string_lossy()` yields
/// `\`-separated strings) must run through this at the boundary
/// before the path goes anywhere a `/` is expected.
///
/// Replaces `\\` with `/` unconditionally rather than gating on
/// `cfg!(windows)` for two reasons: (1) the helper is testable on
/// every dev host, and (2) a backslash from any source — Windows
/// path conversion, a federated atom authored on Windows, a
/// malformed event from a misbehaving peer — gets normalized
/// regardless of where the code is running. The `contains('\\')`
/// short-circuit makes the Unix common case allocation-free.
///
/// See task_34 and the Windows CI failure where
/// `event.projection.invalidated.params.path` shipped
/// `"contacts\\by-name\\S\\Sarah_Chen.md"` because the fastpath
/// watcher emitted `Path::to_string_lossy()` without normalizing.
pub fn normalize_separators(path: &str) -> Cow<'_, str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

/// A projection-path family: the folder root under `$FFS_DATA_DIR`,
/// the predicate whose entities live there, and the claim field that
/// carries the human-readable name. Families are declared by predicate
/// specs (`[path]` table, ADR-028) and read from the registry through a
/// [`FamilyTable`] snapshot; nothing in the substrate hardcodes them.
#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PathFamily {
    pub folder: String,
    pub predicate: PredicateName,
    pub name_field: String,
    /// `by_name` (the default) or `flat` (task_41).
    pub layout: PathLayout,
}

impl PathFamily {
    pub fn from_entry(e: &FamilyEntry) -> Self {
        Self {
            folder: e.family.clone(),
            predicate: PredicateName::new(e.predicate.clone()),
            name_field: e.name_field.clone(),
            layout: e.layout,
        }
    }

    /// Parse a family token from the leading path segment against a
    /// family table. Thin wrapper over [`FamilyTable::for_folder`].
    pub fn try_parse(s: &str, table: &FamilyTable) -> Option<Self> {
        table.for_folder(s)
    }

    /// The primary predicate name for this path family. Atoms with this
    /// predicate appear in the family's listings; a single-entity render
    /// for this family reads the head atom for `(entity, primary_predicate)`.
    pub fn primary_predicate(&self) -> PredicateName {
        self.predicate.clone()
    }

    pub fn as_str(&self) -> &str {
        &self.folder
    }
}

/// Snapshot of the registry's family table. Cheap to build (a handful
/// of rows at personal scale) and rebuilt per operation so predicate
/// hot-reload is observed without a separate watcher.
#[derive(Clone, Debug, Default)]
pub struct FamilyTable {
    families: Vec<PathFamily>,
}

impl FamilyTable {
    pub fn from_registry(reg: &SpecRegistry) -> Self {
        Self::from_entries(reg.families())
    }

    pub fn from_entries(entries: Vec<FamilyEntry>) -> Self {
        let mut families: Vec<PathFamily> = entries.iter().map(PathFamily::from_entry).collect();
        families.sort_by(|a, b| a.folder.cmp(&b.folder));
        Self { families }
    }

    pub fn for_folder(&self, folder: &str) -> Option<PathFamily> {
        self.families.iter().find(|f| f.folder == folder).cloned()
    }

    pub fn for_predicate(&self, predicate: &PredicateName) -> Option<PathFamily> {
        self.families
            .iter()
            .find(|f| &f.predicate == predicate)
            .cloned()
    }

    pub fn all(&self) -> Vec<PathFamily> {
        self.families.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// Test helper: the three MVP families exactly as the starter specs
    /// declare them (`contacts`/`contact.person`/`display_name`,
    /// `people`/`person.generic`/`display_name`, `notes`/`note`/`title`).
    pub fn starter_three() -> Self {
        Self::from_entries(vec![
            FamilyEntry {
                family: "contacts".into(),
                predicate: "contact.person".into(),
                name_field: "display_name".into(),
                layout: PathLayout::ByName,
            },
            FamilyEntry {
                family: "people".into(),
                predicate: "person.generic".into(),
                name_field: "display_name".into(),
                layout: PathLayout::ByName,
            },
            FamilyEntry {
                family: "notes".into(),
                predicate: "note".into(),
                name_field: "title".into(),
                layout: PathLayout::ByName,
            },
        ])
    }
}

/// Reverse map a predicate name to its path-library family through the
/// table. Returns `None` for predicates whose spec declares no `[path]`
/// (e.g., `capability.grant`, `auditor.daily_summary`, `affiliation`);
/// those atoms are inspectable via `atom.get` but have no projection-path
/// home.
pub fn family_for_predicate(table: &FamilyTable, predicate: &PredicateName) -> Option<PathFamily> {
    table.for_predicate(predicate)
}

/// Produce the canonical projection path for `(family, basename)`:
/// `<folder>/by-name/<letter>/<basename>.md` for a `by_name` family,
/// `<folder>/<basename>.md` for a `flat` one (task_41). For `by_name`,
/// returns `None` when the basename starts with a character that has
/// no uppercased alphabetic form (an empty basename or a leading digit;
/// those belong in a `flat` family).
pub fn path_for_basename(family: &PathFamily, basename: &str) -> Option<String> {
    if family.layout == PathLayout::Flat {
        if basename.is_empty() || basename.contains('/') {
            return None;
        }
        return Some(format!("{}/{}.md", family.folder, basename));
    }
    let first = basename.chars().next()?;
    let letter = first.to_uppercase().next()?;
    if !letter.is_alphabetic() {
        return None;
    }
    Some(format!(
        "{}/by-name/{}/{}.md",
        family.folder, letter, basename
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedPath {
    /// `<family>/recent/`
    Recent { family: PathFamily },
    /// `<family>/by-name/<letter>/`
    AlphabeticalLetter { family: PathFamily, letter: String },
    /// `<family>/by-name/<letter>/<basename>.md`. `basename` is the file
    /// stem, not the entity id: the path-to-entity index (ADR-030)
    /// resolves it, and a basename with no index row resolves to the
    /// slug-form entity id of the same spelling.
    SingleEntity {
        family: PathFamily,
        basename: String,
    },
    /// Recognized family, unknown sub-path shape. Parser still returns
    /// the family so callers can produce a useful error.
    Unsupported { family: PathFamily, raw: String },
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("empty projection path")]
    Empty,
    #[error("unknown path family: {0}")]
    UnknownFamily(String),
    #[error("malformed alphabetical-letter segment: {0}")]
    BadLetter(String),
}

/// Parse a projection path string. Accepts both `<family>/...` and
/// `/<family>/...` forms; trailing slashes and the leading slash are
/// normalized away. Backslash separators are normalized to forward
/// slashes via [`normalize_separators`] so a path lifted from
/// `Path::to_string_lossy()` on Windows parses identically to the
/// canonical `/`-shaped form.
pub fn parse(path: &str, table: &FamilyTable) -> Result<ParsedPath, PathError> {
    let canonical = normalize_separators(path);
    let trimmed = canonical.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(PathError::Empty);
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    let family = PathFamily::try_parse(parts[0], table)
        .ok_or_else(|| PathError::UnknownFamily(parts[0].into()))?;

    match parts.as_slice() {
        [_] => Ok(ParsedPath::Unsupported {
            family,
            raw: trimmed.into(),
        }),
        [_, "recent"] => Ok(ParsedPath::Recent { family }),
        // A flat family (task_41) files entities directly under its
        // folder: `briefings/2026-09-20.md`.
        [_, file] if family.layout == PathLayout::Flat && file.ends_with(".md") => {
            let basename = file.strip_suffix(".md").unwrap_or(file);
            Ok(ParsedPath::SingleEntity {
                family,
                basename: basename.to_string(),
            })
        }
        [_, "by-name", letter] => {
            if letter.chars().count() != 1 {
                return Err(PathError::BadLetter((*letter).into()));
            }
            Ok(ParsedPath::AlphabeticalLetter {
                family,
                letter: letter.to_uppercase(),
            })
        }
        [_, "by-name", letter, file] => {
            if letter.chars().count() != 1 {
                return Err(PathError::BadLetter((*letter).into()));
            }
            // Filename without trailing `.md` is the basename; the
            // renderer resolves it to an entity id through the index.
            let basename = file.strip_suffix(".md").unwrap_or(file);
            Ok(ParsedPath::SingleEntity {
                family,
                basename: basename.to_string(),
            })
        }
        _ => Ok(ParsedPath::Unsupported {
            family,
            raw: trimmed.into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    // Construction mapping for the registry-backed refactor (task_38):
    // `PathFamily::Contacts` became `contacts()` (the starter table's
    // row), and a `SingleEntity { entity: EntityId::new("X") }`
    // assertion became `SingleEntity { basename: "X".into() }`. Every
    // assertion keeps its meaning; only how the expected value is
    // built changed.
    use super::*;
    use crate::atom::EntityId;

    fn table() -> FamilyTable {
        FamilyTable::starter_three()
    }
    fn contacts() -> PathFamily {
        table().for_folder("contacts").unwrap()
    }
    fn people() -> PathFamily {
        table().for_folder("people").unwrap()
    }
    fn notes() -> PathFamily {
        table().for_folder("notes").unwrap()
    }
    fn parse(path: &str) -> Result<ParsedPath, PathError> {
        super::parse(path, &table())
    }

    #[test]
    fn parse_recent() {
        assert_eq!(
            parse("contacts/recent/").unwrap(),
            ParsedPath::Recent { family: contacts() }
        );
    }

    #[test]
    fn parse_alphabetical_letter() {
        let p = parse("contacts/by-name/S/").unwrap();
        assert_eq!(
            p,
            ParsedPath::AlphabeticalLetter {
                family: contacts(),
                letter: "S".into()
            }
        );
    }

    #[test]
    fn alphabetical_lowercases_to_upper() {
        let p = parse("contacts/by-name/s/").unwrap();
        assert!(matches!(p, ParsedPath::AlphabeticalLetter { letter, .. } if letter == "S"));
    }

    #[test]
    fn parse_single_entity_strips_md_suffix() {
        let p = parse("contacts/by-name/S/Sarah_Chen.md").unwrap();
        assert_eq!(
            p,
            ParsedPath::SingleEntity {
                family: contacts(),
                basename: "Sarah_Chen".into()
            }
        );
    }

    #[test]
    fn parse_leading_slash_tolerated() {
        let p = parse("/contacts/recent").unwrap();
        assert_eq!(p, ParsedPath::Recent { family: contacts() });
    }

    #[test]
    fn unknown_family_rejected() {
        let err = parse("decisions/recent/").unwrap_err();
        assert!(matches!(err, PathError::UnknownFamily(_)));
    }

    #[test]
    fn empty_path_rejected() {
        assert!(matches!(parse("/").unwrap_err(), PathError::Empty));
        assert!(matches!(parse("").unwrap_err(), PathError::Empty));
    }

    #[test]
    fn multi_char_letter_rejected() {
        assert!(matches!(
            parse("contacts/by-name/SA/").unwrap_err(),
            PathError::BadLetter(_)
        ));
    }

    #[test]
    fn unrecognized_subpath_classifies_as_unsupported() {
        let p = parse("contacts/by-org/AcmeCorp/").unwrap();
        assert!(matches!(p, ParsedPath::Unsupported { .. }));
    }

    #[test]
    fn primary_predicates_match_adr_011() {
        assert_eq!(contacts().primary_predicate().as_str(), "contact.person");
        assert_eq!(people().primary_predicate().as_str(), "person.generic");
        assert_eq!(notes().primary_predicate().as_str(), "note");
    }

    #[test]
    fn family_for_predicate_handles_the_three_mvp_predicates() {
        assert_eq!(
            family_for_predicate(&table(), &PredicateName::new("contact.person")),
            Some(contacts())
        );
        assert_eq!(
            family_for_predicate(&table(), &PredicateName::new("person.generic")),
            Some(people())
        );
        assert_eq!(
            family_for_predicate(&table(), &PredicateName::new("note")),
            Some(notes())
        );
    }

    #[test]
    fn family_for_predicate_returns_none_for_unmapped_predicates() {
        assert_eq!(
            family_for_predicate(&table(), &PredicateName::new("capability.grant")),
            None
        );
        assert_eq!(
            family_for_predicate(&table(), &PredicateName::new("auditor.daily_summary")),
            None
        );
    }

    #[test]
    fn path_for_entity_produces_canonical_form() {
        let p = path_for_basename(&contacts(), EntityId::new("Sara_Chen").as_str())
            .expect("alpha entity has a path");
        assert_eq!(p, "contacts/by-name/S/Sara_Chen.md");
    }

    #[test]
    fn path_for_entity_uppercases_first_letter() {
        let p = path_for_basename(&notes(), EntityId::new("tuesday_standup").as_str())
            .expect("alpha entity has a path");
        assert_eq!(p, "notes/by-name/T/tuesday_standup.md");
    }

    // ---- Backslash normalization (task_34) ----

    #[test]
    fn normalize_separators_is_a_no_op_for_forward_slash_strings() {
        let s = "contacts/by-name/S/Sarah_Chen.md";
        let out = normalize_separators(s);
        assert_eq!(out, s);
        // Same allocation: the no-backslash short-circuit returns
        // Borrowed, not Owned.
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn normalize_separators_replaces_backslashes_with_forward_slashes() {
        let out = normalize_separators(r"contacts\by-name\S\Sarah_Chen.md");
        assert_eq!(out, "contacts/by-name/S/Sarah_Chen.md");
        assert!(matches!(out, std::borrow::Cow::Owned(_)));
    }

    #[test]
    fn normalize_separators_handles_mixed_separators() {
        // Defensive: if someone hands us a half-normalized string,
        // make sure the result is still fully `/`-shaped.
        let out = normalize_separators(r"contacts\by-name/S\Sarah_Chen.md");
        assert_eq!(out, "contacts/by-name/S/Sarah_Chen.md");
    }

    #[test]
    fn parse_accepts_backslash_separated_paths() {
        // This is the Windows CI failure shape — without
        // normalization, parse() classifies as Unknown family
        // because parts[0] = `contacts\by-name\S\Sarah_Chen.md`.
        let p = parse(r"contacts\by-name\S\Sarah_Chen.md").unwrap();
        assert_eq!(
            p,
            ParsedPath::SingleEntity {
                family: contacts(),
                basename: "Sarah_Chen".into(),
            }
        );
    }

    #[test]
    fn parse_backslash_path_matches_forward_slash_path() {
        // Regression guard: the two separator shapes must produce
        // exactly equal ParsedPath values so downstream code (the
        // fastpath classifier, the reverse-map matcher, the
        // dispatch event payloads) doesn't have to branch on host.
        let bs = parse(r"contacts\by-name\S\Sarah_Chen.md").unwrap();
        let fs = parse("contacts/by-name/S/Sarah_Chen.md").unwrap();
        assert_eq!(bs, fs);
    }

    #[test]
    fn path_for_entity_returns_none_for_non_alphabetic_first_char() {
        assert_eq!(
            path_for_basename(&contacts(), EntityId::new("123_numeric").as_str()),
            None
        );
        assert_eq!(
            path_for_basename(&contacts(), EntityId::new("").as_str()),
            None
        );
    }

    // ---- Registry-backed families (task_38, ADR-028) ----

    #[test]
    fn unknown_folder_is_rejected_by_the_table() {
        assert!(table().for_folder("orgs").is_none());
        assert!(matches!(
            parse("orgs/recent/").unwrap_err(),
            PathError::UnknownFamily(f) if f == "orgs"
        ));
    }

    #[test]
    fn a_widgets_family_from_a_fixture_entry_parses_without_code_changes() {
        let mut entries = table().all();
        entries.push(PathFamily {
            folder: "widgets".into(),
            predicate: PredicateName::new("widget.thing"),
            name_field: "display_name".into(),
            layout: PathLayout::ByName,
        });
        let t = FamilyTable::from_entries(
            entries
                .into_iter()
                .map(|f| FamilyEntry {
                    family: f.folder,
                    predicate: f.predicate.as_str().to_string(),
                    name_field: f.name_field,
                    layout: f.layout,
                })
                .collect(),
        );
        let p = super::parse("widgets/by-name/W/Widget.md", &t).unwrap();
        assert_eq!(
            p,
            ParsedPath::SingleEntity {
                family: t.for_folder("widgets").unwrap(),
                basename: "Widget".into(),
            }
        );
        assert_eq!(
            path_for_basename(&t.for_folder("widgets").unwrap(), "Widget").unwrap(),
            "widgets/by-name/W/Widget.md"
        );
        assert_eq!(
            t.for_predicate(&PredicateName::new("widget.thing"))
                .unwrap()
                .folder,
            "widgets"
        );
    }

    #[test]
    fn family_table_from_registry_reflects_declared_path_tables() {
        let dir = tempfile::tempdir().unwrap();
        let starter =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../starter/predicates");
        for name in ["contact.person.toml", "person.generic.toml", "note.toml"] {
            std::fs::copy(starter.join(name), dir.path().join(name)).unwrap();
        }
        let reg = SpecRegistry::new();
        reg.load_dir(dir.path()).unwrap();
        let t = FamilyTable::from_registry(&reg);
        assert_eq!(t.all(), table().all());
    }
}
