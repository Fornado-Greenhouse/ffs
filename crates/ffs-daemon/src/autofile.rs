//! Auto-filing under an `Accept` grant (ADR-029).
//!
//! Runs right after the scribe's proposals land in the quarantine
//! (`quarantine.complete`), from both extraction sites: the
//! `ingest.submit` RPC and the ingest-folder watcher. For each
//! proposal it asks three questions in order and files only when all
//! three say yes:
//!
//! 1. **Who is asking?** The grantee identity comes from the
//!    submission's `source_uri`, never from content: `mcp:agent/<id>`
//!    for MCP submissions, the daemon's configured
//!    `FFS_INGEST_AGENT_IDENTITY` for `file://` drops (the courier),
//!    nothing otherwise. No grantee, no auto-filing.
//! 2. **May they?** The capability evaluator, with `Action::Accept`
//!    against the proposal's target (predicate, resolved entity or
//!    `new`, classification `existence`). A grant carries a daily cap
//!    (`max_per_day`, ADR-029; 50 when the grant omits it, with a warn);
//!    usage is counted from the store, so it survives restarts.
//! 3. **Is it safe?** The quarantine classifier (`ffs_core::classify`):
//!    only additive proposals file (a new entity, an appended section,
//!    an append-only record); anything that would overwrite an existing
//!    fact, an ambiguous identity, or a multi-leaf head stays for the
//!    owner.
//!
//! Filed proposals go through the same signing path as the owner's
//! accept ([`AtomSigner`]), with one extra provenance entry
//! `{kind: auto_accept, uri: ffs://local/atom/<grant>, hash: <grant>}`.
//! The submission ends `AutoAccepted` when everything filed, or
//! `PartiallyAccepted` when a subset stays pending.

use std::collections::HashMap;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde::Serialize;

use ffs_core::capability::{self, Action, CapabilityClaim, Target};
use ffs_core::quarantine::{Filing, IngestQuarantine, Proposal, Resolution, classify};
use ffs_core::store::AtomStore;
use ffs_core::{
    EntityId, Iso8601, Multihash, PredicateName, Provenance, PublicKey, SourceKind, Tier,
    predicate::SpecRegistry,
};

use crate::api::ApiError;
use crate::dispatch::AtomSigner;
use crate::notify::EventPublisher;

/// Default daily cap when an `Accept` grant carries none (ADR-029 wants
/// every accept grant capped; the CLI refuses to author one without a
/// cap, so this only guards grants authored elsewhere).
pub const DEFAULT_MAX_PER_DAY: u32 = 50;

/// Everything the auto-filer needs, as Arcs, so it can run inside the
/// extraction `tokio::spawn` without borrowing the dispatcher.
#[derive(Clone)]
pub struct AutoFiler {
    pub store: Arc<dyn AtomStore>,
    pub registry: Arc<SpecRegistry>,
    pub quarantine: Arc<dyn IngestQuarantine>,
    pub notifier: Arc<EventPublisher>,
    pub signing_key: Option<Arc<SigningKey>>,
    /// Identity `file://` drops act as; see [`grantee_for_source`].
    pub ingest_agent_identity: Option<String>,
}

/// What one auto-file pass decided, for logs, tests, and the tick result.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AutoFileReport {
    pub submission_id: String,
    /// The grantee identity resolved from `source_uri`, if any.
    pub grantee: Option<String>,
    /// Atom hashes filed under the grant, in filing order.
    pub filed: Vec<Multihash>,
    /// Proposals left for the owner: `(local_ref or #index, reason)`.
    pub pending: Vec<(String, String)>,
}

