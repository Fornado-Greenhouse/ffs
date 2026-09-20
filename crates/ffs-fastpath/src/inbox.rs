//! The inbox decision parser (ADR-032 § Decision (2)): an edit to
//! `inbox/<date>.md` is read back as quarantine decisions. The file's
//! grammar is written by `ffs_daemon::inbox::render_inbox`; this module
//! recognizes ticked checkbox lines by the ids carried in their HTML
//! comments and turns them into [`InboxDecision`]s, which a
//! [`DecisionSink`] applies through the daemon's own RPCs. Nothing here
//! authors an atom directly and nothing routes to ingest: an inbox file
//! is a decision surface, never a projection edit.
//!
//! Tick recognition is deliberately loose (`- [x]`, `- [X]`, `* [x]`, or
//! any non-space character inside the brackets: editors choose the
//! case) while the block grammar is strict: two contradictory ticks in
//! one block are a parse warning and no decision.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The inbox folder under the data dir (ADR-032).
pub const INBOX_DIR: &str = "inbox";

/// A parse warning the inbox renderer shows under the section it
/// belongs to (ADR-032): contradictory ticks, a malformed block, an
/// accept-all over an untouched ambiguous child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseWarning {
    /// The submission id the warning belongs to, or empty.
    pub section: String,
    pub message: String,
}

/// What a ticked line asks the quarantine to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DecisionAction {
    /// `accept` (plain), `accept as <candidate>` (`resolved_entity` is
    /// the candidate's id), or `someone new` (`resolved_entity: "new"`).
    Accept {
        resolved_entity: Option<String>,
    },
    Reject,
    /// "these are different people": `a` is the entity the owner picked
    /// in the same block (an id, or `"new"` when they picked someone
    /// new), `b` the rejected candidate.
    AssertDifferent {
        a: String,
        b: String,
    },
    Merge {
        source: String,
        target: String,
    },
    /// Undo an auto-filed atom (retract by supersession).
    Undo {
        atom_hash: String,
    },
    Unmerge {
        same_as_hash: String,
    },
    /// The per-source "accept all under this article" line. The parser
    /// expands it into per-block accepts; a sink receiving it unexpanded
    /// refuses it.
    AcceptAllUnder {
        source_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxDecision {
    pub submission_id: Option<String>,
    pub local_ref: Option<String>,
    pub action: DecisionAction,
}

/// Result of parsing one inbox file: decisions to apply, in file order,
/// and warnings for blocks that could not be read as one decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedInbox {
    pub decisions: Vec<InboxDecision>,
    pub warnings: Vec<ParseWarning>,
}

/// True when `rel_path` (forward-slash, data-dir relative) is an inbox
/// file: `inbox/<anything>.md`.
pub fn is_inbox_path(rel_path: &str) -> bool {
    let p = rel_path.trim_start_matches('/');
    p.starts_with("inbox/") && p.ends_with(".md") && !p["inbox/".len()..].contains('/')
}

/// A checkbox line: `- [ ]` / `- [x]` / `* [X]` and anything non-space
/// inside the brackets counts as ticked.
fn checkbox(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("- [").or_else(|| t.strip_prefix("* ["))?;
    let mut chars = rest.chars();
    let mark = chars.next()?;
    let after = chars.as_str();
    let after = after.strip_prefix(']')?;
    Some((!mark.is_whitespace(), after.trim()))
}

