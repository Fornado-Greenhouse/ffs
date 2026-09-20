//! Foundational types and logic shared across every FFS binary.
//!
//! The MVP scope of this crate (after task 02): atom envelope, signing,
//! content addressing, multibase, multihash, and typed errors. Subsequent
//! tasks introduce predicate spec loading (03), the SQLite store (04),
//! capability evaluation (05), and projection rendering (06).

pub mod atom;
pub mod capability;
pub mod error;
pub mod federation_peers;
pub mod multibase;
pub mod multihash;
pub mod predicate;
pub mod projection;
pub mod quarantine;
pub mod quarantine_sqlite;
pub mod resolution;
pub mod resolve;
pub mod store;
pub mod suppress;
pub mod urlnorm;
pub mod working_set;

pub use atom::{
    AtomEnvelope, AtomTemplate, EntityId, Iso8601, PredicateName, Provenance, PublicKey, Signature,
    SourceKind, Tier,
};
pub use capability::{
    Action, CAPABILITY_PREDICATE, CapabilityClaim, CapabilityError, CapabilityScope, Decision,
    DenyReason, EvalError, Target, build_capability_atom, evaluate, validate_supersession_narrows,
};
pub use error::{BadTimestampError, SignError, VerifyError};
pub use federation_peers::{
    FederationPeer, FederationPeerError, FederationPeerStore, InMemoryFederationPeerStore,
};
pub use multibase::MultibaseError;
pub use multihash::{Multihash, MultihashError};
pub use quarantine::{
    Candidate, CrossRef, InMemoryQuarantine, IngestQuarantine, Proposal, QuarantineError,
    Resolution, Submission, SubmissionStatus,
};
pub use quarantine_sqlite::SqliteQuarantine;
pub use resolution::{
    BlockingConfig, BlockingKey, FieldWeight, ResolutionConfig, ResolutionConfigError, Thresholds,
};
pub use resolve::{normalized_name_key, surname_key};
pub use store::{
    AtomStore, DIFFERENT_FROM_PREDICATE, MemAtomStore, SAME_AS_PREDICATE, SCHEMA_VERSION, Sighting,
    SqliteAtomStore, StoreError,
};
pub use suppress::SuppressionRegistry;
pub use urlnorm::{article_basename, normalize_url, same_content_hash};
pub use working_set::{
    InMemoryPathIndex, InMemoryWorkingSet, PathIndex, WorkingSetEntry, WorkingSetError,
    WorkingSetStore, slugify_display,
};

/// Workspace marker exposed so smoke tests can confirm the crate links.
pub const CRATE_NAME: &str = "ffs-core";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_set() {
        assert_eq!(CRATE_NAME, "ffs-core");
    }
}
