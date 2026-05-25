//! Capability-as-data evaluation. Decides whether an agent may perform
//! an action against a target at a given time, by consulting capability
//! atoms in the store.
//!
//! Capabilities are themselves atoms (predicate = [`CAPABILITY_PREDICATE`],
//! entity = grantee's public key in multibase form). Their claim payload
//! is a [`CapabilityClaim`]. The evaluator pulls all capability atoms
//! granted to the agent at or before `as_of`, filters superseded ones,
//! and returns `Allow` on the first match.
//!
//! Per ARCHITECTURE.md, this module is the Policy Engine in the AARM
//! mapping. Every read, write, federation pull, and MCP tool call passes
//! through [`evaluate`].

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::atom::{
    AtomEnvelope, AtomTemplate, ENVELOPE_VERSION, EntityId, Iso8601, PredicateName, PublicKey, Tier,
};
use crate::error::SignError;
use crate::multihash::Multihash;
use crate::store::AtomStore;

pub mod decision;
pub mod scope;

pub use decision::{CapabilityError, Decision, DenyReason, EvalError};
pub use scope::CapabilityScope;

/// Predicate name reserved for capability-grant atoms.
pub const CAPABILITY_PREDICATE: &str = "capability.grant";

/// Classification tier used on capability atoms themselves (capabilities
/// are not user content; they live in their own classification bucket so
/// they're easy to distinguish in queries).
pub const CAPABILITY_CLASSIFICATION: &str = "capability";

/// Actions a capability may grant. See ADR-007 (PRD-level) for the
/// motivating federation use cases.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Read,
    Write,
    Supersede,
    Erase,
    Classify,
    Federate,
}

/// What the agent wants to act on. `entity` is required; `classification`
/// and `tier` are optional and only checked when a capability restricts
/// them (a scope with `classifications=Some(...)` will deny a target
/// whose `classification` is `None`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub predicate: PredicateName,
    pub entity: EntityId,
    pub classification: Option<Tier>,
    pub tier: Option<Tier>,
}

impl Target {
    pub fn new(predicate: PredicateName, entity: EntityId) -> Self {
        Self {
            predicate,
            entity,
            classification: None,
            tier: None,
        }
    }

    pub fn with_classification(mut self, c: Tier) -> Self {
        self.classification = Some(c);
        self
    }

    pub fn with_tier(mut self, t: Tier) -> Self {
        self.tier = Some(t);
        self
    }
}

/// The claim payload carried by a capability-grant atom.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityClaim {
    pub grantee: PublicKey,
    pub actions: Vec<Action>,
    pub scope: CapabilityScope,
}

impl CapabilityClaim {
    /// Extract a typed claim from a capability atom envelope.
    pub fn from_envelope(env: &AtomEnvelope) -> Result<Self, EvalError> {
        serde_json::from_value(env.claim.clone()).map_err(|e| EvalError::Malformed(e.to_string()))
    }
}

/// Build and sign a capability-grant atom. Caller specifies the granting
/// authority (`grantor`), the grantee, the actions and scope being granted,
/// the bitemporal window, and an optional `supersedes` link to revoke or
/// narrow a prior capability.
#[allow(clippy::too_many_arguments)]
pub fn build_capability_atom(
    grantor: &ed25519_dalek::SigningKey,
    grantee: PublicKey,
    actions: Vec<Action>,
    scope: CapabilityScope,
    valid_from: Iso8601,
    valid_to: Option<Iso8601>,
    tx_time: Iso8601,
    supersedes: Option<Multihash>,
) -> Result<AtomEnvelope, SignError> {
    let claim = CapabilityClaim {
        grantee: grantee.clone(),
        actions,
        scope,
    };
    let claim_value =
        serde_json::to_value(&claim).map_err(|e| SignError::Serialization(e.to_string()))?;
    let tmpl = AtomTemplate {
        v: ENVELOPE_VERSION,
        entity: EntityId::new(grantee.to_multibase()),
        predicate: PredicateName::new(CAPABILITY_PREDICATE),
        claim: claim_value,
        valid_from,
        valid_to,
        tx_time,
        classification: Tier::new(CAPABILITY_CLASSIFICATION),
        supersedes,
        provenance: vec![],
    };
    tmpl.sign(grantor)
}

/// Validate that a superseding capability does not broaden the original.
/// Callers SHOULD invoke this before inserting a supersession; the
/// substrate trusts that this has been done. The evaluator does not
/// re-check at evaluation time (per ARCHITECTURE.md the enforcement is
/// at write time).
pub fn validate_supersession_narrows(
    new: &CapabilityClaim,
    old: &CapabilityClaim,
) -> Result<(), CapabilityError> {
    if new.grantee != old.grantee {
        return Err(CapabilityError::SupersessionGranteeMismatch);
    }
    for a in &new.actions {
        if !old.actions.contains(a) {
            return Err(CapabilityError::SupersessionBroadensActions);
        }
    }
    if !new.scope.narrows_or_equals(&old.scope) {
        return Err(CapabilityError::SupersessionBroadensScope);
    }
    Ok(())
}

