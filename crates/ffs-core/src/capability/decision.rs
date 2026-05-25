//! Decision types returned by [`super::evaluate`].
//!
//! `Allow` carries the matching capability's content hash so callers (the
//! daemon, MCP server, federation transport) can record *which* capability
//! authorized the action in their audit log. `Deny` carries a typed
//! reason so a structured error can be returned to the caller.

use thiserror::Error;

use crate::multihash::Multihash;
use crate::store::StoreError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow { capability: Multihash },
    Deny { reason: DenyReason },
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum DenyReason {
    /// The agent has no capability atom at all (not even for unrelated actions).
    #[error("no capability found for agent")]
    NoCapabilityFound,
    /// At least one capability for the agent exists, but none covers the
    /// requested action and target.
    #[error("no matching capability for the requested action and target")]
    NotInScope,
    /// A matching capability exists but its `valid_to` is in the past at `as_of`.
    #[error("capability has expired (valid_to in past)")]
    Expired,
    /// A matching capability exists but its `valid_from` is in the future at `as_of`.
    #[error("capability is not yet valid (valid_from in future)")]
    NotYetValid,
}

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("capability claim malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum CapabilityError {
    #[error("supersession would change grantee")]
    SupersessionGranteeMismatch,
    #[error("supersession would broaden the action set")]
    SupersessionBroadensActions,
    #[error("supersession would broaden the scope")]
    SupersessionBroadensScope,
}
