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
pub mod autofile;
pub mod dispatch;
pub mod inbox;
pub mod ingest_watcher;
pub mod materializer;
pub mod notify;
pub mod resolver;
pub mod scribe;
pub mod transport;

pub use api::{ApiError, ApiPayload, ApiRequest, ApiResponse, ERR_CAPABILITY_DENIED};
pub use autofile::{AutoFileReport, AutoFiler, agent_identity_key, grantee_for_source};
pub use dispatch::SkillInvoker;
pub use dispatch::{AtomSigner, Dispatcher};
pub use inbox::{
    AutoFiledRow, Housekeeping, InboxError, InboxMaterializer, MergeSuggestion, ParseWarning,
    inbox_relative_path, render_inbox, today_utc,
};
pub use ingest_watcher::{IngestWatcher, IngestWatcherConfig};
pub use materializer::{MaterializeError, Materialized, WorkingSetMaterializer};
pub use notify::{Event, EventPublisher};
pub use resolver::{CandidateLookup, ResolveReport, StoreLookup, SubmissionContext, resolve_set};
pub use scribe::{ResolvingExtractor, SkillsHostInvoker, SkillsHostScribeExtractor};