/// Decide whether `agent` may perform `action` against `target` at `as_of`.
///
/// Algorithm:
/// 1. Fetch all capability atoms granted to `agent` (entity = agent's
///    multibase) at or before `as_of`.
/// 2. Identify the set of superseded capability hashes (any atom whose
///    hash is referenced by another candidate's `supersedes` field).
/// 3. Walk active (non-superseded) capabilities. The first one that
///    matches grantee + action + bitemporal window + scope authorizes
///    the request; return `Allow` with its content hash.
/// 4. If none matches, return `Deny` with the most specific reason
///    encountered (priority: `Expired` > `NotYetValid` > `NotInScope` >
///    `NoCapabilityFound`).
pub fn evaluate(
    store: &dyn AtomStore,
    agent: &PublicKey,
    action: Action,
    target: &Target,
    as_of: &Iso8601,
) -> Result<Decision, EvalError> {
    let agent_entity = EntityId::new(agent.to_multibase());
    let pred = PredicateName::new(CAPABILITY_PREDICATE);
    let candidates = store.list_by_entity(&agent_entity, Some(&pred), Some(as_of))?;

    if candidates.is_empty() {
        return Ok(Decision::Deny {
            reason: DenyReason::NoCapabilityFound,
        });
    }

    let superseded: HashSet<Vec<u8>> = candidates
        .iter()
        .filter_map(|c| c.supersedes.as_ref().map(|m| m.as_bytes().to_vec()))
        .collect();

    let mut fallback: Option<DenyReason> = None;

    for cap_env in &candidates {
        let cap_hash = cap_env
            .content_hash()
            .map_err(|e| EvalError::Malformed(e.to_string()))?;
        if superseded.contains(cap_hash.as_bytes().as_slice()) {
            continue;
        }
        let claim = CapabilityClaim::from_envelope(cap_env)?;
        if claim.grantee != *agent {
            continue;
        }
        if !claim.actions.contains(&action) {
            continue;
        }
        if as_of.as_str() < cap_env.valid_from.as_str() {
            // Capability is in scope/action but not yet effective.
            if claim.scope.covers(target) {
                fallback.get_or_insert(DenyReason::NotYetValid);
            }
            continue;
        }
        if let Some(vt) = &cap_env.valid_to
            && as_of.as_str() > vt.as_str()
        {
            if claim.scope.covers(target) {
                fallback.get_or_insert(DenyReason::Expired);
            }
            continue;
        }
        if !claim.scope.covers(target) {
            fallback.get_or_insert(DenyReason::NotInScope);
            continue;
        }
        return Ok(Decision::Allow {
            capability: cap_hash,
        });
    }

    Ok(Decision::Deny {
        reason: fallback.unwrap_or(DenyReason::NotInScope),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_claim_serde_roundtrip() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let claim = CapabilityClaim {
            grantee: PublicKey::from_verifying(&key.verifying_key()),
            actions: vec![Action::Read, Action::Supersede],
            scope: CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                entities: None,
                classifications: Some(vec![Tier::new("existence")]),
                tier: Some(Tier::new("introducible")),
            },
        };
        let v = serde_json::to_value(&claim).unwrap();
        let back: CapabilityClaim = serde_json::from_value(v).unwrap();
        assert_eq!(claim, back);
    }

    #[test]
    fn supersession_narrowing_validation_rejects_broaden_actions() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let g = PublicKey::from_verifying(&key.verifying_key());
        let old = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read],
            scope: CapabilityScope::default(),
        };
        let new = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read, Action::Write],
            scope: CapabilityScope::default(),
        };
        assert_eq!(
            validate_supersession_narrows(&new, &old),
            Err(CapabilityError::SupersessionBroadensActions)
        );
    }

    #[test]
    fn supersession_narrowing_validation_rejects_broaden_scope() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let g = PublicKey::from_verifying(&key.verifying_key());
        let old = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read],
            scope: CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                ..Default::default()
            },
        };
        let new = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read],
            scope: CapabilityScope {
                predicates: None, // broadening
                ..Default::default()
            },
        };
        assert_eq!(
            validate_supersession_narrows(&new, &old),
            Err(CapabilityError::SupersessionBroadensScope)
        );
    }

    #[test]
    fn supersession_narrowing_validation_rejects_grantee_change() {
        let k1 = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let k2 = ed25519_dalek::SigningKey::from_bytes(&[6u8; 32]);
        let g1 = PublicKey::from_verifying(&k1.verifying_key());
        let g2 = PublicKey::from_verifying(&k2.verifying_key());
        let old = CapabilityClaim {
            grantee: g1,
            actions: vec![Action::Read],
            scope: CapabilityScope::default(),
        };
        let new = CapabilityClaim {
            grantee: g2,
            actions: vec![Action::Read],
            scope: CapabilityScope::default(),
        };
        assert_eq!(
            validate_supersession_narrows(&new, &old),
            Err(CapabilityError::SupersessionGranteeMismatch)
        );
    }

    #[test]
    fn supersession_narrowing_validation_accepts_same_or_narrower() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let g = PublicKey::from_verifying(&key.verifying_key());
        let old = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read, Action::Write],
            scope: CapabilityScope::default(),
        };
        let same = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read, Action::Write],
            scope: CapabilityScope::default(),
        };
        let narrower = CapabilityClaim {
            grantee: g.clone(),
            actions: vec![Action::Read],
            scope: CapabilityScope {
                predicates: Some(vec![PredicateName::new("contact.person")]),
                ..Default::default()
            },
        };
        assert!(validate_supersession_narrows(&same, &old).is_ok());
        assert!(validate_supersession_narrows(&narrower, &old).is_ok());
    }
}
