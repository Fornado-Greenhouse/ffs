//! Daemon-side entity resolver (task_45, ADR-030).
//!
//! Runs once per submission over the whole proposal set the scribe
//! returned, in dependency order (an organization before the person
//! who references it, both before the affiliation that joins them,
//! the article last), and decides per proposal whether the mention is
//! an entity the substrate already knows (`existing`), a fresh one
//! (`new`), or something the owner has to look at (`ambiguous`).
//!
//! The decision rule is Fellegi and Sunter's: per-field agreement
//! weights from `config/resolution.toml` sum to a score, two
//! thresholds split the score into the three outcomes, and a margin
//! makes two close candidates ambiguous even when the best clears the
//! upper threshold. Candidates come from the store (display names,
//! aliases, accepted-resolution priors, full-text hits), always
//! following `entity.same_as` chains so a merged loser lands on its
//! winner, and never including an entity the owner has marked
//! `entity.different_from` anything already bound in this set.
//!
//! The resolver is pure over [`CandidateLookup`]; [`StoreLookup`] is
//! the production implementation over the atom store and the spec
//! registry. Nothing here names a publisher or a feed; the only
//! predicate names in this file are the substrate's own identity
//! predicates and the NIL-policy list from ADR-030.
//!
//! **Coherence seam (Phase 2).** [`SubmissionContext::bound`] carries
//! every entity id already bound earlier in the same set. Scoring
//! reads it today only to compare organizations by id; a later
//! change can raise a person candidate because the article's other
//! mentions agree with that candidate's known affiliations (AIDA-style
//! coherence) without changing the interface.

use std::collections::{HashMap, HashSet};

use ffs_core::predicate::{FamilyEntry, SpecRegistry};
use ffs_core::quarantine::{Candidate, Proposal, Resolution};
use ffs_core::store::AtomStore;
use ffs_core::{
    BlockingKey, EntityId, Iso8601, PredicateName, ResolutionConfig, Sighting, normalized_name_key,
    surname_key,
};

/// Person-shaped predicates: the surname block, the diminutive and
/// initial rules, and the NIL policy apply to these and nothing else.
/// This is policy about people, not about any source.
pub const NIL_GATED_PREDICATES: &[&str] = &["person.generic", "contact.person"];

/// The NIL policy (ADR-030 § 6.6) gates only a person mentioned in
/// passing by an article: a `person.generic` in a set that also
/// carries a `source.article`, with no distinguishing attribute. A
/// contact card or a standalone person note the owner dropped is the
/// owner's own record and always mints.
pub const NIL_MENTION_PREDICATES: &[&str] = &["person.generic"];

/// Claim fields that make a person mention distinguishing enough to
/// mint on first sighting.
const DISTINGUISHING_FIELDS: &[&str] = &[
    "organization",
    "role",
    "email",
    "work_email",
    "personal_email",
    "phone",
    "location",
    "team",
];

/// Fixed fallback rank when the refs graph does not order two
/// proposals: organizations, then people, then the roles joining
/// them, then events, then the article that reports all of it.
fn predicate_rank(p: &str) -> u8 {
    match p {
        "org.company" => 0,
        "person.generic" | "contact.person" => 1,
        "affiliation" => 2,
        "event.business" => 3,
        "source.article" => 4,
        _ => 5,
    }
}

/// What the resolver knows about one candidate entity.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateInfo {
    pub entity: EntityId,
    pub display: String,
    pub aliases: Vec<String>,
    /// The candidate's organization as a display string (resolved
    /// through the store when the claim holds an entity id).
    pub organization: Option<String>,
    /// The candidate's organization as an entity id, when known.
    pub organization_entity: Option<EntityId>,
    pub role: Option<String>,
    pub location: Option<String>,
    /// How the candidate was found: `display_name`, `alias`, `prior`, `fts`.
    pub matched_on: Vec<String>,
}

/// Store-facing lookups the resolver needs. Kept narrow so unit tests
/// can drive the resolver with a hand-built table.
pub trait CandidateLookup {
    /// The projection family (predicate, name field) for a predicate,
    /// if its spec declares one.
    fn family_for_predicate(&self, predicate: &PredicateName) -> Option<FamilyEntry>;
    /// Candidates blocked by `key` (a normalized full name or surname
    /// per the blocking config) within `family`, plus prior and
    /// full-text hits for `display`. Losers of `same_as` merges are
    /// already folded into their winners.
    fn candidates(
        &self,
        family: &FamilyEntry,
        display: &str,
        key: &str,
        key_kind: BlockingKey,
    ) -> Vec<CandidateInfo>;
    fn different_from(&self, entity: &EntityId) -> Vec<EntityId>;
    fn follow_same_as(&self, entity: &EntityId) -> EntityId;
    fn prior_count(&self, form: &str, entity: &EntityId) -> u32;
    /// Record a bare mention under the NIL policy; returns the prior
    /// sighting for the same key when one exists.
    fn record_sighting(&self, key: &str, submission_id: &str, display: &str) -> Option<Sighting>;
    /// Display name for an already-bound entity (for organization
    /// comparison by id), if resolvable.
    fn display_of(&self, entity: &EntityId) -> Option<String>;
}

