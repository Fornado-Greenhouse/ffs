//! `ffs-fastpath` — filesystem watcher + diff classifier + supersession-
//! or-route-to-ingest, per ADR-014 and ARCHITECTURE.md § Concurrency model.
//!
//! Detects projection-file edits from any editor (Obsidian, Notepad, vim,
//! VS Code...), classifies the diff against the active predicate's
//! reverse-map rules, and either:
//!
//! - **Fast path** — authors a supersession atom into the store and
//!   re-renders the projection on disk. Latency budget: ~200ms.
//! - **Slow path** — writes the on-disk content to the ingest folder as a
//!   correction notebook entry, surfacing in the daily-health-summary for
//!   review.
//!
//! Daemon-induced re-render writes are suppressed via content-hash
//! comparison so the watcher does not loop on its own output.

pub mod classifier;
pub mod dispatch;
pub mod suppress;
pub mod watcher;

pub use classifier::{Classification, SlowPathReason, classify, is_federated_path};
pub use dispatch::{
    AppliedReceipt, DispatchError, Receipt, RoutedReceipt, apply_fast_path, dispatch,
    route_to_ingest,
};
pub use suppress::SuppressionRegistry;
pub use watcher::{DEFAULT_DEBOUNCE, FastPathContext, FastPathWatcher, PollingFastPathWatcher};

/// Workspace marker exposed so smoke tests can confirm the crate links.
pub const CRATE_NAME: &str = "ffs-fastpath";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(CRATE_NAME, "ffs-fastpath");
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
