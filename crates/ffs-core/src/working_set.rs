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

use crate::atom::EntityId;
use crate::{Iso8601, Multihash};

#[derive(Debug, thiserror::Error)]
pub enum WorkingSetError {
    #[error("entry not found: {0}")]
    NotFound(String),
    #[error("path index error: {0}")]
    Index(String),
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
    path_index: InMemoryPathIndex,
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

// --------------------------------------------------------------------
// Path-to-entity index (ADR-030)
// --------------------------------------------------------------------

/// The file basename for an entity within a projection family, and the
/// reverse lookup from a basename to the entity it names.
///
/// Entity ids are opaque and permanent; the human-readable name is
/// claim data; the file name is a projection concern. This index owns
/// that last mapping. Rules:
///
/// - `assign` is idempotent per `(family, entity)`: the first assignment
///   wins and later calls return the same basename until `rename`.
/// - On collision the qualifiers are tried in order as a parenthetical
///   (`Sara_Chen_(Acme)`), then a numeric suffix (`Sara_Chen_(2)`) as
///   the last resort. The result is deterministic for a given insertion
///   order; order-independence is not promised (the first Sara Chen
///   keeps the bare name).
/// - A basename with no index row resolves to the slug-form entity id
///   of the same spelling. That is how pre-ADR-030 entities (whose ids
///   are their basenames) keep working with no migration, and it is
///   why `resolve` returns `Ok(None)` only when the caller should treat
///   the basename as the id itself.
pub trait PathIndex: Send + Sync {
    /// Assign (or return the existing) basename for `entity` in `family`.
    fn assign(
        &self,
        family: &str,
        entity: &EntityId,
        display: &str,
        qualifiers: &[String],
    ) -> Result<String, WorkingSetError>;

    fn basename_for(
        &self,
        family: &str,
        entity: &EntityId,
    ) -> Result<Option<String>, WorkingSetError>;

    fn resolve(&self, family: &str, basename: &str) -> Result<Option<EntityId>, WorkingSetError>;

    /// Recompute the basename for a new display name. Returns
    /// `Some(old_basename)` when it changed so the caller can leave a
    /// redirect stub at the old path; `None` when unchanged or when the
    /// entity had no row (in which case this behaves like `assign`).
    fn rename(
        &self,
        family: &str,
        entity: &EntityId,
        new_display: &str,
        qualifiers: &[String],
    ) -> Result<Option<String>, WorkingSetError>;

