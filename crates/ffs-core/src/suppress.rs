//! Suppression registry for daemon-induced file writes.
//!
//! When a daemon component writes content to the working set (the
//! fast-path applying a supersession, or the working-set
//! materializer rendering a projection after an atom commit), the
//! filesystem watcher will see that write and would otherwise
//! re-classify it as a user edit. Per ARCHITECTURE.md concurrency
//! rule #3, the writer records the expected post-write content hash
//! into this registry; the watcher consults the registry on each
//! event and, on a hash match, removes the entry and ignores the
//! event.
//!
//! Hash-keyed rather than time-keyed: a TTL-based suppression
//! window would mis-suppress a same-second user edit; matching on
//! content hash is exact and self-cleans on the watcher's read.
//!
//! Lives in `ffs-core` so the working-set materializer
//! (`ffs-daemon::materializer`) and the fast-path watcher
//! (`ffs-fastpath::watcher`) can share one instance without a
//! dependency cycle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::Multihash;

#[derive(Default)]
pub struct SuppressionRegistry {
    inner: Mutex<HashMap<PathBuf, Multihash>>,
}

impl SuppressionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `path` should soon have content with the given hash.
    pub fn record(&self, path: &Path, content: &[u8]) {
        let hash = Multihash::blake3_of(content);
        let mut g = self.inner.lock().unwrap();
        g.insert(path.to_path_buf(), hash);
    }

    /// Returns true if `content` matches a previously-recorded expectation
    /// for `path`. Consumes the registration on a hit so a second write of
    /// the same content is not suppressed.
    pub fn check(&self, path: &Path, content: &[u8]) -> bool {
        let hash = Multihash::blake3_of(content);
        let mut g = self.inner.lock().unwrap();
        match g.get(path) {
            Some(expected) if expected == &hash => {
                g.remove(path);
                true
            }
            _ => false,
        }
    }

    /// Test/diagnostic accessor: is there a recorded expectation for
    /// `path`? Does not consume the entry.
    pub fn has_pending(&self, path: &Path) -> bool {
        self.inner.lock().unwrap().contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_consumes_match() {
        let reg = SuppressionRegistry::new();
        let path = PathBuf::from("/tmp/x.md");
        reg.record(&path, b"hello");
        assert!(reg.check(&path, b"hello"));
        // Consumed.
        assert!(!reg.check(&path, b"hello"));
    }

    #[test]
    fn mismatch_does_not_consume() {
        let reg = SuppressionRegistry::new();
        let path = PathBuf::from("/tmp/x.md");
        reg.record(&path, b"hello");
        assert!(!reg.check(&path, b"goodbye"));
        // Still recorded.
        assert!(reg.check(&path, b"hello"));
    }

    #[test]
    fn no_record_no_match() {
        let reg = SuppressionRegistry::new();
        let path = PathBuf::from("/tmp/x.md");
        assert!(!reg.check(&path, b"hello"));
    }

    #[test]
    fn has_pending_reports_state_without_consuming() {
        let reg = SuppressionRegistry::new();
        let path = PathBuf::from("/tmp/x.md");
        reg.record(&path, b"hello");
        assert!(reg.has_pending(&path));
        assert!(reg.has_pending(&path)); // non-consuming
        reg.check(&path, b"hello");
        assert!(!reg.has_pending(&path));
    }
}
