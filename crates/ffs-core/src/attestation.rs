//! Attestations and the derived status of a fact (ADR-034, task_46).
//!
//! An attestation is an atom about another atom: its entity is the
//! attested atom's content hash, its claim says who believed the fact
//! held as of when, on what basis, from what source. Nothing here is
//! stored: [`status_of`] is a pure function of the head atom, its
//! attestations, the predicate's `[attestation]` policy, the clock,
//! and whether a later correction deprecated it. Wikipedia's
//! `{{As of}}` and Wikidata's ranks, made concrete.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::atom::{AtomEnvelope, Iso8601, PublicKey, SourceKind};
use crate::multihash::Multihash;
use crate::predicate::AttestationSpec;
use crate::urlnorm::normalize_url;

/// The predicate name attestations are filed under.
pub const ATTESTATION_PREDICATE: &str = "attestation";

/// Why an attester believes the fact held (ADR-034 § Decision (1)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    ReReadSameSource,
    IndependentSource,
    PrimarySource,
    OwnerKnowledge,
    /// The dispute marker: `source` is the contradicting atom's hash.
    ContradictedBy,
}

impl Basis {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "re_read_same_source" => Some(Self::ReReadSameSource),
            "independent_source" => Some(Self::IndependentSource),
            "primary_source" => Some(Self::PrimarySource),
            "owner_knowledge" => Some(Self::OwnerKnowledge),
            "contradicted_by" => Some(Self::ContradictedBy),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReReadSameSource => "re_read_same_source",
            Self::IndependentSource => "independent_source",
            Self::PrimarySource => "primary_source",
            Self::OwnerKnowledge => "owner_knowledge",
            Self::ContradictedBy => "contradicted_by",
        }
    }

    /// Short human label for the "as of" line.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ReReadSameSource => "re-read",
            Self::IndependentSource => "independent source",
            Self::PrimarySource => "primary source",
            Self::OwnerKnowledge => "own knowledge",
            Self::ContradictedBy => "contradicted",
        }
    }
}

/// One attestation, lifted from an `attestation` atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// The date the attester believes the fact held (`YYYY-MM-DD` or a
    /// full timestamp; compared lexically after normalization).
    pub as_of: String,
    pub basis: Basis,
    pub source: String,
    pub note: Option<String>,
    pub attester: PublicKey,
    pub tx_time: Iso8601,
    /// The attestation atom's own hash, when known.
    pub hash: Option<Multihash>,
}

impl Attestation {
    /// Lift an attestation from its atom. `None` when the atom is not an
    /// attestation or its claim is malformed (an unknown basis, no
    /// `as_of`); such atoms are ignored rather than counted.
    pub fn from_atom(env: &AtomEnvelope) -> Option<Self> {
        if env.predicate.as_str() != ATTESTATION_PREDICATE {
            return None;
        }
        let basis = Basis::parse(env.claim.get("basis")?.as_str()?)?;
        let as_of = env.claim.get("as_of")?.as_str()?.to_string();
        let source = env
            .claim
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let note = env
            .claim
            .get("note")
            .and_then(|v| v.as_str())
            .map(String::from);
        Some(Self {
            as_of,
            basis,
            source,
            note,
            attester: env.author.clone(),
            tx_time: env.tx_time.clone(),
            hash: env.content_hash().ok(),
        })
    }

    /// The date part of `as_of`, for display and window comparison.
    pub fn as_of_date(&self) -> &str {
        date_part(&self.as_of)
    }
}

/// The effective `[attestation]` policy for a predicate: the spec's
/// table (or the defaults for a missing table) with any per-substrate
/// override applied. Overrides may raise `k`, never lower it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationPolicy {
    pub k: u32,
    pub window_days: Option<u32>,
    pub independent: bool,
}

impl Default for AttestationPolicy {
    fn default() -> Self {
        Self {
            k: 1,
            window_days: None,
            independent: true,
        }
    }
}

impl AttestationPolicy {
    pub fn from_spec(spec: Option<&AttestationSpec>) -> Self {
        match spec {
            Some(s) => Self {
                k: s.k.max(1),
                window_days: s.window_days,
                independent: s.independent,
            },
            None => Self::default(),
        }
    }

