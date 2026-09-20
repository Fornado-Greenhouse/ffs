//! Rendering pipeline: atoms + predicate spec + Tera template → markdown
//! + reverse-map annotations + render_hash.
//!
//! For single-entity paths, the head atom for `(entity, primary_predicate)`
//! is fetched, capability-checked against the requesting agent, and passed
//! to the spec's Tera template as `claim`. Reverse-map annotations come
//! straight from the spec — the rendered markdown isn't parsed.
//!
//! For listing paths (recent / alphabetical / by-org), the renderer emits
//! a markdown bullet list of links to per-entity `.md` files. Listings
//! produce an empty reverse-map (the listing itself is not editable; the
//! files it links to are).

use std::path::Path;
use std::sync::Arc;

use tera::{Context, Tera};

use crate::atom::{AtomEnvelope, EntityId, Iso8601, PredicateName};
use crate::capability::{self, Action, Decision, Target};
use crate::multihash::Multihash;
use crate::predicate::{ReverseMapRule, SpecRegistry};
use crate::store::AtomStore;
use crate::working_set::{InMemoryPathIndex, PathIndex};

use super::path::{FamilyTable, ParsedPath, PathFamily};
use super::{ProjectionRequest, ProjectionResponse, RenderError, ReverseMapAnnotation};

/// Predicate that records "this entity was merged into another" (ADR-030).
pub const SAME_AS_PREDICATE: &str = "entity.same_as";
/// Predicate whose atoms are roles rendered by reverse lookup (ADR-031).
pub const AFFILIATION_PREDICATE: &str = "affiliation";

/// Loaded projection renderer. Holds an `Arc` to the store, the registry,
/// and the path-to-entity index so multiple renderer instances can share
/// state (the daemon uses one). Families are read from the registry per
/// render (a `FamilyTable` snapshot), so predicate hot-reload is observed.
pub struct ProjectionRenderer {
    store: Arc<dyn AtomStore>,
    registry: Arc<SpecRegistry>,
    index: Arc<dyn PathIndex>,
    tera: Tera,
    /// The owner's key, so the "as of" line can say "you" (ADR-034).
    owner: Option<crate::atom::PublicKey>,
    /// Per-substrate `k` overrides from `config/attestation.toml`.
    attestation_overrides: Vec<(String, u32)>,
}

/// One confirmation as a template sees it (ADR-034 "Confirmed" section).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AttestationRow {
    pub attester_label: String,
    pub basis: String,
    pub basis_label: String,
    pub source: String,
    pub as_of: String,
}

/// A resolved link target for a Tera template: the file basename the
/// `[[basename|display]]` wikilink points at, and the display text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Link {
    pub basename: String,
    pub display: String,
    pub folder: String,
}

/// One row of a person's or org's `## Affiliations` / `## People`
/// section (ADR-031): the other party, the role, and its window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AffiliationRow {
    pub entity: String,
    pub display: String,
    pub basename: Option<String>,
    pub title: String,
    pub kind: String,
    pub valid_from: String,
    pub valid_to: Option<String>,
    /// `"person"` when the rendered entity is the bearer (the row names
    /// the organization), `"organization"` when the rendered entity is
    /// the context (the row names the person).
    pub side: String,
    /// ADR-034: set only when the row needs the owner's eye (`stale`
    /// or `disputed`); a current or merely unconfirmed role renders as
    /// before so existing files stay byte-identical.
    pub status: Option<String>,
}

impl ProjectionRenderer {
    /// Construct a renderer that loads Tera templates from `templates_dir`
    /// (matches `*.tera` under that directory non-recursively). An empty
    /// directory is allowed — the renderer will fail on render if a
    /// referenced template isn't loaded.
    pub fn new(
        store: Arc<dyn AtomStore>,
        registry: Arc<SpecRegistry>,
        templates_dir: &Path,
    ) -> Result<Self, RenderError> {
        let glob = templates_dir.join("*.tera");
        let tera = if templates_dir.exists() {
            Tera::new(
                glob.to_str()
                    .ok_or_else(|| RenderError::Tera("invalid templates dir path".into()))?,
            )
            .map_err(|e| RenderError::Tera(e.to_string()))?
        } else {
            Tera::default()
        };
        Ok(Self {
            store,
            registry,
            index: Arc::new(InMemoryPathIndex::new()),
            tera,
            owner: None,
            attestation_overrides: Vec::new(),
        })
    }