/// The `key:value` pairs inside the line's trailing HTML comment, plus
/// bare words (`all`).
fn comment_fields(line: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(start) = line.rfind("<!--") else {
        return out;
    };
    let Some(end) = line[start..].find("-->") else {
        return out;
    };
    let inner = &line[start + 4..start + end];
    for tok in inner.split_whitespace() {
        match tok.split_once(':') {
            Some((k, v)) => {
                out.insert(k.to_string(), v.to_string());
            }
            None => {
                out.insert(tok.to_string(), String::new());
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Preamble,
    Source,
    Housekeeping,
    AutoFiled,
    Decided,
}

/// One `###` block's ticked lines, classified.
#[derive(Debug, Default)]
struct Block {
    sub: Option<String>,
    lref: Option<String>,
    plain_accept: bool,
    reject: bool,
    picks: Vec<String>,
    differents: Vec<String>,
    tick_count: usize,
    /// The block rendered candidate or different-people lines, so an
    /// untouched one under "accept all" is an ambiguous item with no
    /// pick.
    is_ambiguous_block: bool,
}

impl Block {
    fn has_own_tick(&self) -> bool {
        self.tick_count > 0
    }

    fn decisions(&self, warnings: &mut Vec<ParseWarning>) -> Vec<InboxDecision> {
        let section = self.sub.clone().unwrap_or_default();
        let mut out = Vec::new();
        if !self.has_own_tick() {
            return out;
        }
        let choices = usize::from(self.plain_accept) + usize::from(self.reject) + self.picks.len();
        if choices > 1 {
            warnings.push(ParseWarning {
                section,
                message: format!(
                    "block {} has {} contradictory ticks (accept, reject, or more than one candidate); left pending",
                    self.lref.as_deref().unwrap_or("?"),
                    choices
                ),
            });
            return out;
        }
        if choices == 0 {
            warnings.push(ParseWarning {
                section,
                message: format!(
                    "block {}: \"these are different people\" needs a candidate pick or \"someone new\" in the same block",
                    self.lref.as_deref().unwrap_or("?")
                ),
            });
            return out;
        }
        if self.reject {
            out.push(InboxDecision {
                submission_id: self.sub.clone(),
                local_ref: self.lref.clone(),
                action: DecisionAction::Reject,
            });
            return out;
        }
        let resolved = self.picks.first().cloned();
        out.push(InboxDecision {
            submission_id: self.sub.clone(),
            local_ref: self.lref.clone(),
            action: DecisionAction::Accept {
                resolved_entity: resolved.clone(),
            },
        });
        if let Some(a) = resolved {
            for b in &self.differents {
                out.push(InboxDecision {
                    submission_id: self.sub.clone(),
                    local_ref: self.lref.clone(),
                    action: DecisionAction::AssertDifferent {
                        a: a.clone(),
                        b: b.clone(),
                    },
                });
            }
        } else if !self.differents.is_empty() {
            warnings.push(ParseWarning {
                section,
                message: "\"these are different people\" needs a candidate pick (a plain accept has no entity to compare)".into(),
            });
        }
        out
    }
}

/// Parse an inbox file into decisions. Blocks under `## Decided` are
/// ignored; Housekeeping and Auto-filed lines yield merge, unmerge, and
/// undo decisions; source sections yield accept, reject, candidate,
/// and different-people decisions, with `accept all under this
/// article` expanded into accepts for every block that has no tick of
/// its own and refused when an ambiguous child has no candidate pick.
pub fn parse_inbox(text: &str) -> ParsedInbox {
    let mut parsed = ParsedInbox::default();
    let mut section = Section::Preamble;
    let mut current_source: Option<String> = None;
    let mut accept_all_ticked = false;
    let mut blocks: Vec<Block> = Vec::new();
    let mut block: Option<Block> = None;

    fn flush_source(
        parsed: &mut ParsedInbox,
        source: &Option<String>,
        accept_all: bool,
        blocks: &mut Vec<Block>,
        block: &mut Option<Block>,
    ) {
        if let Some(b) = block.take() {
            blocks.push(b);
        }
        let Some(sub) = source else {
            blocks.clear();
            return;
        };
        let mut own: Vec<InboxDecision> = Vec::new();
        for b in blocks.iter() {
            own.extend(b.decisions(&mut parsed.warnings));
        }
        if accept_all {
            let untouched_ambiguous = blocks.iter().any(|b| {
                !b.has_own_tick() && b.differents.len() + b.picks.len() == 0 && b.is_ambiguous_block
            });
            if untouched_ambiguous {
                parsed.warnings.push(ParseWarning {
                    section: sub.clone(),
                    message: "accept all under this article refused: an ambiguous block has no candidate pick".into(),
                });
            } else {
                for b in blocks.iter().filter(|b| !b.has_own_tick()) {
                    own.push(InboxDecision {
                        submission_id: Some(sub.clone()),
                        local_ref: b.lref.clone(),
                        action: DecisionAction::Accept {
                            resolved_entity: None,
                        },
                    });
                }
            }
        }
        parsed.decisions.extend(own);
        blocks.clear();
    }

    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("## ") {
            flush_source(
                &mut parsed,
                &current_source,
                accept_all_ticked,
                &mut blocks,
                &mut block,
            );
            accept_all_ticked = false;
            current_source = None;
            let fields = comment_fields(rest);
            section = if rest.starts_with("Housekeeping") {
                Section::Housekeeping
            } else if rest.starts_with("Auto-filed") {
                Section::AutoFiled
            } else if rest.starts_with("Decided") {
                Section::Decided
            } else if fields.contains_key("source") {
                current_source = fields.get("sub").cloned();
                Section::Source
            } else {
                Section::Preamble
            };
            continue;
        }
        if let Some(_heading) = line.strip_prefix("### ") {
            if let Some(b) = block.take() {
                blocks.push(b);
            }
            if section == Section::Source {
                block = Some(Block::default());
            }
            continue;
        }
        let Some((ticked, rest)) = checkbox(line) else {
            continue;
        };
        let fields = comment_fields(rest);
        match section {
            Section::Decided | Section::Preamble => {}
            Section::Housekeeping => {
                if !ticked {
                    continue;
                }
                if let Some(pair) = fields.get("merge")
                    && let Some((a, b)) = pair.split_once(':')
                {
                    parsed.decisions.push(InboxDecision {
                        submission_id: None,
                        local_ref: None,
                        action: DecisionAction::Merge {
                            source: a.to_string(),
                            target: b.to_string(),
                        },
                    });
                } else if let Some(h) = fields.get("unmerge") {
                    parsed.decisions.push(InboxDecision {
                        submission_id: None,
                        local_ref: None,
                        action: DecisionAction::Unmerge {
                            same_as_hash: h.clone(),
                        },
                    });
                }
            }
            Section::AutoFiled => {
                if !ticked {
                    continue;
                }
                if let Some(h) = fields.get("retract") {
                    parsed.decisions.push(InboxDecision {
                        submission_id: None,
                        local_ref: None,
                        action: DecisionAction::Undo {
                            atom_hash: h.clone(),
                        },
                    });
                } else if let Some(h) = fields.get("unmerge") {
                    parsed.decisions.push(InboxDecision {
                        submission_id: None,
                        local_ref: None,
                        action: DecisionAction::Unmerge {
                            same_as_hash: h.clone(),
                        },
                    });
                }
            }
            Section::Source => {
                if fields.contains_key("all") {
                    if ticked {
                        accept_all_ticked = true;
                    }
                    continue;
                }
                let Some(b) = block.as_mut() else {
                    continue;
                };
                if b.sub.is_none() {
                    b.sub = fields
                        .get("sub")
                        .cloned()
                        .or_else(|| current_source.clone());
                }
                if b.lref.is_none() {
                    b.lref = fields.get("ref").cloned();
                }
                let is_candidate_line =
                    fields.contains_key("entity") || fields.contains_key("different");
                if is_candidate_line {
                    b.is_ambiguous_block = true;
                }
                if !ticked {
                    continue;
                }
                b.tick_count += 1;
                if let Some(e) = fields.get("entity") {
                    b.picks.push(e.clone());
                } else if let Some(d) = fields.get("different") {
                    b.differents.push(d.clone());
                } else if rest.starts_with("reject") {
                    b.reject = true;
                } else if rest.starts_with("accept") {
                    b.plain_accept = true;
                }
            }
        }
    }
    flush_source(
        &mut parsed,
        &current_source,
        accept_all_ticked,
        &mut blocks,
        &mut block,
    );
    parsed
}

/// Applies decisions. The daemon implements it over its own RPCs so the
/// fast path never signs an atom itself for an inbox edit.
#[async_trait]
pub trait DecisionSink: Send + Sync {
    /// Apply one decision; `Ok` carries the RPC result.
    async fn apply(&self, decision: &InboxDecision) -> Result<Value, String>;
    /// Parse warnings from the edit, to render under the sections.
    fn report_warnings(&self, _warnings: Vec<ParseWarning>) {}
    /// Called after a batch so the file can be re-materialized.
    async fn finished(&self) {}
}

/// Apply every decision through the sink, in order; returns how many
/// applied and the first error text of each failure.
pub async fn apply_decisions(
    sink: &dyn DecisionSink,
    parsed: &ParsedInbox,
) -> (usize, Vec<String>) {
    sink.report_warnings(parsed.warnings.clone());
    let mut applied = 0usize;
    let mut errors = Vec::new();
    for d in &parsed.decisions {
        match sink.apply(d).await {
            Ok(_) => applied += 1,
            Err(e) => errors.push(format!("{:?}: {e}", d.action)),
        }
    }
    sink.finished().await;
    (applied, errors)
}

/// Map a decision to its RPC method and params.
pub fn decision_rpc(d: &InboxDecision) -> Result<(&'static str, Value), String> {
    let sub = d.submission_id.clone();
    match &d.action {
        DecisionAction::Accept { resolved_entity } => {
            let sub = sub.ok_or("accept without a submission id")?;
            let mut params = json!({ "submission_id": sub });
            if let Some(e) = resolved_entity {
                params["resolved_entity"] = json!(e);
                if let Some(r) = &d.local_ref {
                    params["choices"] = json!({ r: e });
                }
            }
            Ok(("ingest.accept", params))
        }
        DecisionAction::Reject => {
            let sub = sub.ok_or("reject without a submission id")?;
            Ok(("ingest.reject", json!({ "submission_id": sub })))
        }
        DecisionAction::AssertDifferent { a, b } => Ok((
            "entity.assert_different",
            json!({ "a": a, "b": b, "criterion": "owner ticked \"these are different people\" in the inbox" }),
        )),
        DecisionAction::Merge { source, target } => Ok((
            "entity.merge",
            json!({ "source": source, "target": target, "reason": "owner ticked merge in the inbox", "criterion": "inbox" }),
        )),
        DecisionAction::Undo { atom_hash } => {
            Ok(("ingest.retract", json!({ "atom_hash": atom_hash })))
        }
        DecisionAction::Unmerge { same_as_hash } => {
            Ok(("entity.unmerge", json!({ "same_as_hash": same_as_hash })))
        }
        DecisionAction::AcceptAllUnder { .. } => {
            Err("accept-all must be expanded by the parser before it reaches a sink".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"---
date: 2026-09-21
pending: 3
---

# Inbox 2026-09-21

## Widget maker <!-- source sub:sub-1 -->

- url: https://example.test/a

- [ ] accept all under this article <!-- sub:sub-1 all -->

### org.company: Acme Widgets

- display_name: Acme Widgets

- [x] accept <!-- sub:sub-1 ref:org-1 -->
- [ ] reject <!-- sub:sub-1 ref:org-1 -->

### person.generic: Pat Example

- resolution: ambiguous (who is this?)

- [ ] accept as Pat Example (Acme Widgets) (7.50, display_name+organization) <!-- sub:sub-1 ref:person-1 entity:zPat1 -->
- [X] accept as Pat Example (City Council) (6.90, display_name) <!-- sub:sub-1 ref:person-1 entity:zPat2 -->
- [ ] someone new <!-- sub:sub-1 ref:person-1 entity:new -->
- [ ] these are different people: Pat Example (Acme Widgets) <!-- sub:sub-1 ref:person-1 different:zPat1 -->
- [ ] these are different people: Pat Example (City Council) <!-- sub:sub-1 ref:person-1 different:zPat2 -->
- [ ] reject <!-- sub:sub-1 ref:person-1 -->

### source.article: Widget maker

- [ ] accept <!-- sub:sub-1 ref:article -->
- [ ] reject <!-- sub:sub-1 ref:article -->

## Housekeeping

- [x] merge Acme Corp into Acme Widgets <!-- merge:zA:zB -->

## Auto-filed today

* [x] undo source.article from a.md <!-- retract:zHash1 -->

## Decided

- [x] stale tick under decided <!-- sub:sub-9 ref:x -->
"#;

    #[test]
    fn ticks_of_any_case_or_bullet_style_are_recognized_and_ids_come_from_comments() {
        let parsed = parse_inbox(FILE);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(
            parsed.decisions,
            vec![
                InboxDecision {
                    submission_id: Some("sub-1".into()),
                    local_ref: Some("org-1".into()),
                    action: DecisionAction::Accept {
                        resolved_entity: None
                    }
                },
                InboxDecision {
                    submission_id: Some("sub-1".into()),
                    local_ref: Some("person-1".into()),
                    action: DecisionAction::Accept {
                        resolved_entity: Some("zPat2".into())
                    }
                },
                InboxDecision {
                    submission_id: None,
                    local_ref: None,
                    action: DecisionAction::Merge {
                        source: "zA".into(),
                        target: "zB".into()
                    }
                },
                InboxDecision {
                    submission_id: None,
                    local_ref: None,
                    action: DecisionAction::Undo {
                        atom_hash: "zHash1".into()
                    }
                },
            ]
        );
    }

    #[test]
    fn any_non_space_character_inside_the_brackets_is_a_tick() {
        for mark in ["x", "X", "v", "*", "1"] {
            let text = format!(
                "## S <!-- source sub:s -->\n### note: n\n- [{mark}] accept <!-- sub:s ref:r -->\n- [ ] reject <!-- sub:s ref:r -->\n"
            );
            let parsed = parse_inbox(&text);
            assert_eq!(parsed.decisions.len(), 1, "mark {mark:?}");
        }
        let parsed = parse_inbox(
            "## S <!-- source sub:s -->\n### note: n\n- [ ] accept <!-- sub:s ref:r -->\n",
        );
        assert!(parsed.decisions.is_empty());
    }

    #[test]
    fn two_ticks_in_one_block_is_a_warning_and_no_decision() {
        let text = "## S <!-- source sub:s -->\n### note: n\n- [x] accept <!-- sub:s ref:r -->\n- [x] reject <!-- sub:s ref:r -->\n";
        let parsed = parse_inbox(text);
        assert!(parsed.decisions.is_empty());
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.warnings[0].section, "s");
        assert!(parsed.warnings[0].message.contains("contradictory"));
    }

    #[test]
    fn different_people_needs_a_pick_in_the_same_block() {
        let alone = "## S <!-- source sub:s -->\n### person.generic: p\n- [ ] accept as A <!-- sub:s ref:r entity:zA -->\n- [x] these are different people: B <!-- sub:s ref:r different:zB -->\n";
        let parsed = parse_inbox(alone);
        assert!(parsed.decisions.is_empty());
        assert!(
            parsed.warnings[0]
                .message
                .contains("needs a candidate pick")
        );

        let with_pick = "## S <!-- source sub:s -->\n### person.generic: p\n- [x] accept as A <!-- sub:s ref:r entity:zA -->\n- [x] these are different people: B <!-- sub:s ref:r different:zB -->\n";
        let parsed = parse_inbox(with_pick);
        assert!(parsed.warnings.is_empty());
        assert_eq!(parsed.decisions.len(), 2);
        assert_eq!(
            parsed.decisions[1].action,
            DecisionAction::AssertDifferent {
                a: "zA".into(),
                b: "zB".into()
            }
        );
    }

    #[test]
    fn accept_all_expands_to_untouched_children_and_is_refused_over_an_unpicked_ambiguous_child() {
        let ok = "## S <!-- source sub:s -->\n- [x] accept all under this article <!-- sub:s all -->\n### a: a\n- [ ] accept <!-- sub:s ref:a -->\n### b: b\n- [x] reject <!-- sub:s ref:b -->\n";
        let parsed = parse_inbox(ok);
        assert!(parsed.warnings.is_empty());
        let actions: Vec<(Option<String>, DecisionAction)> = parsed
            .decisions
            .iter()
            .map(|d| (d.local_ref.clone(), d.action.clone()))
            .collect();
        assert!(actions.contains(&(Some("b".into()), DecisionAction::Reject)));
        assert!(actions.contains(&(
            Some("a".into()),
            DecisionAction::Accept {
                resolved_entity: None
            }
        )));
        assert_eq!(actions.len(), 2);

        let refused = "## S <!-- source sub:s -->\n- [x] accept all under this article <!-- sub:s all -->\n### a: a\n- [ ] accept <!-- sub:s ref:a -->\n### p: p\n- [ ] accept as A <!-- sub:s ref:p entity:zA -->\n- [ ] someone new <!-- sub:s ref:p entity:new -->\n";
        let parsed = parse_inbox(refused);
        assert!(parsed.decisions.is_empty());
        assert!(parsed.warnings[0].message.contains("refused"));
    }

    #[test]
    fn decided_section_ticks_are_ignored_and_inbox_paths_are_recognized() {
        assert!(is_inbox_path("inbox/2026-09-21.md"));
        assert!(is_inbox_path("/inbox/2026-09-21.md"));
        assert!(!is_inbox_path("inbox/sub/2026-09-21.md"));
        assert!(!is_inbox_path("contacts/by-name/S/Sara.md"));
        let parsed = parse_inbox("## Decided\n\n- [x] old <!-- sub:s ref:r -->\n");
        assert!(parsed.decisions.is_empty());
    }

    #[test]
    fn candidate_tick_maps_to_accept_with_resolved_entity_and_choices() {
        let d = InboxDecision {
            submission_id: Some("sub-1".into()),
            local_ref: Some("person-1".into()),
            action: DecisionAction::Accept {
                resolved_entity: Some("zPat2".into()),
            },
        };
        let (method, params) = decision_rpc(&d).unwrap();
        assert_eq!(method, "ingest.accept");
        assert_eq!(params["resolved_entity"], "zPat2");
        assert_eq!(params["choices"]["person-1"], "zPat2");
        let (m2, p2) = decision_rpc(&InboxDecision {
            submission_id: None,
            local_ref: None,
            action: DecisionAction::Undo {
                atom_hash: "zH".into(),
            },
        })
        .unwrap();
        assert_eq!(
            (m2, p2["atom_hash"].as_str()),
            ("ingest.retract", Some("zH"))
        );
    }
}