/// Per-submission state the resolver threads through the set.
#[derive(Debug, Clone)]
pub struct SubmissionContext {
    /// Identifies the submission for NIL sightings (the quarantine id
    /// when known, else the source URI).
    pub submission_id: String,
    pub now: Iso8601,
    /// `local_ref` -> entity id, for every proposal already resolved
    /// to an existing entity in this set. The coherence seam.
    pub bound: HashMap<String, EntityId>,
    /// Organization context of the article (the first organization
    /// display in the set), used in the NIL sighting key.
    pub article_org: Option<String>,
}

impl SubmissionContext {
    pub fn new(submission_id: impl Into<String>, now: Iso8601) -> Self {
        Self {
            submission_id: submission_id.into(),
            now,
            bound: HashMap::new(),
            article_org: None,
        }
    }
}

/// What the resolver wants the caller to know beyond the mutated set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolveReport {
    pub warnings: Vec<String>,
    /// `(earlier submission id, display)` for bare mentions that just
    /// crossed the second-sighting line: the earlier article's
    /// mention should be back-filled with the entity minted now.
    pub backfill: Vec<(String, String)>,
    /// `local_ref`s (or `#index`) of proposals removed from the set
    /// because the NIL policy kept them as mentions only.
    pub dropped: Vec<String>,
}

fn claim_str<'a>(claim: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    claim
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn local_ref_of(p: &Proposal, idx: usize) -> String {
    p.local_ref.clone().unwrap_or_else(|| format!("#{idx}"))
}

/// Dependency order: a proposal after every proposal it references,
/// falling back to the fixed predicate rank. Returns indexes into the
/// set. Bounded so a reference cycle degrades to rank order.
pub fn dependency_order(set: &[Proposal]) -> Vec<usize> {
    let by_ref: HashMap<String, usize> = set
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.local_ref.clone().map(|r| (r, i)))
        .collect();
    let mut order: Vec<usize> = (0..set.len()).collect();
    order.sort_by_key(|&i| (predicate_rank(set[i].predicate.as_str()), i));
    let mut placed: Vec<usize> = Vec::with_capacity(set.len());
    let mut placed_set: HashSet<usize> = HashSet::new();
    let mut pending = order;
    let mut rounds = 0;
    while !pending.is_empty() && rounds < set.len() + 1 {
        rounds += 1;
        let mut next_pending = Vec::new();
        for i in pending {
            let deps_ok = set[i].refs.iter().all(|r| match by_ref.get(&r.local_ref) {
                Some(&j) if j != i => placed_set.contains(&j),
                _ => true,
            });
            if deps_ok {
                placed.push(i);
                placed_set.insert(i);
            } else {
                next_pending.push(i);
            }
        }
        pending = next_pending;
    }
    // Cycle or self-reference: append whatever is left in rank order.
    placed.extend(pending);
    placed
}

/// Organization display for a proposal: the display of the org
/// proposal it references in this set, else its own `organization`
/// string. Also returns the bound org entity id when the referenced
/// proposal already resolved to one.
fn proposal_org(
    p: &Proposal,
    set: &[Proposal],
    ctx: &SubmissionContext,
) -> (Option<String>, Option<EntityId>) {
    let org_ref = p
        .refs
        .iter()
        .find(|r| r.field == "organization")
        .map(|r| r.local_ref.clone());
    if let Some(r) = org_ref {
        let bound = ctx.bound.get(&r).cloned();
        let display = set
            .iter()
            .find(|q| q.local_ref.as_deref() == Some(r.as_str()))
            .and_then(|q| {
                claim_str(&q.claim, "display_name")
                    .or_else(|| claim_str(&q.claim, "title"))
                    .map(str::to_string)
            })
            .or_else(|| claim_str(&p.claim, "organization").map(str::to_string));
        return (display, bound);
    }
    (
        claim_str(&p.claim, "organization").map(str::to_string),
        None,
    )
}

/// Same surname and first names prefix-compatible: the diminutive
/// case ("Sam" for "Samuel") or a bare initial ("S." for "Sara").
fn first_name_compatible(a: &str, b: &str) -> bool {
    let ka = normalized_name_key(a);
    let kb = normalized_name_key(b);
    let mut ta = ka.split(' ');
    let mut tb = kb.split(' ');
    let (Some(fa), Some(fb)) = (ta.next(), tb.next()) else {
        return false;
    };
    if surname_key(a) != surname_key(b) || ka.split(' ').count() < 2 || kb.split(' ').count() < 2 {
        return false;
    }
    let shorter = fa.len().min(fb.len());
    // Either a diminutive ("sam" / "samuel") or a bare initial ("s" /
    // "sara"); an initial alone is weak, which is why this only earns
    // alias-strength credit and the organization and role decide.
    (shorter >= 3 || shorter == 1) && (fa.starts_with(fb) || fb.starts_with(fa))
}

fn agree(cfg: &ResolutionConfig, field: &str, matched: bool) -> f64 {
    match cfg.weight(field) {
        Some(w) => {
            if matched {
                w.agree
            } else {
                w.disagree
            }
        }
        None => 0.0,
    }
}

/// The proposal side of a comparison.
struct Side<'a> {
    person_like: bool,
    display: &'a str,
    aliases: &'a [String],
    org: &'a (Option<String>, Option<EntityId>),
    role: Option<&'a str>,
    location: Option<&'a str>,
}