    /// The owner's public key: attestations it signed render as "you".
    pub fn with_owner(mut self, owner: crate::atom::PublicKey) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Per-substrate `k` overrides (ADR-034; `config/attestation.toml`).
    pub fn with_attestation_overrides(mut self, overrides: Vec<(String, u32)>) -> Self {
        self.attestation_overrides = overrides;
        self
    }

    fn attester_label(&self, key: &crate::atom::PublicKey) -> String {
        if self.owner.as_ref() == Some(key) {
            "you".to_string()
        } else {
            let s = key.to_multibase();
            s.chars().take(9).collect()
        }
    }

    /// Use a shared path-to-entity index (the daemon passes its SQLite
    /// store, which implements `PathIndex`). Without this the renderer
    /// uses a private in-memory index, which is enough for tests and
    /// for pre-ADR-030 entities whose ids equal their basenames.
    pub fn with_path_index(mut self, index: Arc<dyn PathIndex>) -> Self {
        self.index = index;
        self
    }

    pub fn path_index(&self) -> Arc<dyn PathIndex> {
        Arc::clone(&self.index)
    }

    /// Snapshot of the registry's family table.
    pub fn family_table(&self) -> FamilyTable {
        FamilyTable::from_registry(&self.registry)
    }

    /// Resolve a file basename in a family to an entity id: through the
    /// index when a row exists, otherwise the basename itself (the
    /// slug-form id of a pre-ADR-030 entity).
    pub fn resolve_basename(
        &self,
        family: &PathFamily,
        basename: &str,
    ) -> Result<EntityId, RenderError> {
        Ok(self
            .index
            .resolve(&family.folder, basename)
            .map_err(|e| RenderError::Index(e.to_string()))?
            .unwrap_or_else(|| EntityId::new(basename)))
    }

    /// Basename for an entity in a family: the index row when present,
    /// otherwise the entity id itself.
    pub fn basename_for(
        &self,
        family: &PathFamily,
        entity: &EntityId,
    ) -> Result<String, RenderError> {
        Ok(self
            .index
            .basename_for(&family.folder, entity)
            .map_err(|e| RenderError::Index(e.to_string()))?
            .unwrap_or_else(|| entity.as_str().to_string()))
    }

    /// Register a single template at runtime. Useful for tests that don't
    /// want to write files; production loads from disk via [`new`].
    pub fn add_raw_template(&mut self, name: &str, content: &str) -> Result<(), RenderError> {
        self.tera
            .add_raw_template(name, content)
            .map_err(|e| RenderError::Tera(e.to_string()))
    }

    pub fn render(&self, req: &ProjectionRequest) -> Result<ProjectionResponse, RenderError> {
        let table = self.family_table();
        match super::path::parse(&req.path, &table)? {
            ParsedPath::SingleEntity { family, basename } => {
                let entity = self.resolve_basename(&family, &basename)?;
                self.render_single_entity(req, family, &entity)
            }
            ParsedPath::Recent { family } => self.render_recent(req, family),
            ParsedPath::AlphabeticalLetter { family, letter } => {
                self.render_alphabetical(req, family, &letter)
            }
            ParsedPath::Unsupported { family: _, raw } => Err(RenderError::UnsupportedSubpath(raw)),
        }
    }

