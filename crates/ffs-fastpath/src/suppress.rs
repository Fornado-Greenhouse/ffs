//! Suppression registry — re-export from `ffs-core`.
//!
//! The implementation moved to `ffs-core::suppress` so the
//! working-set materializer (in `ffs-daemon`) and the fast-path
//! watcher (here) can share one instance without a dependency
//! cycle. This module re-exports the type for back-compat with
//! existing call sites in this crate.

pub use ffs_core::SuppressionRegistry;
