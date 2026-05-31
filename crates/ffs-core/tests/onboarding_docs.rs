//! Onboarding docs regression test (task_23).
//!
//! Validates the integrity of the three onboarding markdown
//! documents under `docs/onboarding/`:
//!
//! - Internal relative links resolve (the target file exists).
//! - Section anchors on internal links resolve to a heading in the
//!   target file (e.g., `troubleshooting.md#federation-handshake-
//!   fails` requires a `Federation handshake fails` H2 in
//!   `troubleshooting.md`).
//! - Every TechSpec § Known Risk has a distinctive keyword phrase
//!   reflected in `troubleshooting.md`, so the troubleshooting
//!   guide and the spec stay in sync as risks evolve.
//!
//! Why this lives in `ffs-core`: that crate is the lowest in the
//! workspace dep graph and is always built, so the test runs in
//! every nextest invocation. The test itself depends only on the
//! repository layout (paths) and stdlib (no parser crate).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Convert a markdown heading text into the GitHub-flavored
/// anchor slug the rendered HTML uses (lowercase, spaces → '-',
/// punctuation stripped). Keeps `-` and `_` so anchors like
/// `step-1--install-5-min` round-trip. This isn't trying to be
/// 100% GFM-accurate — only enough for our actual heading set.
fn slugify(heading: &str) -> String {
    let mut out = String::with_capacity(heading.len());
    for c in heading.chars() {
        match c {
            ' ' | '\t' => out.push('-'),
            c if c.is_alphanumeric() => {
                for lc in c.to_lowercase() {
                    out.push(lc);
                }
            }
            '-' | '_' => out.push(c),
            _ => {}
        }
    }
    out
}

/// Walk markdown headings (`#`-prefixed lines) and emit their
/// slugged anchors. Tracks duplicates the way GFM does (first
/// occurrence wins; subsequent get `-1`, `-2`) so the test
/// doesn't false-alarm on legitimately-repeated section names.
fn anchors_of(md: &str) -> HashSet<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out: HashSet<String> = HashSet::new();
    let mut in_code_fence = false;
    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            // Strip additional '#' chars (H1-H6) then trim.
            let heading = rest.trim_start_matches('#').trim();
            let base = slugify(heading);
            let count = counts.entry(base.clone()).or_insert(0);
            let anchor = if *count == 0 {
                base.clone()
            } else {
                format!("{base}-{count}")
            };
            *count += 1;
            out.insert(anchor);
        }
    }
    out
}

