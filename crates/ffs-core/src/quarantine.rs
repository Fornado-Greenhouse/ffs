//! Ingest quarantine: holds proposed atoms from the scribe (task 11)
//! awaiting user acceptance. A submission lands here whenever a writer
//! drops content into `~/.ffs/ingest/` or calls `ingest.submit`. The
//! daemon routes the content through the scribe skill to produce
//! `Proposal`s, then stores them for review on the daily-health-summary.
//!
//! For MVP this is in-memory only (`InMemoryQuarantine`). A SQLite-
//! backed implementation lands when the auditor needs cross-restart
//! persistence (post-MVP). The trait is async because the future
//! SQLite backend will go through tokio's blocking pool, and the
//! scribe-invocation pipeline already lives in async code.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{EntityId, Iso8601, Multihash, PredicateName, Provenance};

#[derive(Debug, thiserror::Error)]
pub enum QuarantineError {
    #[error("submission not found: {0}")]
    NotFound(String),
    #[error("invalid status transition: {from} → {to}")]
    BadTransition { from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    /// Stored; awaiting scribe extraction.
    Pending,
    /// Scribe returned proposals; awaiting user acceptance.
    Extracted,
    /// Scribe failed (crash, timeout, malformed output).
    Failed,
    /// User accepted the proposals via the daily-health-summary
    /// panel; the daemon signed them and inserted them into the
    /// store. The submission stays in the quarantine for audit
    /// trail purposes — `accepted_atom_hashes` records what landed.
    Accepted,
    /// User rejected the proposals. The submission stays in the
    /// quarantine for the audit trail; the proposals never become
    /// atoms.
    Rejected,
}

/// Outcome of daemon-side entity resolution for one proposal
/// (ADR-030): the mention matched a known entity, is a new one, or
/// sits between the thresholds (or between two close candidates) and
/// needs the owner's eye. `ambiguous` is always conflicting under
/// ADR-029, so no Accept grant can auto-file it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    Existing,
    New,
    Ambiguous,
}

/// One candidate the resolver considered for a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub entity: EntityId,
    pub score: f64,
    /// Which fields matched: `display_name`, `alias`, `fts`, `prior`,
    /// `organization`, `role`, `location`.
    pub matched_on: Vec<String>,
    /// The candidate's current display name, for the review picker.
    pub display: String,
}

/// A cross-reference between proposals in one submission: `field` is
/// a JSON-pointer-like path inside this proposal's claim (`organization`,
/// `person`, `mentions/2/entity`, `participants/0/entity`) that must be
/// filled with the entity id the proposal tagged `local_ref` resolves
/// to. The Python wire writes `mentions[2].entity`; see
/// [`CrossRef::from_wire_field`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossRef {
    pub field: String,
    pub local_ref: String,
}

impl CrossRef {
    /// Normalize a wire field (`mentions[2].entity`, `organization`) to
    /// the pointer form (`mentions/2/entity`).
    pub fn from_wire_field(field: &str) -> String {
        let mut out = String::with_capacity(field.len());
        for ch in field.chars() {
            match ch {
                '[' | '.' => out.push('/'),
                ']' => {}
                other => out.push(other),
            }
        }
        out.trim_matches('/').to_string()
    }

    /// Set `value` at the pointer path `field` inside `claim`, creating
    /// intermediate objects and the final key as needed. Array segments
    /// must already exist (a cross-reference never invents a mention).
    /// Returns false when the path cannot be applied.
    pub fn apply(claim: &mut serde_json::Value, field: &str, value: serde_json::Value) -> bool {
        let path = Self::from_wire_field(field);
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return false;
        }
        let mut cur = claim;
        for (i, seg) in segments.iter().enumerate() {
            let last = i + 1 == segments.len();
            if let Ok(idx) = seg.parse::<usize>() {
                let Some(arr) = cur.as_array_mut() else {
                    return false;
                };
                let Some(item) = arr.get_mut(idx) else {
                    return false;
                };
                if last {
                    *item = value;
                    return true;
                }
                cur = item;
            } else {
                if !cur.is_object() {
                    if cur.is_null() {
                        *cur = serde_json::Value::Object(Default::default());
                    } else {
                        return false;
                    }
                }
                let obj = cur.as_object_mut().expect("object");
                if last {
                    obj.insert((*seg).to_string(), value);
                    return true;
                }
                cur = obj
                    .entry((*seg).to_string())
                    .or_insert(serde_json::Value::Null);
            }
        }
        false
    }
}