/// The grantee identity a submission acts as, from its `source_uri`
/// alone: `mcp:agent/<id>` submissions name themselves; `file://` drops
/// act as the daemon's configured ingest identity; anything else has no
/// grantee. Content never influences this.
pub fn grantee_for_source(source_uri: &str, ingest_identity: Option<&str>) -> Option<String> {
    if source_uri.starts_with("mcp:agent/") {
        return Some(source_uri.to_string());
    }
    if source_uri.starts_with("file://") {
        return ingest_identity
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    None
}

/// The public-key-shaped identity a capability grant names for an
/// agent. A real Ed25519 multibase key is used as is; any other
/// identity string (`mcp:agent/courier`) maps to a deterministic
/// 32-byte id derived from the string, so grants and evaluation agree
/// without the agent holding a key. The derivation is one-way and
/// domain-separated; it is an identity label, not a signing key.
pub fn agent_identity_key(identity: &str) -> PublicKey {
    if let Ok(bytes) = ffs_core::multibase::decode_base58btc(identity)
        && bytes.len() == 32
    {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        return PublicKey::from_bytes(arr);
    }
    let mh = Multihash::blake3_of(format!("ffs-agent-identity:{identity}").as_bytes());
    let mut arr = [0u8; 32];
    arr.copy_from_slice(mh.digest());
    PublicKey::from_bytes(arr)
}

/// `now` minus `hours`, as an `Iso8601` (the health summary's window).
pub fn since_hours_ago(now: &Iso8601, hours: i64) -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let parsed = time::OffsetDateTime::parse(now.as_str(), &Fmt::DEFAULT)
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let earlier = parsed - time::Duration::hours(hours);
    let s = earlier
        .format(&Fmt::DEFAULT)
        .unwrap_or_else(|_| now.as_str().to_string());
    Iso8601::new(s).unwrap_or_else(|_| now.clone())
}

/// Start of the current UTC day for `now` (an `Iso8601`), used as the
/// `max_per_day` counting window.
pub fn midnight_utc(now: &Iso8601) -> Iso8601 {
    let s = now.as_str();
    let day = &s[..10.min(s.len())];
    Iso8601::new(format!("{day}T00:00:00Z")).unwrap_or_else(|_| now.clone())
}

impl AutoFiler {
    fn signer(&self) -> AtomSigner {
        AtomSigner {
            store: self.store.clone(),
            registry: self.registry.clone(),
            notifier: self.notifier.clone(),
        }
    }