/// Extract `[label](target)` link targets. Drops:
/// - external links (http://, https://, mailto:)
/// - image references (`![...](...)`) — those are handled
///   identically to text links but we want to keep them in
///   scope for asset validation; for our docs all images are
///   relative, so we keep them in.
fn links_of(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = md.as_bytes();
    let mut i = 0;
    let mut in_code_fence = false;
    while i < bytes.len() {
        // Track fenced code blocks to skip example markdown
        // inside the docs (we don't want to validate links
        // shown as examples).
        if bytes[i] == b'\n' {
            i += 1;
            // peek for a fence opening on the next line start
            let line_end = bytes[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|n| i + n)
                .unwrap_or(bytes.len());
            let line = std::str::from_utf8(&bytes[i..line_end]).unwrap_or("");
            if line.trim_start().starts_with("```") {
                in_code_fence = !in_code_fence;
            }
            continue;
        }
        if in_code_fence {
            i += 1;
            continue;
        }
        if bytes[i] == b'[' {
            // Find the closing `]` and following `(...)`. Stay on
            // the same line — pathological multi-line link
            // syntax isn't used in our docs.
            let close_bracket = bytes[i..].iter().position(|&b| b == b']' || b == b'\n');
            let Some(cb) = close_bracket else {
                break;
            };
            let cb = i + cb;
            if bytes[cb] != b']' || cb + 1 >= bytes.len() || bytes[cb + 1] != b'(' {
                i += 1;
                continue;
            }
            let paren_close = bytes[cb + 2..]
                .iter()
                .position(|&b| b == b')' || b == b'\n');
            let Some(pc) = paren_close else {
                i = cb + 2;
                continue;
            };
            let pc = cb + 2 + pc;
            if bytes[pc] != b')' {
                i = pc;
                continue;
            }
            let target = std::str::from_utf8(&bytes[cb + 2..pc]).unwrap_or("");
            out.push(target.to_string());
            i = pc + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn is_external(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://") || target.starts_with("mailto:")
}

/// Strip the GFM anchor suffix; returns `(path_part, anchor_or_empty)`.
fn split_anchor(target: &str) -> (&str, &str) {
    match target.split_once('#') {
        Some((p, a)) => (p, a),
        None => (target, ""),
    }
}

#[test]
fn onboarding_docs_have_intact_internal_links() {
    let root = repo_root();
    let onboarding = root.join("docs").join("onboarding");
    assert!(
        onboarding.is_dir(),
        "docs/onboarding/ missing — run task_23 first"
    );

    let docs = [
        onboarding.join("technical-friend-checklist.md"),
        onboarding.join("first-use-guide.md"),
        onboarding.join("troubleshooting.md"),
        onboarding.join("screenshots").join("README.md"),
    ];

    // Pre-index each doc's anchor set so cross-doc anchor links
    // are checkable.
    let mut anchors_by_path: std::collections::HashMap<PathBuf, HashSet<String>> =
        std::collections::HashMap::new();
    for d in &docs {
        let md = read(d);
        anchors_by_path.insert(d.canonicalize().unwrap(), anchors_of(&md));
    }

    let mut failures: Vec<String> = Vec::new();
    for d in &docs {
        let md = read(d);
        let here = d.parent().unwrap();
        for target in links_of(&md) {
            if is_external(&target) || target.starts_with('#') {
                continue; // external or in-page anchor (in-page anchors are
                // covered transitively when the same anchor is
                // referenced cross-doc; not enforcing here keeps
                // the test focused on the dangerous case: broken
                // cross-doc paths).
            }
            let (path_part, anchor) = split_anchor(&target);
            if path_part.is_empty() {
                continue;
            }
            let resolved = here.join(path_part);
            let canonical = match resolved.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    failures.push(format!(
                        "{}: link target {target:?} → {} unresolvable: {e}",
                        d.display(),
                        resolved.display()
                    ));
                    continue;
                }
            };
            if !canonical.exists() {
                failures.push(format!(
                    "{}: link target {target:?} → {} does not exist",
                    d.display(),
                    canonical.display()
                ));
                continue;
            }
            // Anchor check: only when the target is one of OUR
            // indexed docs. External-to-our-set anchors (e.g.,
            // ../../README.md#section) aren't load-bearing for
            // the onboarding flow.
            if !anchor.is_empty()
                && let Some(set) = anchors_by_path.get(&canonical)
                && !set.contains(anchor)
            {
                failures.push(format!(
                    "{}: anchor {target:?} not found in {} (have {} anchors: {:?})",
                    d.display(),
                    canonical.display(),
                    set.len(),
                    {
                        let mut sample: Vec<&String> = set.iter().take(8).collect();
                        sample.sort();
                        sample
                    }
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "broken onboarding links:\n  - {}",
        failures.join("\n  - ")
    );
}

/// Each known risk gets a distinctive keyword that troubleshooting.md
/// must mention. Picked so a future risk-list edit fails this test
/// loudly if the troubleshooting doc isn't updated alongside.
const KNOWN_RISK_KEYWORDS: &[&str] = &[
    "Reverse-map",          // Reverse-map rule mistakes silently mis-author atoms
    "SQLCipher",            // SQLCipher cross-compilation friction
    "named-pipe",           // Obsidian plugin's Windows named-pipe path
    "Federation handshake", // Federation handshake UX is unforgiving
    "subprocess hangs",     // Skill subprocess hangs
    "Working-set policy",   // Working-set policy is wrong
    "Capability evaluator", // Capability evaluator subtle bugs
    "MCP capability",       // MCP capability-check correctness
];

#[test]
fn troubleshooting_doc_covers_every_known_risk() {
    let path = repo_root()
        .join("docs")
        .join("onboarding")
        .join("troubleshooting.md");
    let md = read(&path);
    let mut missing = Vec::new();
    for k in KNOWN_RISK_KEYWORDS {
        if !md.contains(k) {
            missing.push(*k);
        }
    }
    assert!(
        missing.is_empty(),
        "troubleshooting.md missing keyword(s) for known risks: {missing:?}"
    );
}
