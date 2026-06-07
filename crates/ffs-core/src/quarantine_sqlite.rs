// May you do good and not evil.
// May you find forgiveness for yourself and forgive others.
// May you share freely, never taking more than you give.
//   — the SQLite blessing, carried with gratitude

//! SQLCipher-backed `IngestQuarantine` (task_29).
//!
//! Persists submissions and their scribe-produced proposals to the
//! same encrypted database the atom store uses, so the DEK
//! protects both at rest and the user has one file to back up. The
//! v2 migration in `store::schema` adds the two tables this module
//! reads/writes:
//!
//! ```text
//! quarantine_submissions (id, source_uri, content_hash, content,
//!   tx_time, status, failure_reason, accepted_atom_hashes)
//! quarantine_proposals   (submission_id, seq, predicate, claim,
//!   provenance, rationale)
//! ```
//!
//! The trait surface stays unchanged from `InMemoryQuarantine`. The
//! daemon binary swaps the wiring at startup; tests continue to use
//! the in-memory backend.

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::multihash::Multihash;
use crate::quarantine::{
    IngestQuarantine, Proposal, QuarantineError, Submission, SubmissionStatus,
};
use crate::store::{StoreError, migrations};
use crate::{Iso8601, PredicateName, Provenance};

/// SQLCipher-backed quarantine. Opens its own connection to the
/// same `atoms.db` the atom store opens. WAL mode (enabled by the
/// atom store) allows concurrent reads from both connections; the
/// Rust-side `Mutex` here serializes our own write transactions.
pub struct SqliteQuarantine {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteQuarantine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteQuarantine").finish_non_exhaustive()
    }
}

