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

use crate::atom::Iso8601;
use crate::capability::{self, Action, Decision, Target};
use crate::multihash::Multihash;
use crate::predicate::{ReverseMapRule, SpecRegistry};
use crate::store::AtomStore;

use super::path::{ParsedPath, PathFamily};
use super::{ProjectionRequest, ProjectionResponse, RenderError, ReverseMapAnnotation};

/// Loaded projection renderer. Holds an `Arc` to the store and registry
/// so multiple renderer instances can share state (the daemon will use one).
pub struct ProjectionRenderer {
    store: Arc<dyn AtomStore>,
    registry: Arc<SpecRegistry>,
    tera: Tera,
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
            tera,
        })
    }

    /// Register a single template at runtime. Useful for tests that don't
    /// want to write files; production loads from disk via [`new`].
    pub fn add_raw_template(&mut self, name: &str, content: &str) -> Result<(), RenderError> {
        self.tera
            .add_raw_template(name, content)
            .map_err(|e| RenderError::Tera(e.to_string()))
    }

    pub fn render(&self, req: &ProjectionRequest) -> Result<ProjectionResponse, RenderError> {
        match super::path::parse(&req.path)? {
            ParsedPath::SingleEntity { family, entity } => {
                self.render_single_entity(req, family, &entity)
            }
            ParsedPath::Recent { family } => self.render_recent(req, family),
            ParsedPath::AlphabeticalLetter { family, letter } => {
                self.render_alphabetical(req, family, &letter)
            }
            ParsedPath::Unsupported { family: _, raw } => Err(RenderError::UnsupportedSubpath(raw)),
        }
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

        // Render via Tera.
        let mut ctx = Context::new();
        ctx.insert("entity", entity.as_str());
        ctx.insert("claim", &head.claim);
        ctx.insert("classification", head.classification.as_str());
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

        Ok(ProjectionResponse {
            markdown,
            render_hash,
            source_atoms: vec![head_hash],
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
            entries.push((ent_str, hash.clone()));
            source_atoms.push(hash);
        }

        let markdown = render_listing(family, "recent", &entries);
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
            // First letter of entity id, uppercased, matches the requested letter.
            let first = ent_str
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
            entries.push((ent_str, hash.clone()));
            source_atoms.push(hash);
        }
        // Alphabetical-by-entity-id sort for deterministic output.
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let markdown = render_listing(family, &format!("by-name / {letter_upper}"), &entries);
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

fn render_listing(family: PathFamily, sub_label: &str, entries: &[(String, Multihash)]) -> String {
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
