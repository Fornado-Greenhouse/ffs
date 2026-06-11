//! Projection-path parsing. Translates filesystem-style paths under
//! `~/.ffs/<family>/...` into a structured `ParsedPath` that the renderer
//! turns into store queries.
//!
//! MVP shapes supported (per ADR-011 — three families):
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

use crate::atom::{EntityId, PredicateName};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathFamily {
    Contacts,
    People,
    Notes,
}

impl PathFamily {
    /// Parse a family token from the leading path segment. Named
    /// `try_parse` rather than `from_str` to avoid colliding with the
    /// `std::str::FromStr` trait (which would require `Err` typing).
    pub fn try_parse(s: &str) -> Option<Self> {
        match s {
            "contacts" => Some(Self::Contacts),
            "people" => Some(Self::People),
            "notes" => Some(Self::Notes),
            _ => None,
        }
    }

    /// The primary predicate name for this path family. Atoms with this
    /// predicate appear in the family's listings; a single-entity render
    /// for this family reads the head atom for `(entity, primary_predicate)`.
    pub fn primary_predicate(&self) -> PredicateName {
        match self {
            Self::Contacts => PredicateName::new("contact.person"),
            Self::People => PredicateName::new("person.generic"),
            Self::Notes => PredicateName::new("note"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Contacts => "contacts",
            Self::People => "people",
            Self::Notes => "notes",
        }
    }
}

/// Reverse map a predicate name to its path-library family. Returns
/// `None` for predicates outside the MVP three-family library
/// (e.g., `capability.grant`, `auditor.daily_summary`); those atoms
/// are inspectable via `atom.get` but have no projection-path home
/// in MVP.
pub fn family_for_predicate(predicate: &PredicateName) -> Option<PathFamily> {
    match predicate.as_str() {
        "contact.person" => Some(PathFamily::Contacts),
        "person.generic" => Some(PathFamily::People),
        "note" => Some(PathFamily::Notes),
        _ => None,
    }
}

/// Produce the canonical projection path for `(family, entity)` in
/// the form `<family>/by-name/<letter>/<entity>.md`. Returns `None`
/// when the entity id starts with a character that has no
/// uppercased form (e.g., an empty entity, or one whose first
/// codepoint already has no alphabetic mapping — the path library
/// has no destination for those at MVP).
pub fn path_for_entity(family: PathFamily, entity: &EntityId) -> Option<String> {
    let first = entity.as_str().chars().next()?;
    let letter = first.to_uppercase().next()?;
    if !letter.is_alphabetic() {
        return None;
    }
    Some(format!(
        "{}/by-name/{}/{}.md",
        family.as_str(),
        letter,
        entity.as_str()
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedPath {
    /// `<family>/recent/`
    Recent { family: PathFamily },
    /// `<family>/by-name/<letter>/`
    AlphabeticalLetter { family: PathFamily, letter: String },
    /// `<family>/by-name/<letter>/<entity>.md`
    SingleEntity {
        family: PathFamily,
        entity: EntityId,
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
pub fn parse(path: &str) -> Result<ParsedPath, PathError> {
    let canonical = normalize_separators(path);
    let trimmed = canonical.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(PathError::Empty);
    }
    let parts: Vec<&str> = trimmed.split('/').collect();
    let family =
        PathFamily::try_parse(parts[0]).ok_or_else(|| PathError::UnknownFamily(parts[0].into()))?;

    match parts.as_slice() {
        [_] => Ok(ParsedPath::Unsupported {
            family,
            raw: trimmed.into(),
        }),
        [_, "recent"] => Ok(ParsedPath::Recent { family }),
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
            // Filename without trailing `.md` becomes the entity id.
            let entity_name = file.strip_suffix(".md").unwrap_or(file);
            Ok(ParsedPath::SingleEntity {
                family,
                entity: EntityId::new(entity_name),
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
    use super::*;

    #[test]
    fn parse_recent() {
        assert_eq!(
            parse("contacts/recent/").unwrap(),
            ParsedPath::Recent {
                family: PathFamily::Contacts
            }
        );
    }

    #[test]
    fn parse_alphabetical_letter() {
        let p = parse("contacts/by-name/S/").unwrap();
        assert_eq!(
            p,
            ParsedPath::AlphabeticalLetter {
                family: PathFamily::Contacts,
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
                family: PathFamily::Contacts,
                entity: EntityId::new("Sarah_Chen")
            }
        );
    }

    #[test]
    fn parse_leading_slash_tolerated() {
        let p = parse("/contacts/recent").unwrap();
        assert_eq!(
            p,
            ParsedPath::Recent {
                family: PathFamily::Contacts
            }
        );
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
        assert_eq!(
            PathFamily::Contacts.primary_predicate().as_str(),
            "contact.person"
        );
        assert_eq!(
            PathFamily::People.primary_predicate().as_str(),
            "person.generic"
        );
        assert_eq!(PathFamily::Notes.primary_predicate().as_str(), "note");
    }

    #[test]
    fn family_for_predicate_handles_the_three_mvp_predicates() {
        assert_eq!(
            family_for_predicate(&PredicateName::new("contact.person")),
            Some(PathFamily::Contacts)
        );
        assert_eq!(
            family_for_predicate(&PredicateName::new("person.generic")),
            Some(PathFamily::People)
        );
        assert_eq!(
            family_for_predicate(&PredicateName::new("note")),
            Some(PathFamily::Notes)
        );
    }

    #[test]
    fn family_for_predicate_returns_none_for_unmapped_predicates() {
        assert_eq!(
            family_for_predicate(&PredicateName::new("capability.grant")),
            None
        );
        assert_eq!(
            family_for_predicate(&PredicateName::new("auditor.daily_summary")),
            None
        );
    }

    #[test]
    fn path_for_entity_produces_canonical_form() {
        let p = path_for_entity(PathFamily::Contacts, &EntityId::new("Sara_Chen"))
            .expect("alpha entity has a path");
        assert_eq!(p, "contacts/by-name/S/Sara_Chen.md");
    }

    #[test]
    fn path_for_entity_uppercases_first_letter() {
        let p = path_for_entity(PathFamily::Notes, &EntityId::new("tuesday_standup"))
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
                family: PathFamily::Contacts,
                entity: EntityId::new("Sarah_Chen"),
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
            path_for_entity(PathFamily::Contacts, &EntityId::new("123_numeric")),
            None
        );
        assert_eq!(
            path_for_entity(PathFamily::Contacts, &EntityId::new("")),
            None
        );
    }
}