impl SqliteQuarantine {
    /// Open the SQLCipher database at `path` with the given DEK and
    /// run any pending migrations. Idempotent: opening against a
    /// database already at the current schema version is a no-op.
    pub fn open_with_key(path: impl AsRef<Path>, key: &[u8; 32]) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        configure(&conn, key)?;
        migrations::apply(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory SQLCipher database with the supplied key.
    /// Used by tests; in-memory databases have no on-disk
    /// encryption to verify, but the key is still applied so the
    /// codepath matches production.
    pub fn open_in_memory(key: &[u8; 32]) -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        configure(&conn, key)?;
        migrations::apply(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

fn configure(conn: &Connection, key: &[u8; 32]) -> Result<(), StoreError> {
    let hex_key: String = key.iter().map(|b| format!("{b:02x}")).collect();
    conn.execute_batch(&format!("PRAGMA key = \"x'{hex_key}'\";"))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

fn current_iso8601() -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let now = time::OffsetDateTime::now_utc();
    let s = now
        .format(&Fmt::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    Iso8601::new(s).expect("formatted ISO8601 must parse")
}

fn status_to_str(s: &SubmissionStatus) -> &'static str {
    match s {
        SubmissionStatus::Pending => "pending",
        SubmissionStatus::Extracted => "extracted",
        SubmissionStatus::Failed => "failed",
        SubmissionStatus::Accepted => "accepted",
        SubmissionStatus::Rejected => "rejected",
    }
}

fn status_from_str(s: &str) -> Result<SubmissionStatus, QuarantineError> {
    match s {
        "pending" => Ok(SubmissionStatus::Pending),
        "extracted" => Ok(SubmissionStatus::Extracted),
        "failed" => Ok(SubmissionStatus::Failed),
        "accepted" => Ok(SubmissionStatus::Accepted),
        "rejected" => Ok(SubmissionStatus::Rejected),
        other => Err(QuarantineError::BadTransition {
            from: other.into(),
            to: "<unknown>".into(),
        }),
    }
}

/// Wire-shape for the `accepted_atom_hashes` column. We store the
/// list as a JSON array of multibase strings so the column stays
/// readable in a SQLite GUI and a future schema-only inspection
/// can decode it without traversing the `Multihash` type.
#[derive(Serialize, Deserialize)]
struct StoredHashes(Vec<String>);

fn encode_hashes(hashes: &[Multihash]) -> String {
    let list: Vec<String> = hashes.iter().map(|h| h.to_multibase()).collect();
    serde_json::to_string(&StoredHashes(list)).unwrap_or_else(|_| "[]".into())
}

fn decode_hashes(s: &str) -> Vec<Multihash> {
    let stored: StoredHashes = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    stored
        .0
        .into_iter()
        .filter_map(|mb| Multihash::from_multibase(&mb).ok())
        .collect()
}

fn map_io(e: rusqlite::Error) -> QuarantineError {
    // The `IngestQuarantine` trait doesn't expose a generic
    // "storage failed" error variant — for MVP we collapse rusqlite
    // errors into BadTransition with the sqlite message as
    // diagnostic. The daemon binary logs the resulting error at
    // its own layer.
    QuarantineError::BadTransition {
        from: "<sqlite>".into(),
        to: e.to_string(),
    }
}

/// Row tuple returned by the submissions-table SELECT. Pulled out
/// into its own struct so the helper that turns a row into a
/// `Submission` doesn't trip clippy's `too_many_arguments` lint.
struct SubmissionRow {
    id: String,
    source_uri: String,
    content_hash: Vec<u8>,
    content: Vec<u8>,
    tx_time: String,
    status: String,
    failure_reason: Option<String>,
    accepted_hashes_json: String,
}

fn row_to_submission(conn: &Connection, row: SubmissionRow) -> Result<Submission, QuarantineError> {
    let SubmissionRow {
        id,
        source_uri,
        content_hash,
        content,
        tx_time,
        status,
        failure_reason,
        accepted_hashes_json,
    } = row;
    let mut stmt = conn
        .prepare(
            "SELECT predicate, claim, provenance, rationale
             FROM quarantine_proposals
             WHERE submission_id = ?1
             ORDER BY seq ASC",
        )
        .map_err(map_io)?;
    let proposals: Vec<Proposal> = stmt
        .query_map(params![id], |row| {
            let predicate: String = row.get(0)?;
            let claim_json: String = row.get(1)?;
            let provenance_json: String = row.get(2)?;
            let rationale: String = row.get(3)?;
            let claim: serde_json::Value =
                serde_json::from_str(&claim_json).unwrap_or(serde_json::Value::Null);
            let provenance: Vec<Provenance> =
                serde_json::from_str(&provenance_json).unwrap_or_default();
            Ok(Proposal {
                predicate: PredicateName::new(predicate),
                claim,
                provenance,
                rationale,
            })
        })
        .map_err(map_io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_io)?;

    Ok(Submission {
        id,
        source_uri,
        content_hash: Multihash::from_bytes(&content_hash)
            .unwrap_or_else(|_| Multihash::blake3_of(&[])),
        content,
        tx_time: Iso8601::new(&tx_time).unwrap_or_else(|_| current_iso8601()),
        status: status_from_str(&status)?,
        proposals,
        failure_reason,
        accepted_atom_hashes: decode_hashes(&accepted_hashes_json),
    })
}

#[async_trait]
impl IngestQuarantine for SqliteQuarantine {
    async fn submit(
        &self,
        source_uri: String,
        content: Vec<u8>,
    ) -> Result<String, QuarantineError> {
        let content_hash = Multihash::blake3_of(&content);
        let conn = self.conn.lock().unwrap();
        // Sequence number = number of existing rows + 1, padded
        // for stable lexical ordering. The hash suffix keeps the
        // id unique even if two submissions land in the same tick.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM quarantine_submissions", [], |row| {
                row.get(0)
            })
            .map_err(map_io)?;
        let id = format!("sub-{n:08}-{}", &content_hash.to_multibase()[..8]);
        let tx_time = current_iso8601();
        conn.execute(
            "INSERT INTO quarantine_submissions
                (id, source_uri, content_hash, content, tx_time, status,
                 failure_reason, accepted_atom_hashes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, '[]')",
            params![
                id,
                source_uri,
                content_hash.as_bytes().to_vec(),
                content,
                tx_time.as_str(),
                status_to_str(&SubmissionStatus::Pending),
            ],
        )
        .map_err(map_io)?;
        Ok(id)
    }

    async fn get(&self, id: &str) -> Option<Submission> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, source_uri, content_hash, content, tx_time, status,
                        failure_reason, accepted_atom_hashes
                 FROM quarantine_submissions WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .ok()?;
        row_to_submission(
            &conn,
            SubmissionRow {
                id: row.0,
                source_uri: row.1,
                content_hash: row.2,
                content: row.3,
                tx_time: row.4,
                status: row.5,
                failure_reason: row.6,
                accepted_hashes_json: row.7,
            },
        )
        .ok()
    }

