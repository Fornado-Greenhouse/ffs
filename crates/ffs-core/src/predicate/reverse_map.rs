//! Reverse-map rule types.
//!
//! Each rule says: "an edit to this rendered output element corresponds to
//! a supersession of this atom field with this edit kind." The fast-path
//! classifier (task 09) consumes these rules to translate user edits into
//! atom mutations.
//!
//! Output strings have one of three structural shapes:
//!
//! - `frontmatter.<field_name>` — a frontmatter field.
//! - `section.<section_name>` — the entire section body.
//! - `section.<section_name>.list_item` — a list item in an additive section.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReverseMapRule {
    /// e.g., "frontmatter.display_name", "section.Notes.list_item"
    pub output: String,
    /// e.g., "claim.display_name", "claim.notes[]"
    pub atom_field: String,
    pub edit_kind: EditKind,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EditKind {
    SingleLineText,
    FrontmatterValue,
    AdditiveSection,
}

/// Parsed shape of an `output` string. Used by `validate_outputs` to check
/// reverse-map rules reference defined rendering elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRef<'a> {
    /// `frontmatter.<field>`
    Frontmatter(&'a str),
    /// `section.<name>`
    Section(&'a str),
    /// `section.<name>.list_item`
    SectionListItem(&'a str),
}

impl<'a> OutputRef<'a> {
    pub fn parse(output: &'a str) -> Option<Self> {
        let parts: Vec<&str> = output.split('.').collect();
        match parts.as_slice() {
            ["frontmatter", field] => Some(OutputRef::Frontmatter(field)),
            ["section", name] => Some(OutputRef::Section(name)),
            ["section", name, "list_item"] => Some(OutputRef::SectionListItem(name)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter() {
        assert_eq!(
            OutputRef::parse("frontmatter.display_name"),
            Some(OutputRef::Frontmatter("display_name"))
        );
    }

    #[test]
    fn parse_section() {
        assert_eq!(
            OutputRef::parse("section.Notes"),
            Some(OutputRef::Section("Notes"))
        );
    }

    #[test]
    fn parse_section_list_item() {
        assert_eq!(
            OutputRef::parse("section.Notes.list_item"),
            Some(OutputRef::SectionListItem("Notes"))
        );
    }

    #[test]
    fn parse_unknown_shape() {
        assert_eq!(OutputRef::parse("body.something"), None);
        assert_eq!(OutputRef::parse("frontmatter"), None);
        assert_eq!(OutputRef::parse(""), None);
    }
}