/// Score one candidate against a proposal. Fields absent on either
/// side contribute nothing; `matched_on` collects the agreeing ones.
fn score_candidate(
    cfg: &ResolutionConfig,
    side: &Side<'_>,
    cand: &CandidateInfo,
    lookups: &dyn CandidateLookup,
) -> (f64, Vec<String>) {
    let Side {
        person_like,
        display,
        aliases: proposal_aliases,
        org,
        role,
        location,
    } = *side;
    let mut score = 0.0;
    let mut matched: Vec<String> = Vec::new();
    let key = normalized_name_key(display);

    let display_match = normalized_name_key(&cand.display) == key;
    // An alias on either side explains the name: the candidate is known
    // by the mention text, or the mention names the candidate's current
    // name as a former one ("Elena Marsh-Tanaka, formerly Elena Marsh").
    let cand_key = normalized_name_key(&cand.display);
    let alias_match = cand.aliases.iter().any(|a| normalized_name_key(a) == key)
        || proposal_aliases.iter().any(|a| {
            let ak = normalized_name_key(a);
            ak == cand_key || cand.aliases.iter().any(|c| normalized_name_key(c) == ak)
        });
    if display_match {
        score += agree(cfg, "display_name", true);
        matched.push("display_name".into());
    } else if alias_match {
        score += agree(cfg, "alias", true);
        matched.push("alias".into());
    } else if person_like && first_name_compatible(display, &cand.display) {
        // "Sam Okafor" against "Samuel Okafor": same surname and one
        // first name is a prefix of the other. Alias-strength evidence,
        // not display-strength; the organization and role decide.
        score += agree(cfg, "alias", true);
        matched.push("display_partial".into());
    } else {
        score += agree(cfg, "display_name", false);
    }

    // Surnames are a people signal; an article title's last word is
    // not a surname. When an alias explains a different surname (a
    // rename), the penalty does not apply.
    let rename_explains_surname = alias_match && !display_match;
    if person_like && !key.is_empty() && !rename_explains_surname {
        let surname_match = surname_key(&cand.display) == surname_key(display);
        score += agree(cfg, "surname", surname_match);
        if surname_match {
            matched.push("surname".into());
        }
    }

    // Organization: by id when both sides are bound, else by display.
    let org_cmp = match (&org.1, &cand.organization_entity) {
        (Some(a), Some(b)) => Some(a == b),
        _ => match (&org.0, &cand.organization) {
            (Some(a), Some(b)) => Some(normalized_name_key(a) == normalized_name_key(b)),
            _ => None,
        },
    };
    if let Some(m) = org_cmp {
        score += agree(cfg, "organization", m);
        if m {
            matched.push("organization".into());
        }
    }

    if let (Some(a), Some(b)) = (role, cand.role.as_deref()) {
        let m = normalized_name_key(a) == normalized_name_key(b);
        score += agree(cfg, "role", m);
        if m {
            matched.push("role".into());
        }
    }
    if let (Some(a), Some(b)) = (location, cand.location.as_deref()) {
        let m = normalized_name_key(a) == normalized_name_key(b);
        score += agree(cfg, "location", m);
        if m {
            matched.push("location".into());
        }
    }

    let prior = lookups.prior_count(&key, &cand.entity);
    if prior > 0 {
        score += (1.0 + f64::from(prior)).ln();
        matched.push("prior".into());
    }
    for m in &cand.matched_on {
        if m == "fts" && !matched.contains(m) {
            matched.push(m.clone());
        }
    }
    (score, matched)
}

fn has_distinguishing_attribute(p: &Proposal) -> bool {
    p.refs.iter().any(|r| r.field == "organization")
        || DISTINGUISHING_FIELDS
            .iter()
            .any(|f| claim_str(&p.claim, f).is_some())
}