/// A single proposed atom produced by the scribe from a submission.
/// Proposals carry their own provenance (back to the submission) and
/// a rationale string so the user understands what the scribe inferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub predicate: PredicateName,
    pub claim: serde_json::Value,
    pub provenance: Vec<Provenance>,
    /// Short human-readable explanation of what the scribe inferred
    /// and why. Surfaced in the daily-health-summary.
    pub rationale: String,
    /// Which extraction engine produced this proposal (ADR-026):
    /// `"heuristic"` or `"llm"`. `None` for proposals that predate
    /// task_36 or come from non-scribe sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Model identifier when `engine == "llm"`; empty or `None`
    /// for the heuristic engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Submission-local handle other proposals in the same set refer
    /// to (task_45 cross-references). Not an entity id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_ref: Option<String>,
    /// Fields of this claim that must be filled with the entity ids of
    /// other proposals in the same set, by their `local_ref`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<CrossRef>,
    /// Bitemporal window the scribe asserts for this claim (a role's
    /// start, an article's publication date). `None` means "now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<Iso8601>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<Iso8601>,
    /// True when this proposal ends an existing role (ADR-031): it is
    /// a supersession setting `valid_to` on the head affiliation, never
    /// a new atom, so ADR-029 routes it to review.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ends_role: bool,
    /// The entity this proposal is about, once known: bound by the
    /// resolver when `resolution == Existing`, or minted at accept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
}

impl Proposal {
    /// A proposal with only the pre-task_45 fields set; every
    /// resolution field is empty. Keeps struct literals short in
    /// callers that do not resolve.
    pub fn new(
        predicate: PredicateName,
        claim: serde_json::Value,
        provenance: Vec<Provenance>,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            predicate,
            claim,
            provenance,
            rationale: rationale.into(),
            engine: None,
            model: None,
            local_ref: None,
            refs: Vec::new(),
            valid_from: None,
            valid_to: None,
            ends_role: false,
            entity: None,
            resolution: None,
            candidates: Vec::new(),
        }
    }
}

/// A unit of work submitted to the ingest pipeline. Each submission
/// carries the raw bytes, a content-addressed hash so duplicates can
/// be detected, and (after extraction) the proposals the scribe
/// produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Submission {
    pub id: String,
    pub source_uri: String,
    pub content_hash: Multihash,
    pub content: Vec<u8>,
    pub tx_time: Iso8601,
    pub status: SubmissionStatus,
    pub proposals: Vec<Proposal>,
    /// Set when status == Failed. Free-form description.
    pub failure_reason: Option<String>,
    /// Set when status == Accepted. Lists the content hashes of the
    /// atoms the daemon signed + inserted on acceptance.
    #[serde(default)]
    pub accepted_atom_hashes: Vec<Multihash>,
}

/// Storage trait for the ingest quarantine. Methods are async so a
/// future SQLite implementation can offload to the blocking pool.
#[async_trait]
pub trait IngestQuarantine: Send + Sync {
    async fn submit(&self, source_uri: String, content: Vec<u8>)
    -> Result<String, QuarantineError>;
    async fn get(&self, id: &str) -> Option<Submission>;
    async fn list(&self, status_filter: Option<SubmissionStatus>) -> Vec<Submission>;
    /// Attach scribe-produced proposals and transition `Pending` →
    /// `Extracted`. Idempotent: a second call with the same proposals
    /// is a no-op rather than an error so the pipeline tolerates
    /// stutter from a flaky scribe.
    async fn complete(&self, id: &str, proposals: Vec<Proposal>) -> Result<(), QuarantineError>;
    /// Transition `Pending` → `Failed` with a reason string.
    async fn fail(&self, id: &str, reason: String) -> Result<(), QuarantineError>;
    /// Transition `Extracted` → `Accepted`, recording which atom
    /// hashes the daemon signed and inserted. Caller is responsible
    /// for doing the signing + insertion; this just flips the status.
    async fn accept(&self, id: &str, atom_hashes: Vec<Multihash>) -> Result<(), QuarantineError>;
    /// Transition `Extracted` → `Rejected`. The proposals never
    /// become atoms; the submission stays for the audit trail.
    async fn reject(&self, id: &str) -> Result<(), QuarantineError>;
}

/// In-memory quarantine. The default backend; sufficient for MVP and
/// for tests. A future SQLite-backed impl plugs in behind the trait
/// without API changes.
#[derive(Debug, Default)]
pub struct InMemoryQuarantine {
    submissions: Mutex<HashMap<String, Submission>>,
    counter: std::sync::atomic::AtomicU64,
}

