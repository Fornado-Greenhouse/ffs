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
// macOS-only: drop-to-`security-framework` keychain helpers that set
// `kSecAttrAccessGroup` so all FFS binaries land in one logical
// bucket. The keyring crate (even with `apple-native`) doesn't expose
// that knob — see ADR-023 + crates/ffs-core/src/store/keyring_macos.rs.
#[cfg(target_os = "macos")]
mod keyring_macos;
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
/// - v3 (task_36): `engine` and `model` columns on `quarantine_proposals`.
/// - v4 (task_38, ADR-030): `path_index` table (family, basename ->
///   entity, display) so entity ids can be opaque.
/// - v5 (task_45, ADR-030): resolution columns, `resolution_priors`,
///   `nil_sightings`.
/// - v6 (task_39, ADR-029): `auto_accepted_atom_hashes` on submissions
///   and a provenance (kind, hash) index for `max_per_day` counting.
pub const SCHEMA_VERSION: u32 = 7;

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

    // ---- entity identity helpers (task_45, ADR-030) ----

    /// The `target` of the head `entity.same_as` atom for `entity`, if
    /// one exists: this entity has been merged into the target.
    fn same_as_target(
        &self,
        entity: &EntityId,
        as_of: Option<&Iso8601>,
    ) -> Result<Option<EntityId>, StoreError> {
        let Some(head) =
            self.head_of_chain(entity, &PredicateName::new(SAME_AS_PREDICATE), as_of)?
        else {
            return Ok(None);
        };
        if head.valid_to.is_some() {
            // An undone merge: the same_as atom was superseded with a
            // valid_to, so the redirect no longer applies.
            return Ok(None);
        }
        Ok(head
            .claim
            .get("target")
            .and_then(|v| v.as_str())
            .map(EntityId::new))
    }

    /// Follow `entity.same_as` chains to the final winner. Returns the
    /// input when no merge applies. Bounded to 16 hops so a cycle
    /// (two atoms pointing at each other) cannot spin.
    fn follow_same_as(
        &self,
        entity: &EntityId,
        as_of: Option<&Iso8601>,
    ) -> Result<EntityId, StoreError> {
        let mut cur = entity.clone();
        for _ in 0..16 {
            match self.same_as_target(&cur, as_of)? {
                Some(next) if next != cur => cur = next,
                _ => break,
            }
        }
        Ok(cur)
    }

    /// Every entity the owner has asserted is `different_from` this
    /// one, in either direction (the assertion is symmetric). The
    /// reverse direction scans the predicate; fine at personal scale.
    fn different_from(
        &self,
        entity: &EntityId,
        as_of: Option<&Iso8601>,
    ) -> Result<Vec<EntityId>, StoreError> {
        let pred = PredicateName::new(DIFFERENT_FROM_PREDICATE);
        let mut out: Vec<EntityId> = Vec::new();
        for atom in self.list_by_entity(entity, Some(&pred), as_of)? {
            if let Some(other) = atom.claim.get("other").and_then(|v| v.as_str()) {
                let other = EntityId::new(other);
                if !out.contains(&other) {
                    out.push(other);
                }
            }
        }
        for atom in self.list_by_predicate(&pred, None, 10_000)? {
            if as_of.is_some_and(|t| atom.tx_time.as_str() > t.as_str()) {
                continue;
            }
            let other_is_me = atom
                .claim
                .get("other")
                .and_then(|v| v.as_str())
                .is_some_and(|o| o == entity.as_str());
            if other_is_me && atom.entity != *entity && !out.contains(&atom.entity) {
                out.push(atom.entity.clone());
            }
        }
        Ok(out)
    }

    /// Record one accepted resolution of surface form `form` to `entity`
    /// (the prior grows from use, ADR-030).
    fn record_resolution(&self, form: &str, entity: &EntityId) -> Result<(), StoreError>;

    /// Accepted-resolution counts for a surface form, highest first.
    fn prior_counts(&self, form: &str) -> Result<Vec<(EntityId, u32)>, StoreError>;

    /// Record a bare mention that did not mint (NIL policy). Returns
    /// the prior sighting for the same key when one exists, so the
    /// caller can mint on the second sighting and back-fill the first.
    /// The row is left in place either way.
    fn record_sighting(
        &self,
        key: &str,
        submission_id: &str,
        display: &str,
        when: &Iso8601,
    ) -> Result<Option<Sighting>, StoreError>;

    /// Forget a NIL sighting (after minting).
    fn clear_sighting(&self, key: &str) -> Result<(), StoreError>;

    // ---- auto-filing helpers (task_39, ADR-029) ----

    /// How many atoms carry an `auto_accept` provenance entry whose hash
    /// is `grant_hash` and whose `tx_time >= since`. This is the
    /// `max_per_day` usage count: read from the store on every check so
    /// it survives daemon restarts.
    fn count_auto_accepted_since(
        &self,
        grant_hash: &Multihash,
        since: &Iso8601,
    ) -> Result<u32, StoreError>;

    /// Entities whose head `entity.same_as` points at `winner` (the
    /// merge losers). Scans the predicate; fine at personal scale.
    fn same_as_losers(
        &self,
        winner: &EntityId,
        as_of: Option<&Iso8601>,
    ) -> Result<Vec<EntityId>, StoreError> {
        let pred = PredicateName::new(SAME_AS_PREDICATE);
        let mut out: Vec<EntityId> = Vec::new();
        for atom in self.list_by_predicate(&pred, None, 10_000)? {
            if out.contains(&atom.entity) || atom.entity == *winner {
                continue;
            }
            if self.same_as_target(&atom.entity, as_of)?.as_ref() == Some(winner) {
                out.push(atom.entity.clone());
            }
        }
        Ok(out)
    }

    /// `follow_same_as` under the name ADR-029 and task_39 use.
    fn resolve_same_as(
        &self,
        entity: &EntityId,
        as_of: Option<&Iso8601>,
    ) -> Result<EntityId, StoreError> {
        self.follow_same_as(entity, as_of)
    }
}

/// Predicate of the merge-redirect atom (ADR-030).
pub const SAME_AS_PREDICATE: &str = "entity.same_as";
/// Predicate of the owner's "these are different people" assertion.
pub const DIFFERENT_FROM_PREDICATE: &str = "entity.different_from";

/// A bare mention recorded under the NIL policy, keyed by normalized
/// name plus article organization context.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sighting {
    pub key: String,
    pub submission_id: String,
    pub display: String,
    pub first_seen: Iso8601,
}

pub use self::keyring::{
    DEK_SERVICE, OWNER_KEY_SERVICE, dek_from_keyring, owner_key_from_keyring, save_key_to_keychain,
};
