//! In-memory [`AtomStore`] backend used by downstream tests.
//!
//! Same trait conformance as [`super::SqliteAtomStore`] so a test can
//! exercise capability evaluation, projection rendering, federation, etc.,
//! without standing up a SQLCipher database. Uses a `BTreeMap` for ordered
//! iteration; concurrent access goes through a single `Mutex`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::atom::{AtomEnvelope, EntityId, Iso8601, PredicateName};
use crate::multihash::Multihash;

use super::{AtomStore, StoreError};

#[derive(Default)]
struct Inner {
    /// content_hash → envelope
    atoms: BTreeMap<Vec<u8>, AtomEnvelope>,
}

#[derive(Default)]
pub struct MemAtomStore {
    inner: Mutex<Inner>,
}

impl MemAtomStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AtomStore for MemAtomStore {
    fn insert(&self, envelope: &AtomEnvelope) -> Result<Multihash, StoreError> {
        envelope
            .verify()
            .map_err(|e| StoreError::InvalidSignature(e.to_string()))?;
        let hash = envelope
            .content_hash()
            .map_err(|e| StoreError::Serialization(e.to_string()))?;
        let mut inner = self.inner.lock().unwrap();
        inner
            .atoms
            .entry(hash.as_bytes().to_vec())
            .or_insert_with(|| envelope.clone());
        Ok(hash)
    }

    fn get(&self, hash: &Multihash) -> Result<Option<AtomEnvelope>, StoreError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.atoms.get(hash.as_bytes().as_slice()).cloned())
    }

    fn exists(&self, hash: &Multihash) -> Result<bool, StoreError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.atoms.contains_key(hash.as_bytes().as_slice()))
    }

    fn list_by_entity(
        &self,
        entity: &EntityId,
        predicate: Option<&PredicateName>,
        as_of: Option<&Iso8601>,
    ) -> Result<Vec<AtomEnvelope>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<AtomEnvelope> = inner
            .atoms
            .values()
            .filter(|a| a.entity == *entity)
            .filter(|a| predicate.is_none_or(|p| &a.predicate == p))
            .filter(|a| as_of.is_none_or(|t| a.tx_time.as_str() <= t.as_str()))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.tx_time.as_str().cmp(a.tx_time.as_str()));
        Ok(out)
    }

    fn head_of_chain(
        &self,
        entity: &EntityId,
        predicate: &PredicateName,
        as_of: Option<&Iso8601>,
    ) -> Result<Option<AtomEnvelope>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let candidates: Vec<&AtomEnvelope> = inner
            .atoms
            .values()
            .filter(|a| a.entity == *entity)
            .filter(|a| a.predicate == *predicate)
            .filter(|a| as_of.is_none_or(|t| a.tx_time.as_str() <= t.as_str()))
            .collect();
        let superseded: std::collections::HashSet<&[u8]> = candidates
            .iter()
            .filter_map(|a| a.supersedes.as_ref())
            .map(|m| m.as_bytes().as_slice())
            .collect();
        let mut leaves: Vec<(&AtomEnvelope, Multihash)> = candidates
            .iter()
            .filter_map(|a| {
                a.content_hash()
                    .ok()
                    .map(|h| (*a, h))
                    .filter(|(_, h)| !superseded.contains(h.as_bytes().as_slice()))
            })
            .collect();
        // Tie-break: latest tx_time, then content_hash DESC.
        leaves.sort_by(|a, b| {
            b.0.tx_time
                .as_str()
                .cmp(a.0.tx_time.as_str())
                .then_with(|| b.1.as_bytes().cmp(a.1.as_bytes()))
        });
        Ok(leaves.first().map(|(a, _)| (*a).clone()))
    }

    fn list_by_predicate(
        &self,
        predicate: &PredicateName,
        since_tx: Option<&Iso8601>,
        limit: usize,
    ) -> Result<Vec<AtomEnvelope>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<AtomEnvelope> = inner
            .atoms
            .values()
            .filter(|a| a.predicate == *predicate)
            .filter(|a| since_tx.is_none_or(|t| a.tx_time.as_str() > t.as_str()))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.tx_time.as_str().cmp(a.tx_time.as_str()));
        out.truncate(limit);
        Ok(out)
    }

    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<Multihash>, StoreError> {
        // The mem backend doesn't index FTS5; do a substring scan over claim
        // payloads. Sufficient for tests exercising "search returns expected
        // hashes for a known input" parity with the Sqlite backend.
        let inner = self.inner.lock().unwrap();
        let q = query.to_lowercase();
        let mut out: Vec<Multihash> = Vec::new();
        for atom in inner.atoms.values() {
            let payload = serde_json::to_string(&atom.claim).unwrap_or_default();
            if payload.to_lowercase().contains(&q)
                && let Ok(h) = atom.content_hash()
            {
                out.push(h);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }
}
