//! Server-to-client notification publisher.
//!
//! The types live in `ffs_core::events` since task_49 (ADR-036): the
//! fast-path crate publishes `Event::ProjectionInvalidated` and
//! `Event::FastPathApplied` and the daemon depends on the fast path to
//! start its watcher, so the shared types sit below both. This module
//! re-exports them so existing daemon paths read unchanged.

pub use ffs_core::events::{CHANNEL_CAPACITY, Event, EventPublisher};
