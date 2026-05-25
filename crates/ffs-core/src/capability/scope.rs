//! Capability scope: what predicates, entities, classifications, and tiers
//! a capability covers.
//!
//! Each dimension is `Option<Vec<T>>` (or `Option<T>` for the single-valued
//! `tier`). `None` means "any" (no restriction); `Some(list)` restricts
//! coverage to elements of the list.
//!
//! Two operations are defined on scopes:
//!
//! - [`CapabilityScope::covers`] — true iff the scope authorizes the target.
//! - [`CapabilityScope::narrows_or_equals`] — true iff this scope is a
//!   subset of the other (used to validate that capability supersessions
//!   do not broaden access).

use serde::{Deserialize, Serialize};

use crate::atom::{EntityId, PredicateName, Tier};

use super::Target;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicates: Option<Vec<PredicateName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<EntityId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifications: Option<Vec<Tier>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
}

impl CapabilityScope {
    /// True if this scope authorizes the target. A `None` dimension on the
    /// scope means "any" and matches unconditionally. A `Some` dimension
    /// requires the target's corresponding value to be in the list (for
    /// list dimensions) or equal (for the single `tier` dimension). If the
    /// target's dimension is `None` but the scope restricts it, that's a
    /// mismatch (caller didn't specify enough to be authorized).
    pub fn covers(&self, target: &Target) -> bool {
        let pred_ok = match &self.predicates {
            None => true,
            Some(list) => list.iter().any(|p| p == &target.predicate),
        };
        let ent_ok = match &self.entities {
            None => true,
            Some(list) => list.iter().any(|e| e == &target.entity),
        };
        let class_ok = match (&self.classifications, &target.classification) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(allowed), Some(t)) => allowed.iter().any(|c| c == t),
        };
        let tier_ok = match (&self.tier, &target.tier) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(allowed), Some(t)) => allowed == t,
        };
        pred_ok && ent_ok && class_ok && tier_ok
    }

    /// True if this scope is a subset of `other` (or equal).
    ///
    /// Narrowing rules per dimension:
    /// - `other = None` (any): `self` may be anything (None or Some).
    /// - `other = Some(list)`: `self = Some(sublist)` is fine; `self = None`
    ///   would broaden (rejected). For the single-valued `tier`, `Some(t)`
    ///   narrows iff `other` is `Some(t)`.
    pub fn narrows_or_equals(&self, other: &Self) -> bool {
        list_narrows(&self.predicates, &other.predicates)
            && list_narrows(&self.entities, &other.entities)
            && list_narrows(&self.classifications, &other.classifications)
            && tier_narrows(&self.tier, &other.tier)
    }
}

fn list_narrows<T: PartialEq>(new: &Option<Vec<T>>, old: &Option<Vec<T>>) -> bool {
    match (new, old) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(n), Some(o)) => n.iter().all(|x| o.iter().any(|y| y == x)),
    }
}

fn tier_narrows<T: PartialEq>(new: &Option<T>, old: &Option<T>) -> bool {
    match (new, old) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(n), Some(o)) => n == o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(s: &str) -> EntityId {
        EntityId::new(s)
    }
    fn pred(s: &str) -> PredicateName {
        PredicateName::new(s)
    }
    fn tier(s: &str) -> Tier {
        Tier::new(s)
    }

    fn target(p: &str, e: &str, c: Option<&str>, t: Option<&str>) -> Target {
        Target {
            predicate: pred(p),
            entity: ent(e),
            classification: c.map(tier),
            tier: t.map(tier),
        }
    }

    #[test]
    fn unrestricted_scope_covers_everything() {
        let s = CapabilityScope::default();
        assert!(s.covers(&target("contact.person", "alice", None, None)));
        assert!(s.covers(&target(
            "note",
            "bob",
            Some("existence"),
            Some("introducible")
        )));
    }

    #[test]
    fn predicate_list_restricts() {
        let s = CapabilityScope {
            predicates: Some(vec![pred("contact.person")]),
            ..Default::default()
        };
        assert!(s.covers(&target("contact.person", "alice", None, None)));
        assert!(!s.covers(&target("note", "alice", None, None)));
    }

    #[test]
    fn classification_restricts() {
        let s = CapabilityScope {
            classifications: Some(vec![tier("existence")]),
            ..Default::default()
        };
        assert!(s.covers(&target("x", "y", Some("existence"), None)));
        assert!(!s.covers(&target("x", "y", Some("personal_email"), None)));
        // Unspecified target classification while scope restricts → mismatch.
        assert!(!s.covers(&target("x", "y", None, None)));
    }

    #[test]
    fn unspecified_scope_narrows_anything() {
        let any = CapabilityScope::default();
        let narrow = CapabilityScope {
            predicates: Some(vec![pred("p1")]),
            ..Default::default()
        };
        assert!(narrow.narrows_or_equals(&any));
        // Any does NOT narrow narrow (any is broader).
        assert!(!any.narrows_or_equals(&narrow));
    }

    #[test]
    fn list_subset_narrows() {
        let parent = CapabilityScope {
            predicates: Some(vec![pred("p1"), pred("p2")]),
            ..Default::default()
        };
        let child = CapabilityScope {
            predicates: Some(vec![pred("p1")]),
            ..Default::default()
        };
        assert!(child.narrows_or_equals(&parent));
        assert!(!parent.narrows_or_equals(&child));
    }

    #[test]
    fn tier_must_match_or_be_unrestricted_on_parent() {
        let parent = CapabilityScope {
            tier: Some(tier("introducible")),
            ..Default::default()
        };
        let same_tier = CapabilityScope {
            tier: Some(tier("introducible")),
            ..Default::default()
        };
        let other_tier = CapabilityScope {
            tier: Some(tier("discreet")),
            ..Default::default()
        };
        assert!(same_tier.narrows_or_equals(&parent));
        assert!(!other_tier.narrows_or_equals(&parent));
    }
}
