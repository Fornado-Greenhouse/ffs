//! Suppression registry for daemon-induced file writes.
//!
//! When the fast-path applies an edit it re-renders the projection and
//! writes the canonical bytes back to disk. The FS watcher will see that
//! write and would otherwise re-classify our own output. Per
//! ARCHITECTURE.md concurrency rule #3, we record the expected
//! post-write hash and ignore matching events.
//!
//! Implementation: a `Mutex<HashMap<PathBuf, Multihash>>` storing the
//! expected hash per file. The watcher consults this map; on a match it
//! removes the entry and ignores the event.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ffs_core::Multihash;

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
}