    fn remove(&self, family: &str, entity: &EntityId) -> Result<(), WorkingSetError>;
}

/// Turn a display name into a file basename: whitespace and `_` collapse
/// to single underscores, letters, digits, `-` and `.` are kept, and
/// everything else is dropped. Matches the rule the daemon used for
/// slug-form entity ids so files created before ADR-030 keep their names.
pub fn slugify_display(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    let mut last_was_underscore = false;
    for c in display.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '.' {
            out.push(c);
            last_was_underscore = false;
        } else if (c.is_whitespace() || c == '_') && !last_was_underscore {
            out.push('_');
            last_was_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

/// Candidate basenames in the order the collision rule tries them.
pub(crate) fn basename_candidates(display: &str, qualifiers: &[String]) -> Vec<String> {
    let base = slugify_display(display);
    let mut out = vec![base.clone()];
    for q in qualifiers {
        let q = slugify_display(q);
        if q.is_empty() || q == "untitled" {
            continue;
        }
        let candidate = format!("{base}_({q})");
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// In-memory [`PathIndex`]; the production index lives in the SQLite
/// store (`path_index` table, schema v4).
#[derive(Debug, Default)]
pub struct InMemoryPathIndex {
    /// (family, basename) -> (entity, display)
    rows: std::sync::Mutex<HashMap<(String, String), (String, String)>>,
}

impl InMemoryPathIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn assign_locked(
        rows: &mut HashMap<(String, String), (String, String)>,
        family: &str,
        entity: &EntityId,
        display: &str,
        qualifiers: &[String],
    ) -> String {
        for candidate in basename_candidates(display, qualifiers) {
            let key = (family.to_string(), candidate.clone());
            match rows.get(&key) {
                None => {
                    rows.insert(key, (entity.as_str().to_string(), display.to_string()));
                    return candidate;
                }
                Some((e, _)) if e == entity.as_str() => return candidate,
                Some(_) => continue,
            }
        }
        let base = slugify_display(display);
        let mut n = 2u32;
        loop {
            let candidate = format!("{base}_({n})");
            let key = (family.to_string(), candidate.clone());
            if let std::collections::hash_map::Entry::Vacant(slot) = rows.entry(key) {
                slot.insert((entity.as_str().to_string(), display.to_string()));
                return candidate;
            }
            n += 1;
        }
    }
}

impl PathIndex for InMemoryPathIndex {
    fn assign(
        &self,
        family: &str,
        entity: &EntityId,
        display: &str,
        qualifiers: &[String],
    ) -> Result<String, WorkingSetError> {
        let mut rows = self.rows.lock().unwrap();
        if let Some(((_, b), _)) = rows
            .iter()
            .find(|((f, _), (e, _))| f == family && e == entity.as_str())
        {
            return Ok(b.clone());
        }
        Ok(Self::assign_locked(
            &mut rows, family, entity, display, qualifiers,
        ))
    }

    fn basename_for(
        &self,
        family: &str,
        entity: &EntityId,
    ) -> Result<Option<String>, WorkingSetError> {
        let rows = self.rows.lock().unwrap();
        Ok(rows
            .iter()
            .find(|((f, _), (e, _))| f == family && e == entity.as_str())
            .map(|((_, b), _)| b.clone()))
    }

    fn resolve(&self, family: &str, basename: &str) -> Result<Option<EntityId>, WorkingSetError> {
        let rows = self.rows.lock().unwrap();
        Ok(rows
            .get(&(family.to_string(), basename.to_string()))
            .map(|(e, _)| EntityId::new(e.clone())))
    }

    fn rename(
        &self,
        family: &str,
        entity: &EntityId,
        new_display: &str,
        qualifiers: &[String],
    ) -> Result<Option<String>, WorkingSetError> {
        let mut rows = self.rows.lock().unwrap();
        let old = rows
            .iter()
            .find(|((f, _), (e, _))| f == family && e == entity.as_str())
            .map(|((_, b), _)| b.clone());
        if let Some(old_basename) = &old {
            rows.remove(&(family.to_string(), old_basename.clone()));
        }
        let new_basename = Self::assign_locked(&mut rows, family, entity, new_display, qualifiers);
        Ok(match old {
            Some(o) if o != new_basename => Some(o),
            _ => None,
        })
    }

    fn remove(&self, family: &str, entity: &EntityId) -> Result<(), WorkingSetError> {
        let mut rows = self.rows.lock().unwrap();
        rows.retain(|(f, _), (e, _)| !(f == family && e == entity.as_str()));
        Ok(())
    }
}

/// The in-memory working set carries an in-memory path index so a
/// single object can serve both roles in tests and the dev daemon.
impl PathIndex for InMemoryWorkingSet {
    fn assign(
        &self,
        family: &str,
        entity: &EntityId,
        display: &str,
        qualifiers: &[String],
    ) -> Result<String, WorkingSetError> {
        self.path_index.assign(family, entity, display, qualifiers)
    }
    fn basename_for(
        &self,
        family: &str,
        entity: &EntityId,
    ) -> Result<Option<String>, WorkingSetError> {
        self.path_index.basename_for(family, entity)
    }
    fn resolve(&self, family: &str, basename: &str) -> Result<Option<EntityId>, WorkingSetError> {
        self.path_index.resolve(family, basename)
    }
    fn rename(
        &self,
        family: &str,
        entity: &EntityId,
        new_display: &str,
        qualifiers: &[String],
    ) -> Result<Option<String>, WorkingSetError> {
        self.path_index
            .rename(family, entity, new_display, qualifiers)
    }
    fn remove(&self, family: &str, entity: &EntityId) -> Result<(), WorkingSetError> {
        PathIndex::remove(&self.path_index, family, entity)
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

    // ---- Path-to-entity index (task_38, ADR-030) ----

    fn q(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn slugify_display_matches_the_legacy_slug_rule() {
        assert_eq!(slugify_display("Sara Chen"), "Sara_Chen");
        assert_eq!(slugify_display("  Sara   Chen "), "Sara_Chen");
        assert_eq!(slugify_display("Acme, Inc."), "Acme_Inc.");
        assert_eq!(slugify_display("(weird)"), "weird");
        assert_eq!(slugify_display("   "), "untitled");
    }

    #[test]
    fn two_sara_chens_get_parenthetical_qualifiers_in_order() {
        let idx = InMemoryPathIndex::new();
        let a = idx
            .assign("people", &EntityId::new("zA"), "Sara Chen", &q(&["Acme"]))
            .unwrap();
        let b = idx
            .assign(
                "people",
                &EntityId::new("zB"),
                "Sara Chen",
                &q(&["City Council"]),
            )
            .unwrap();
        assert_eq!(a, "Sara_Chen");
        assert_eq!(b, "Sara_Chen_(City_Council)");
        assert_eq!(
            idx.resolve("people", "Sara_Chen")
                .unwrap()
                .unwrap()
                .as_str(),
            "zA"
        );
        assert_eq!(
            idx.resolve("people", "Sara_Chen_(City_Council)")
                .unwrap()
                .unwrap()
                .as_str(),
            "zB"
        );
        // Idempotent: re-assigning returns the same basename.
        assert_eq!(
            idx.assign(
                "people",
                &EntityId::new("zB"),
                "Sara Chen",
                &q(&["City Council"])
            )
            .unwrap(),
            "Sara_Chen_(City_Council)"
        );
        assert_eq!(
            idx.basename_for("people", &EntityId::new("zB"))
                .unwrap()
                .unwrap(),
            "Sara_Chen_(City_Council)"
        );
    }

    #[test]
    fn qualifiers_fall_through_organization_role_year_then_numeric() {
        let idx = InMemoryPathIndex::new();
        idx.assign("people", &EntityId::new("z1"), "Sara Chen", &[])
            .unwrap();
        // Same org as an existing qualified entry forces the next qualifier.
        idx.assign("people", &EntityId::new("z2"), "Sara Chen", &q(&["Acme"]))
            .unwrap();
        let third = idx
            .assign(
                "people",
                &EntityId::new("z3"),
                "Sara Chen",
                &q(&["Acme", "CEO", "2026"]),
            )
            .unwrap();
        assert_eq!(third, "Sara_Chen_(CEO)");
        let fourth = idx
            .assign(
                "people",
                &EntityId::new("z4"),
                "Sara Chen",
                &q(&["Acme", "CEO", "2026"]),
            )
            .unwrap();
        assert_eq!(fourth, "Sara_Chen_(2026)");
        let fifth = idx
            .assign(
                "people",
                &EntityId::new("z5"),
                "Sara Chen",
                &q(&["Acme", "CEO", "2026"]),
            )
            .unwrap();
        assert_eq!(fifth, "Sara_Chen_(2)");
        // No qualifiers at all: straight to the numeric suffix.
        let sixth = idx
            .assign("people", &EntityId::new("z6"), "Sara Chen", &[])
            .unwrap();
        assert_eq!(sixth, "Sara_Chen_(3)");
    }

    #[test]
    fn rename_returns_the_old_basename_and_moves_resolution() {
        let idx = InMemoryPathIndex::new();
        idx.assign("people", &EntityId::new("zA"), "Sara Chen", &[])
            .unwrap();
        let old = idx
            .rename("people", &EntityId::new("zA"), "Sara Chen-Lopez", &[])
            .unwrap();
        assert_eq!(old.as_deref(), Some("Sara_Chen"));
        assert!(idx.resolve("people", "Sara_Chen").unwrap().is_none());
        assert_eq!(
            idx.resolve("people", "Sara_Chen-Lopez")
                .unwrap()
                .unwrap()
                .as_str(),
            "zA"
        );
        // Renaming to the same display is a no-op.
        assert!(
            idx.rename("people", &EntityId::new("zA"), "Sara Chen-Lopez", &[])
                .unwrap()
                .is_none()
        );
        // Renaming an unknown entity assigns and reports no old name.
        assert!(
            idx.rename("people", &EntityId::new("zZ"), "New Person", &[])
                .unwrap()
                .is_none()
        );
        assert_eq!(
            idx.basename_for("people", &EntityId::new("zZ"))
                .unwrap()
                .unwrap(),
            "New_Person"
        );
    }

    #[test]
    fn slug_and_opaque_ids_both_work_as_keys_and_families_are_separate() {
        let idx = InMemoryPathIndex::new();
        let slug = EntityId::new("Sara_Chen");
        let opaque = EntityId::mint();
        assert_eq!(
            idx.assign("contacts", &slug, "Sara Chen", &[]).unwrap(),
            "Sara_Chen"
        );
        assert_eq!(
            idx.assign("people", &opaque, "Sara Chen", &[]).unwrap(),
            "Sara_Chen"
        );
        assert_eq!(idx.resolve("contacts", "Sara_Chen").unwrap().unwrap(), slug);
        assert_eq!(idx.resolve("people", "Sara_Chen").unwrap().unwrap(), opaque);
        PathIndex::remove(&idx, "people", &opaque).unwrap();
        assert!(idx.resolve("people", "Sara_Chen").unwrap().is_none());
    }

    #[test]
    fn assignment_is_deterministic_for_a_given_insertion_order() {
        let run = || {
            let idx = InMemoryPathIndex::new();
            let mut out = Vec::new();
            for (id, org) in [("z1", "Acme"), ("z2", "City Council"), ("z3", "Acme")] {
                out.push(
                    idx.assign("people", &EntityId::new(id), "Sara Chen", &q(&[org]))
                        .unwrap(),
                );
            }
            out
        };
        assert_eq!(run(), run());
        assert_eq!(
            run(),
            vec!["Sara_Chen", "Sara_Chen_(City_Council)", "Sara_Chen_(Acme)"]
        );
    }

    #[tokio::test]
    async fn in_memory_working_set_also_serves_as_a_path_index() {
        let ws = InMemoryWorkingSet::new();
        assert_eq!(
            ws.assign("notes", &EntityId::new("zN"), "Tuesday standup", &[])
                .unwrap(),
            "Tuesday_standup"
        );
        assert_eq!(
            ws.resolve("notes", "Tuesday_standup")
                .unwrap()
                .unwrap()
                .as_str(),
            "zN"
        );
        // The working-set role is untouched.
        ws.upsert(
            "notes/by-name/T/Tuesday_standup.md".into(),
            h(b"x"),
            ts("2026-05-26T10:00:00Z"),
        )
        .await
        .unwrap();
        assert!(ws.get("notes/by-name/T/Tuesday_standup.md").await.is_some());
    }
}
