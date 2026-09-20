//! Docs tests for the morning-read skill (task_48, ADR-035): every MCP
//! tool the skill names exists in the catalog, and the five refusal
//! rules are present verbatim so a later edit cannot drop them.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ffs_mcp::tool_catalog;

fn skill_text() -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("docs/agent-memory/skill/ffs-morning-read/SKILL.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

const REFUSAL_RULES: [&str; 5] = [
    "One article at a time, on the owner's cue; never before it.",
    "No bulk opening: a request to open all of today's links is refused as bulk behavior, whatever the publisher.",
    "No prefetching, no background opening, no scheduled opening in the read; only articles named in the owner's own digest; never crawling.",
    "No article body is stored except the one the owner said to clip, and that one lands under the clip tier.",
    "Summaries are spoken; only chosen notes and clips are filed; inferences are marked; persistence claims name only what the tools returned.",
];

#[test]
fn skill_names_only_catalog_tools() {
    let text = skill_text();
    let catalog: BTreeSet<String> = tool_catalog().into_iter().map(|t| t.name).collect();
    let mut named = BTreeSet::new();
    for seg in text.split('`').skip(1).step_by(2) {
        let name: String = seg
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.starts_with("ffs_") {
            named.insert(name);
        }
    }
    assert!(!named.is_empty(), "the skill names no ffs_* tools");
    let unknown: Vec<_> = named.iter().filter(|n| !catalog.contains(*n)).collect();
    assert!(
        unknown.is_empty(),
        "skill names tools not in the catalog: {unknown:?}"
    );
    assert!(
        named.contains("ffs_accept_proposal"),
        "the in-session accept tool must be named"
    );
}

#[test]
fn skill_contains_refusal_rules_verbatim() {
    let text = skill_text();
    for rule in REFUSAL_RULES {
        assert!(text.contains(rule), "missing refusal rule: {rule}");
    }
    // The rules come before any workflow step.
    let first_rule = text.find(REFUSAL_RULES[0]).unwrap();
    let workflow = text.find("## 3. The session, step by step").unwrap();
    assert!(
        first_rule < workflow,
        "refusal rules must precede the workflow"
    );
}