    async fn list(&self, status_filter: Option<SubmissionStatus>) -> Vec<Submission> {
        let conn = self.conn.lock().unwrap();
        let (sql, status_str): (&str, Option<&str>) = match status_filter.as_ref() {
            None => (
                "SELECT id, source_uri, content_hash, content, tx_time, status,
                        failure_reason, accepted_atom_hashes
                 FROM quarantine_submissions
                 ORDER BY id ASC",
                None,
            ),
            Some(s) => (
                "SELECT id, source_uri, content_hash, content, tx_time, status,
                        failure_reason, accepted_atom_hashes
                 FROM quarantine_submissions
                 WHERE status = ?1
                 ORDER BY id ASC",
                Some(status_to_str(s)),
            ),
        };
        let Ok(mut stmt) = conn.prepare(sql) else {
            return Vec::new();
        };

        let mapper = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
            ))
        };
        let rows_iter = if let Some(s) = status_str {
            stmt.query_map(params![s], mapper)
        } else {
            stmt.query_map([], mapper)
        };
        let Ok(rows_iter) = rows_iter else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for row in rows_iter.flatten() {
            if let Ok(sub) = row_to_submission(
                &conn,
                SubmissionRow {
                    id: row.0,
                    source_uri: row.1,
                    content_hash: row.2,
                    content: row.3,
                    tx_time: row.4,
                    status: row.5,
                    failure_reason: row.6,
                    accepted_hashes_json: row.7,
                },
            ) {
                out.push(sub);
            }
        }
        out
    }

    async fn complete(&self, id: &str, proposals: Vec<Proposal>) -> Result<(), QuarantineError> {
        let conn = self.conn.lock().unwrap();
        let current_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_submissions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|_| QuarantineError::NotFound(id.to_string()))?;
        let current = status_from_str(&current_status)?;

        // Idempotence: a second `complete` against a submission
        // that's already Extracted is a no-op. Failed → Extracted
        // is forbidden (the scribe shouldn't write proposals on top
        // of a failed extraction).
        if current == SubmissionStatus::Extracted {
            return Ok(());
        }
        if current == SubmissionStatus::Failed {
            return Err(QuarantineError::BadTransition {
                from: "failed".into(),
                to: "extracted".into(),
            });
        }

        let tx = conn.unchecked_transaction().map_err(map_io)?;
        tx.execute(
            "UPDATE quarantine_submissions SET status = ?1 WHERE id = ?2",
            params![status_to_str(&SubmissionStatus::Extracted), id],
        )
        .map_err(map_io)?;
        // Replace whatever proposals were previously attached (none
        // expected, but defensive against retries).
        tx.execute(
            "DELETE FROM quarantine_proposals WHERE submission_id = ?1",
            params![id],
        )
        .map_err(map_io)?;
        for (seq, p) in proposals.iter().enumerate() {
            tx.execute(
                "INSERT INTO quarantine_proposals
                    (submission_id, seq, predicate, claim, provenance, rationale)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    seq as i64,
                    p.predicate.as_str(),
                    serde_json::to_string(&p.claim).unwrap_or_else(|_| "null".into()),
                    serde_json::to_string(&p.provenance).unwrap_or_else(|_| "[]".into()),
                    p.rationale,
                ],
            )
            .map_err(map_io)?;
        }
        tx.commit().map_err(map_io)?;
        Ok(())
    }

    async fn fail(&self, id: &str, reason: String) -> Result<(), QuarantineError> {
        let conn = self.conn.lock().unwrap();
        let current_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_submissions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|_| QuarantineError::NotFound(id.to_string()))?;
        let current = status_from_str(&current_status)?;
        if current == SubmissionStatus::Extracted {
            return Err(QuarantineError::BadTransition {
                from: "extracted".into(),
                to: "failed".into(),
            });
        }
        conn.execute(
            "UPDATE quarantine_submissions
             SET status = ?1, failure_reason = ?2
             WHERE id = ?3",
            params![status_to_str(&SubmissionStatus::Failed), reason, id],
        )
        .map_err(map_io)?;
        Ok(())
    }

    async fn accept(&self, id: &str, atom_hashes: Vec<Multihash>) -> Result<(), QuarantineError> {
        let conn = self.conn.lock().unwrap();
        let current_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_submissions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|_| QuarantineError::NotFound(id.to_string()))?;
        let current = status_from_str(&current_status)?;
        if current != SubmissionStatus::Extracted {
            return Err(QuarantineError::BadTransition {
                from: status_to_str(&current).into(),
                to: "accepted".into(),
            });
        }
        conn.execute(
            "UPDATE quarantine_submissions
             SET status = ?1, accepted_atom_hashes = ?2
             WHERE id = ?3",
            params![
                status_to_str(&SubmissionStatus::Accepted),
                encode_hashes(&atom_hashes),
                id
            ],
        )
        .map_err(map_io)?;
        Ok(())
    }

    async fn reject(&self, id: &str) -> Result<(), QuarantineError> {
        let conn = self.conn.lock().unwrap();
        let current_status: String = conn
            .query_row(
                "SELECT status FROM quarantine_submissions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|_| QuarantineError::NotFound(id.to_string()))?;
        let current = status_from_str(&current_status)?;
        if current != SubmissionStatus::Extracted {
            return Err(QuarantineError::BadTransition {
                from: status_to_str(&current).into(),
                to: "rejected".into(),
            });
        }
        conn.execute(
            "UPDATE quarantine_submissions SET status = ?1 WHERE id = ?2",
            params![status_to_str(&SubmissionStatus::Rejected), id],
        )
        .map_err(map_io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dek() -> [u8; 32] {
        [0x42u8; 32]
    }

    fn proposal(predicate: &str) -> Proposal {
        Proposal {
            predicate: PredicateName::new(predicate),
            claim: serde_json::json!({"display_name": "Sara Chen"}),
            provenance: vec![],
            rationale: "test".into(),
        }
    }

    #[tokio::test]
    async fn submit_then_get_round_trips() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let id = q
            .submit("file:///note.md".into(), b"hello".to_vec())
            .await
            .unwrap();
        let sub = q.get(&id).await.expect("get");
        assert_eq!(sub.source_uri, "file:///note.md");
        assert_eq!(sub.content, b"hello");
        assert_eq!(sub.status, SubmissionStatus::Pending);
        assert!(sub.proposals.is_empty());
    }

    #[tokio::test]
    async fn complete_then_get_returns_proposals() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let id = q.submit("u".into(), b"x".to_vec()).await.unwrap();
        q.complete(&id, vec![proposal("contact.person")])
            .await
            .unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Extracted);
        assert_eq!(sub.proposals.len(), 1);
        assert_eq!(sub.proposals[0].predicate.as_str(), "contact.person");
    }

    #[tokio::test]
    async fn complete_is_idempotent() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let id = q.submit("u".into(), b"x".to_vec()).await.unwrap();
        q.complete(&id, vec![proposal("contact.person")])
            .await
            .unwrap();
        // A second complete with identical proposals must not error.
        q.complete(&id, vec![proposal("contact.person")])
            .await
            .unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.proposals.len(), 1);
    }

    #[tokio::test]
    async fn accept_only_after_extracted() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let id = q.submit("u".into(), b"x".to_vec()).await.unwrap();
        // Accepting before complete fails.
        let err = q
            .accept(&id, vec![Multihash::blake3_of(b"h")])
            .await
            .unwrap_err();
        assert!(matches!(err, QuarantineError::BadTransition { .. }));
        // After complete it succeeds.
        q.complete(&id, vec![proposal("contact.person")])
            .await
            .unwrap();
        q.accept(&id, vec![Multihash::blake3_of(b"h")])
            .await
            .unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Accepted);
        assert_eq!(sub.accepted_atom_hashes.len(), 1);
    }

    #[tokio::test]
    async fn fail_locks_out_extraction_after() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let id = q.submit("u".into(), b"x".to_vec()).await.unwrap();
        q.fail(&id, "scribe crashed".into()).await.unwrap();
        let err = q
            .complete(&id, vec![proposal("contact.person")])
            .await
            .unwrap_err();
        assert!(matches!(err, QuarantineError::BadTransition { .. }));
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let a = q.submit("a".into(), b"a".to_vec()).await.unwrap();
        let b = q.submit("b".into(), b"b".to_vec()).await.unwrap();
        q.complete(&a, vec![proposal("note")]).await.unwrap();
        // a is Extracted; b is Pending.
        let extracted = q.list(Some(SubmissionStatus::Extracted)).await;
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].id, a);
        let all = q.list(None).await;
        assert_eq!(all.len(), 2);
        let pending = q.list(Some(SubmissionStatus::Pending)).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, b);
    }

    #[tokio::test]
    async fn reject_requires_extracted() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        let id = q.submit("u".into(), b"x".to_vec()).await.unwrap();
        // Pending → reject fails.
        assert!(q.reject(&id).await.is_err());
        q.complete(&id, vec![proposal("note")]).await.unwrap();
        q.reject(&id).await.unwrap();
        let sub = q.get(&id).await.unwrap();
        assert_eq!(sub.status, SubmissionStatus::Rejected);
    }

    #[tokio::test]
    async fn unknown_id_returns_none_or_not_found() {
        let q = SqliteQuarantine::open_in_memory(&dek()).unwrap();
        assert!(q.get("missing").await.is_none());
        let err = q.complete("missing", vec![]).await.unwrap_err();
        assert!(matches!(err, QuarantineError::NotFound(_)));
    }

    #[tokio::test]
    async fn submissions_survive_reopen() {
        // Roundtrip the same DB file: write, drop the connection,
        // open again with the same DEK, and confirm the submission
        // is still there with the proposals attached.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("q.db");
        {
            let q = SqliteQuarantine::open_with_key(&path, &dek()).unwrap();
            let id = q
                .submit("file:///x.md".into(), b"x".to_vec())
                .await
                .unwrap();
            q.complete(&id, vec![proposal("contact.person")])
                .await
                .unwrap();
            assert_eq!(q.get(&id).await.unwrap().proposals.len(), 1);
        }
        // Reopen.
        let q2 = SqliteQuarantine::open_with_key(&path, &dek()).unwrap();
        let pending_or_extracted = q2.list(None).await;
        assert_eq!(pending_or_extracted.len(), 1);
        assert_eq!(pending_or_extracted[0].status, SubmissionStatus::Extracted);
        assert_eq!(pending_or_extracted[0].proposals.len(), 1);
    }
}
