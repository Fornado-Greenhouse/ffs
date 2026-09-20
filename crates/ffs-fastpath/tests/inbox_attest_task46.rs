//! task_46 (ADR-034): the inbox's "Past their window" lines parse into
//! attestation decisions, "all unchanged" expands over the untouched
//! `unchanged` lines, and an undo line may carry a correction reason.

use ffs_fastpath::inbox::{DecisionAction, parse_inbox};

const FILE: &str = r#"---
date: 2026-09-21
pending: 0
---

# Inbox 2026-09-21

## Housekeeping

### Past their window

- Sara Chen (person.generic): last confirmed 2026-06-01, confirmed by you alone
  - [x] unchanged <!-- attest:zAtomA basis:owner_knowledge -->
  - [ ] re-read source <!-- attest:zAtomA basis:re_read_same_source -->
- Acme Widgets (org.company): last confirmed 2026-03-01
  - [ ] unchanged <!-- attest:zAtomB basis:owner_knowledge -->
  - [X] re-read source <!-- attest:zAtomB basis:re_read_same_source -->
- Pat Example (person.generic): last confirmed never
  - [ ] unchanged <!-- attest:zAtomC basis:owner_knowledge -->
  - [ ] re-read source <!-- attest:zAtomC basis:re_read_same_source -->
- [ ] all unchanged <!-- attest-all:2026-09-21 -->

## Auto-filed today

- [x] undo org.company Blue Ridge <!-- retract:zAutoHash reason:never_true -->

## Decided
"#;

fn attests(parsed: &ffs_fastpath::inbox::ParsedInbox) -> Vec<(String, String)> {
    parsed
        .decisions
        .iter()
        .filter_map(|d| match &d.action {
            DecisionAction::Attest { subject, basis } => Some((subject.clone(), basis.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn ticked_attest_lines_become_attestation_decisions_with_their_basis() {
    let parsed = parse_inbox(FILE);
    let got = attests(&parsed);
    assert_eq!(
        got,
        vec![
            ("zAtomA".to_string(), "owner_knowledge".to_string()),
            ("zAtomB".to_string(), "re_read_same_source".to_string()),
        ]
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn all_unchanged_expands_over_untouched_unchanged_lines_only() {
    let ticked = FILE.replace(
        "- [ ] all unchanged <!-- attest-all:2026-09-21 -->",
        "- [x] all unchanged <!-- attest-all:2026-09-21 -->",
    );
    let parsed = parse_inbox(&ticked);
    let got = attests(&parsed);
    // A's own tick, B's re-read tick, then the expansion: B's and C's
    // untouched `unchanged` lines (A's was ticked, so not repeated).
    assert_eq!(
        got,
        vec![
            ("zAtomA".to_string(), "owner_knowledge".to_string()),
            ("zAtomB".to_string(), "re_read_same_source".to_string()),
            ("zAtomB".to_string(), "owner_knowledge".to_string()),
            ("zAtomC".to_string(), "owner_knowledge".to_string()),
        ]
    );
}

#[test]
fn undo_line_carries_the_correction_reason() {
    let parsed = parse_inbox(FILE);
    let undo = parsed
        .decisions
        .iter()
        .find_map(|d| match &d.action {
            DecisionAction::Undo { atom_hash, reason } => Some((atom_hash.clone(), reason.clone())),
            _ => None,
        })
        .expect("an undo decision");
    assert_eq!(
        undo,
        ("zAutoHash".to_string(), Some("never_true".to_string()))
    );
}
