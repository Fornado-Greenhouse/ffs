//! Working-set state: which projection paths the daemon has
//! materialized on disk, when they were last touched, what their last
//! rendered hash was, and whether the user has pinned them. The
//! librarian skill (task 12) uses this to drive drift detection,
//! refresh, and size-cap eviction. The Obsidian plugin (task 17+)
//! uses `touch()` to bump a projection's recency on user view.
//!
//! Two operations sit at the core:
//!
//! - **Drift detection** — compare a stored `last_render_hash`
//!   against a freshly-computed render hash for the same path. A
//!   mismatch means the underlying atoms changed since last
//!   materialization; the projection is stale.
//! - **Eviction** — when the working set exceeds a configurable cap,
//!   drop the oldest non-pinned entries first.
//!
//! Both operate on the entire set and are cheap at MVP scale (the
//! working set is bounded to a few thousand projections per user).
//! The in-memory implementation suffices; a SQLite-backed impl will
//! be added when the librarian needs cross-restart persistence.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{Iso8601, Multihash};

#[derive(Debug, thiserror::Error)]
pub enum WorkingSetError {
    #[error("entry not found: {0}")]
    NotFound(String),
}

/// A single materialized projection on disk plus the metadata the
/// librarian needs to manage it. `path` is the projection path
/// (e.g., `contacts/by-name/S/Sarah.md`) — relative to the user's
/// `~/.ffs/` root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingSetEntry {
    pub path: String,
    pub last_render_hash: Multihash,
    /// ISO 8601 timestamp the entry was last touched. Recency
    /// determines eviction order; new materializations and explicit
    /// `touch()` calls both update this.
    pub last_touched_at: Iso8601,
    /// User-pinned entries are never evicted regardless of recency.
    pub pinned: bool,
}

/// State store for the working set. Methods are async so a future
/// SQLite-backed implementation can offload to the blocking pool;
/// the in-memory impl below satisfies the trait directly.
#[async_trait]
pub trait WorkingSetStore: Send + Sync {
    /// Insert or replace an entry (called on materialization). Sets
    /// `last_touched_at` to `now` and clears the `pinned` bit unless
    /// the caller supplies one — pinning is opt-in via `pin()`.
    async fn upsert(
        &self,
        path: String,
        last_render_hash: Multihash,
        now: Iso8601,
    ) -> Result<(), WorkingSetError>;

    /// Bump `last_touched_at` for an existing entry. No-op if the
    /// entry is missing — the caller can decide whether to materialize.
    async fn touch(&self, path: &str, now: Iso8601) -> Result<(), WorkingSetError>;

    /// Set or clear the `pinned` flag.
    async fn pin(&self, path: &str, pinned: bool) -> Result<(), WorkingSetError>;

    /// Get one entry.
    async fn get(&self, path: &str) -> Option<WorkingSetEntry>;

    /// All entries, sorted oldest-first by `last_touched_at`. Stable
    /// ordering is important: tests and the daily-summary panel both
    /// rely on the same ordering for repeatable output.
    async fn list_oldest_first(&self) -> Vec<WorkingSetEntry>;

    /// Remove an entry. Used by `evict_to_cap`.
    async fn remove(&self, path: &str) -> Result<(), WorkingSetError>;

    /// If the set exceeds `cap`, remove the oldest non-pinned entries
    /// until it fits. Returns the paths that were evicted. Pinned
    /// entries are never removed even if that means staying over cap.
    async fn evict_to_cap(&self, cap: usize) -> Vec<String> {
        let entries = self.list_oldest_first().await;
        if entries.len() <= cap {
            return Vec::new();
        }
        let over = entries.len() - cap;
        let mut evicted = Vec::new();
        for entry in entries.into_iter().filter(|e| !e.pinned).take(over) {
            if self.remove(&entry.path).await.is_ok() {
                evicted.push(entry.path);
            }
        }
        evicted
    }
}

#[derive(Debug, Default)]
pub struct InMemoryWorkingSet {
    entries: Mutex<HashMap<String, WorkingSetEntry>>,
}

impl InMemoryWorkingSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl WorkingSetStore for InMemoryWorkingSet {
    async fn upsert(
        &self,
        path: String,
        last_render_hash: Multihash,
        now: Iso8601,
    ) -> Result<(), WorkingSetError> {
        let mut guard = self.entries.lock().await;
        // Preserve the pinned bit across re-materialization; a pinned
        // projection stays pinned when the librarian refreshes it.
        let pinned = guard.get(&path).map(|e| e.pinned).unwrap_or(false);
        guard.insert(
            path.clone(),
            WorkingSetEntry {
                path,
                last_render_hash,
                last_touched_at: now,
                pinned,
            },
        );
        Ok(())
    }

    async fn touch(&self, path: &str, now: Iso8601) -> Result<(), WorkingSetError> {
        let mut guard = self.entries.lock().await;
        if let Some(entry) = guard.get_mut(path) {
            entry.last_touched_at = now;
        }
        Ok(())
    }

    async fn pin(&self, path: &str, pinned: bool) -> Result<(), WorkingSetError> {
        let mut guard = self.entries.lock().await;
        let entry = guard
            .get_mut(path)
            .ok_or_else(|| WorkingSetError::NotFound(path.to_string()))?;
        entry.pinned = pinned;
        Ok(())
    }

