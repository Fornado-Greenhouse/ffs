// May you do good and not evil.
// May you find forgiveness for yourself and forgive others.
// May you share freely, never taking more than you give.
//   — the SQLite blessing, carried with gratitude

//! SQLCipher-encrypted SQLite-backed [`AtomStore`].
//!
//! On open, the supplied 32-byte DEK is bound to the connection via
//! `PRAGMA key`. WAL mode is enabled for concurrent readers; foreign keys
//! are enforced. The schema is applied idempotently per
//! [`super::migrations::apply`].
//!
//! All connection access is serialized through a single `Mutex<Connection>`
//! at the Rust level. SQLite's WAL mode allows the same Connection to
//! serve readers and writers without per-call lock thrashing inside SQLite;
//! the Rust mutex prevents foreign-key check races that occur when two
//! Rust callers issue overlapping prepared statements through the same
//! handle. A connection pool is a future optimization (Phase 3+).

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags, params};

use crate::atom::{AtomEnvelope, EntityId, Iso8601, PredicateName, PublicKey};
use crate::multihash::Multihash;

use super::{AtomStore, StoreError, migrations};

/// Production atom store. SQLCipher-encrypted, file-backed.
pub struct SqliteAtomStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteAtomStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteAtomStore").finish_non_exhaustive()
    }
}