    /// Link target for a value that may be an entity id in any family:
    /// the first family whose index knows the id, or whose store holds
    /// a head atom for `(id, family.predicate)`. `None` means the value
    /// is not a known entity and templates print it as plain text.
    fn link_for(&self, value: &str, as_of: Option<&Iso8601>) -> Result<Option<Link>, RenderError> {
        if value.is_empty() {
            return Ok(None);
        }
        let entity = EntityId::new(value);
        for family in self.family_table().all() {
            let indexed = self
                .index
                .basename_for(&family.folder, &entity)
                .map_err(|e| RenderError::Index(e.to_string()))?;
            let head = self
                .store
                .head_of_chain(&entity, &family.predicate, as_of)
                .map_err(RenderError::Store)?;
            if indexed.is_none() && head.is_none() {
                continue;
            }
            let display = head
                .as_ref()
                .and_then(|h| h.claim.get(&family.name_field))
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| value.to_string());
            let basename = indexed.unwrap_or_else(|| value.to_string());
            return Ok(Some(Link {
                basename,
                display,
                folder: family.folder.clone(),
            }));
        }
        Ok(None)
    }

    /// Resolve `{entity, display, <extra>}` objects (article mentions,
    /// event participants) into the same objects plus `basename`.
    fn resolve_refs(
        &self,
        items: Option<&serde_json::Value>,
        as_of: Option<&Iso8601>,
    ) -> Result<Vec<serde_json::Value>, RenderError> {
        let mut out = Vec::new();
        let Some(arr) = items.and_then(|v| v.as_array()) else {
            return Ok(out);
        };
        for item in arr {
            let mut obj = match item.as_object() {
                Some(o) => o.clone(),
                None => continue,
            };
            let id = obj.get("entity").and_then(|v| v.as_str()).unwrap_or("");
            let link = self.link_for(id, as_of)?;
            obj.insert(
                "basename".into(),
                match &link {
                    Some(l) => serde_json::Value::String(l.basename.clone()),
                    None => serde_json::Value::Null,
                },
            );
            if obj.get("display").and_then(|v| v.as_str()).is_none()
                && let Some(l) = &link
            {
                obj.insert(
                    "display".into(),
                    serde_json::Value::String(l.display.clone()),
                );
            }
            out.push(serde_json::Value::Object(obj));
        }
        Ok(out)
    }

    /// Walk a whole claim and resolve every entity reference in it
    /// (task_41): any JSON object carrying a string `entity` key gains
    /// `basename` (the target's projection file stem, or null when the
    /// id is not a known entity) and, when it has no `display`, the
    /// target's display name. Arrays and nested objects are walked;
    /// everything else is copied. Templates whose claims nest
    /// references at arbitrary depth (the auditor's briefing) render
    /// `[[basename|display]]` wikilinks from the result without the
    /// renderer naming any claim field.
    fn resolve_refs_deep(
        &self,
        value: &serde_json::Value,
        as_of: Option<&Iso8601>,
    ) -> Result<serde_json::Value, RenderError> {
        match value {
            serde_json::Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.resolve_refs_deep(item, as_of)?);
                }
                Ok(serde_json::Value::Array(out))
            }
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len() + 1);
                for (k, v) in map {
                    out.insert(k.clone(), self.resolve_refs_deep(v, as_of)?);
                }
                if let Some(id) = map.get("entity").and_then(|v| v.as_str()) {
                    let link = self.link_for(id, as_of)?;
                    out.insert(
                        "basename".into(),
                        match &link {
                            Some(l) => serde_json::Value::String(l.basename.clone()),
                            None => serde_json::Value::Null,
                        },
                    );
                    if out.get("display").and_then(|v| v.as_str()).is_none()
                        && let Some(l) = &link
                    {
                        out.insert(
                            "display".into(),
                            serde_json::Value::String(l.display.clone()),
                        );
                    }
                }
                Ok(serde_json::Value::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    /// Reverse lookup of `affiliation` atoms naming this entity as the
    /// bearer (`claim.person`) or the context (`claim.organization`).
    /// Returns the rows plus the contributing atom hashes. Empty when
    /// no `affiliation` spec is registered.
    fn affiliations_for(
        &self,
        entity: &EntityId,
        merged_losers: &[EntityId],
        as_of: Option<&Iso8601>,
    ) -> Result<(Vec<AffiliationRow>, Vec<Multihash>), RenderError> {
        if self.registry.get(AFFILIATION_PREDICATE).is_none() {
            return Ok((Vec::new(), Vec::new()));
        }
        let predicate = PredicateName::new(AFFILIATION_PREDICATE);
        let atoms = self
            .store
            .list_by_predicate(&predicate, None, 10_000)
            .map_err(RenderError::Store)?;
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut rows = Vec::new();
        let mut hashes = Vec::new();
        for atom in atoms {
            if !seen.insert(atom.entity.as_str().to_string()) {
                continue;
            }
            let Some(head) = self
                .store
                .head_of_chain(&atom.entity, &predicate, as_of)
                .map_err(RenderError::Store)?
            else {
                continue;
            };
            let person = head
                .claim
                .get("person")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let org = head
                .claim
                .get("organization")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // A merge loser's affiliations belong to the winner (ADR-030).
            let is_self =
                |id: &str| id == entity.as_str() || merged_losers.iter().any(|l| l.as_str() == id);
            let (side, other) = if is_self(person) {
                ("person", org)
            } else if is_self(org) {
                ("organization", person)
            } else {
                continue;
            };
            let link = self.link_for(other, as_of)?;
            let policy = crate::attestation::policy_for(
                self.registry.get(AFFILIATION_PREDICATE).as_ref(),
                &self.attestation_overrides,
            );
            let now_for_status = as_of.cloned().unwrap_or_else(current_iso8601);
            let (report, atts) =
                crate::attestation::report_for_head(&*self.store, &head, &policy, &now_for_status)
                    .map_err(RenderError::Store)?;
            let status = match report.status {
                crate::attestation::Status::Stale | crate::attestation::Status::Disputed => {
                    Some(report.status.as_str().to_string())
                }
                _ => None,
            };
            hashes.extend(atts.iter().filter_map(|a| a.hash.clone()));
            rows.push(AffiliationRow {
                entity: other.to_string(),
                display: link
                    .as_ref()
                    .map(|l| l.display.clone())
                    .unwrap_or_else(|| other.to_string()),
                basename: link.map(|l| l.basename),
                title: head
                    .claim
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                kind: head
                    .claim
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                valid_from: head.valid_from.as_str().to_string(),
                valid_to: head.valid_to.as_ref().map(|t| t.as_str().to_string()),
                side: side.to_string(),
                status,
            });
            hashes.push(
                head.content_hash()
                    .map_err(|e| RenderError::Serialization(e.to_string()))?,
            );
        }
        // Current roles first, then by start date, then by display for
        // deterministic output.
        rows.sort_by(|a, b| {
            a.valid_to
                .is_some()
                .cmp(&b.valid_to.is_some())
                .then_with(|| b.valid_from.cmp(&a.valid_from))
                .then_with(|| a.display.cmp(&b.display))
        });
        Ok((rows, hashes))
    }

    /// The `entity.same_as` head for an entity, if it has been merged.
    fn merged_into(
        &self,
        entity: &EntityId,
        as_of: Option<&Iso8601>,
    ) -> Result<Option<AtomEnvelope>, RenderError> {
        if self.registry.get(SAME_AS_PREDICATE).is_none() {
            return Ok(None);
        }
        let head = self
            .store
            .head_of_chain(entity, &PredicateName::new(SAME_AS_PREDICATE), as_of)
            .map_err(RenderError::Store)?;
        // An undone merge (the same_as head carries a `valid_to`) no
        // longer redirects; the loser renders on its own again. Mirrors
        // `AtomStore::same_as_target`.
        Ok(head.filter(|h| h.valid_to.is_none()))
    }

    fn render_single_entity(
        &self,
        req: &ProjectionRequest,
        family: PathFamily,
        entity: &crate::atom::EntityId,
    ) -> Result<ProjectionResponse, RenderError> {
        let predicate = family.primary_predicate();
        let spec = self
            .registry
            .get(predicate.as_str())
            .ok_or_else(|| RenderError::UnknownPredicate(predicate.as_str().into()))?;

        // Merged entity (ADR-030): the loser's projection is a one-line
        // redirect; its atoms stay in place and render under the winner.
        if let Some(same_as) = self.merged_into(entity, req.as_of.as_ref())? {
            let target = same_as
                .claim
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let link = self.link_for(target, req.as_of.as_ref())?;
            let (basename, display) = match link {
                Some(l) => (l.basename, l.display),
                None => (target.to_string(), target.to_string()),
            };
            let markdown = format!("Merged into [[{basename}|{display}]]\n");
            let render_hash = Multihash::blake3_of(markdown.as_bytes());
            let hash = same_as
                .content_hash()
                .map_err(|e| RenderError::Serialization(e.to_string()))?;
            return Ok(ProjectionResponse {
                markdown,
                render_hash,
                source_atoms: vec![hash],
                reverse_map: vec![],
            });
        }

        let head = self
            .store
            .head_of_chain(entity, &predicate, req.as_of.as_ref())
            .map_err(RenderError::Store)?
            .ok_or_else(|| RenderError::AtomNotFound {
                entity: entity.as_str().into(),
                predicate: predicate.as_str().into(),
            })?;

        // Capability check.
        let now = req.as_of.clone().unwrap_or_else(current_iso8601);
        let target = Target {
            predicate: predicate.clone(),
            entity: entity.clone(),
            classification: Some(head.classification.clone()),
            tier: None,
        };
        let decision = capability::evaluate(&*self.store, &req.agent, Action::Read, &target, &now)
            .map_err(RenderError::Eval)?;
        match decision {
            Decision::Allow { .. } => {}
            Decision::Deny { reason } => return Err(RenderError::CapabilityDenied(reason)),
        }

        // Render via Tera. The context carries the raw claim plus derived
        // link objects (ADR-028 wikilinks) and the affiliation rows
        // (ADR-031 reverse lookup). Templates that ignore the extras
        // render byte-identically to before task_38.
        let as_of = req.as_of.as_ref();

        // Merge winners (ADR-029, ADR-030): a loser's atoms stay in place
        // and render under the winner. Fold each loser's head of this
        // predicate into the context: `merged_from` names them, their
        // additive arrays join the rendered claim (winner's items first,
        // then the loser's not already present), and their head hashes
        // count as sources so a change on either side re-renders.
        let mut claim = head.claim.clone();
        let mut merged_from: Vec<serde_json::Value> = Vec::new();
        let mut loser_hashes: Vec<Multihash> = Vec::new();
        let mut loser_ids: Vec<EntityId> = Vec::new();
        if self.registry.get(SAME_AS_PREDICATE).is_some() {
            let name_field = spec
                .path
                .as_ref()
                .map(|p| p.name_field.clone())
                .unwrap_or_else(|| "display_name".to_string());
            for loser in self
                .store
                .same_as_losers(entity, as_of)
                .map_err(RenderError::Store)?
            {
                let Some(loser_head) = self
                    .store
                    .head_of_chain(&loser, &predicate, as_of)
                    .map_err(RenderError::Store)?
                else {
                    continue;
                };
                let display = loser_head
                    .claim
                    .get(&name_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or(loser.as_str())
                    .to_string();
                merged_from.push(serde_json::json!({
                    "entity": loser.as_str(),
                    "display": display,
                    "basename": self.basename_for(&family, &loser)?,
                }));
                loser_ids.push(loser.clone());
                if let (Some(dst), Some(src)) =
                    (claim.as_object_mut(), loser_head.claim.as_object())
                {
                    for (key, value) in src {
                        let Some(items) = value.as_array() else {
                            continue;
                        };
                        let entry = dst
                            .entry(key.clone())
                            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                        if let Some(existing) = entry.as_array_mut() {
                            for item in items {
                                if !existing.contains(item) {
                                    existing.push(item.clone());
                                }
                            }
                        }
                    }
                }
                loser_hashes.push(
                    loser_head
                        .content_hash()
                        .map_err(|e| RenderError::Serialization(e.to_string()))?,
                );
            }
        }

        let mut ctx = Context::new();
        ctx.insert("entity", entity.as_str());
        ctx.insert("claim", &claim);
        ctx.insert("merged_from", &merged_from);
        ctx.insert("classification", head.classification.as_str());
        ctx.insert("basename", &self.basename_for(&family, entity)?);
        for (field, key) in [
            ("organization", "organization_link"),
            ("person", "person_link"),
            ("target", "target_link"),
            ("source", "source_link"),
        ] {
            let value = head.claim.get(field).and_then(|v| v.as_str()).unwrap_or("");
            ctx.insert(key, &self.link_for(value, as_of)?);
        }
        ctx.insert(
            "mentions_resolved",
            &self.resolve_refs(head.claim.get("mentions"), as_of)?,
        );
        ctx.insert(
            "participants_resolved",
            &self.resolve_refs(head.claim.get("participants"), as_of)?,
        );
        ctx.insert("claim_resolved", &self.resolve_refs_deep(&claim, as_of)?);
        let (affiliations, affiliation_hashes) =
            self.affiliations_for(entity, &loser_ids, as_of)?;
        ctx.insert("affiliations", &affiliations);

        // ADR-034: the fact's derived standing. Computed, never stored.
        // `status_visible` keeps files without any confirmation (and
        // without a stale or disputed marker) byte-identical to their
        // pre-ADR-034 render; every atom the owner accepts carries one
        // attestation, so in use the line shows.
        let policy = crate::attestation::policy_for(Some(&spec), &self.attestation_overrides);
        let (report, atts) =
            crate::attestation::report_for_head(&*self.store, &head, &policy, &now)
                .map_err(RenderError::Store)?;
        let attestation_hashes: Vec<Multihash> =
            atts.iter().filter_map(|a| a.hash.clone()).collect();
        let status_visible = !report.confirmed_by.is_empty()
            || matches!(
                report.status,
                crate::attestation::Status::Stale
                    | crate::attestation::Status::Disputed
                    | crate::attestation::Status::Deprecated
            );
        let as_of_line = report.as_of.as_ref().map(|d| {
            let who: Vec<String> = report
                .confirmed_by
                .iter()
                .map(|c| format!("{} ({})", self.attester_label(&c.attester), c.basis.label()))
                .collect();
            format!("as of {d}, confirmed by {}", who.join(", "))
        });
        let status_marker: Option<String> = match (report.status, report.since.as_deref()) {
            (crate::attestation::Status::Unconfirmed, Some(d)) => {
                Some(format!("[unconfirmed since {d}]"))
            }
            (crate::attestation::Status::Stale, Some(d)) => Some(format!("[stale since {d}]")),
            (crate::attestation::Status::Disputed, _) => Some("[disputed]".to_string()),
            (crate::attestation::Status::Deprecated, _) => Some("[deprecated]".to_string()),
            _ => None,
        };
        let attestation_rows: Vec<AttestationRow> = report
            .confirmed_by
            .iter()
            .map(|c| AttestationRow {
                attester_label: self.attester_label(&c.attester),
                basis: c.basis.as_str().to_string(),
                basis_label: c.basis.label().to_string(),
                source: c.source.clone(),
                as_of: c.as_of.clone(),
            })
            .collect();
        ctx.insert("status", report.status.as_str());
        ctx.insert("status_report", &report);
        ctx.insert("status_visible", &status_visible);
        ctx.insert("as_of_line", &as_of_line);
        ctx.insert("status_marker", &status_marker);
        ctx.insert("attestations", &attestation_rows);

        let markdown = self
            .tera
            .render(&spec.rendering.template, &ctx)
            .map_err(|e| {
                RenderError::Tera(format!("template `{}`: {e}", spec.rendering.template))
            })?;

        let head_hash = head
            .content_hash()
            .map_err(|e| RenderError::Serialization(e.to_string()))?;
        let reverse_map = annotations_for(&head_hash, &spec.reverse_map);
        let render_hash = Multihash::blake3_of(markdown.as_bytes());

        let mut source_atoms = vec![head_hash];
        source_atoms.extend(affiliation_hashes);
        source_atoms.extend(loser_hashes);
        source_atoms.extend(attestation_hashes);
        Ok(ProjectionResponse {
            markdown,
            render_hash,
            source_atoms,
            reverse_map,
        })
    }

    fn render_recent(
        &self,
        req: &ProjectionRequest,
        family: PathFamily,
    ) -> Result<ProjectionResponse, RenderError> {
        let predicate = family.primary_predicate();
        // List the most recent N atoms for the family's predicate. We collect
        // up to 100; downstream paginators can tune this once they exist.
        let atoms = self
            .store
            .list_by_predicate(&predicate, None, 100)
            .map_err(RenderError::Store)?;

        let mut entries: Vec<(String, Multihash)> = Vec::new();
        let mut source_atoms: Vec<Multihash> = Vec::new();
        let now = req.as_of.clone().unwrap_or_else(current_iso8601);

        // Deduplicate by entity — only the latest atom per entity contributes
        // a listing entry (list_by_predicate orders tx_time DESC so the first
        // occurrence wins).
        let mut seen_entities: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for atom in atoms {
            let ent_str = atom.entity.as_str().to_owned();
            if !seen_entities.insert(ent_str.clone()) {
                continue;
            }
            let target = Target {
                predicate: predicate.clone(),
                entity: atom.entity.clone(),
                classification: Some(atom.classification.clone()),
                tier: None,
            };
            let allowed = matches!(
                capability::evaluate(&*self.store, &req.agent, Action::Read, &target, &now)
                    .map_err(RenderError::Eval)?,
                Decision::Allow { .. }
            );
            if !allowed {
                continue;
            }
            let hash = atom
                .content_hash()
                .map_err(|e| RenderError::Serialization(e.to_string()))?;
            let basename = self.basename_for(&family, &atom.entity)?;
            entries.push((basename, hash.clone()));
            source_atoms.push(hash);
        }

        let markdown = render_listing(&family, "recent", &entries);
        let render_hash = Multihash::blake3_of(markdown.as_bytes());
        Ok(ProjectionResponse {
            markdown,
            render_hash,
            source_atoms,
            reverse_map: vec![],
        })
    }

    fn render_alphabetical(
        &self,
        req: &ProjectionRequest,
        family: PathFamily,
        letter: &str,
    ) -> Result<ProjectionResponse, RenderError> {
        let predicate = family.primary_predicate();
        // Pull a generous slice; for MVP scale a single window is sufficient.
        let atoms = self
            .store
            .list_by_predicate(&predicate, None, 1000)
            .map_err(RenderError::Store)?;

        let now = req.as_of.clone().unwrap_or_else(current_iso8601);

        let mut entries: Vec<(String, Multihash)> = Vec::new();
        let mut source_atoms: Vec<Multihash> = Vec::new();
        let mut seen_entities: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();

        let letter_upper = letter.to_uppercase();
        for atom in atoms {
            let ent_str = atom.entity.as_str().to_owned();
            // First letter of the file basename, uppercased, matches the
            // requested letter. (For pre-ADR-030 entities the basename
            // is the entity id.)
            let basename = self.basename_for(&family, &atom.entity)?;
            let first = basename
                .chars()
                .next()
                .map(|c| c.to_uppercase().next().unwrap_or(c))
                .unwrap_or('?');
            if first.to_string() != letter_upper {
                continue;
            }
            if !seen_entities.insert(ent_str.clone()) {
                continue;
            }
            let target = Target {
                predicate: predicate.clone(),
                entity: atom.entity.clone(),
                classification: Some(atom.classification.clone()),
                tier: None,
            };
            let allowed = matches!(
                capability::evaluate(&*self.store, &req.agent, Action::Read, &target, &now)
                    .map_err(RenderError::Eval)?,
                Decision::Allow { .. }
            );
            if !allowed {
                continue;
            }
            let hash = atom
                .content_hash()
                .map_err(|e| RenderError::Serialization(e.to_string()))?;
            entries.push((basename, hash.clone()));
            source_atoms.push(hash);
        }
        // Alphabetical-by-basename sort for deterministic output.
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let markdown = render_listing(&family, &format!("by-name / {letter_upper}"), &entries);
        let render_hash = Multihash::blake3_of(markdown.as_bytes());
        Ok(ProjectionResponse {
            markdown,
            render_hash,
            source_atoms,
            reverse_map: vec![],
        })
    }
}

fn annotations_for(source_atom: &Multihash, rules: &[ReverseMapRule]) -> Vec<ReverseMapAnnotation> {
    rules
        .iter()
        .map(|r| ReverseMapAnnotation {
            output_element: r.output.clone(),
            source_atom: source_atom.clone(),
            source_field: r.atom_field.clone(),
            edit_kind: r.edit_kind,
        })
        .collect()
}

fn render_listing(family: &PathFamily, sub_label: &str, entries: &[(String, Multihash)]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}: {sub_label}\n\n", family.as_str()));
    if entries.is_empty() {
        out.push_str("_(no entries)_\n");
        return out;
    }
    for (ent, _) in entries {
        out.push_str(&format!("- [{ent}]({ent}.md)\n"));
    }
    out
}

fn current_iso8601() -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let now = time::OffsetDateTime::now_utc();
    let s = now.format(&Fmt::DEFAULT).unwrap_or_else(|_| {
        // Extremely unlikely; fall back to a safe placeholder. Callers can
        // override by passing `as_of` explicitly.
        "1970-01-01T00:00:00Z".into()
    });
    Iso8601::new(s).expect("formatted ISO8601 must parse")
}