/// Resolve every proposal in `set` in dependency order. Proposals
/// without a projection family (affiliations, identity assertions)
/// are marked `new` untouched: their entity is minted at accept.
/// Bare-name people the NIL policy keeps as mentions are removed from
/// the set (and from other proposals' refs) and listed in the report.
pub fn resolve_set(
    set: &mut Vec<Proposal>,
    lookups: &dyn CandidateLookup,
    cfg: &ResolutionConfig,
    ctx: &mut SubmissionContext,
) -> ResolveReport {
    let mut report = ResolveReport::default();

    if ctx.article_org.is_none() {
        ctx.article_org = set
            .iter()
            .find(|p| p.predicate.as_str() == "org.company")
            .and_then(|p| claim_str(&p.claim, "display_name").map(str::to_string));
    }

    let order = dependency_order(set);
    let mut drop: Vec<usize> = Vec::new();
    // The NIL policy only applies to mentions inside an article set.
    let in_article_set = set.iter().any(|p| p.predicate.as_str() == "source.article");

    for &i in &order {
        let snapshot: Vec<Proposal> = set.clone();
        let p = &mut set[i];
        if p.ends_role {
            // A role ending supersedes an existing affiliation at
            // accept; nothing to resolve here.
            p.resolution = Some(Resolution::Existing);
            continue;
        }
        let Some(family) = lookups.family_for_predicate(&p.predicate) else {
            p.resolution = Some(Resolution::New);
            continue;
        };
        let Some(display) = claim_str(&p.claim, &family.name_field).map(str::to_string) else {
            p.resolution = Some(Resolution::New);
            report.warnings.push(format!(
                "{}: no {} on the claim; minted as new",
                local_ref_of(p, i),
                family.name_field
            ));
            continue;
        };

        let key = match cfg.blocking.key {
            BlockingKey::FullName => normalized_name_key(&display),
            BlockingKey::Surname => surname_key(&display),
        };
        let org = proposal_org(p, &snapshot, ctx);
        let role = claim_str(&p.claim, "role").map(str::to_string);
        let location = claim_str(&p.claim, "location").map(str::to_string);
        let proposal_aliases: Vec<String> = p
            .claim
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // Candidates, with different_from hard blocks against anything
        // already bound in this set.
        let bound_ids: Vec<EntityId> = ctx.bound.values().cloned().collect();
        let mut scored: Vec<Candidate> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        // Candidates for the display and for every alias the proposal
        // itself carries (a former name is a surface form too).
        let mut raw: Vec<CandidateInfo> =
            lookups.candidates(&family, &display, &key, cfg.blocking.key);
        for alias in &proposal_aliases {
            let akey = match cfg.blocking.key {
                BlockingKey::FullName => normalized_name_key(alias),
                BlockingKey::Surname => surname_key(alias),
            };
            raw.extend(lookups.candidates(&family, alias, &akey, cfg.blocking.key));
        }
        for cand in raw {
            let winner = lookups.follow_same_as(&cand.entity);
            if !seen.insert(winner.as_str().to_string()) {
                continue;
            }
            let blocked = lookups
                .different_from(&winner)
                .iter()
                .any(|d| bound_ids.contains(d));
            if blocked {
                continue;
            }
            let person_like = NIL_GATED_PREDICATES.contains(&p.predicate.as_str());
            let side = Side {
                person_like,
                display: &display,
                aliases: &proposal_aliases,
                org: &org,
                role: role.as_deref(),
                location: location.as_deref(),
            };
            let (score, matched_on) = score_candidate(cfg, &side, &cand, lookups);
            scored.push(Candidate {
                entity: winner,
                score,
                matched_on,
                display: cand.display.clone(),
            });
        }
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(5);

        let outcome = match scored.first() {
            None => None,
            Some(best) if best.score < cfg.thresholds.review_floor => None,
            Some(best) if best.score >= cfg.thresholds.auto_link => {
                // A runner-up the owner has separated from the best is
                // not a tie; skip it when applying the margin.
                let best_diff = lookups.different_from(&best.entity);
                let runner = scored
                    .iter()
                    .skip(1)
                    .find(|c| !best_diff.contains(&c.entity));
                match runner {
                    Some(second) if best.score - second.score <= cfg.thresholds.margin => {
                        Some(Resolution::Ambiguous)
                    }
                    _ => Some(Resolution::Existing),
                }
            }
            Some(_) => Some(Resolution::Ambiguous),
        };

        let lref = local_ref_of(p, i);
        match outcome {
            Some(Resolution::Existing) => {
                let best = scored[0].clone();
                p.entity = Some(best.entity.clone());
                p.resolution = Some(Resolution::Existing);
                p.candidates = scored;
                ctx.bound.insert(lref, best.entity);
            }
            Some(Resolution::Ambiguous) => {
                p.resolution = Some(Resolution::Ambiguous);
                p.candidates = scored;
            }
            Some(Resolution::New) | None => {
                let gated =
                    in_article_set && NIL_MENTION_PREDICATES.contains(&p.predicate.as_str());
                if gated && !has_distinguishing_attribute(p) {
                    let sighting_key = format!(
                        "{}|{}",
                        normalized_name_key(&display),
                        ctx.article_org
                            .as_deref()
                            .map(normalized_name_key)
                            .unwrap_or_default()
                    );
                    match lookups.record_sighting(&sighting_key, &ctx.submission_id, &display) {
                        Some(prior) if prior.submission_id != ctx.submission_id => {
                            p.resolution = Some(Resolution::New);
                            p.candidates = scored;
                            report
                                .backfill
                                .push((prior.submission_id.clone(), prior.display.clone()));
                        }
                        _ => {
                            report.warnings.push(format!(
                                "{lref}: bare mention of {display:?} kept as a mention only (NIL policy); \
                                 mints on a second sighting"
                            ));
                            report.dropped.push(lref);
                            drop.push(i);
                        }
                    }
                } else {
                    p.resolution = Some(Resolution::New);
                    p.candidates = scored;
                }
            }
        }
    }

    // Remove NIL-dropped proposals and any refs pointing at them; the
    // article's mention keeps its display and simply has no entity.
    if !drop.is_empty() {
        let dropped_refs: HashSet<String> = drop
            .iter()
            .filter_map(|&i| set[i].local_ref.clone())
            .collect();
        for p in set.iter_mut() {
            p.refs.retain(|r| !dropped_refs.contains(&r.local_ref));
        }
        drop.sort_unstable();
        for &i in drop.iter().rev() {
            set.remove(i);
        }
    }
    report
}

/// Production lookups over the atom store and the spec registry.
pub struct StoreLookup<'a> {
    pub store: &'a dyn AtomStore,
    pub registry: &'a SpecRegistry,
}