impl InMemoryQuarantine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl IngestQuarantine for InMemoryQuarantine {
    async fn submit(
        &self,
        source_uri: String,
        content: Vec<u8>,
    ) -> Result<String, QuarantineError> {
        let content_hash = Multihash::blake3_of(&content);
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("sub-{n:08}-{}", &content_hash.to_multibase()[..8]);
        let sub = Submission {
            id: id.clone(),
            source_uri,
            content_hash,
            content,
            tx_time: current_iso8601(),
            status: SubmissionStatus::Pending,
            proposals: Vec::new(),
            failure_reason: None,
            accepted_atom_hashes: Vec::new(),
        };
        self.submissions.lock().await.insert(id.clone(), sub);
        Ok(id)
    }

    async fn get(&self, id: &str) -> Option<Submission> {
        self.submissions.lock().await.get(id).cloned()
    }

    async fn list(&self, status_filter: Option<SubmissionStatus>) -> Vec<Submission> {
        let guard = self.submissions.lock().await;
        let mut out: Vec<Submission> = match status_filter {
            None => guard.values().cloned().collect(),
            Some(s) => guard
                .values()
                .filter(|sub| sub.status == s)
                .cloned()
                .collect(),
        };
        // Stable ordering for tests + UI: by submission id.
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    async fn complete(&self, id: &str, proposals: Vec<Proposal>) -> Result<(), QuarantineError> {
        let mut guard = self.submissions.lock().await;
        let sub = guard
            .get_mut(id)
            .ok_or_else(|| QuarantineError::NotFound(id.to_string()))?;
        if sub.status == SubmissionStatus::Failed {
            return Err(QuarantineError::BadTransition {
                from: "failed".into(),
                to: "extracted".into(),
            });
        }
        sub.status = SubmissionStatus::Extracted;
        sub.proposals = proposals;
        Ok(())
    }

    async fn fail(&self, id: &str, reason: String) -> Result<(), QuarantineError> {
        let mut guard = self.submissions.lock().await;
        let sub = guard
            .get_mut(id)
            .ok_or_else(|| QuarantineError::NotFound(id.to_string()))?;
        if sub.status == SubmissionStatus::Extracted {
            return Err(QuarantineError::BadTransition {
                from: "extracted".into(),
                to: "failed".into(),
            });
        }
        sub.status = SubmissionStatus::Failed;
        sub.failure_reason = Some(reason);
        Ok(())
    }

    async fn accept(&self, id: &str, atom_hashes: Vec<Multihash>) -> Result<(), QuarantineError> {
        let mut guard = self.submissions.lock().await;
        let sub = guard
            .get_mut(id)
            .ok_or_else(|| QuarantineError::NotFound(id.to_string()))?;
        if sub.status != SubmissionStatus::Extracted {
            return Err(QuarantineError::BadTransition {
                from: format!("{:?}", sub.status).to_lowercase(),
                to: "accepted".into(),
            });
        }
        sub.status = SubmissionStatus::Accepted;
        sub.accepted_atom_hashes = atom_hashes;
        Ok(())
    }

    async fn reject(&self, id: &str) -> Result<(), QuarantineError> {
        let mut guard = self.submissions.lock().await;
        let sub = guard
            .get_mut(id)
            .ok_or_else(|| QuarantineError::NotFound(id.to_string()))?;
        if sub.status != SubmissionStatus::Extracted {
            return Err(QuarantineError::BadTransition {
                from: format!("{:?}", sub.status).to_lowercase(),
                to: "rejected".into(),
            });
        }
        sub.status = SubmissionStatus::Rejected;
        Ok(())
    }
}

fn current_iso8601() -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let now = time::OffsetDateTime::now_utc();
    let s = now
        .format(&Fmt::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    Iso8601::new(s).expect("formatted ISO8601 must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_creates_pending_submission_with_hash() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"# hello".to_vec())
            .await
            .unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.source_uri, "file:///a.md");
        assert_eq!(sub.status, SubmissionStatus::Pending);
        assert_eq!(sub.proposals.len(), 0);
        assert_eq!(sub.content_hash, Multihash::blake3_of(b"# hello"));
    }