    /// Run one auto-file pass over an `Extracted` submission. Never
    /// fails the submission: any error leaves it pending for the owner
    /// and is returned for the caller to log.
    pub async fn auto_file(&self, submission_id: &str) -> Result<AutoFileReport, ApiError> {
        let mut report = AutoFileReport {
            submission_id: submission_id.to_string(),
            ..Default::default()
        };
        let Some(sub) = self.quarantine.get(submission_id).await else {
            return Ok(report);
        };
        use ffs_core::quarantine::SubmissionStatus;
        if !matches!(
            sub.status,
            SubmissionStatus::Extracted | SubmissionStatus::PartiallyAccepted
        ) {
            return Ok(report);
        }
        let Some(grantee) =
            grantee_for_source(&sub.source_uri, self.ingest_agent_identity.as_deref())
        else {
            for (i, p) in sub.proposals.iter().enumerate() {
                report
                    .pending
                    .push((lref(p, i), "no grantee identity for this source".into()));
            }
            return Ok(report);
        };
        report.grantee = Some(grantee.clone());
        let Some(key) = self.signing_key.clone() else {
            return Ok(report);
        };
        let agent = agent_identity_key(&grantee);
        let now = crate::dispatch::current_iso8601();
        let since = midnight_utc(&now);

        // Usage per grant within this pass, on top of what the store
        // already counts for today.
        let mut used: HashMap<String, u32> = HashMap::new();
        // Proposal index → the grant that allowed it, for the additive
        // subset this pass files.
        let mut allowed: HashMap<usize, Multihash> = HashMap::new();
        // Proposals an earlier pass already landed (a re-run over a
        // partly filed submission): skipped, and their ids bind refs.
        let already = filed_bindings(&*self.store, &sub);
        let order = crate::resolver::dependency_order(&sub.proposals);
        for i in order {
            let p = &sub.proposals[i];
            let this_ref = lref(p, i);
            if already.contains_key(&this_ref) {
                continue;
            }
            let target = Target {
                predicate: p.predicate.clone(),
                entity: p.entity.clone().unwrap_or_else(|| EntityId::new("new")),
                classification: Some(Tier::new("existence")),
                tier: None,
            };
            let decision =
                match capability::evaluate(&*self.store, &agent, Action::Accept, &target, &now) {
                    Ok(d) => d,
                    Err(e) => {
                        report
                            .pending
                            .push((this_ref, format!("capability evaluation failed: {e}")));
                        continue;
                    }
                };
            let Some(grant_hash) = decision.allowed_by().cloned() else {
                report
                    .pending
                    .push((this_ref, "no Accept grant covers this proposal".into()));
                continue;
            };

            // Daily cap, counted from the store plus this pass.
            let cap = self.grant_cap(&grant_hash);
            let already = self
                .store
                .count_auto_accepted_since(&grant_hash, &since)
                .unwrap_or(0);
            let in_pass = used.get(&grant_hash.to_multibase()).copied().unwrap_or(0);
            if already + in_pass >= cap {
                report
                    .pending
                    .push((this_ref, format!("daily cap reached ({cap})")));
                continue;
            }

            // Safety: the classifier against the entity's current heads.
            // Cross-reference fields are bindings the signer fills with
            // entity ids, not values; the display text they carry is not
            // compared against the head's id.
            let (heads, multi_leaf) = self.heads_for(p);
            let spec = self.registry.get(p.predicate.as_str());
            let mut classified = p.clone();
            if let Some(obj) = classified.claim.as_object_mut() {
                for r in &p.refs {
                    if !r.field.contains('/') {
                        obj.remove(&r.field);
                    }
                }
            }
            match classify(&classified, &heads, spec.as_ref(), multi_leaf) {
                Filing::Additive => {
                    *used.entry(grant_hash.to_multibase()).or_insert(0) += 1;
                    allowed.insert(i, grant_hash);
                }
                Filing::Conflicting(reason) => report.pending.push((this_ref, reason)),
            }
        }

        if allowed.is_empty() {
            return Ok(report);
        }

        // File the additive subset through the shared signing path in
        // one pass, so cross-references between filed proposals bind to
        // the ids minted here; the rest stay for the owner.
        let only: Vec<usize> = allowed.keys().copied().collect();
        let extra_for = |i: usize| {
            allowed.get(&i).map(|grant_hash| Provenance {
                kind: SourceKind::AutoAccept,
                uri: format!("ffs://local/atom/{}", grant_hash.to_multibase()),
                hash: grant_hash.clone(),
            })
        };
        let filed_all = self
            .signer()
            .sign_and_insert_subset(
                &sub.proposals,
                &HashMap::new(),
                &key,
                &now,
                &already,
                Some(&only),
                &extra_for,
            )
            .await?;
        let remaining_pending = !report.pending.is_empty();
        self.quarantine
            .auto_accept(submission_id, filed_all.clone(), remaining_pending)
            .await
            .map_err(|e| ApiError {
                code: crate::api::ERR_INTERNAL,
                message: format!("quarantine auto_accept: {e}"),
                data: None,
            })?;
        self.notifier
            .publish(crate::notify::Event::QuarantineChanged {
                submission_id: Some(submission_id.to_string()),
            });
        report.filed = filed_all;
        Ok(report)
    }

    /// The grant's `max_per_day`, or the default with a warning.
    fn grant_cap(&self, grant_hash: &Multihash) -> u32 {
        let cap = self
            .store
            .get(grant_hash)
            .ok()
            .flatten()
            .and_then(|env| CapabilityClaim::from_envelope(&env).ok())
            .and_then(|c| c.scope.max_per_day);
        match cap {
            Some(n) => n,
            None => {
                tracing::warn!(
                    grant = %grant_hash.to_multibase(),
                    default = DEFAULT_MAX_PER_DAY,
                    "Accept grant carries no max_per_day; applying the default"
                );
                DEFAULT_MAX_PER_DAY
            }
        }
    }

    /// The current head atoms for the proposal's resolved entity (its
    /// own predicate) and whether that chain has more than one
    /// unsuperseded leaf.
    fn heads_for(&self, p: &Proposal) -> (Vec<ffs_core::AtomEnvelope>, bool) {
        let Some(entity) = p.entity.as_ref() else {
            return (Vec::new(), false);
        };
        if p.resolution != Some(Resolution::Existing) {
            return (Vec::new(), false);
        }
        let entity = self
            .store
            .follow_same_as(entity, None)
            .unwrap_or_else(|_| entity.clone());
        let head = self
            .store
            .head_of_chain(&entity, &p.predicate, None)
            .ok()
            .flatten();
        let multi_leaf = self.multi_leaf(&entity, &p.predicate);
        (head.into_iter().collect(), multi_leaf)
    }