impl StoreLookup<'_> {
    /// Latest atom per entity for a predicate (the store lists newest
    /// first), skipping entities merged into another.
    fn heads_for(&self, predicate: &PredicateName) -> Vec<ffs_core::AtomEnvelope> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for atom in self
            .store
            .list_by_predicate(predicate, None, 10_000)
            .unwrap_or_default()
        {
            if seen.insert(atom.entity.as_str().to_string()) {
                out.push(atom);
            }
        }
        out
    }

    fn info_from_atom(
        &self,
        atom: &ffs_core::AtomEnvelope,
        name_field: &str,
        matched_on: &str,
    ) -> Option<CandidateInfo> {
        let display = claim_str(&atom.claim, name_field)?.to_string();
        let aliases: Vec<String> = atom
            .claim
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let (organization, organization_entity) = match claim_str(&atom.claim, "organization") {
            Some(o) => {
                let id = EntityId::new(o);
                match self.display_of(&id) {
                    Some(d) => (Some(d), Some(id)),
                    None => (Some(o.to_string()), None),
                }
            }
            None => (None, None),
        };
        Some(CandidateInfo {
            entity: atom.entity.clone(),
            display,
            aliases,
            organization,
            organization_entity,
            role: claim_str(&atom.claim, "role").map(str::to_string),
            location: claim_str(&atom.claim, "location").map(str::to_string),
            matched_on: vec![matched_on.to_string()],
        })
    }
}

