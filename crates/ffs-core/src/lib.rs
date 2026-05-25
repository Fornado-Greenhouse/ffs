//! Foundational types and logic shared across every FFS binary.
//!
//! The MVP scope of this crate (after task 02): atom envelope, signing,
//! content addressing, multibase, multihash, and typed errors. Subsequent
//! tasks introduce predicate spec loading (03), the SQLite store (04),
//! capability evaluation (05), and projection rendering (06).

pub mod atom;
pub mod capability;
pub mod error;
pub mod multibase;
pub mod multihash;
pub mod predicate;
pub mod store;

pub use atom::{
    AtomEnvelope, AtomTemplate, EntityId, Iso8601, PredicateName, Provenance, PublicKey, Signature,
    SourceKind, Tier,
};
pub use capability::{
    Action, CAPABILITY_PREDICATE, CapabilityClaim, CapabilityError, CapabilityScope, Decision,
    DenyReason, EvalError, Target, build_capability_atom, evaluate, validate_supersession_narrows,
};
pub use error::{BadTimestampError, SignError, VerifyError};
pub use multibase::MultibaseError;
pub use multihash::{Multihash, MultihashError};
pub use store::{AtomStore, MemAtomStore, SCHEMA_VERSION, SqliteAtomStore, StoreError};

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