    async fn get(&self, path: &str) -> Option<WorkingSetEntry> {
        self.entries.lock().await.get(path).cloned()
    }

    async fn list_oldest_first(&self) -> Vec<WorkingSetEntry> {
        let guard = self.entries.lock().await;
        let mut out: Vec<WorkingSetEntry> = guard.values().cloned().collect();
        out.sort_by(|a, b| {
            a.last_touched_at
                .as_str()
                .cmp(b.last_touched_at.as_str())
                .then_with(|| a.path.cmp(&b.path))
        });
        out
    }

    async fn remove(&self, path: &str) -> Result<(), WorkingSetError> {
        let mut guard = self.entries.lock().await;
        if guard.remove(path).is_none() {
            return Err(WorkingSetError::NotFound(path.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Iso8601 {
        Iso8601::new(s).unwrap()
    }

    fn h(b: &[u8]) -> Multihash {
        Multihash::blake3_of(b)
    }

    #[tokio::test]
    async fn upsert_and_get_round_trip() {
        let ws = InMemoryWorkingSet::new();
        ws.upsert(
            "contacts/by-name/S/Sara.md".into(),
            h(b"x"),
            ts("2026-05-26T10:00:00Z"),
        )
        .await
        .unwrap();
        let e = ws.get("contacts/by-name/S/Sara.md").await.unwrap();
        assert_eq!(e.last_render_hash, h(b"x"));
        assert!(!e.pinned);
    }

    #[tokio::test]
    async fn upsert_preserves_pinned_bit_on_re_materialize() {
        let ws = InMemoryWorkingSet::new();
        ws.upsert("p".into(), h(b"a"), ts("2026-05-26T10:00:00Z"))
            .await
            .unwrap();
        ws.pin("p", true).await.unwrap();
        ws.upsert("p".into(), h(b"b"), ts("2026-05-26T11:00:00Z"))
            .await
            .unwrap();
        assert!(ws.get("p").await.unwrap().pinned, "pin survives refresh");
    }

    #[tokio::test]
    async fn touch_updates_only_last_touched_at() {
        let ws = InMemoryWorkingSet::new();
        ws.upsert("p".into(), h(b"a"), ts("2026-05-26T10:00:00Z"))
            .await
            .unwrap();
        ws.touch("p", ts("2026-05-26T12:00:00Z")).await.unwrap();
        let e = ws.get("p").await.unwrap();
        assert_eq!(e.last_touched_at.as_str(), "2026-05-26T12:00:00Z");
        assert_eq!(e.last_render_hash, h(b"a"));
    }

    #[tokio::test]
    async fn list_returns_entries_oldest_first() {
        let ws = InMemoryWorkingSet::new();
        ws.upsert("c".into(), h(b"c"), ts("2026-05-26T12:00:00Z"))
            .await
            .unwrap();
        ws.upsert("a".into(), h(b"a"), ts("2026-05-26T10:00:00Z"))
            .await
            .unwrap();
        ws.upsert("b".into(), h(b"b"), ts("2026-05-26T11:00:00Z"))
            .await
            .unwrap();
        let entries = ws.list_oldest_first().await;
        let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
        assert_eq!(paths, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn evict_to_cap_drops_oldest_non_pinned_first() {
        let ws = InMemoryWorkingSet::new();
        for (path, t) in [
            ("oldest", "2026-05-26T10:00:00Z"),
            ("middle", "2026-05-26T11:00:00Z"),
            ("newest", "2026-05-26T12:00:00Z"),
        ] {
            ws.upsert(path.into(), h(path.as_bytes()), ts(t))
                .await
                .unwrap();
        }
        let evicted = ws.evict_to_cap(2).await;
        assert_eq!(evicted, vec!["oldest".to_string()]);
        let remaining: Vec<_> = ws
            .list_oldest_first()
            .await
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(remaining, vec!["middle", "newest"]);
    }

    #[tokio::test]
    async fn evict_to_cap_skips_pinned_entries() {
        let ws = InMemoryWorkingSet::new();
        for (path, t) in [
            ("oldest-pinned", "2026-05-26T10:00:00Z"),
            ("middle", "2026-05-26T11:00:00Z"),
            ("newest", "2026-05-26T12:00:00Z"),
        ] {
            ws.upsert(path.into(), h(path.as_bytes()), ts(t))
                .await
                .unwrap();
        }
        ws.pin("oldest-pinned", true).await.unwrap();
        let evicted = ws.evict_to_cap(2).await;
        // oldest-pinned survives despite being oldest; the eviction
        // picks `middle` instead (next-oldest, non-pinned).
        assert_eq!(evicted, vec!["middle".to_string()]);
        let remaining: Vec<_> = ws
            .list_oldest_first()
            .await
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(remaining, vec!["oldest-pinned", "newest"]);
    }

    #[tokio::test]
    async fn evict_to_cap_returns_empty_when_under_cap() {
        let ws = InMemoryWorkingSet::new();
        ws.upsert("a".into(), h(b"a"), ts("2026-05-26T10:00:00Z"))
            .await
            .unwrap();
        let evicted = ws.evict_to_cap(10).await;
        assert!(evicted.is_empty());
    }

    #[tokio::test]
    async fn pin_on_missing_entry_errors() {
        let ws = InMemoryWorkingSet::new();
        let err = ws.pin("nope", true).await.unwrap_err();
        assert!(matches!(err, WorkingSetError::NotFound(_)));
    }
}
