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

use crate::{Iso8601, Multihash, PredicateName, Provenance};

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

/// A single proposed atom produced by the scribe from a submission.
/// Proposals carry their own provenance (back to the submission) and
/// a rationale string so the user understands what the scribe inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub predicate: PredicateName,
    pub claim: serde_json::Value,
    pub provenance: Vec<Provenance>,
    /// Short human-readable explanation of what the scribe inferred
    /// and why. Surfaced in the daily-health-summary.
    pub rationale: String,
}

/// A unit of work submitted to the ingest pipeline. Each submission
/// carries the raw bytes, a content-addressed hash so duplicates can
/// be detected, and (after extraction) the proposals the scribe
/// produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
