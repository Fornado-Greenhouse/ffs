//! The inbox review surface (ADR-032, accepted 2026-09-20): the
//! quarantine rendered as `inbox/<YYYY-MM-DD>.md`, one section per
//! SOURCE with every proposal derived from that source nested under it,
//! each carrying a strict checkbox decision block. The fast path (in
//! `ffs-fastpath::inbox`) parses ticks back into quarantine decisions;
//! this module only renders and writes the file.
//!
//! Rendering is plain Rust rather than a Tera template: the inbox is a
//! decision surface with an exact grammar the parser depends on, not a
//! projection of a predicate, and a deterministic string builder is
//! easier to test byte for byte than a template.
//!
//! Grammar (every decision line carries its ids in an HTML comment):
//!
//! ```text
//! ## <source title> <!-- source sub:<submission_id> -->
//! - [ ] accept all under this article <!-- sub:<id> all -->
//! ### <predicate>: <display>
//! - [ ] accept <!-- sub:<id> ref:<local_ref> -->
//! - [ ] reject <!-- sub:<id> ref:<local_ref> -->
//! - [ ] accept as <display> (<score>, <matched_on>) <!-- sub:<id> ref:<ref> entity:<entity id> -->
//! - [ ] someone new <!-- sub:<id> ref:<ref> entity:new -->
//! - [ ] these are different people: <display> <!-- sub:<id> ref:<ref> different:<entity id> -->
//! ## Housekeeping
//! - [ ] merge <a> into <b> <!-- merge:<a id>:<b id> -->
//! ## Auto-filed today
//! - [ ] undo <what> <!-- retract:<atom hash> -->
//! - [ ] undo merge <what> <!-- unmerge:<same_as hash> -->
//! ## Decided
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use ffs_core::quarantine::{IngestQuarantine, Proposal, Resolution, Submission, SubmissionStatus};
use ffs_core::{EntityId, Iso8601, Multihash, SuppressionRegistry};

use crate::notify::EventPublisher;

/// One atom the quarantine filed on its own today (ADR-029), rendered
/// with an undo line.
#[derive(Debug, Clone)]
pub struct AutoFiledRow {
    pub atom_hash: Multihash,
    /// What the line says: the predicate and the source basename.
    pub display: String,
    pub submission_id: String,
}

/// A merge the briefing (task_41) or the resolver suggests. Rendered
/// under Housekeeping as a `merge <a> into <b>` line.
#[derive(Debug, Clone)]
pub struct MergeSuggestion {
    pub a: EntityId,
    pub a_display: String,
    pub b: EntityId,
    pub b_display: String,
    pub reason: String,
}

pub use ffs_fastpath::inbox::ParseWarning;

/// Cross-source items for the Housekeeping section.
#[derive(Debug, Clone, Default)]
pub struct Housekeeping {
    pub merges: Vec<MergeSuggestion>,
    pub warnings: Vec<ParseWarning>,
    /// Active merges the owner may undo: (same_as atom hash, what).
    pub unmerges: Vec<(Multihash, String)>,
    /// ADR-034: facts whose confirmation window elapsed, each with an
    /// "unchanged" and a "re-read source" line the owner may tick.
    pub past_window: Vec<crate::api::PastWindowItem>,
}

/// Supplies the "Past their window" list at render time (the daemon's
/// dispatcher computes it from the store; nothing is stored).
pub trait PastWindowProvider: Send + Sync {
    fn past_window(&self) -> Vec<crate::api::PastWindowItem>;
}

#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("quarantine: {0}")]
    Quarantine(String),
}

/// Today's date, UTC, as `YYYY-MM-DD`. The inbox is one file per UTC
/// day (ADR-032 § Decision (1)).
pub fn today_utc() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

/// `inbox/<date>.md`, relative to the data dir.
pub fn inbox_relative_path(date: &str) -> String {
    format!("inbox/{date}.md")
}