    /// More than one unsuperseded atom for `(entity, predicate)`.
    fn multi_leaf(&self, entity: &EntityId, predicate: &PredicateName) -> bool {
        let Ok(atoms) = self.store.list_by_entity(entity, Some(predicate), None) else {
            return false;
        };
        let superseded: std::collections::HashSet<String> = atoms
            .iter()
            .filter_map(|a| a.supersedes.as_ref().map(|h| h.to_multibase()))
            .collect();
        let leaves = atoms
            .iter()
            .filter(|a| {
                a.content_hash()
                    .map(|h| !superseded.contains(&h.to_multibase()))
                    .unwrap_or(false)
            })
            .count();
        leaves > 1
    }
}

fn lref(p: &Proposal, i: usize) -> String {
    p.local_ref.clone().unwrap_or_else(|| format!("#{i}"))
}

/// Which of a submission's proposals an earlier pass already filed,
/// as `local_ref → entity id`, recovered from the atoms the submission
/// records (`auto_accepted_atom_hashes` and `accepted_atom_hashes`).
/// A proposal matches an atom on predicate and either the resolved
/// entity (`existing`) or the family's name field (`new`). Used so a
/// later accept or auto-file pass neither files a proposal twice nor
/// loses the cross-references into what already landed.
pub fn filed_bindings(
    store: &dyn AtomStore,
    sub: &ffs_core::quarantine::Submission,
) -> HashMap<String, EntityId> {
    let mut out = HashMap::new();
    let hashes = sub
        .auto_accepted_atom_hashes
        .iter()
        .chain(sub.accepted_atom_hashes.iter());
    let mut atoms: Vec<ffs_core::AtomEnvelope> = Vec::new();
    for h in hashes {
        if let Ok(Some(env)) = store.get(h) {
            atoms.push(env);
        }
    }
    if atoms.is_empty() {
        return out;
    }
    for (i, p) in sub.proposals.iter().enumerate() {
        let name_field = ["display_name", "title", "name"]
            .into_iter()
            .find(|f| p.claim.get(f).and_then(|v| v.as_str()).is_some());
        let hit = atoms.iter().find(|a| {
            if a.predicate != p.predicate {
                return false;
            }
            if let Some(e) = p.entity.as_ref()
                && p.resolution == Some(Resolution::Existing)
            {
                return &a.entity == e;
            }
            match name_field {
                Some(f) => a.claim.get(f) == p.claim.get(f),
                None => a.claim == p.claim,
            }
        });
        if let Some(a) = hit {
            out.insert(lref(p, i), a.entity.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grantee_comes_from_source_uri_only() {
        assert_eq!(
            grantee_for_source("mcp:agent/claude-code", None).as_deref(),
            Some("mcp:agent/claude-code")
        );
        assert_eq!(grantee_for_source("file:///x/ingest/a.md", None), None);
        assert_eq!(
            grantee_for_source("file:///x/ingest/a.md", Some("mcp:agent/courier")).as_deref(),
            Some("mcp:agent/courier")
        );
        assert_eq!(
            grantee_for_source("https://example.test/a", Some("x")),
            None
        );
    }

    #[test]
    fn agent_identity_key_is_deterministic_and_distinct() {
        let a = agent_identity_key("mcp:agent/courier");
        let b = agent_identity_key("mcp:agent/courier");
        let c = agent_identity_key("mcp:agent/other");
        assert_eq!(a, b);
        assert_ne!(a, c);
        let real = PublicKey::from_bytes([7u8; 32]).to_multibase();
        assert_eq!(agent_identity_key(&real).to_multibase(), real);
    }

    #[test]
    fn midnight_utc_truncates_to_the_day() {
        let n = Iso8601::new("2026-09-20T15:04:05Z").unwrap();
        assert_eq!(midnight_utc(&n).as_str(), "2026-09-20T00:00:00Z");
    }
}