impl SqliteAtomStore {
    /// Open a SQLCipher database at `path`, encrypted with `key` (32 bytes).
    /// On a fresh path the schema is created. On an existing path the key
    /// is verified by reading the schema_version table — wrong keys cause
    /// the read to fail and the open to error.
    pub fn open_with_key(path: impl AsRef<Path>, key: &[u8; 32]) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        Self::configure(&conn, key)?;
        migrations::apply(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory SQLCipher database with the supplied key. Used by
    /// tests; in-memory databases have no on-disk encryption to verify, but
    /// the key is still applied so the codepath matches production.
    pub fn open_in_memory(key: &[u8; 32]) -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn, key)?;
        migrations::apply(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn configure(conn: &Connection, key: &[u8; 32]) -> Result<(), StoreError> {
        // Apply key first — every other PRAGMA must wait until decryption is set.
        let hex_key: String = key.iter().map(|b| format!("{b:02x}")).collect();
        conn.execute_batch(&format!("PRAGMA key = \"x'{hex_key}'\";"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        Ok(())
    }
}

impl AtomStore for SqliteAtomStore {
    fn insert(&self, envelope: &AtomEnvelope) -> Result<Multihash, StoreError> {
        envelope
            .verify()
            .map_err(|e| StoreError::InvalidSignature(e.to_string()))?;
        let canonical = envelope
            .canonical_bytes()
            .map_err(|e| StoreError::Serialization(e.to_string()))?;
        let hash = envelope
            .content_hash()
            .map_err(|e| StoreError::Serialization(e.to_string()))?;

        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;

        // Idempotent: skip if already present. We compare on content_hash.
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM atoms WHERE content_hash = ?1",
                params![hash.as_bytes().as_slice()],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if exists {
            return Ok(hash);
        }

        tx.execute(
            "INSERT INTO atoms(
                content_hash, entity_id, predicate, author,
                valid_from, valid_to, tx_time, classification,
                supersedes, signature, envelope
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                hash.as_bytes().as_slice(),
                envelope.entity.as_str(),
                envelope.predicate.as_str(),
                envelope.author.as_bytes().as_slice(),
                envelope.valid_from.as_str(),
                envelope.valid_to.as_ref().map(|v| v.as_str()),
                envelope.tx_time.as_str(),
                envelope.classification.as_str(),
                envelope
                    .supersedes
                    .as_ref()
                    .map(|m| m.as_bytes().as_slice()),
                envelope.signature.as_bytes().as_slice(),
                canonical.as_slice(),
            ],
        )?;

        // Derived classifications row capturing the atom's primary classification.
        tx.execute(
            "INSERT INTO classifications(atom_hash, tier, classifier, tx_time)
             VALUES (?1,?2,?3,?4)",
            params![
                hash.as_bytes().as_slice(),
                envelope.classification.as_str(),
                envelope.author.as_bytes().as_slice(),
                envelope.tx_time.as_str(),
            ],
        )?;

        // Provenance rows.
        for p in &envelope.provenance {
            let kind = match &p.kind {
                crate::atom::SourceKind::IngestFile => "ingest_file",
                crate::atom::SourceKind::McpAgent => "mcp_agent",
                crate::atom::SourceKind::FederationPull => "federation_pull",
                crate::atom::SourceKind::FastPath => "fast_path",
            };
            tx.execute(
                "INSERT INTO provenance(atom_hash, source_kind, source_uri, source_hash)
                 VALUES (?1,?2,?3,?4)",
                params![
                    hash.as_bytes().as_slice(),
                    kind,
                    p.uri,
                    p.hash.as_bytes().as_slice(),
                ],
            )?;
        }

        // Index claim payload into FTS5. Indexing the JSON-serialized claim
        // is sufficient for the substrate's text-search use cases at MVP scale.
        let claim_text = serde_json::to_string(&envelope.claim)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;
        tx.execute(
            "INSERT INTO claims_fts(content_hash, payload) VALUES (?1, ?2)",
            params![hash.as_bytes().as_slice(), claim_text],
        )?;

        // Ensure the entity row exists.
        tx.execute(
            "INSERT OR IGNORE INTO entities(entity_id, canonical_label) VALUES (?1, NULL)",
            params![envelope.entity.as_str()],
        )?;

        tx.commit()?;
        Ok(hash)
    }

    fn get(&self, hash: &Multihash) -> Result<Option<AtomEnvelope>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT envelope FROM atoms WHERE content_hash = ?1")?;
        let row: Option<Vec<u8>> = stmt
            .query_row(params![hash.as_bytes().as_slice()], |r| r.get(0))
            .ok();
        match row {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<AtomEnvelope>(&bytes)
                .map(Some)
                .map_err(|e| StoreError::Serialization(e.to_string())),
        }
    }

    fn exists(&self, hash: &Multihash) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM atoms WHERE content_hash = ?1",
            params![hash.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    fn list_by_entity(
        &self,
        entity: &EntityId,
        predicate: Option<&PredicateName>,
        as_of: Option<&Iso8601>,
    ) -> Result<Vec<AtomEnvelope>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from("SELECT envelope FROM atoms WHERE entity_id = ?1");
        if predicate.is_some() {
            sql.push_str(" AND predicate = ?2");
        }
        if as_of.is_some() {
            // Bind position depends on whether predicate was bound.
            if predicate.is_some() {
                sql.push_str(" AND tx_time <= ?3");
            } else {
                sql.push_str(" AND tx_time <= ?2");
            }
        }
        sql.push_str(" ORDER BY tx_time DESC");

        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<Vec<u8>> = match (predicate, as_of) {
            (Some(p), Some(a)) => stmt
                .query_map(params![entity.as_str(), p.as_str(), a.as_str()], |r| {
                    r.get(0)
                })?
                .collect::<Result<_, _>>()?,
            (Some(p), None) => stmt
                .query_map(params![entity.as_str(), p.as_str()], |r| r.get(0))?
                .collect::<Result<_, _>>()?,
            (None, Some(a)) => stmt
                .query_map(params![entity.as_str(), a.as_str()], |r| r.get(0))?
                .collect::<Result<_, _>>()?,
            (None, None) => stmt
                .query_map(params![entity.as_str()], |r| r.get(0))?
                .collect::<Result<_, _>>()?,
        };
        rows.into_iter()
            .map(|b| {
                serde_json::from_slice::<AtomEnvelope>(&b)
                    .map_err(|e| StoreError::Serialization(e.to_string()))
            })
            .collect()
    }

    fn head_of_chain(
        &self,
        entity: &EntityId,
        predicate: &PredicateName,
        as_of: Option<&Iso8601>,
    ) -> Result<Option<AtomEnvelope>, StoreError> {
        // Active candidates: atoms in (entity, predicate) at or before as_of.
        // Superseded set: atoms referenced by any candidate's `supersedes`.
        // Head: candidate not in superseded set; tie-break by tx_time DESC, content_hash DESC.
        let conn = self.conn.lock().unwrap();
        let sql = "
            WITH candidates AS (
              SELECT content_hash, supersedes, envelope, tx_time
              FROM atoms
              WHERE entity_id = ?1 AND predicate = ?2 AND tx_time <= ?3
            ),
            superseded AS (
              SELECT supersedes AS h FROM candidates WHERE supersedes IS NOT NULL
            )
            SELECT envelope FROM candidates
            WHERE content_hash NOT IN (SELECT h FROM superseded)
            ORDER BY tx_time DESC, content_hash DESC
            LIMIT 1
        ";
        let cutoff = as_of
            .map(|a| a.as_str().to_owned())
            .unwrap_or_else(|| "9999-12-31T23:59:59Z".into());
        let mut stmt = conn.prepare(sql)?;
        let row: Option<Vec<u8>> = stmt
            .query_row(params![entity.as_str(), predicate.as_str(), cutoff], |r| {
                r.get(0)
            })
            .ok();
        match row {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<AtomEnvelope>(&bytes)
                .map(Some)
                .map_err(|e| StoreError::Serialization(e.to_string())),
        }
    }

    fn list_by_predicate(
        &self,
        predicate: &PredicateName,
        since_tx: Option<&Iso8601>,
        limit: usize,
    ) -> Result<Vec<AtomEnvelope>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let (sql, rows): (&str, Vec<Vec<u8>>) = if let Some(since) = since_tx {
            let s = "SELECT envelope FROM atoms WHERE predicate = ?1 AND tx_time > ?2 \
                     ORDER BY tx_time DESC LIMIT ?3";
            let mut stmt = conn.prepare(s)?;
            let r: Vec<Vec<u8>> = stmt
                .query_map(
                    params![predicate.as_str(), since.as_str(), limit as i64],
                    |r| r.get(0),
                )?
                .collect::<Result<_, _>>()?;
            (s, r)
        } else {
            let s = "SELECT envelope FROM atoms WHERE predicate = ?1 \
                     ORDER BY tx_time DESC LIMIT ?2";
            let mut stmt = conn.prepare(s)?;
            let r: Vec<Vec<u8>> = stmt
                .query_map(params![predicate.as_str(), limit as i64], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            (s, r)
        };
        let _ = sql;
        rows.into_iter()
            .map(|b| {
                serde_json::from_slice::<AtomEnvelope>(&b)
                    .map_err(|e| StoreError::Serialization(e.to_string()))
            })
            .collect()
    }

    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<Multihash>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT content_hash FROM claims_fts WHERE claims_fts MATCH ?1 LIMIT ?2")?;
        let hashes: Vec<Vec<u8>> = stmt
            .query_map(params![query, limit as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        hashes
            .into_iter()
            .map(|b| Multihash::from_bytes(&b).map_err(|e| StoreError::Malformed(e.to_string())))
            .collect()
    }
}

// Marker so future readers know we deliberately don't expose author lookup
// (intentionally omitted from MVP trait surface; a query-by-author RPC will
// arrive with task 07's dispatcher if needed).
#[allow(dead_code)]
fn _author_query_marker(_: &PublicKey) {}
