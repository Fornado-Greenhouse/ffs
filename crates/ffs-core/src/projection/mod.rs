//! Projection rendering: atoms → markdown for any editor to open.
//!
//! A projection is a virtual filesystem path materialized on demand from
//! the atom store. The user types `cat ~/.ffs/contacts/by-name/S/Sarah_Chen.md`
//! and the daemon (eventually — task 07) routes the read through this
//! renderer.
//!
//! Pipeline per ARCHITECTURE.md:
//!
//! 1. Parse the path into a `(family, sub-path)` shape.
//! 2. Resolve the family's primary predicate via the predicate-spec registry.
//! 3. Fetch atoms (head-of-chain at `as_of`) from the store.
//! 4. Capability-check each atom for the requesting agent.
//! 5. Render via the spec's Tera template (per-entity) or a built-in
//!    listing format (recent / alphabetical / by-org).
//! 6. Emit reverse-map annotations alongside — the fast-path classifier
//!    (task 09) consumes them to translate user edits back to atom fields.
//! 7. Hash the rendered markdown so callers can detect unchanged
//!    re-renders.

use serde::{Deserialize, Serialize};

pub mod path;
pub mod render;

pub use path::{ParsedPath, PathError, PathFamily};
pub use render::ProjectionRenderer;

use crate::atom::{Iso8601, PublicKey};
use crate::capability::{DenyReason, EvalError};
use crate::multihash::Multihash;
use crate::predicate::EditKind;
use crate::store::StoreError;

/// A request to render a projection at the given path on behalf of `agent`.
#[derive(Clone, Debug)]
pub struct ProjectionRequest {
    pub path: String,
    pub as_of: Option<Iso8601>,
    pub agent: PublicKey,
}

/// The rendered projection plus everything callers need to:
///   - serve the markdown to the editor,
///   - detect when a re-render would be a no-op (`render_hash`),
///   - audit which atoms contributed to the view (`source_atoms`),
///   - translate user edits back to atom fields (`reverse_map`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionResponse {
    pub markdown: String,
    pub render_hash: Multihash,
    pub source_atoms: Vec<Multihash>,
    pub reverse_map: Vec<ReverseMapAnnotation>,
}

/// Mapping from a rendered output element (e.g., `frontmatter.display_name`)
/// to the atom + claim field that produced it. Consumed by the fast-path
/// classifier per ADR-014.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseMapAnnotation {
    pub output_element: String,
    pub source_atom: Multihash,
    pub source_field: String,
    pub edit_kind: EditKind,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("path parse error: {0}")]
    Path(#[from] PathError),
    #[error("unknown predicate in registry: {0}")]
    UnknownPredicate(String),
    #[error("no atom found for entity={entity} predicate={predicate}")]
    AtomNotFound { entity: String, predicate: String },
    #[error("capability denied: {0}")]
    CapabilityDenied(DenyReason),
    #[error("store error: {0}")]
    Store(StoreError),
    #[error("capability eval error: {0}")]
    Eval(EvalError),
    #[error("tera error: {0}")]
    Tera(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("unsupported sub-path for MVP: {0}")]
    UnsupportedSubpath(String),
}
