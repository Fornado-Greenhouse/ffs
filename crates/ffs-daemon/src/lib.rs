//! `ffs-daemon` — the long-running per-user FFS process.
//!
//! Owns the substrate's state (atom store, predicate registry, projection
//! renderer) and exposes it to local clients over a Unix domain socket
//! (Linux/macOS) or Windows named pipe with JSON-RPC 2.0. Server-to-client
//! notifications fan out via a broadcast publisher.
//!
//! This crate also exposes its library so integration tests and helper
//! binaries (e.g., `ffs-cli` once it lands in task 08) can reuse the API
//! and dispatcher types directly.

pub mod api;
pub mod dispatch;
pub mod notify;
pub mod transport;

pub use dispatch::Dispatcher;
pub use notify::{Event, EventPublisher};