fn claim_str<'a>(claim: &'a Value, key: &str) -> Option<&'a str> {
    claim
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The human name of a proposal: display_name, then title, then
/// summary, then the predicate.
fn proposal_display(p: &Proposal) -> String {
    for key in ["display_name", "title", "summary", "name"] {
        if let Some(s) = claim_str(&p.claim, key) {
            return s.to_string();
        }
    }
    p.predicate.as_str().to_string()
}

fn local_ref(p: &Proposal, idx: usize) -> String {
    p.local_ref.clone().unwrap_or_else(|| format!("#{idx}"))
}

/// A role change: an affiliation that supersedes an existing head
/// (ADR-029: always review), or any proposal that ends a role.
fn is_role_change(p: &Proposal) -> bool {
    p.ends_role
        || (p.predicate.as_str() == "affiliation" && p.resolution == Some(Resolution::Existing))
}

fn needs_eye(p: &Proposal) -> bool {
    p.resolution == Some(Resolution::Ambiguous) || is_role_change(p)
}

/// The source title of a submission: the article proposal's title, else
/// the first proposal's display, else the source uri's basename.
fn source_title(sub: &Submission) -> String {
    if let Some(p) = sub
        .proposals
        .iter()
        .find(|p| p.claim.get("url").is_some() && p.claim.get("title").is_some())
        && let Some(t) = claim_str(&p.claim, "title")
    {
        return t.to_string();
    }
    if let Some(p) = sub.proposals.first() {
        return proposal_display(p);
    }
    uri_basename(&sub.source_uri)
}

fn source_url(sub: &Submission) -> Option<String> {
    sub.proposals
        .iter()
        .find_map(|p| claim_str(&p.claim, "url").map(str::to_string))
}

pub(crate) fn uri_basename(uri: &str) -> String {
    let trimmed = uri.trim_end_matches('/');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let decoded = base.replace("%20", " ");
    if decoded.is_empty() {
        uri.to_string()
    } else {
        decoded
    }
}