    /// Apply an override: `k` is raised to `k_override` when that is
    /// larger; a smaller override is ignored (ADR-034: never lower).
    pub fn raised_to(self, k_override: Option<u32>) -> Self {
        match k_override {
            Some(k) if k > self.k => Self { k, ..self },
            _ => self,
        }
    }
}

/// The derived status of a fact (ADR-034 § Decision (3)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Current,
    Unconfirmed,
    Stale,
    Disputed,
    Deprecated,
    /// The fact's `valid_to` has passed: it was true and ended. Not in
    /// ADR-034's table, added by task_46 so an ended affiliation is
    /// history rather than a re-confirmation chore forever.
    Ended,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Unconfirmed => "unconfirmed",
            Self::Stale => "stale",
            Self::Disputed => "disputed",
            Self::Deprecated => "deprecated",
            Self::Ended => "ended",
        }
    }
}

/// Why a superseding atom replaced its parent (ADR-034 § Decision (4)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionReason {
    /// Wikidata's normal rank with an end time: the old atom is history.
    WorldChanged,
    /// Wikidata's deprecated rank: the old atom was never true.
    NeverTrue,
}

impl CorrectionReason {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "world_changed" => Some(Self::WorldChanged),
            "never_true" => Some(Self::NeverTrue),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorldChanged => "world_changed",
            Self::NeverTrue => "never_true",
        }
    }

    /// The provenance `uri` a correction entry carries.
    pub fn provenance_uri(&self) -> String {
        format!("correction:{}", self.as_str())
    }
}

/// The correction reason a superseding atom carries, if any: a
/// provenance entry of kind `correction` whose uri is
/// `correction:<reason>`.
pub fn correction_reason_of(superseding: &AtomEnvelope) -> Option<CorrectionReason> {
    superseding.provenance.iter().find_map(|p| {
        if p.kind != SourceKind::Correction {
            return None;
        }
        p.uri
            .strip_prefix("correction:")
            .and_then(CorrectionReason::parse)
    })
}

/// Everything a renderer or report needs about one fact's standing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusReport {
    pub status: Status,
    /// The newest qualifying attestation's `as_of` date.
    pub as_of: Option<String>,
    /// `unconfirmed`/`stale`: the date the marker counts from (the
    /// atom's `valid_from` date, or the last attestation's date).
    pub since: Option<String>,
    /// The attesters behind `as_of`, newest first: who and on what basis.
    pub confirmed_by: Vec<Confirmation>,
    /// How many distinct independent confirmations the policy counted.
    pub independent_count: u32,
    pub k: u32,
    pub window_days: Option<u32>,
    /// True when the predicate has a window (the status is worth
    /// rendering on predicates without one only when asked).
    pub windowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Confirmation {
    pub attester: PublicKey,
    pub basis: Basis,
    pub source: String,
    pub as_of: String,
}

/// The date part of an ISO 8601 string (`2026-09-20T07:00:00Z` →
/// `2026-09-20`; a bare date passes through).
pub fn date_part(s: &str) -> &str {
    s.split('T').next().unwrap_or(s).trim()
}

/// A source's identity for independence counting: urls are normalized
/// (scheme and host lowercased, tracking parameters stripped), other
/// strings are trimmed and lowercased.
pub fn source_key(source: &str) -> String {
    let s = source.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        normalize_url(s).to_lowercase()
    } else {
        s.to_lowercase()
    }
}

/// Days between two `YYYY-MM-DD` dates (`later - earlier`), or `None`
/// when either does not parse. Timestamps are reduced to their date.
pub fn days_between(earlier: &str, later: &str) -> Option<i64> {
    let e = parse_date(date_part(earlier))?;
    let l = parse_date(date_part(later))?;
    Some((l - e).whole_days())
}

fn parse_date(s: &str) -> Option<time::Date> {
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(s, &fmt).ok()
}

