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

use crate::atom::{EntityId, PredicateName};

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
/// normalized away.
pub fn parse(path: &str) -> Result<ParsedPath, PathError> {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
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
}
