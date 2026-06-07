//! The atom store: signed, content-addressed, bitemporal, capability-checked.
//!
//! Two backends share a single trait:
//!
//! - [`SqliteAtomStore`] — production. SQLCipher-encrypted SQLite per ADR-016
//!   and ADR-018. The DEK is sourced from the OS keychain (or supplied
//!   explicitly for tests) and the database file is binary-encrypted at
//!   rest. WAL mode allows concurrent readers; writes serialize through a
//!   single mutex.
//! - [`MemAtomStore`] — in-memory, used by downstream tests so they need
//!   not stand up SQLCipher.
//!
//! Every insert is verified: the envelope's signature is checked against
//! its canonical-JSON bytes (with the signature field elided), and the
//! content hash is recomputed and matched. Tampered envelopes are
//! rejected with a typed error.
//!
//! Bitemporal queries follow the pattern `WHERE entity_id = ? AND
//! predicate = ? AND tx_time <= ? ORDER BY tx_time DESC` and resolve
//! supersession heads via the `(supersedes)` index per the rules in
//! ARCHITECTURE.md § Concurrency model.

use thiserror::Error;

mod keyring;
mod mem;
pub(crate) mod migrations;
mod schema;
mod sqlite;

pub use mem::MemAtomStore;
pub use sqlite::SqliteAtomStore;

use crate::atom::{AtomEnvelope, EntityId, Iso8601, PredicateName};
use crate::multihash::Multihash;

/// Schema version supported by this build of `ffs-core`. Stores at higher
/// versions refuse to open.
///
/// History:
/// - v1: initial atom store, classifications, capabilities,
///   provenance, entities, claims_fts, federation_peers,
///   working_set, and a placeholder `ingest_quarantine` table.
/// - v2 (task_29): real `quarantine_submissions` + `quarantine_proposals`
///   tables that match the runtime `IngestQuarantine` trait shape.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("atom signature does not verify: {0}")]
    InvalidSignature(String),

    #[error("atom content hash mismatch")]
    HashMismatch,

    #[error("atom is malformed: {0}")]
    Malformed(String),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("schema version {found} is newer than supported version {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error("keyring error: {0}")]
    Keyring(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// The substrate's atom store. Implementations: [`SqliteAtomStore`]
/// (production, encrypted) and [`MemAtomStore`] (in-memory, for tests).
pub trait AtomStore: Send + Sync {
    /// Insert a verified atom envelope. Verifies the signature and recomputes
    /// the content hash before insert; rejects on either mismatch. Returns
    /// the content hash on success. Idempotent: re-inserting an existing
    /// atom is a no-op that returns the same hash.
    fn insert(&self, envelope: &AtomEnvelope) -> Result<Multihash, StoreError>;

    /// Look up an atom by content hash.
    fn get(&self, hash: &Multihash) -> Result<Option<AtomEnvelope>, StoreError>;

    /// Check whether an atom with the given hash exists.
    fn exists(&self, hash: &Multihash) -> Result<bool, StoreError>;

    /// All atoms about an entity, optionally filtered by predicate, with
    /// optional bitemporal cutoff (atoms with `tx_time <= as_of`).
    /// Ordered by `tx_time DESC`.
    fn list_by_entity(
        &self,
        entity: &EntityId,
        predicate: Option<&PredicateName>,
        as_of: Option<&Iso8601>,
    ) -> Result<Vec<AtomEnvelope>, StoreError>;

    /// The non-superseded leaf for `(entity, predicate)` at `as_of`. With
    /// multiple unsuperseded leaves, picks the leaf with the latest `tx_time`,
    /// breaking ties on the content hash. Returns `None` if no atoms exist
    /// for the pair.
    fn head_of_chain(
        &self,
        entity: &EntityId,
        predicate: &PredicateName,
        as_of: Option<&Iso8601>,
    ) -> Result<Option<AtomEnvelope>, StoreError>;

    /// All atoms for a predicate, optionally after a `tx_time` watermark,
    /// with `limit`. Ordered by `tx_time DESC`. Used for path-family enumeration.
    fn list_by_predicate(
        &self,
        predicate: &PredicateName,
        since_tx: Option<&Iso8601>,
        limit: usize,
    ) -> Result<Vec<AtomEnvelope>, StoreError>;

    /// Full-text search over claim payloads. Returns content hashes for
    /// matched atoms in arbitrary order. The query is an FTS5 MATCH expression.
    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<Multihash>, StoreError>;
}

pub use self::keyring::{
    DEK_SERVICE, OWNER_KEY_SERVICE, dek_from_keyring, owner_key_from_keyring, save_key_to_keychain,
};