/// Derive the status of `head` from its attestations.
///
/// - `deprecated` when a later correction says `never_true`;
/// - `disputed` when a `contradicted_by` attestation is newer than every
///   confirming attestation;
/// - `ended` when `valid_to` is before `now`;
/// - `unconfirmed` with fewer than `k` independent confirmations;
/// - `stale` when the newest confirmation is older than the window;
/// - `current` otherwise.
///
/// With `independent = true`, confirmations sharing a `(basis, source)`
/// pair (sources normalized) count once; otherwise every attestation
/// counts. Attestations dated after `now` are ignored.
pub fn status_of(
    head: &AtomEnvelope,
    attestations: &[Attestation],
    policy: &AttestationPolicy,
    now: &Iso8601,
    correction: Option<CorrectionReason>,
) -> StatusReport {
    let today = date_part(now.as_str());
    let k = policy.k.max(1);
    let windowed = policy.window_days.is_some();

    let mut confirming: Vec<&Attestation> = attestations
        .iter()
        .filter(|a| a.basis != Basis::ContradictedBy)
        .filter(|a| a.as_of_date() <= today)
        .collect();
    confirming.sort_by(|a, b| {
        b.as_of_date()
            .cmp(a.as_of_date())
            .then(b.tx_time.as_str().cmp(a.tx_time.as_str()))
    });
    let newest_dispute = attestations
        .iter()
        .filter(|a| a.basis == Basis::ContradictedBy)
        .map(|a| a.tx_time.as_str())
        .max();
    let newest_confirmation_tx = confirming.iter().map(|a| a.tx_time.as_str()).max();

    let independent_count: u32 = if policy.independent {
        let keys: BTreeSet<(Basis, String)> = confirming
            .iter()
            .map(|a| (a.basis, source_key(&a.source)))
            .collect();
        keys.len() as u32
    } else {
        confirming.len() as u32
    };

    let newest = confirming.first();
    let as_of = newest.map(|a| a.as_of_date().to_string());
    let confirmed_by: Vec<Confirmation> = confirming
        .iter()
        .map(|a| Confirmation {
            attester: a.attester.clone(),
            basis: a.basis,
            source: a.source.clone(),
            as_of: a.as_of_date().to_string(),
        })
        .collect();
    let valid_from_date = date_part(head.valid_from.as_str()).to_string();

    let status = if correction == Some(CorrectionReason::NeverTrue) {
        Status::Deprecated
    } else if newest_dispute.is_some_and(|d| newest_confirmation_tx.is_none_or(|c| d > c)) {
        Status::Disputed
    } else if head
        .valid_to
        .as_ref()
        .is_some_and(|v| date_part(v.as_str()) < today)
    {
        Status::Ended
    } else if independent_count < k {
        Status::Unconfirmed
    } else if let (Some(window), Some(n)) = (policy.window_days, newest)
        && days_between(n.as_of_date(), today).is_some_and(|d| d > i64::from(window))
    {
        Status::Stale
    } else {
        Status::Current
    };

    let since = match status {
        Status::Unconfirmed => Some(valid_from_date),
        Status::Stale => as_of.clone(),
        _ => None,
    };

    StatusReport {
        status,
        as_of,
        since,
        confirmed_by,
        independent_count,
        k,
        window_days: policy.window_days,
        windowed,
    }
}

/// Per-substrate `k` overrides from `$FFS_DATA_DIR/config/attestation.toml`:
///
/// ```toml
/// [affiliation]
/// k = 2
/// ```
///
/// Only `k` is read; a value below the spec's `k` is ignored at policy
/// time (ADR-034: overrides raise, never lower). A missing file is an
/// empty map.
pub fn load_overrides(path: &Path) -> Result<Vec<(String, u32)>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_overrides(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse_overrides(text: &str) -> Result<Vec<(String, u32)>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    collect_overrides("", &table, &mut out)?;
    out.sort();
    Ok(out)
}

/// Walk nested tables so both `["org.company"]` (a quoted key) and
/// `[org.company]` (TOML's dotted form, which nests `company` under
/// `org`) name the predicate `org.company`. A table with a `k` is a
/// predicate; a table without one is a path segment.
fn collect_overrides(
    prefix: &str,
    table: &toml::Table,
    out: &mut Vec<(String, u32)>,
) -> Result<(), String> {
    for (key, value) in table {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let Some(t) = value.as_table() else {
            return Err(format!("[{name}] must be a table with `k`"));
        };
        if let Some(k) = t.get("k") {
            let k = k
                .as_integer()
                .filter(|k| *k >= 1)
                .ok_or_else(|| format!("[{name}] k must be a positive integer"))?;
            out.push((name, k as u32));
        } else {
            collect_overrides(&name, t, out)?;
        }
    }
    Ok(())
}