fn render_claim(out: &mut String, claim: &Value) {
    let Some(obj) = claim.as_object() else {
        return;
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for k in keys {
        match &obj[k] {
            Value::String(s) => {
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                let one_line = s.replace('\n', " ");
                let shown = if one_line.chars().count() > 160 {
                    let cut: String = one_line.chars().take(157).collect();
                    format!("{cut}...")
                } else {
                    one_line
                };
                out.push_str(&format!("- {k}: {shown}\n"));
            }
            Value::Number(n) => out.push_str(&format!("- {k}: {n}\n")),
            Value::Bool(b) => out.push_str(&format!("- {k}: {b}\n")),
            Value::Array(items) if !items.is_empty() => {
                out.push_str(&format!("- {k}:\n"));
                for item in items {
                    let text = match item {
                        Value::String(s) => s.clone(),
                        Value::Object(o) => {
                            let display = o.get("display").and_then(|v| v.as_str()).unwrap_or("");
                            let extra = o
                                .get("context")
                                .or_else(|| o.get("role"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let entity = o.get("entity").and_then(|v| v.as_str()).unwrap_or("");
                            let mut t = display.to_string();
                            if !extra.is_empty() {
                                t.push_str(" (");
                                t.push_str(extra);
                                t.push(')');
                            }
                            if !entity.is_empty() {
                                t.push_str(" [");
                                t.push_str(entity);
                                t.push(']');
                            }
                            t
                        }
                        other => other.to_string(),
                    };
                    out.push_str(&format!("  - {text}\n"));
                }
            }
            _ => {}
        }
    }
}

fn render_proposal(out: &mut String, sub_id: &str, idx: usize, p: &Proposal) {
    let display = proposal_display(p);
    let lref = local_ref(p, idx);
    out.push_str(&format!("### {}: {}\n\n", p.predicate.as_str(), display));
    match p.resolution {
        Some(Resolution::Existing) => {
            let ent = p.entity.as_ref().map(|e| e.as_str()).unwrap_or("?");
            out.push_str(&format!("- resolution: existing ({ent})\n"));
        }
        Some(Resolution::New) => out.push_str("- resolution: new\n"),
        Some(Resolution::Ambiguous) => out.push_str("- resolution: ambiguous (who is this?)\n"),
        None => {}
    }
    if is_role_change(p) {
        let title = claim_str(&p.claim, "title").unwrap_or("this role");
        let org = claim_str(&p.claim, "organization").unwrap_or("its organization");
        if p.ends_role {
            out.push_str(&format!(
                "- **this supersedes** the current affiliation: {title} at {org} ends\n"
            ));
        } else {
            out.push_str(&format!(
                "- **this supersedes** the current affiliation at {org} with {title}\n"
            ));
        }
    }
    render_claim(out, &p.claim);
    if !p.rationale.is_empty() {
        out.push_str(&format!(
            "- rationale: {}\n",
            p.rationale.replace('\n', " ")
        ));
    }
    match (&p.engine, &p.model) {
        (Some(e), Some(m)) if !m.is_empty() => out.push_str(&format!("- engine: {e} ({m})\n")),
        (Some(e), _) => out.push_str(&format!("- engine: {e}\n")),
        _ => {}
    }
    out.push('\n');
    if p.resolution == Some(Resolution::Ambiguous) {
        for c in &p.candidates {
            let matched = if c.matched_on.is_empty() {
                "unmatched".to_string()
            } else {
                c.matched_on.join("+")
            };
            out.push_str(&format!(
                "- [ ] accept as {} ({:.2}, {}) <!-- sub:{} ref:{} entity:{} -->\n",
                c.display,
                c.score,
                matched,
                sub_id,
                lref,
                c.entity.as_str()
            ));
        }
        out.push_str(&format!(
            "- [ ] someone new <!-- sub:{sub_id} ref:{lref} entity:new -->\n"
        ));
        for c in &p.candidates {
            out.push_str(&format!(
                "- [ ] these are different people: {} <!-- sub:{} ref:{} different:{} -->\n",
                c.display,
                sub_id,
                lref,
                c.entity.as_str()
            ));
        }
        out.push_str(&format!("- [ ] reject <!-- sub:{sub_id} ref:{lref} -->\n"));
    } else {
        out.push_str(&format!("- [ ] accept <!-- sub:{sub_id} ref:{lref} -->\n"));
        out.push_str(&format!("- [ ] reject <!-- sub:{sub_id} ref:{lref} -->\n"));
    }
    out.push('\n');
}

fn render_source(out: &mut String, sub: &Submission, warnings: &[ParseWarning]) {
    let title = source_title(sub);
    out.push_str(&format!("## {} <!-- source sub:{} -->\n\n", title, sub.id));
    if let Some(url) = source_url(sub) {
        out.push_str(&format!("- url: {url}\n"));
    }
    out.push_str(&format!("- source: {}\n", sub.source_uri));
    out.push_str(&format!("- submission: {}\n", sub.id));
    let eye = sub.proposals.iter().filter(|p| needs_eye(p)).count();
    if eye > 0 {
        out.push_str(&format!("- needs your eye: {eye}\n"));
    }
    for w in warnings.iter().filter(|w| w.section == sub.id) {
        out.push_str(&format!("- parse warning: {}\n", w.message));
    }
    out.push('\n');
    out.push_str(&format!(
        "- [ ] accept all under this article <!-- sub:{} all -->\n\n",
        sub.id
    ));
    for (i, p) in sub.proposals.iter().enumerate() {
        render_proposal(out, &sub.id, i, p);
    }
}

fn status_word(s: &SubmissionStatus) -> &'static str {
    match s {
        SubmissionStatus::Accepted => "accepted",
        SubmissionStatus::Rejected => "rejected",
        SubmissionStatus::AutoAccepted => "auto-filed",
        SubmissionStatus::PartiallyAccepted => "partly auto-filed",
        SubmissionStatus::Failed => "failed",
        _ => "pending",
    }
}

/// Render the inbox file. `pending` are the submissions still waiting
/// (Extracted and PartiallyAccepted); `decided` are today's finished
/// submissions (collapsed to one line each); `auto_filed` are today's
/// auto-accepted atoms with an undo line each. Output is deterministic
/// for a given input: sources needing the owner's eye come first, then
/// the rest in submission order.
pub fn render_inbox(
    date: &str,
    pending: &[Submission],
    decided: &[Submission],
    auto_filed: &[AutoFiledRow],
    housekeeping: &Housekeeping,
) -> String {
    let pending_count: usize = pending.iter().map(|s| s.proposals.len()).sum();
    let eye_count: usize = pending
        .iter()
        .flat_map(|s| s.proposals.iter())
        .filter(|p| needs_eye(p))
        .count();
    let ambiguous: usize = pending
        .iter()
        .flat_map(|s| s.proposals.iter())
        .filter(|p| p.resolution == Some(Resolution::Ambiguous))
        .count();
    let role_changes = eye_count - ambiguous.min(eye_count);

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("date: {date}\n"));
    out.push_str(&format!("pending: {pending_count}\n"));
    out.push_str(&format!("need_your_eye: {eye_count}\n"));
    out.push_str(&format!("decided: {}\n", decided.len()));
    out.push_str(&format!("auto_filed: {}\n", auto_filed.len()));
    out.push_str("---\n\n");
    out.push_str(&format!("# Inbox {date}\n\n"));
    out.push_str(&format!(
        "{pending_count} pending. {eye_count} need your eye ({role_changes} role changes, {ambiguous} ambiguous identities). \
{} auto-filed today. Tick a box and save. One tick per block; contradictory ticks leave the block pending with a warning.\n\n",
        auto_filed.len()
    ));

    let mut ordered: Vec<&Submission> = pending.iter().collect();
    // Stable partition: sources with items needing the owner first,
    // original order preserved inside each group.
    ordered.sort_by_key(|s| {
        if s.proposals.iter().any(needs_eye) {
            0
        } else {
            1
        }
    });
    for sub in ordered {
        render_source(&mut out, sub, &housekeeping.warnings);
    }

    out.push_str("## Housekeeping\n\n");
    let mut wrote = false;
    for m in &housekeeping.merges {
        wrote = true;
        out.push_str(&format!(
            "- [ ] merge {} into {} <!-- merge:{}:{} -->\n",
            m.a_display,
            m.b_display,
            m.a.as_str(),
            m.b.as_str()
        ));
        if !m.reason.is_empty() {
            out.push_str(&format!("  - why: {}\n", m.reason));
        }
    }
    for (hash, what) in &housekeeping.unmerges {
        wrote = true;
        out.push_str(&format!(
            "- [ ] undo merge {} <!-- unmerge:{} -->\n",
            what,
            hash.to_multibase()
        ));
    }
    if !housekeeping.past_window.is_empty() {
        wrote = true;
        out.push_str("\n### Past their window\n\n");
        out.push_str(
            "Facts whose confirmation window elapsed (ADR-034). Tick `unchanged` when you know it still holds, or `re-read source` after checking the source. Nothing here changes a fact on its own.\n\n",
        );
        for item in &housekeeping.past_window {
            let who = if item.owner_alone {
                ", confirmed by you alone"
            } else {
                ""
            };
            out.push_str(&format!(
                "- {} ({}): last confirmed {}{}\n  - [ ] unchanged <!-- attest:{} basis:owner_knowledge -->\n  - [ ] re-read source <!-- attest:{} basis:re_read_same_source -->\n",
                item.display,
                item.predicate.as_str(),
                item.last_confirmed.as_deref().unwrap_or("never"),
                who,
                item.atom_hash,
                item.atom_hash,
            ));
        }
        out.push_str(&format!("- [ ] all unchanged <!-- attest-all:{date} -->\n"));
    }
    let orphan_warnings: Vec<&ParseWarning> = housekeeping
        .warnings
        .iter()
        .filter(|w| w.section.is_empty() || !pending.iter().any(|s| s.id == w.section))
        .collect();
    for w in orphan_warnings {
        wrote = true;
        out.push_str(&format!("- parse warning: {}\n", w.message));
    }
    if !wrote {
        out.push_str("- nothing to tidy\n");
    }
    out.push('\n');

    out.push_str("## Auto-filed today\n\n");
    if auto_filed.is_empty() {
        out.push_str("- nothing auto-filed today\n");
    }
    for row in auto_filed {
        out.push_str(&format!(
            "- [ ] undo {} <!-- retract:{} -->\n",
            row.display,
            row.atom_hash.to_multibase()
        ));
    }
    out.push('\n');

    out.push_str("## Decided\n\n");
    if decided.is_empty() {
        out.push_str("- nothing decided yet today\n");
    }
    for sub in decided {
        let hashes: Vec<String> = sub
            .accepted_atom_hashes
            .iter()
            .chain(sub.auto_accepted_atom_hashes.iter())
            .map(|h| h.to_multibase())
            .collect();
        let tail = if hashes.is_empty() {
            String::new()
        } else {
            format!(" ({})", hashes.join(", "))
        };
        out.push_str(&format!(
            "- {}: {}{} <!-- decided sub:{} -->\n",
            source_title(sub),
            status_word(&sub.status),
            tail,
            sub.id
        ));
    }
    out.push('\n');
    out
}

/// Renders and writes `inbox/<today>.md` from the quarantine, through
/// the suppression registry so the fast-path watcher ignores the
/// daemon's own write. Re-render on every quarantine change and on a
/// timer so the file stays current even when a producer forgets to
/// publish the event.
pub struct InboxMaterializer {
    quarantine: Arc<dyn IngestQuarantine>,
    suppression: Arc<SuppressionRegistry>,
    data_dir: PathBuf,
    warnings: Mutex<Vec<ParseWarning>>,
    merges: Mutex<Vec<MergeSuggestion>>,
    past_window: Mutex<Option<Arc<dyn PastWindowProvider>>>,
}

impl InboxMaterializer {
    pub fn new(
        quarantine: Arc<dyn IngestQuarantine>,
        suppression: Arc<SuppressionRegistry>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            quarantine,
            suppression,
            data_dir,
            warnings: Mutex::new(Vec::new()),
            merges: Mutex::new(Vec::new()),
            past_window: Mutex::new(None),
        }
    }

    /// Who computes the "Past their window" list (ADR-034). Set once
    /// the dispatcher exists; the inbox renders without it otherwise.
    pub fn set_past_window_provider(&self, provider: Arc<dyn PastWindowProvider>) {
        *self.past_window.lock().unwrap() = Some(provider);
    }

    /// Replace the parse warnings rendered into the next file. The fast
    /// path's decision sink sets these after parsing an edit.
    pub fn set_warnings(&self, warnings: Vec<ParseWarning>) {
        *self.warnings.lock().unwrap() = warnings;
    }

    pub fn set_merge_suggestions(&self, merges: Vec<MergeSuggestion>) {
        *self.merges.lock().unwrap() = merges;
    }

    pub fn inbox_path(&self, date: &str) -> PathBuf {
        self.data_dir.join(inbox_relative_path(date))
    }

    /// Render today's inbox and write it when it changed. Returns the
    /// path written, or `None` when the bytes on disk already match.
    pub async fn refresh(&self) -> Result<Option<PathBuf>, InboxError> {
        let date = today_utc();
        let content = self.render_today(&date).await?;
        let path = self.inbox_path(&date);
        if let Ok(existing) = std::fs::read(&path)
            && existing == content.as_bytes()
        {
            return Ok(None);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.suppression.record(&path, content.as_bytes());
        write_atomically(&path, content.as_bytes())?;
        debug!(path = %path.display(), "inbox materialized");
        Ok(Some(path))
    }

    async fn render_today(&self, date: &str) -> Result<String, InboxError> {
        let mut pending = self
            .quarantine
            .list(Some(SubmissionStatus::Extracted))
            .await;
        pending.extend(
            self.quarantine
                .list(Some(SubmissionStatus::PartiallyAccepted))
                .await,
        );
        pending.sort_by(|a, b| a.tx_time.as_str().cmp(b.tx_time.as_str()));

        let midnight = Iso8601::new(format!("{date}T00:00:00Z"))
            .map_err(|e| InboxError::Quarantine(e.to_string()))?;
        let mut decided: Vec<Submission> = Vec::new();
        for status in [
            SubmissionStatus::Accepted,
            SubmissionStatus::Rejected,
            SubmissionStatus::AutoAccepted,
        ] {
            for s in self.quarantine.list(Some(status)).await {
                // Submissions carry their submit time, not a decision
                // time; "decided today" is approximated by "submitted
                // today and finished", which is the morning's batch.
                if s.tx_time.as_str() >= midnight.as_str() {
                    decided.push(s);
                }
            }
        }
        decided.sort_by(|a, b| a.tx_time.as_str().cmp(b.tx_time.as_str()));

        let auto_filed: Vec<AutoFiledRow> = self
            .quarantine
            .list_auto_filed(Some(&midnight))
            .await
            .map_err(|e| InboxError::Quarantine(e.to_string()))?
            .into_iter()
            .map(|(hash, sub)| {
                let predicates: Vec<&str> =
                    sub.proposals.iter().map(|p| p.predicate.as_str()).collect();
                let mut preds = predicates.clone();
                preds.dedup();
                AutoFiledRow {
                    atom_hash: hash,
                    display: format!(
                        "{} from {}",
                        preds.join(", "),
                        uri_basename(&sub.source_uri)
                    ),
                    submission_id: sub.id.clone(),
                }
            })
            .collect();

        let housekeeping = Housekeeping {
            merges: self.merges.lock().unwrap().clone(),
            warnings: self.warnings.lock().unwrap().clone(),
            unmerges: Vec::new(),
            past_window: self
                .past_window
                .lock()
                .unwrap()
                .as_ref()
                .map(|p| p.past_window())
                .unwrap_or_default(),
        };
        Ok(render_inbox(
            date,
            &pending,
            &decided,
            &auto_filed,
            &housekeeping,
        ))
    }

    /// Subscribe to the daemon's event stream and re-render on every
    /// `event.quarantine.changed` and `event.atom.committed`, plus a
    /// 30-second timer as a safety net.
    pub fn spawn(self: Arc<Self>, publisher: Arc<EventPublisher>) -> JoinHandle<()> {
        let mut rx = publisher.subscribe();
        tokio::spawn(async move {
            if let Err(e) = self.refresh().await {
                warn!(error = %e, "inbox: initial materialization failed");
            }
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    msg = rx.recv() => match msg {
                        Ok(line) => {
                            if (line.contains("event.quarantine.changed")
                                || line.contains("event.atom.committed"))
                                && let Err(e) = self.refresh().await
                            {
                                warn!(error = %e, "inbox: re-materialization failed");
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = self.refresh().await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    },
                    _ = tick.tick() => {
                        if let Err(e) = self.refresh().await {
                            warn!(error = %e, "inbox: timed re-materialization failed");
                        }
                    }
                }
            }
        })
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffs_core::quarantine::{Candidate, InMemoryQuarantine};
    use ffs_core::{PredicateName, Provenance, SourceKind};

    fn prov() -> Vec<Provenance> {
        vec![Provenance {
            kind: SourceKind::IngestFile,
            uri: "file:///ingest/x.md".into(),
            hash: Multihash::blake3_of(b"x"),
        }]
    }

    fn proposal(predicate: &str, claim: Value, lref: &str) -> Proposal {
        let mut p = Proposal::new(PredicateName::new(predicate), claim, prov(), "why");
        p.local_ref = Some(lref.into());
        p
    }

    fn submission(id: &str, source: &str, proposals: Vec<Proposal>) -> Submission {
        Submission {
            id: id.into(),
            source_uri: source.into(),
            content_hash: Multihash::blake3_of(id.as_bytes()),
            content: Vec::new(),
            tx_time: Iso8601::new("2026-09-21T08:00:00Z").unwrap(),
            status: SubmissionStatus::Extracted,
            proposals,
            failure_reason: None,
            accepted_atom_hashes: Vec::new(),
            auto_accepted_atom_hashes: Vec::new(),
        }
    }

    fn article_set() -> Submission {
        let mut org = proposal(
            "org.company",
            serde_json::json!({"display_name": "Acme Widgets", "industry": "manufacturing"}),
            "org-1",
        );
        org.resolution = Some(Resolution::New);
        let mut person = proposal(
            "person.generic",
            serde_json::json!({"display_name": "Pat Example", "role": "CEO", "organization": "Acme Widgets"}),
            "person-1",
        );
        person.resolution = Some(Resolution::Ambiguous);
        person.candidates = vec![
            Candidate {
                entity: EntityId::new("zPat1"),
                score: 7.5,
                matched_on: vec!["display_name".into(), "organization".into()],
                display: "Pat Example (Acme Widgets)".into(),
            },
            Candidate {
                entity: EntityId::new("zPat2"),
                score: 6.9,
                matched_on: vec!["display_name".into()],
                display: "Pat Example (City Council)".into(),
            },
        ];
        let article = proposal(
            "source.article",
            serde_json::json!({"title": "Widget maker breaks ground", "url": "https://example.test/a", "mentions": [{"display": "Pat Example", "context": "CEO"}]}),
            "article",
        );
        submission(
            "sub-1",
            "file:///ingest/example-ledger-2026-09-21-widget.md",
            vec![article, org, person],
        )
    }

    #[test]
    fn renders_sources_with_nested_decision_blocks_and_ids_in_comments() {
        let text = render_inbox(
            "2026-09-21",
            &[article_set()],
            &[],
            &[],
            &Housekeeping::default(),
        );
        assert!(text.starts_with("---\ndate: 2026-09-21\npending: 3\nneed_your_eye: 1\n"));
        assert!(text.contains("## Widget maker breaks ground <!-- source sub:sub-1 -->"));
        assert!(text.contains("- [ ] accept all under this article <!-- sub:sub-1 all -->"));
        assert!(text.contains("### org.company: Acme Widgets"));
        assert!(text.contains("- [ ] accept <!-- sub:sub-1 ref:org-1 -->"));
        assert!(text.contains("- [ ] reject <!-- sub:sub-1 ref:org-1 -->"));
        assert!(text.contains("### person.generic: Pat Example"));
        assert!(text.contains(
            "- [ ] accept as Pat Example (Acme Widgets) (7.50, display_name+organization) <!-- sub:sub-1 ref:person-1 entity:zPat1 -->"
        ));
        assert!(text.contains("- [ ] someone new <!-- sub:sub-1 ref:person-1 entity:new -->"));
        assert!(text.contains(
            "- [ ] these are different people: Pat Example (City Council) <!-- sub:sub-1 ref:person-1 different:zPat2 -->"
        ));
        assert!(text.contains("## Housekeeping"));
        assert!(text.contains("## Auto-filed today"));
        assert!(text.contains("## Decided"));
        // deterministic
        let again = render_inbox(
            "2026-09-21",
            &[article_set()],
            &[],
            &[],
            &Housekeeping::default(),
        );
        assert_eq!(text, again);
    }

    #[test]
    fn sources_needing_the_owner_come_first_and_role_changes_are_marked() {
        let plain = submission(
            "sub-plain",
            "file:///ingest/plain.md",
            vec![proposal(
                "note",
                serde_json::json!({"title": "Plain note"}),
                "n",
            )],
        );
        let mut aff = proposal(
            "affiliation",
            serde_json::json!({"person": "zP", "organization": "Acme Widgets", "title": "Chair"}),
            "aff-1",
        );
        aff.resolution = Some(Resolution::Existing);
        aff.entity = Some(EntityId::new("zAff"));
        let role = submission("sub-role", "file:///ingest/role.md", vec![aff]);
        let text = render_inbox(
            "2026-09-21",
            &[plain, role],
            &[],
            &[],
            &Housekeeping::default(),
        );
        let plain_at = text.find("<!-- source sub:sub-plain -->").unwrap();
        let role_at = text.find("<!-- source sub:sub-role -->").unwrap();
        assert!(
            role_at < plain_at,
            "role change section renders before the plain one"
        );
        assert!(
            text.contains("**this supersedes** the current affiliation at Acme Widgets with Chair")
        );
        assert!(text.contains("need_your_eye: 1"));
    }

    #[test]
    fn housekeeping_auto_filed_and_decided_render_their_lines() {
        let mut done = submission("sub-done", "file:///ingest/done.md", vec![]);
        done.status = SubmissionStatus::Accepted;
        done.accepted_atom_hashes = vec![Multihash::blake3_of(b"atom")];
        let hk = Housekeeping {
            past_window: Vec::new(),
            merges: vec![MergeSuggestion {
                a: EntityId::new("zA"),
                a_display: "Acme Corp".into(),
                b: EntityId::new("zB"),
                b_display: "Acme Widgets".into(),
                reason: "aliases overlap".into(),
            }],
            warnings: vec![ParseWarning {
                section: String::new(),
                message: "orphan warning".into(),
            }],
            unmerges: vec![(
                Multihash::blake3_of(b"same"),
                "Acme Corp into Acme Widgets".into(),
            )],
        };
        let rows = vec![AutoFiledRow {
            atom_hash: Multihash::blake3_of(b"auto"),
            display: "source.article from a.md".into(),
            submission_id: "sub-auto".into(),
        }];
        let text = render_inbox("2026-09-21", &[], &[done], &rows, &hk);
        assert!(text.contains("- [ ] merge Acme Corp into Acme Widgets <!-- merge:zA:zB -->"));
        assert!(text.contains("- parse warning: orphan warning"));
        assert!(text.contains(&format!(
            "- [ ] undo merge Acme Corp into Acme Widgets <!-- unmerge:{} -->",
            Multihash::blake3_of(b"same").to_multibase()
        )));
        assert!(text.contains(&format!(
            "- [ ] undo source.article from a.md <!-- retract:{} -->",
            Multihash::blake3_of(b"auto").to_multibase()
        )));
        assert!(text.contains("- done.md: accepted ("));
        assert!(text.contains("<!-- decided sub:sub-done -->"));
    }

    #[test]
    fn parse_warnings_render_under_their_section() {
        let hk = Housekeeping {
            past_window: Vec::new(),
            warnings: vec![ParseWarning {
                section: "sub-1".into(),
                message: "two ticks in one block".into(),
            }],
            ..Default::default()
        };
        let text = render_inbox("2026-09-21", &[article_set()], &[], &[], &hk);
        let section = text.find("<!-- source sub:sub-1 -->").unwrap();
        let warning = text
            .find("- parse warning: two ticks in one block")
            .unwrap();
        let first_block = text.find("### source.article").unwrap();
        assert!(section < warning && warning < first_block);
    }

    #[tokio::test]
    async fn materializer_writes_through_suppression_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let q = Arc::new(InMemoryQuarantine::new());
        let id = q
            .submit("file:///ingest/x.md".into(), b"x".to_vec())
            .await
            .unwrap();
        q.complete(
            &id,
            vec![proposal("note", serde_json::json!({"title": "T"}), "n")],
        )
        .await
        .unwrap();
        let suppression = Arc::new(SuppressionRegistry::new());
        let m = InboxMaterializer::new(q, suppression.clone(), dir.path().to_path_buf());
        let written = m.refresh().await.unwrap().expect("first refresh writes");
        assert_eq!(
            written,
            dir.path().join("inbox").join(format!("{}.md", today_utc()))
        );
        let bytes = std::fs::read(&written).unwrap();
        assert!(
            suppression.check(&written, &bytes),
            "write was recorded for the watcher to ignore"
        );
        assert!(
            m.refresh().await.unwrap().is_none(),
            "unchanged content is a no-op"
        );
        m.set_warnings(vec![ParseWarning {
            section: id.clone(),
            message: "w".into(),
        }]);
        assert!(
            m.refresh().await.unwrap().is_some(),
            "a new warning re-renders"
        );
    }
}