    #[tokio::test]
    async fn complete_attaches_proposals_and_transitions_to_extracted() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"x".to_vec())
            .await
            .unwrap();
        let p = Proposal {
            predicate: PredicateName::new("contact.person"),
            claim: serde_json::json!({"display_name": "Sara"}),
            provenance: vec![],
            rationale: "extracted from frontmatter".into(),
            engine: Some("heuristic".into()),
            model: None,
            ..Proposal::new(
                PredicateName::new("contact.person"),
                serde_json::Value::Null,
                vec![],
                "",
            )
        };
        q.complete(&id, vec![p.clone()]).await.unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Extracted);
        assert_eq!(sub.proposals, vec![p]);
    }

    #[tokio::test]
    async fn fail_records_reason_and_transitions_to_failed() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"x".to_vec())
            .await
            .unwrap();
        q.fail(&id, "scribe crashed".into()).await.unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Failed);
        assert_eq!(sub.failure_reason.as_deref(), Some("scribe crashed"));
    }

    #[tokio::test]
    async fn cannot_complete_a_failed_submission() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"x".to_vec())
            .await
            .unwrap();
        q.fail(&id, "boom".into()).await.unwrap();
        let err = q.complete(&id, vec![]).await.unwrap_err();
        assert!(matches!(err, QuarantineError::BadTransition { .. }));
    }

    #[tokio::test]
    async fn list_with_status_filter() {
        let q = InMemoryQuarantine::new();
        let a = q.submit("a".into(), b"x".to_vec()).await.unwrap();
        let _b = q.submit("b".into(), b"y".to_vec()).await.unwrap();
        let c = q.submit("c".into(), b"z".to_vec()).await.unwrap();
        q.complete(&a, vec![]).await.unwrap();
        q.fail(&c, "boom".into()).await.unwrap();
        let pending = q.list(Some(SubmissionStatus::Pending)).await;
        assert_eq!(pending.len(), 1);
        let extracted = q.list(Some(SubmissionStatus::Extracted)).await;
        assert_eq!(extracted.len(), 1);
        let all = q.list(None).await;
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn accept_flips_extracted_to_accepted_and_records_hashes() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"x".to_vec())
            .await
            .unwrap();
        q.complete(&id, vec![]).await.unwrap();
        let h1 = Multihash::blake3_of(b"atom-1");
        let h2 = Multihash::blake3_of(b"atom-2");
        q.accept(&id, vec![h1.clone(), h2.clone()]).await.unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Accepted);
        assert_eq!(sub.accepted_atom_hashes, vec![h1, h2]);
    }

    #[tokio::test]
    async fn reject_flips_extracted_to_rejected() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"x".to_vec())
            .await
            .unwrap();
        q.complete(&id, vec![]).await.unwrap();
        q.reject(&id).await.unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Rejected);
    }

    #[tokio::test]
    async fn cannot_accept_a_pending_submission() {
        let q = InMemoryQuarantine::new();
        let id = q
            .submit("file:///a.md".into(), b"x".to_vec())
            .await
            .unwrap();
        let err = q.accept(&id, vec![]).await.unwrap_err();
        assert!(matches!(err, QuarantineError::BadTransition { .. }));
    }
}

#[cfg(test)]
mod cross_ref_tests {
    use super::*;

    #[test]
    fn wire_fields_normalize_to_pointer_form() {
        assert_eq!(
            CrossRef::from_wire_field("mentions[2].entity"),
            "mentions/2/entity"
        );
        assert_eq!(CrossRef::from_wire_field("organization"), "organization");
        assert_eq!(
            CrossRef::from_wire_field("participants/0/entity"),
            "participants/0/entity"
        );
    }

    #[test]
    fn apply_sets_top_level_and_array_item_keys() {
        let mut claim =
            serde_json::json!({"title": "t", "mentions": [{"display": "A"}, {"display": "B"}]});
        assert!(CrossRef::apply(
            &mut claim,
            "mentions[1].entity",
            serde_json::json!("zabc")
        ));
        assert_eq!(claim["mentions"][1]["entity"], "zabc");
        assert!(claim["mentions"][0].get("entity").is_none());
        assert!(CrossRef::apply(
            &mut claim,
            "organization",
            serde_json::json!("zorg")
        ));
        assert_eq!(claim["organization"], "zorg");
        assert!(
            !CrossRef::apply(&mut claim, "mentions[9].entity", serde_json::json!("x")),
            "missing item is not invented"
        );
        assert!(!CrossRef::apply(&mut claim, "", serde_json::json!("x")));
    }

    #[test]
    fn resolution_serializes_lowercase_and_empty_fields_are_omitted() {
        let p = Proposal::new(
            PredicateName::new("note"),
            serde_json::json!({}),
            vec![],
            "r",
        );
        let v = serde_json::to_value(&p).unwrap();
        assert!(
            v.get("refs").is_none()
                && v.get("candidates").is_none()
                && v.get("ends_role").is_none()
        );
        let mut q = p.clone();
        q.resolution = Some(Resolution::Ambiguous);
        q.ends_role = true;
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v["resolution"], "ambiguous");
        assert_eq!(v["ends_role"], true);
        let back: Proposal = serde_json::from_value(v).unwrap();
        assert_eq!(back.resolution, Some(Resolution::Ambiguous));
    }
}
