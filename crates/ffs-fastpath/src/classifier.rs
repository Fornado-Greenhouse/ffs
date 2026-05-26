//! Pure diff classifier. Takes the substrate's last-rendered projection
//! plus the on-disk content after a user edit and returns a structured
//! `Classification` — either a fast-path supersession (with the modified
//! claim ready to sign) or a slow-path route-to-ingest reason.
//!
//! Diff shapes recognized at MVP:
//! - **Frontmatter value change**: exactly one key in the YAML-style
//!   frontmatter changed value.
//! - **Section bullet appended**: exactly one `- ...` line appended to an
//!   additive section in the body.
//!
//! Anything else (multi-line change, deletion, structural rewrite) routes
//! to the slow path. Per ADR-014 this is the conservative MVP envelope;
//! richer diff handling is Phase 2.

use ffs_core::AtomEnvelope;
use ffs_core::predicate::{EditKind, PredicateSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// The diff matches a reverse-map rule; build the supersession atom.
    Applied {
        edit_kind: EditKind,
        rule_output: String,
        modified_claim: serde_json::Value,
    },
    /// The diff does not match cleanly; route to ingest as a correction.
    RoutedToIngest { reason: SlowPathReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlowPathReason {
    /// Multi-line / structural diff.
    AmbiguousDiff,
    /// Projection lives under `from/<peer>/`.
    FederatedProjection,
    /// The predicate spec has no reverse-map rules.
    NoReverseMapRules,
    /// The diff matches more than one rule.
    AmbiguousRuleMatch,
    /// No reverse-map rule matches the diff shape.
    NoMatchingRule,
    /// Could not parse the projection path or the head atom is missing.
    PathOrHeadUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Markdown<'a> {
    /// Ordered `(key, value)` pairs of the YAML-style frontmatter.
    frontmatter: Vec<(String, String)>,
    /// Body lines, by section. The empty string keys the implicit preamble
    /// section (any body text before the first `## ` header).
    sections: Vec<(String, Vec<&'a str>)>,
}

fn parse_markdown(content: &str) -> Markdown<'_> {
    let mut frontmatter: Vec<(String, String)> = Vec::new();
    let mut body_iter: Vec<&str> = content.lines().collect();
    // Frontmatter detection: must start with a `---` line.
    if body_iter.first().is_some_and(|l| l.trim() == "---") {
        let end_idx = body_iter
            .iter()
            .skip(1)
            .position(|l| l.trim() == "---")
            .map(|p| p + 1);
        if let Some(end) = end_idx {
            for line in body_iter.iter().take(end).skip(1) {
                if let Some((k, v)) = line.split_once(':') {
                    frontmatter.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            // Body lines start after the second `---`.
            body_iter = body_iter.split_off(end + 1);
        }
    }

    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut current: (String, Vec<&str>) = (String::new(), Vec::new());
    let is_meaningful = |s: &(String, Vec<&str>)| -> bool {
        !s.0.is_empty() || s.1.iter().any(|l| !l.trim().is_empty())
    };
    for line in body_iter {
        if let Some(name) = line.strip_prefix("## ") {
            if is_meaningful(&current) {
                sections.push(current);
            }
            current = (name.trim().to_string(), Vec::new());
        } else {
            current.1.push(line);
        }
    }
    if is_meaningful(&current) {
        sections.push(current);
    }

    Markdown {
        frontmatter,
        sections,
    }
}

/// Compute the shape of the difference between `old` and `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffShape {
    /// One frontmatter key changed value.
    FrontmatterValueChange { key: String, new_value: String },
    /// One bullet was appended to a section's list.
    SectionBulletAppended { section: String, new_bullet: String },
    /// Anything else.
    Other,
}

fn diff(old: &str, new: &str) -> DiffShape {
    let old_md = parse_markdown(old);
    let new_md = parse_markdown(new);

    // Find any frontmatter differences.
    let mut fm_changes: Vec<(String, String)> = Vec::new();
    if old_md.frontmatter.len() == new_md.frontmatter.len() {
        for (i, (k_new, v_new)) in new_md.frontmatter.iter().enumerate() {
            let (k_old, v_old) = &old_md.frontmatter[i];
            if k_old == k_new && v_old != v_new {
                fm_changes.push((k_new.clone(), v_new.clone()));
            } else if k_old != k_new {
                // Key order changed → ambiguous.
                return DiffShape::Other;
            }
        }
    } else {
        return DiffShape::Other;
    }

    // Find section bullet additions.
    let mut bullet_additions: Vec<(String, String)> = Vec::new();
    if old_md.sections.len() == new_md.sections.len() {
        for ((name_old, lines_old), (name_new, lines_new)) in
            old_md.sections.iter().zip(new_md.sections.iter())
        {
            if name_old != name_new {
                return DiffShape::Other;
            }
            // Lines may differ if a single new bullet was appended OR if the
            // section is unchanged. Trim blank trailing lines for comparison.
            let trimmed_old: Vec<&&str> = lines_old
                .iter()
                .rev()
                .skip_while(|l| l.trim().is_empty())
                .collect();
            let trimmed_new: Vec<&&str> = lines_new
                .iter()
                .rev()
                .skip_while(|l| l.trim().is_empty())
                .collect();
            if trimmed_old.len() == trimmed_new.len() {
                // Section unchanged (modulo trailing blanks).
                continue;
            }
            if trimmed_new.len() == trimmed_old.len() + 1 {
                // Could be an appended bullet. Reverse to original order.
                let old_lines: Vec<&str> = trimmed_old.into_iter().rev().copied().collect();
                let new_lines: Vec<&str> = trimmed_new.into_iter().rev().copied().collect();
                // The first `old_lines.len()` lines of new should equal old.
                if new_lines[..old_lines.len()] == old_lines[..] {
                    let added = new_lines.last().unwrap();
                    if let Some(bullet) = added.strip_prefix("- ") {
                        bullet_additions.push((name_new.clone(), bullet.trim().to_string()));
                        continue;
                    }
                }
            }
            // Anything else in this section is ambiguous.
            return DiffShape::Other;
        }
    } else {
        return DiffShape::Other;
    }

    // Exactly one change overall is a clean match.
    match (fm_changes.len(), bullet_additions.len()) {
        (1, 0) => {
            let (key, new_value) = fm_changes.pop().unwrap();
            DiffShape::FrontmatterValueChange { key, new_value }
        }
        (0, 1) => {
            let (section, new_bullet) = bullet_additions.pop().unwrap();
            DiffShape::SectionBulletAppended {
                section,
                new_bullet,
            }
        }
        _ => DiffShape::Other,
    }
}

/// Match a diff shape against the spec's reverse-map rules. Returns the
/// matched rule's output string, edit kind, and the modified claim.
pub fn classify(
    spec: &PredicateSpec,
    head_claim: &serde_json::Value,
    old_markdown: &str,
    new_content: &str,
) -> Classification {
    if spec.reverse_map.is_empty() {
        return Classification::RoutedToIngest {
            reason: SlowPathReason::NoReverseMapRules,
        };
    }
    let shape = diff(old_markdown, new_content);
    let target_output = match &shape {
        DiffShape::FrontmatterValueChange { key, .. } => format!("frontmatter.{key}"),
        DiffShape::SectionBulletAppended { section, .. } => {
            format!("section.{section}.list_item")
        }
        DiffShape::Other => {
            return Classification::RoutedToIngest {
                reason: SlowPathReason::AmbiguousDiff,
            };
        }
    };
    let matches: Vec<_> = spec
        .reverse_map
        .iter()
        .filter(|r| r.output == target_output)
        .collect();
    if matches.is_empty() {
        return Classification::RoutedToIngest {
            reason: SlowPathReason::NoMatchingRule,
        };
    }
    if matches.len() > 1 {
        return Classification::RoutedToIngest {
            reason: SlowPathReason::AmbiguousRuleMatch,
        };
    }
    let rule = matches[0];
    let modified_claim = match apply_to_claim(head_claim, &rule.atom_field, &shape) {
        Some(c) => c,
        None => {
            return Classification::RoutedToIngest {
                reason: SlowPathReason::NoMatchingRule,
            };
        }
    };
    Classification::Applied {
        edit_kind: rule.edit_kind,
        rule_output: rule.output.clone(),
        modified_claim,
    }
}

fn apply_to_claim(
    head_claim: &serde_json::Value,
    atom_field: &str,
    shape: &DiffShape,
) -> Option<serde_json::Value> {
    let rest = atom_field.strip_prefix("claim.")?;
    let mut claim = head_claim.clone();
    let obj = claim.as_object_mut()?;
    if let Some(field_name) = rest.strip_suffix("[]") {
        // Array append.
        match shape {
            DiffShape::SectionBulletAppended { new_bullet, .. } => {
                let entry = serde_json::Value::String(new_bullet.clone());
                match obj.get_mut(field_name) {
                    Some(serde_json::Value::Array(arr)) => arr.push(entry),
                    _ => {
                        obj.insert(
                            field_name.to_string(),
                            serde_json::Value::Array(vec![entry]),
                        );
                    }
                }
            }
            _ => return None,
        }
    } else {
        // Scalar set.
        match shape {
            DiffShape::FrontmatterValueChange { new_value, .. } => {
                obj.insert(
                    rest.to_string(),
                    serde_json::Value::String(new_value.clone()),
                );
            }
            _ => return None,
        }
    }
    Some(claim)
}

/// Return true if the relative projection path is a federated projection
/// (under `from/<peer>/`) and therefore not fast-path eligible.
pub fn is_federated_path(path: &str) -> bool {
    // Match `<family>/from/<peer>/...` or `from/<peer>/...` as a prefix.
    path.split('/').any(|seg| seg == "from")
}

/// Render the head atom for a projection back to markdown so the
/// classifier can diff it against the on-disk content. The renderer's
/// reverse-map annotations come back too — currently unused here but
/// helpful for richer diff strategies in Phase 2.
pub fn rendered_markdown_for_head(_head: &AtomEnvelope, rendered: String) -> String {
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffs_core::predicate::{EditKind, RenderingConvention, ReverseMapRule};

    fn spec_with_rules(rules: Vec<ReverseMapRule>) -> PredicateSpec {
        PredicateSpec {
            name: "contact.person".into(),
            version: 1,
            parent_predicate: None,
            claim_schema: serde_json::json!({"type": "object"}),
            rendering: RenderingConvention {
                template: "contact-person.md.tera".into(),
                frontmatter_fields: vec!["display_name".into(), "tier".into()],
                body_sections: vec!["Notes".into()],
                additive_sections: vec!["Notes".into()],
            },
            reverse_map: rules,
            pagination: None,
        }
    }

    fn rule(output: &str, atom_field: &str, kind: EditKind) -> ReverseMapRule {
        ReverseMapRule {
            output: output.into(),
            atom_field: atom_field.into(),
            edit_kind: kind,
        }
    }

    #[test]
    fn parses_frontmatter_and_sections() {
        let md = parse_markdown("---\nkey: value\n---\n\n## Notes\n- one\n- two\n");
        assert_eq!(md.frontmatter, vec![("key".into(), "value".into())]);
        assert_eq!(md.sections.len(), 1);
        assert_eq!(md.sections[0].0, "Notes");
        assert!(md.sections[0].1.contains(&"- one"));
    }

    #[test]
    fn detects_frontmatter_value_change() {
        let old = "---\ndisplay_name: Sarah\ntier: introducible\n---\n";
        let new = "---\ndisplay_name: Sara\ntier: introducible\n---\n";
        let shape = diff(old, new);
        assert_eq!(
            shape,
            DiffShape::FrontmatterValueChange {
                key: "display_name".into(),
                new_value: "Sara".into()
            }
        );
    }

    #[test]
    fn detects_section_bullet_appended() {
        let old = "---\nx: y\n---\n## Notes\n- one\n";
        let new = "---\nx: y\n---\n## Notes\n- one\n- two\n";
        let shape = diff(old, new);
        assert_eq!(
            shape,
            DiffShape::SectionBulletAppended {
                section: "Notes".into(),
                new_bullet: "two".into()
            }
        );
    }

    #[test]
    fn ambiguous_when_two_changes_at_once() {
        let old = "---\na: 1\nb: 2\n---\n";
        let new = "---\na: x\nb: y\n---\n";
        let shape = diff(old, new);
        assert_eq!(shape, DiffShape::Other);
    }

    #[test]
    fn ambiguous_when_section_rewritten() {
        let old = "---\nx: y\n---\n## Notes\n- one\n- two\n";
        let new = "---\nx: y\n---\n## Notes\n- something completely different\n";
        let shape = diff(old, new);
        assert_eq!(shape, DiffShape::Other);
    }

    #[test]
    fn classify_single_line_text_via_frontmatter_rule() {
        let s = spec_with_rules(vec![rule(
            "frontmatter.display_name",
            "claim.display_name",
            EditKind::SingleLineText,
        )]);
        let head = serde_json::json!({"display_name": "Sarah"});
        let old = "---\ndisplay_name: Sarah\n---\n";
        let new = "---\ndisplay_name: Sara\n---\n";
        let c = classify(&s, &head, old, new);
        match c {
            Classification::Applied {
                edit_kind,
                modified_claim,
                ..
            } => {
                assert_eq!(edit_kind, EditKind::SingleLineText);
                assert_eq!(modified_claim["display_name"], "Sara");
            }
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    #[test]
    fn classify_frontmatter_value_kind() {
        let s = spec_with_rules(vec![rule(
            "frontmatter.tier",
            "claim.tier",
            EditKind::FrontmatterValue,
        )]);
        let head = serde_json::json!({"tier": "introducible"});
        let old = "---\ntier: introducible\n---\n";
        let new = "---\ntier: discreet\n---\n";
        let c = classify(&s, &head, old, new);
        match c {
            Classification::Applied {
                edit_kind,
                modified_claim,
                ..
            } => {
                assert_eq!(edit_kind, EditKind::FrontmatterValue);
                assert_eq!(modified_claim["tier"], "discreet");
            }
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    #[test]
    fn classify_additive_section_bullet() {
        let s = spec_with_rules(vec![rule(
            "section.Notes.list_item",
            "claim.notes[]",
            EditKind::AdditiveSection,
        )]);
        let head = serde_json::json!({"notes": ["one"]});
        let old = "---\nx: y\n---\n## Notes\n- one\n";
        let new = "---\nx: y\n---\n## Notes\n- one\n- two\n";
        let c = classify(&s, &head, old, new);
        match c {
            Classification::Applied {
                edit_kind,
                modified_claim,
                ..
            } => {
                assert_eq!(edit_kind, EditKind::AdditiveSection);
                assert_eq!(modified_claim["notes"], serde_json::json!(["one", "two"]));
            }
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    #[test]
    fn no_reverse_map_routes_to_ingest() {
        let s = spec_with_rules(vec![]);
        let head = serde_json::json!({});
        let c = classify(&s, &head, "", "");
        assert_eq!(
            c,
            Classification::RoutedToIngest {
                reason: SlowPathReason::NoReverseMapRules
            }
        );
    }

    #[test]
    fn ambiguous_diff_routes_to_ingest() {
        let s = spec_with_rules(vec![rule(
            "frontmatter.display_name",
            "claim.display_name",
            EditKind::SingleLineText,
        )]);
        let head = serde_json::json!({"display_name": "Sarah"});
        let old = "---\na: 1\nb: 2\n---\n";
        let new = "---\na: x\nb: y\n---\n";
        let c = classify(&s, &head, old, new);
        assert!(matches!(
            c,
            Classification::RoutedToIngest {
                reason: SlowPathReason::AmbiguousDiff
            }
        ));
    }

    #[test]
    fn no_matching_rule_routes_to_ingest() {
        let s = spec_with_rules(vec![rule(
            "frontmatter.display_name",
            "claim.display_name",
            EditKind::SingleLineText,
        )]);
        let head = serde_json::json!({"work_email": "x"});
        let old = "---\nwork_email: x\n---\n";
        let new = "---\nwork_email: y\n---\n";
        let c = classify(&s, &head, old, new);
        assert!(matches!(
            c,
            Classification::RoutedToIngest {
                reason: SlowPathReason::NoMatchingRule
            }
        ));
    }

    #[test]
    fn federated_path_detected() {
        assert!(is_federated_path("contacts/from/alice/by-name/S/Sarah.md"));
        assert!(is_federated_path("from/alice/x"));
        assert!(!is_federated_path("contacts/by-name/S/Sarah.md"));
    }
}