impl CandidateLookup for StoreLookup<'_> {
    fn family_for_predicate(&self, predicate: &PredicateName) -> Option<FamilyEntry> {
        self.registry.family_for_predicate(predicate.as_str())
    }

    fn candidates(
        &self,
        family: &FamilyEntry,
        display: &str,
        key: &str,
        key_kind: BlockingKey,
    ) -> Vec<CandidateInfo> {
        let predicate = PredicateName::new(&family.predicate);
        let key_of = |s: &str| match key_kind {
            BlockingKey::FullName => normalized_name_key(s),
            BlockingKey::Surname => surname_key(s),
        };
        let mut out: Vec<CandidateInfo> = Vec::new();
        let mut push = |info: CandidateInfo| {
            if let Some(existing) = out.iter_mut().find(|c| c.entity == info.entity) {
                for m in info.matched_on {
                    if !existing.matched_on.contains(&m) {
                        existing.matched_on.push(m);
                    }
                }
            } else {
                out.push(info);
            }
        };
        let heads = self.heads_for(&predicate);
        let surname = if NIL_GATED_PREDICATES.contains(&family.predicate.as_str()) {
            surname_key(display)
        } else {
            String::new()
        };
        for atom in &heads {
            let Some(name) = claim_str(&atom.claim, &family.name_field) else {
                continue;
            };
            if key_of(name) == key {
                if let Some(info) = self.info_from_atom(atom, &family.name_field, "display_name") {
                    push(info);
                }
                continue;
            }
            // Surname block: reaches scoring so organization and role
            // can confirm a diminutive or a rename; a bare surname match
            // scores below the review floor on its own.
            if !surname.is_empty()
                && surname_key(name) == surname
                && let Some(info) = self.info_from_atom(atom, &family.name_field, "surname")
            {
                push(info);
                continue;
            }
            let alias_hit = atom
                .claim
                .get("aliases")
                .and_then(|v| v.as_array())
                .is_some_and(|a| {
                    a.iter()
                        .any(|v| v.as_str().is_some_and(|s| key_of(s) == key))
                });
            if alias_hit && let Some(info) = self.info_from_atom(atom, &family.name_field, "alias")
            {
                push(info);
            }
        }
        // Priors: entities the owner accepted this surface form for.
        let form = normalized_name_key(display);
        for (entity, _) in self.store.prior_counts(&form).unwrap_or_default() {
            if let Ok(Some(head)) = self.store.head_of_chain(&entity, &predicate, None)
                && let Some(info) = self.info_from_atom(&head, &family.name_field, "prior")
            {
                push(info);
            }
        }
        // Full text: quoted so the display is a phrase, not MATCH syntax.
        let quoted = format!("\"{}\"", display.replace('"', " "));
        for hash in self.store.search_fts(&quoted, 20).unwrap_or_default() {
            if let Ok(Some(atom)) = self.store.get(&hash)
                && atom.predicate == predicate
                && let Ok(Some(head)) = self.store.head_of_chain(&atom.entity, &predicate, None)
                && let Some(info) = self.info_from_atom(&head, &family.name_field, "fts")
            {
                push(info);
            }
        }
        out
    }

    fn different_from(&self, entity: &EntityId) -> Vec<EntityId> {
        self.store.different_from(entity, None).unwrap_or_default()
    }

    fn follow_same_as(&self, entity: &EntityId) -> EntityId {
        self.store
            .follow_same_as(entity, None)
            .unwrap_or_else(|_| entity.clone())
    }

    fn prior_count(&self, form: &str, entity: &EntityId) -> u32 {
        self.store
            .prior_counts(form)
            .unwrap_or_default()
            .into_iter()
            .find(|(e, _)| e == entity)
            .map(|(_, c)| c)
            .unwrap_or(0)
    }

    fn record_sighting(&self, key: &str, submission_id: &str, display: &str) -> Option<Sighting> {
        let now = Iso8601::new(
            time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Iso8601::DEFAULT)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into()),
        )
        .unwrap_or_else(|_| Iso8601::new("1970-01-01T00:00:00Z").expect("constant timestamp"));
        self.store
            .record_sighting(key, submission_id, display, &now)
            .unwrap_or(None)
    }

    fn display_of(&self, entity: &EntityId) -> Option<String> {
        for family in self.registry.families() {
            let pred = PredicateName::new(&family.predicate);
            if let Ok(Some(head)) = self.store.head_of_chain(entity, &pred, None)
                && let Some(d) = claim_str(&head.claim, &family.name_field)
            {
                return Some(d.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffs_core::quarantine::CrossRef;
    use ffs_core::resolution::{BlockingConfig, FieldWeight, Thresholds};
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    fn cfg() -> ResolutionConfig {
        let mut weights = BTreeMap::new();
        for (k, a, d) in [
            ("display_name", 8.0, -4.0),
            ("alias", 5.0, 0.0),
            ("organization", 4.0, -5.0),
            ("role", 1.5, -0.5),
            ("location", 1.0, -0.5),
            ("surname", 0.5, -6.0),
        ] {
            weights.insert(
                k.to_string(),
                FieldWeight {
                    agree: a,
                    disagree: d,
                },
            );
        }
        ResolutionConfig {
            thresholds: Thresholds {
                auto_link: 8.0,
                review_floor: 2.0,
                margin: 1.0,
            },
            weights,
            blocking: BlockingConfig::default(),
        }
    }

    #[derive(Default)]
    struct Table {
        people: Vec<CandidateInfo>,
        orgs: Vec<CandidateInfo>,
        different: Vec<(EntityId, EntityId)>,
        same_as: HashMap<String, EntityId>,
        priors: HashMap<(String, String), u32>,
        sightings: RefCell<HashMap<String, Sighting>>,
    }

    impl CandidateLookup for Table {
        fn family_for_predicate(&self, predicate: &PredicateName) -> Option<FamilyEntry> {
            match predicate.as_str() {
                "person.generic" => Some(FamilyEntry {
                    family: "people".into(),
                    predicate: "person.generic".into(),
                    name_field: "display_name".into(),
                }),
                "org.company" => Some(FamilyEntry {
                    family: "orgs".into(),
                    predicate: "org.company".into(),
                    name_field: "display_name".into(),
                }),
                "source.article" => Some(FamilyEntry {
                    family: "articles".into(),
                    predicate: "source.article".into(),
                    name_field: "title".into(),
                }),
                _ => None,
            }
        }
        fn candidates(
            &self,
            family: &FamilyEntry,
            _display: &str,
            key: &str,
            _kind: BlockingKey,
        ) -> Vec<CandidateInfo> {
            let pool = if family.family == "people" {
                &self.people
            } else {
                &self.orgs
            };
            pool.iter()
                .filter(|c| {
                    normalized_name_key(&c.display) == key
                        || c.aliases.iter().any(|a| normalized_name_key(a) == key)
                })
                .cloned()
                .collect()
        }
        fn different_from(&self, entity: &EntityId) -> Vec<EntityId> {
            self.different
                .iter()
                .filter_map(|(a, b)| {
                    if a == entity {
                        Some(b.clone())
                    } else if b == entity {
                        Some(a.clone())
                    } else {
                        None
                    }
                })
                .collect()
        }
        fn follow_same_as(&self, entity: &EntityId) -> EntityId {
            self.same_as
                .get(entity.as_str())
                .cloned()
                .unwrap_or_else(|| entity.clone())
        }
        fn prior_count(&self, form: &str, entity: &EntityId) -> u32 {
            *self
                .priors
                .get(&(form.to_string(), entity.as_str().to_string()))
                .unwrap_or(&0)
        }
        fn record_sighting(
            &self,
            key: &str,
            submission_id: &str,
            display: &str,
        ) -> Option<Sighting> {
            let mut s = self.sightings.borrow_mut();
            let prior = s.get(key).cloned();
            s.entry(key.to_string()).or_insert(Sighting {
                key: key.to_string(),
                submission_id: submission_id.to_string(),
                display: display.to_string(),
                first_seen: Iso8601::new("2026-09-01T00:00:00Z").unwrap(),
            });
            prior
        }
        fn display_of(&self, entity: &EntityId) -> Option<String> {
            self.people
                .iter()
                .chain(self.orgs.iter())
                .find(|c| &c.entity == entity)
                .map(|c| c.display.clone())
        }
    }

    fn person(display: &str, org: Option<&str>, role: Option<&str>) -> CandidateInfo {
        CandidateInfo {
            entity: EntityId::new(format!("z{}", display.replace(' ', ""))),
            display: display.into(),
            aliases: vec![],
            organization: org.map(str::to_string),
            organization_entity: None,
            role: role.map(str::to_string),
            location: None,
            matched_on: vec!["display_name".into()],
        }
    }

    fn proposal(predicate: &str, claim: serde_json::Value, local_ref: &str) -> Proposal {
        let mut p = Proposal::new(PredicateName::new(predicate), claim, vec![], "test");
        p.local_ref = Some(local_ref.into());
        p
    }

    fn ctx() -> SubmissionContext {
        SubmissionContext::new("sub-a", Iso8601::new("2026-09-20T00:00:00Z").unwrap())
    }

    #[test]
    fn three_outcomes_follow_the_thresholds_and_thresholds_are_data() {
        let table = Table {
            people: vec![person("Sara Chen", Some("Acme"), Some("CEO"))],
            ..Default::default()
        };
        // Full agreement: 8 + 0.5 + 4 + 1.5 = 14 >= 8 -> existing.
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Acme", "role": "CEO"}),
            "p1",
        )];
        let c = cfg();
        resolve_set(&mut set, &table, &c, &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::Existing));
        assert_eq!(set[0].entity.as_ref().unwrap().as_str(), "zSaraChen");
        assert!(
            set[0].candidates[0]
                .matched_on
                .contains(&"organization".to_string())
        );

        // Same name, different org and role: 8 + 0.5 - 5 - 0.5 = 3 -> ambiguous.
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "City Council", "role": "Clerk"}),
            "p1",
        )];
        resolve_set(&mut set, &table, &c, &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::Ambiguous));
        assert_eq!(set[0].candidates.len(), 1);
        assert!(set[0].entity.is_none());

        // Raise the floor above 3: the same case becomes new.
        let mut high = cfg();
        high.thresholds.review_floor = 3.5;
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "City Council", "role": "Clerk"}),
            "p1",
        )];
        resolve_set(&mut set, &table, &high, &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::New));

        // Lower auto_link to 3: it becomes existing.
        let mut low = cfg();
        low.thresholds.auto_link = 3.0;
        low.thresholds.review_floor = 1.0;
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "City Council", "role": "Clerk"}),
            "p1",
        )];
        resolve_set(&mut set, &table, &low, &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::Existing));
    }

    #[test]
    fn two_candidates_within_the_margin_are_ambiguous() {
        let mut a = person("Sara Chen", Some("Acme"), None);
        a.entity = EntityId::new("zA");
        let mut b = person("Sara Chen", Some("Acme"), None);
        b.entity = EntityId::new("zB");
        let table = Table {
            people: vec![a, b],
            ..Default::default()
        };
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Acme"}),
            "p1",
        )];
        resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::Ambiguous));
        assert_eq!(set[0].candidates.len(), 2);
    }

    #[test]
    fn same_as_chain_lands_on_the_winner_and_dedupes() {
        let mut loser = person("Sara Chen", Some("Acme"), Some("CEO"));
        loser.entity = EntityId::new("zLoser");
        loser.aliases = vec!["S. Chen".into()];
        let mut winner = person("Sara Chen", Some("Acme"), Some("CEO"));
        winner.entity = EntityId::new("zWinner");
        let mut same_as = HashMap::new();
        same_as.insert("zLoser".to_string(), EntityId::new("zWinner"));
        let table = Table {
            people: vec![loser, winner],
            same_as,
            ..Default::default()
        };
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "S. Chen", "organization": "Acme", "role": "CEO"}),
            "p1",
        )];
        resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::Existing));
        assert_eq!(set[0].entity.as_ref().unwrap().as_str(), "zWinner");
        assert_eq!(set[0].candidates.len(), 1, "loser folded into winner");
    }

    #[test]
    fn different_from_blocks_a_perfect_name_match() {
        let mut sara = person("Sara Chen", Some("Acme"), Some("CEO"));
        sara.entity = EntityId::new("zSara");
        let mut acme = person("Acme", None, None);
        acme.entity = EntityId::new("zAcme");
        let table = Table {
            people: vec![sara],
            orgs: vec![acme],
            different: vec![(EntityId::new("zSara"), EntityId::new("zAcme"))],
            ..Default::default()
        };
        // The org resolves first and binds zAcme; Sara is different_from
        // zAcme, so she is never a candidate, even with a perfect match.
        let mut set = vec![
            proposal(
                "org.company",
                serde_json::json!({"display_name": "Acme"}),
                "o1",
            ),
            {
                let mut p = proposal(
                    "person.generic",
                    serde_json::json!({"display_name": "Sara Chen", "organization": "Acme", "role": "CEO"}),
                    "p1",
                );
                p.refs = vec![CrossRef {
                    field: "organization".into(),
                    local_ref: "o1".into(),
                }];
                p
            },
        ];
        resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::Existing));
        assert_eq!(set[1].resolution, Some(Resolution::New));
        assert!(set[1].candidates.is_empty());
    }

    #[test]
    fn nil_policy_keeps_a_bare_name_as_a_mention_and_mints_on_second_sighting() {
        let table = Table::default();
        let article = |mention: &str| {
            let mut a = proposal(
                "source.article",
                serde_json::json!({"title": "Local news", "url": "https://example.com/a", "mentions": [{"display": mention, "context": "quoted"}]}),
                "article",
            );
            a.refs = vec![CrossRef {
                field: "mentions[0].entity".into(),
                local_ref: "p1".into(),
            }];
            a
        };
        let mut set = vec![
            proposal(
                "person.generic",
                serde_json::json!({"display_name": "Pat Example"}),
                "p1",
            ),
            article("Pat Example"),
        ];
        let report = resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set.len(), 1, "bare person dropped from the set");
        assert_eq!(set[0].predicate.as_str(), "source.article");
        assert!(
            set[0].refs.is_empty(),
            "the dangling ref is removed; the mention keeps its display"
        );
        assert_eq!(report.dropped, vec!["p1".to_string()]);
        assert!(report.backfill.is_empty());

        // Second sighting from another submission: mints and asks for a backfill.
        let mut set2 = vec![
            proposal(
                "person.generic",
                serde_json::json!({"display_name": "Pat Example"}),
                "p1",
            ),
            article("Pat Example"),
        ];
        let mut ctx2 =
            SubmissionContext::new("sub-b", Iso8601::new("2026-09-21T00:00:00Z").unwrap());
        let report2 = resolve_set(&mut set2, &table, &cfg(), &mut ctx2);
        assert_eq!(set2.len(), 2);
        assert_eq!(set2[0].resolution, Some(Resolution::New));
        assert_eq!(
            report2.backfill,
            vec![("sub-a".to_string(), "Pat Example".to_string())]
        );
    }

    #[test]
    fn a_standalone_person_note_or_contact_card_is_never_nil_gated() {
        let table = Table::default();
        // No article in the set: the owner's own person note mints.
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Pat Example"}),
            "p1",
        )];
        let report = resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].resolution, Some(Resolution::New));
        assert!(report.dropped.is_empty());
        // A contact card inside an article set is still the owner's record.
        let mut set = vec![
            proposal(
                "source.article",
                serde_json::json!({"title": "t", "url": "https://example.com/a"}),
                "article",
            ),
            proposal(
                "contact.person",
                serde_json::json!({"display_name": "Pat Example", "email": "pat@example.com"}),
                "c1",
            ),
        ];
        let report = resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set.len(), 2);
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn a_person_with_an_organization_mints_on_first_sighting() {
        let table = Table::default();
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Pat Example", "organization": "Acme"}),
            "p1",
        )];
        let report = resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::New));
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn organizations_resolve_before_the_people_referencing_them_and_raise_them() {
        let mut acme = person("Acme Widgets", None, None);
        acme.entity = EntityId::new("zAcme");
        let mut sara = person("Sara Chen", Some("Acme Widgets"), None);
        sara.entity = EntityId::new("zSara");
        sara.organization_entity = Some(EntityId::new("zAcme"));
        let table = Table {
            people: vec![sara],
            orgs: vec![acme],
            ..Default::default()
        };
        // Person listed first in the set; the org must still resolve first.
        let mut set = vec![
            {
                let mut p = proposal(
                    "person.generic",
                    serde_json::json!({"display_name": "Sara Chen", "organization": "Acme Widgets"}),
                    "p1",
                );
                p.refs = vec![CrossRef {
                    field: "organization".into(),
                    local_ref: "o1".into(),
                }];
                p
            },
            proposal(
                "org.company",
                serde_json::json!({"display_name": "Acme Widgets"}),
                "o1",
            ),
        ];
        assert_eq!(dependency_order(&set), vec![1, 0]);
        let mut c = ctx();
        resolve_set(&mut set, &table, &cfg(), &mut c);
        assert_eq!(set[1].resolution, Some(Resolution::Existing));
        assert_eq!(c.bound.get("o1").unwrap().as_str(), "zAcme");
        // 8 + 0.5 + 4 (organization by bound id) = 12.5 -> existing
        // (the org itself scored 8 on its exact name).
        assert_eq!(set[0].resolution, Some(Resolution::Existing));
        assert!(
            set[0].candidates[0]
                .matched_on
                .contains(&"organization".to_string())
        );
    }

    #[test]
    fn proposals_without_a_family_are_new_and_role_endings_are_existing() {
        let table = Table::default();
        let mut set = vec![
            proposal(
                "affiliation",
                serde_json::json!({"person": "Sara Chen", "organization": "Acme", "title": "CEO", "kind": "executive"}),
                "a1",
            ),
            {
                let mut e = proposal(
                    "affiliation",
                    serde_json::json!({"person": "Sara Chen", "organization": "Acme"}),
                    "a2",
                );
                e.ends_role = true;
                e
            },
        ];
        resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(set[0].resolution, Some(Resolution::New));
        assert_eq!(set[1].resolution, Some(Resolution::Existing));
    }

    #[test]
    fn a_former_name_in_the_proposal_aliases_links_across_a_rename() {
        let mut elena = person("Elena Marsh", Some("Birchwood Law"), Some("Partner"));
        elena.entity = EntityId::new("zElena");
        let table = Table {
            people: vec![elena],
            ..Default::default()
        };
        // Different surname, but the proposal says the old name is hers:
        // alias 5 + organization 4 + role 1.5 = 10.5, no surname penalty.
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Elena Marsh-Tanaka", "organization": "Birchwood Law", "role": "Partner", "aliases": ["Elena Marsh"]}),
            "p1",
        )];
        resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert_eq!(
            set[0].resolution,
            Some(Resolution::Existing),
            "{:?}",
            set[0].candidates
        );
        assert!(
            set[0].candidates[0]
                .matched_on
                .contains(&"alias".to_string())
        );
    }

    #[test]
    fn prior_counts_raise_a_candidate() {
        let mut sara = person("Sara Chen", Some("Other Co"), None);
        sara.entity = EntityId::new("zSara");
        let mut priors = HashMap::new();
        priors.insert(("sara chen".to_string(), "zSara".to_string()), 3u32);
        let table = Table {
            people: vec![sara],
            priors,
            ..Default::default()
        };
        // 8 + 0.5 - 5 (organization disagrees) + ln(4) = 4.89: the prior
        // lifts a 3.5 higher in the review band, still short of 8.
        let mut set = vec![proposal(
            "person.generic",
            serde_json::json!({"display_name": "Sara Chen", "organization": "Acme"}),
            "p1",
        )];
        resolve_set(&mut set, &table, &cfg(), &mut ctx());
        assert!(
            set[0].candidates[0]
                .matched_on
                .contains(&"prior".to_string())
        );
        assert!(set[0].candidates[0].score > 4.8 && set[0].candidates[0].score < 5.0);
        assert_eq!(set[0].resolution, Some(Resolution::Ambiguous));
    }
}