// ---- store-backed helpers shared by the renderer and the daemon ----

use crate::atom::{EntityId, PredicateName};
use crate::predicate::PredicateSpec;
use crate::store::{AtomStore, StoreError};

/// The effective policy for a predicate: its spec's `[attestation]`
/// table (defaults when absent) raised by any per-substrate override.
pub fn policy_for(spec: Option<&PredicateSpec>, overrides: &[(String, u32)]) -> AttestationPolicy {
    let base = AttestationPolicy::from_spec(spec.and_then(|s| s.attestation.as_ref()));
    let k = spec.and_then(|s| {
        overrides
            .iter()
            .find(|(name, _)| name == &s.name)
            .map(|(_, k)| *k)
    });
    base.raised_to(k)
}

/// Every attestation filed against `subject` (an atom's content hash).
pub fn attestations_of(
    store: &dyn AtomStore,
    subject: &Multihash,
) -> Result<Vec<Attestation>, StoreError> {
    let entity = EntityId::new(subject.to_multibase());
    let atoms = store.list_by_entity(
        &entity,
        Some(&PredicateName::new(ATTESTATION_PREDICATE)),
        None,
    )?;
    Ok(atoms.iter().filter_map(Attestation::from_atom).collect())
}

/// The correction a superseding atom applied to `atom`, if any: the
/// child of `atom` in its chain that carries a `correction` provenance.
pub fn correction_for(
    store: &dyn AtomStore,
    atom: &AtomEnvelope,
) -> Result<Option<CorrectionReason>, StoreError> {
    let hash = atom
        .content_hash()
        .map_err(|e| StoreError::Serialization(e.to_string()))?;
    let chain = store.list_by_entity(&atom.entity, Some(&atom.predicate), None)?;
    Ok(chain
        .iter()
        .filter(|c| c.supersedes.as_ref() == Some(&hash))
        .find_map(correction_reason_of))
}

/// The status report for a head atom, plus the attestations it was
/// derived from (their hashes belong in a render's `source_atoms`).
pub fn report_for_head(
    store: &dyn AtomStore,
    head: &AtomEnvelope,
    policy: &AttestationPolicy,
    now: &Iso8601,
) -> Result<(StatusReport, Vec<Attestation>), StoreError> {
    let hash = head
        .content_hash()
        .map_err(|e| StoreError::Serialization(e.to_string()))?;
    let atts = attestations_of(store, &hash)?;
    let correction = correction_for(store, head)?;
    let report = status_of(head, &atts, policy, now, correction);
    Ok((report, atts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_helpers() {
        assert_eq!(date_part("2026-09-20T07:00:00Z"), "2026-09-20");
        assert_eq!(date_part("2026-09-20"), "2026-09-20");
        assert_eq!(days_between("2026-06-21", "2026-09-20"), Some(91));
        assert_eq!(days_between("2026-09-20", "2026-06-21"), Some(-91));
        assert_eq!(days_between("nope", "2026-06-21"), None);
    }

    #[test]
    fn source_keys_collapse_tracking_and_case() {
        assert_eq!(
            source_key("https://Example.com/a?utm_source=x"),
            source_key("https://example.com/a")
        );
        assert_ne!(
            source_key("https://example.com/a"),
            source_key("https://example.com/b")
        );
        assert_eq!(source_key(" Person:Owner "), "person:owner");
    }

    #[test]
    fn overrides_parse_and_reject_garbage() {
        let v = parse_overrides("[affiliation]\nk = 2\n[org.company]\nk = 3\n").unwrap();
        assert_eq!(
            v,
            vec![("affiliation".into(), 2), ("org.company".into(), 3)]
        );
        assert!(parse_overrides("[affiliation]\nk = 0\n").is_err());
        assert!(parse_overrides("affiliation = 2\n").is_err());
        assert!(parse_overrides("").unwrap().is_empty());
    }

    #[test]
    fn policy_override_raises_but_never_lowers() {
        let p = AttestationPolicy {
            k: 2,
            window_days: Some(90),
            independent: true,
        };
        assert_eq!(p.raised_to(Some(3)).k, 3);
        assert_eq!(p.raised_to(Some(1)).k, 2);
        assert_eq!(p.raised_to(None).k, 2);
    }
}
