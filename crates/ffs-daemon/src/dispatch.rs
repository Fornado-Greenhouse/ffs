//! Method dispatcher. Maps JSON-RPC method names to handler functions
//! that consult `ffs-core` modules and produce results / errors.
//!
//! Every state-touching method evaluates capabilities (per ARCHITECTURE.md
//! AARM mapping) before returning data. The daemon's "owner" public key
//! is the identity used for capability checks at MVP — future tasks
//! (MCP server, federation transport) will pass per-connection identities.

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use ed25519_dalek::{Signer, SigningKey};

use ffs_core::capability::{self, Decision, EvalError, Target};
use ffs_core::federation_peers::{FederationPeer, FederationPeerStore};
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::quarantine::{IngestQuarantine, Proposal, SubmissionStatus};
use ffs_core::store::AtomStore;
use ffs_core::working_set::WorkingSetStore;
use ffs_core::{
    AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier,
    predicate::SpecRegistry,
};

use ffs_federation::client::FederationClient;
use ffs_federation::handshake::rotation_signing_bytes;
use ffs_federation::handshake::{HANDSHAKE_PROTOCOL_VERSION, HandshakeRequest, RotateRequest};
use ffs_federation::mount::PeerMountStore;
use ffs_federation::scheduler::tick_once_for_peer;

use crate::api::*;
use crate::notify::EventPublisher;

pub struct Dispatcher {
    pub store: Arc<dyn AtomStore>,
    pub registry: Arc<SpecRegistry>,
    pub renderer: Arc<ProjectionRenderer>,
    pub notifier: Arc<EventPublisher>,
    /// Identity used for capability checks on requests arriving via the
    /// local UDS / named pipe. Future tasks (MCP server, federation pull)
    /// will route requests with their own per-call identity.
    pub owner: PublicKey,
    /// Ingest quarantine: stores submitted content and the scribe's
    /// extracted proposals. Wired by the daemon binary at startup.
    pub quarantine: Arc<dyn IngestQuarantine>,
    /// Scribe extractor hook: when set, `ingest.submit` invokes it on
    /// each submission to populate the proposals. The hook is async
    /// and owns its own concurrency strategy (the production wiring
    /// dispatches via `ffs-skills-host`; tests inject a stub).
    pub scribe: Option<Arc<dyn ScribeExtractor>>,
    /// Working-set state: which projections the librarian has
    /// materialized on disk, their render hashes, recency, and
    /// pin bits. The librarian skill (task 12) drives drift
    /// detection and eviction through this.
    pub working_set: Arc<dyn WorkingSetStore>,
    /// Signing key the daemon uses when authoring atoms on behalf of
    /// long-running skills (auditor's daily summary, future
    /// scribe-promoted atoms). When `None`, methods that require
    /// signing return `ERR_NOT_IMPLEMENTED` so a dispatcher without a
    /// configured key is still usable for read-only flows.
    pub signing_key: Option<Arc<SigningKey>>,
    /// Federation peer state (pinned fingerprints, capability hashes,
    /// pull watermarks). Populated by `bridge.establish` and read
    /// by `federation.peer.list`.
    pub federation_peers: Arc<dyn FederationPeerStore>,
    /// Federation transport client. `None` disables outbound bridge
    /// calls (the daemon can still serve incoming federation
    /// requests but cannot initiate handshakes). The production
    /// reqwest+rustls binding plugs in here; tests inject
    /// `InMemoryFederationClient`.
    pub federation_client: Option<Arc<dyn FederationClient>>,
    /// This substrate's TLS certificate fingerprint. Sent to peers
    /// so they can pin us at the TLS layer. `None` until the
    /// daemon has generated its cert at startup.
    pub our_cert_fingerprint: Option<Multihash>,
    /// Per-peer mount tracking: which atoms came from which peer.
    /// Used by federation.pull + the `from/<peer>/` projection to
    /// attribute atoms back to their source, and by revocation to
    /// drop a peer's mount when their capability is rescinded.
    pub peer_mounts: Arc<dyn PeerMountStore>,
    /// The substrate's data directory, when known. Read by
    /// `health.summary` and `courier.status` for the courier's
    /// last-run file (task_40). `None` in tests that never touch it.
    pub data_dir: Option<std::path::PathBuf>,
    /// Hook for invoking daemon-hosted skills by name (`courier.run`).
    /// Production wires the skills host; tests inject a stub.
    pub skill_invoker: Option<Arc<dyn SkillInvoker>>,
}

/// Invoke a daemon-hosted skill by bundle name with a JSON input and
/// return its result verbatim. The daemon binary implements this over
/// `ffs-skills-host`; tests use an in-process stub.
#[async_trait::async_trait]
pub trait SkillInvoker: Send + Sync {
    async fn invoke(&self, skill: &str, input: Value) -> Result<Value, String>;
}

/// Abstraction over the scribe extractor. The daemon binary wires
/// this to a `ffs-skills-host::SkillProcess`, but the trait lets tests
/// inject a synchronous in-process stub without standing up a Python
/// subprocess.
#[async_trait::async_trait]
pub trait ScribeExtractor: Send + Sync {
    async fn extract(
        &self,
        source_uri: &str,
        content: &[u8],
    ) -> Result<Vec<Proposal>, ScribeExtractError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ScribeExtractError {
    #[error("scribe failed: {0}")]
    Failed(String),
}

/// One `entity.search` result row while ranking: the best tier seen
/// for an entity and the hit it produced.
struct SearchRow {
    hit: EntitySearchHit,
    tier: u8,
}

/// The string and string-array property names a claim schema declares
/// (`entity.search` v2 matches across exactly these; no field names
/// live in code).
fn schema_string_fields(schema: &Value) -> (Vec<String>, Vec<String>) {
    let mut strings = Vec::new();
    let mut arrays = Vec::new();
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (name, spec) in props {
            match spec.get("type").and_then(|t| t.as_str()) {
                Some("string") => strings.push(name.clone()),
                Some("array")
                    if spec
                        .get("items")
                        .and_then(|i| i.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("string") =>
                {
                    arrays.push(name.clone())
                }
                _ => {}
            }
        }
    }
    (strings, arrays)
}

impl Dispatcher {
    pub async fn handle(&self, req: ApiRequest) -> ApiResponse {
        let id = req.id.clone();
        if req.jsonrpc != "2.0" {
            return ApiResponse::error(
                id,
                ApiError {
                    code: ERR_INVALID_REQUEST,
                    message: format!("jsonrpc must be \"2.0\", got {:?}", req.jsonrpc),
                    data: None,
                },
            );
        }
        let method = req.method.clone();
        tracing::debug!(method = %method, "dispatch");

        let result = match method.as_str() {
            "atom.get" => self.atom_get(req.params).await,
            "atom.list" => self.atom_list(req.params).await,
            "projection.render" => self.projection_render(req.params).await,
            "path.list" => self.path_list(req.params).await,
            "path.families" => self.path_families().await,
            "ingest.submit" => self.ingest_submit(req.params).await,
            "fastpath.submit" => stub_not_implemented("task_09"),
            "capability.evaluate" => self.capability_evaluate(req.params).await,
            "federation.peer.add" => self.federation_peer_add(req.params).await,
            "federation.peer.list" => self.federation_peer_list().await,
            "bridge.establish" => self.bridge_establish(req.params).await,
            "bridge.rotate" => self.bridge_rotate(req.params).await,
            "federation.pull" => self.federation_pull(req.params).await,
            "predicate.inspect" => self.predicate_inspect(req.params).await,
            "health.summary" => self.health_summary().await,
            "working_set.list" => self.working_set_list().await,
            "working_set.touch" => self.working_set_touch(req.params).await,
            "working_set.pin" => self.working_set_pin(req.params).await,
            "working_set.materialize" => self.working_set_materialize(req.params).await,
            "working_set.detect_drift" => self.working_set_detect_drift().await,
            "working_set.refresh_drifted" => self.working_set_refresh_drifted().await,
            "working_set.evict_to_cap" => self.working_set_evict_to_cap(req.params).await,
            "audit.publish_summary" => self.audit_publish_summary(req.params).await,
            "audit.query" => self.audit_query(req.params).await,
            "ingest.list_pending" => self.ingest_list_pending().await,
            "ingest.accept" => self.ingest_accept(req.params).await,
            "ingest.reject" => self.ingest_reject(req.params).await,
            "entity.search" => self.entity_search(req.params).await,
            "courier.run" => self.courier_run(req.params).await,
            "courier.status" => self.courier_status().await,
            other => Err(ApiError {
                code: ERR_METHOD_NOT_FOUND,
                message: format!("unknown method: {other}"),
                data: None,
            }),
        };

        match result {
            Ok(v) => ApiResponse::success(id, v),
            Err(e) => ApiResponse::error(id, e),
        }
    }

    // ---- handlers ----

    async fn atom_get(&self, params: Value) -> Result<Value, ApiError> {
        let p: AtomGetParams = parse_params(params)?;
        let env = self
            .store
            .get(&p.hash)
            .map_err(store_err)?
            .ok_or_else(|| ApiError {
                code: ERR_NOT_FOUND,
                message: format!("atom not found: {}", p.hash.to_multibase()),
                data: None,
            })?;

        let target = Target {
            predicate: env.predicate.clone(),
            entity: env.entity.clone(),
            classification: Some(env.classification.clone()),
            tier: None,
        };
        let now = current_iso8601();
        let decision = capability::evaluate(
            &*self.store,
            &self.owner,
            capability::Action::Read,
            &target,
            &now,
        )
        .map_err(eval_err)?;
        if let Decision::Deny { reason } = decision {
            return Err(capability_denied(&reason));
        }
        to_value(&env)
    }

    async fn atom_list(&self, params: Value) -> Result<Value, ApiError> {
        let p: AtomListParams = parse_params(params)?;
        let entity = p.entity.ok_or_else(|| ApiError {
            code: ERR_INVALID_PARAMS,
            message: "atom.list requires `entity` (entity-less listing not in MVP)".into(),
            data: None,
        })?;
        let atoms = self
            .store
            .list_by_entity(&entity, p.predicate.as_ref(), p.as_of.as_ref())
            .map_err(store_err)?;

        let now = current_iso8601();
        // Capability-filter the returned list.
        let mut allowed: Vec<_> = Vec::with_capacity(atoms.len());
        for env in atoms {
            let target = Target {
                predicate: env.predicate.clone(),
                entity: env.entity.clone(),
                classification: Some(env.classification.clone()),
                tier: None,
            };
            let decision = capability::evaluate(
                &*self.store,
                &self.owner,
                capability::Action::Read,
                &target,
                &now,
            )
            .map_err(eval_err)?;
            if matches!(decision, Decision::Allow { .. }) {
                allowed.push(env);
            }
        }
        to_value(&allowed)
    }

    async fn projection_render(&self, params: Value) -> Result<Value, ApiError> {
        let p: ProjectionRenderParams = parse_params(params)?;
        let req = ProjectionRequest {
            path: p.path,
            as_of: p.as_of,
            agent: self.owner.clone(),
        };
        let resp = self.renderer.render(&req).map_err(render_err)?;
        to_value(&resp)
    }

    async fn path_list(&self, params: Value) -> Result<Value, ApiError> {
        // For MVP, path.list is implemented as a projection render of the listing
        // form (recent / by-name letter). Pagination is a Phase 2 refinement.
        let p: PathListParams = parse_params(params)?;
        let req = ProjectionRequest {
            path: p.path,
            as_of: None,
            agent: self.owner.clone(),
        };
        let resp = self.renderer.render(&req).map_err(render_err)?;
        to_value(&resp)
    }

    /// The registry's family table (ADR-028): one row per spec that
    /// declares `[path]`. Consumers (the Obsidian plugin, the fast-path
    /// watcher through its own registry) enumerate folders from this
    /// instead of a hardcoded list.
    async fn path_families(&self) -> Result<Value, ApiError> {
        to_value(&self.registry.families())
    }

    async fn ingest_submit(&self, params: Value) -> Result<Value, ApiError> {
        let p: IngestSubmitParams = parse_params(params)?;

        // Capability check: the caller must hold a `Write` capability
        // for the scribe's target predicate space. Per ADR-013, the
        // quarantine is a `note`-scoped operation at the boundary —
        // the actual atom-level capability check fires when the user
        // accepts a proposal. Use `note` as the target predicate so
        // the check is meaningful for the MVP: any agent that can
        // create notes can submit raw content for scribing.
        let now = current_iso8601();
        let target = Target {
            predicate: PredicateName::new("note"),
            entity: EntityId::new("ingest"),
            classification: None,
            tier: None,
        };
        let decision = capability::evaluate(
            &*self.store,
            &self.owner,
            capability::Action::Write,
            &target,
            &now,
        )
        .map_err(eval_err)?;
        if let Decision::Deny { reason } = decision {
            return Err(capability_denied(&reason));
        }

        let content_bytes = p.content.into_bytes();
        let id = self
            .quarantine
            .submit(p.source_uri.clone(), content_bytes.clone())
            .await
            .map_err(quarantine_err)?;

        // Fire scribe extraction in the background so `ingest.submit`
        // returns immediately with the submission id. The user reads
        // proposals via `health.summary` / the daily summary panel.
        if let Some(scribe) = self.scribe.clone() {
            let quarantine = self.quarantine.clone();
            let submission_id = id.clone();
            let source_uri = p.source_uri;
            tokio::spawn(async move {
                match scribe.extract(&source_uri, &content_bytes).await {
                    Ok(proposals) => {
                        if let Err(e) = quarantine.complete(&submission_id, proposals).await {
                            tracing::warn!(error = %e, id = %submission_id, "quarantine_complete_failed");
                        }
                    }
                    Err(e) => {
                        if let Err(e2) = quarantine
                            .fail(&submission_id, format!("scribe: {e}"))
                            .await
                        {
                            tracing::warn!(error = %e2, id = %submission_id, "quarantine_fail_failed");
                        }
                    }
                }
            });
        }

        to_value(&IngestSubmitResult { submission_id: id })
    }

    /// List submissions waiting for user action (status == Extracted).
    /// The daily-summary panel calls this to render the accept/reject
    /// queue.
    async fn ingest_list_pending(&self) -> Result<Value, ApiError> {
        let subs = self
            .quarantine
            .list(Some(SubmissionStatus::Extracted))
            .await;
        to_value(&subs)
    }

    /// Accept a quarantined submission's proposals: sign each as an
    /// atom with the daemon's signing key and insert into the store.
    /// Records the inserted atom hashes on the submission and flips
    /// its status to `Accepted`. Capability-checks `Write` on the
    /// owner (per the existing ingest pipeline convention).
    async fn ingest_accept(&self, params: Value) -> Result<Value, ApiError> {
        let p: IngestAcceptParams = parse_params(params)?;
        let key = self.signing_key.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "ingest.accept requires a configured daemon signing key".into(),
            data: None,
        })?;

        // Capability check on the substrate's write surface — the
        // user's daily-summary action authors atoms, so the same
        // Write capability that gates ingest.submit gates this.
        let now = current_iso8601();
        let target = Target {
            predicate: PredicateName::new("note"),
            entity: EntityId::new("ingest"),
            classification: None,
            tier: None,
        };
        let decision = capability::evaluate(
            &*self.store,
            &self.owner,
            capability::Action::Write,
            &target,
            &now,
        )
        .map_err(eval_err)?;
        if let Decision::Deny { reason } = decision {
            return Err(capability_denied(&reason));
        }

        let sub = self
            .quarantine
            .get(&p.submission_id)
            .await
            .ok_or_else(|| ApiError {
                code: ERR_NOT_FOUND,
                message: format!("submission not found: {}", p.submission_id),
                data: None,
            })?;
        if sub.status != SubmissionStatus::Extracted {
            return Err(ApiError {
                code: ERR_INVALID_PARAMS,
                message: format!(
                    "submission {} is not in Extracted state (got {:?})",
                    p.submission_id, sub.status
                ),
                data: None,
            });
        }

        let hashes = self
            .sign_and_insert_set(&sub.proposals, &p.choices, key, &now)
            .await?;

        self.quarantine
            .accept(&p.submission_id, hashes.clone())
            .await
            .map_err(quarantine_err)?;
        to_value(&serde_json::json!({"accepted_atom_hashes": hashes}))
    }

    /// Sign and insert a resolved proposal set (task_45, ADR-030,
    /// ADR-031) in dependency order:
    ///
    /// 1. `ambiguous` proposals need an owner choice (`choices`), an
    ///    entity id or `"new"`; otherwise accept refuses and lists them.
    /// 2. `existing` binds the entity (following `same_as` again, in
    ///    case a merge landed since extraction) and the new atom
    ///    supersedes that entity's head, merging additively: scalars
    ///    the head already has are kept, arrays are unioned, and the
    ///    mention text joins `aliases[]` when it differs from the head's
    ///    display (alias growth).
    /// 3. `new` mints an opaque id.
    /// 4. Cross-references are rewritten from displays to the bound ids.
    /// 5. `ends_role` supersedes the matching head affiliation with
    ///    `valid_to`; it never creates a new atom.
    /// 6. Every existing bind records a prior for its surface form;
    ///    every minted person clears its NIL sighting.
    async fn sign_and_insert_set(
        &self,
        proposals: &[Proposal],
        choices: &std::collections::HashMap<String, String>,
        key: &SigningKey,
        now: &Iso8601,
    ) -> Result<Vec<Multihash>, ApiError> {
        use ffs_core::quarantine::{CrossRef, Resolution};
        use std::collections::HashMap;

        let lref = |p: &Proposal, i: usize| p.local_ref.clone().unwrap_or_else(|| format!("#{i}"));

        // 1. Ambiguous proposals need the owner's pick.
        let unresolved: Vec<String> = proposals
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                p.resolution == Some(Resolution::Ambiguous) && !choices.contains_key(&lref(p, *i))
            })
            .map(|(i, p)| lref(p, i))
            .collect();
        if !unresolved.is_empty() {
            return Err(ApiError {
                code: ERR_INVALID_PARAMS,
                message: format!(
                    "ambiguous proposals need a choice (entity id or \"new\") before accept: {}",
                    unresolved.join(", ")
                ),
                data: Some(serde_json::json!({"ambiguous": unresolved})),
            });
        }

        // The article's published_at, the default start of any role
        // stated in it (ADR-031): "as reported" dating.
        let article_date: Option<Iso8601> = proposals
            .iter()
            .find(|p| p.predicate.as_str() == "source.article")
            .and_then(|p| p.claim.get("published_at").and_then(|v| v.as_str()))
            .and_then(crate::scribe::parse_scribe_date);

        let order = crate::resolver::dependency_order(proposals);
        let mut bound: HashMap<String, EntityId> = HashMap::new();
        let mut hashes: Vec<Multihash> = Vec::with_capacity(proposals.len());
        let affiliation = PredicateName::new("affiliation");

        for i in order {
            let proposal = &proposals[i];
            let this_ref = lref(proposal, i);
            let mut claim = proposal.claim.clone();
            // 4. Rewrite refs already bound.
            for r in &proposal.refs {
                if let Some(id) = bound.get(&r.local_ref) {
                    CrossRef::apply(&mut claim, &r.field, Value::String(id.as_str().to_string()));
                }
            }

            // 5. Role endings supersede an existing affiliation head.
            if proposal.ends_role {
                let person = claim
                    .get("person")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let org = claim
                    .get("organization")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let head = self
                    .store
                    .list_by_predicate(&affiliation, None, 10_000)
                    .map_err(store_err)?
                    .into_iter()
                    .filter(|a| {
                        a.claim.get("person").and_then(|v| v.as_str()) == person.as_deref()
                            && a.claim.get("organization").and_then(|v| v.as_str())
                                == org.as_deref()
                    })
                    .find_map(|a| {
                        self.store
                            .head_of_chain(&a.entity, &affiliation, None)
                            .ok()
                            .flatten()
                            .filter(|h| h.valid_to.is_none())
                    });
                match head {
                    Some(head) => {
                        let hash = head.content_hash().map_err(|e| ApiError {
                            code: ERR_INTERNAL,
                            message: format!("hash: {e}"),
                            data: None,
                        })?;
                        let tmpl = AtomTemplate {
                            v: 1,
                            entity: head.entity.clone(),
                            predicate: head.predicate.clone(),
                            claim: head.claim.clone(),
                            valid_from: head.valid_from.clone(),
                            valid_to: Some(
                                proposal
                                    .valid_to
                                    .clone()
                                    .or_else(|| article_date.clone())
                                    .unwrap_or_else(|| now.clone()),
                            ),
                            tx_time: now.clone(),
                            classification: head.classification.clone(),
                            supersedes: Some(hash),
                            provenance: proposal.provenance.clone(),
                        };
                        let h = self.sign_insert_publish(tmpl, key)?;
                        hashes.push(h);
                    }
                    None => tracing::warn!(
                        local_ref = %this_ref,
                        "ends_role: no current affiliation head for this person and organization; skipped"
                    ),
                }
                continue;
            }

            // 2 and 3. Decide the entity and whether this supersedes a head.
            let choice = choices.get(&this_ref).map(String::as_str);
            let resolution = match (proposal.resolution, choice) {
                (Some(Resolution::Ambiguous), Some("new")) => Resolution::New,
                (Some(Resolution::Ambiguous), Some(_)) => Resolution::Existing,
                (Some(r), _) => r,
                (None, _) => Resolution::New,
            };
            let mut supersedes: Option<Multihash> = None;
            let mut valid_from = proposal.valid_from.clone().unwrap_or_else(|| now.clone());
            let entity = match resolution {
                Resolution::Existing => {
                    let chosen = match choice {
                        Some(id) if id != "new" => EntityId::new(id),
                        _ => proposal.entity.clone().ok_or_else(|| ApiError {
                            code: ERR_INTERNAL,
                            message: format!("{this_ref}: existing resolution without an entity"),
                            data: None,
                        })?,
                    };
                    let entity = self
                        .store
                        .follow_same_as(&chosen, None)
                        .map_err(store_err)?;
                    if let Some(head) = self
                        .store
                        .head_of_chain(&entity, &proposal.predicate, None)
                        .map_err(store_err)?
                    {
                        // Additive merge + alias growth (ADR-030 § 6.2).
                        let name_field = self
                            .registry
                            .family_for_predicate(proposal.predicate.as_str())
                            .map(|f| f.name_field)
                            .unwrap_or_else(|| "display_name".into());
                        let mention = claim
                            .get(&name_field)
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let head_display = head
                            .claim
                            .get(&name_field)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        // A rename: the proposal lists the head's current
                        // name among its aliases ("formerly ..."), so its
                        // own name becomes primary and the old one an alias.
                        let is_rename = claim
                            .get("aliases")
                            .and_then(|v| v.as_array())
                            .is_some_and(|a| {
                                a.iter().any(|v| {
                                    v.as_str().is_some_and(|s| {
                                        ffs_core::resolve::normalized_name_key(s)
                                            == ffs_core::resolve::normalized_name_key(&head_display)
                                    })
                                })
                            });
                        // A record keyed by its URL or content hash is the
                        // same record re-read: its scalars refresh (a changed
                        // title supersedes), arrays still merge. Everything
                        // else merges additively and keeps the head's scalars.
                        let keyed = proposal.candidates.first().is_some_and(|c| {
                            c.matched_on
                                .iter()
                                .any(|m| m == "url" || m == "content_hash")
                        });
                        claim = if keyed {
                            merge_refresh(&head.claim, &claim)
                        } else {
                            merge_additive(&head.claim, &claim, &name_field)
                        };
                        if let Some(m) = mention {
                            if is_rename {
                                claim[name_field.as_str()] = Value::String(m.clone());
                                push_alias(&mut claim, &head_display);
                            } else if ffs_core::resolve::normalized_name_key(&m)
                                != ffs_core::resolve::normalized_name_key(&head_display)
                            {
                                push_alias(&mut claim, &m);
                            }
                            self.store
                                .record_resolution(
                                    &ffs_core::resolve::normalized_name_key(&m),
                                    &entity,
                                )
                                .map_err(store_err)?;
                        }
                        supersedes = Some(head.content_hash().map_err(|e| ApiError {
                            code: ERR_INTERNAL,
                            message: format!("hash: {e}"),
                            data: None,
                        })?);
                        valid_from = head.valid_from.clone();
                    }
                    entity
                }
                Resolution::New | Resolution::Ambiguous => {
                    let minted = EntityId::mint();
                    if crate::resolver::NIL_GATED_PREDICATES.contains(&proposal.predicate.as_str())
                        && let Some(d) = claim.get("display_name").and_then(|v| v.as_str())
                    {
                        // Best-effort: the sighting key used the article's
                        // organization context; clear both the bare and the
                        // contextual form.
                        let base = ffs_core::resolve::normalized_name_key(d);
                        let _ = self.store.clear_sighting(&format!("{base}|"));
                        if let Some(org) = claim.get("organization").and_then(|v| v.as_str()) {
                            let org_display = self
                                .registry
                                .families()
                                .into_iter()
                                .find_map(|f| {
                                    self.store
                                        .head_of_chain(
                                            &EntityId::new(org),
                                            &PredicateName::new(&f.predicate),
                                            None,
                                        )
                                        .ok()
                                        .flatten()
                                        .and_then(|h| {
                                            h.claim
                                                .get(&f.name_field)
                                                .and_then(|v| v.as_str())
                                                .map(str::to_string)
                                        })
                                })
                                .unwrap_or_else(|| org.to_string());
                            let _ = self.store.clear_sighting(&format!(
                                "{base}|{}",
                                ffs_core::resolve::normalized_name_key(&org_display)
                            ));
                        }
                    }
                    minted
                }
            };
            if proposal.predicate.as_str() == "affiliation" && proposal.valid_from.is_none() {
                valid_from = article_date.clone().unwrap_or_else(|| now.clone());
            }
            bound.insert(this_ref, entity.clone());

            let tmpl = AtomTemplate {
                v: 1,
                entity,
                predicate: proposal.predicate.clone(),
                claim,
                valid_from,
                valid_to: proposal.valid_to.clone(),
                tx_time: now.clone(),
                classification: Tier::new("existence"),
                supersedes,
                provenance: proposal.provenance.clone(),
            };
            let h = self.sign_insert_publish(tmpl, key)?;
            hashes.push(h);
        }
        Ok(hashes)
    }

    fn sign_insert_publish(
        &self,
        tmpl: AtomTemplate,
        key: &SigningKey,
    ) -> Result<Multihash, ApiError> {
        let env = tmpl.sign(key).map_err(|e| ApiError {
            code: ERR_INTERNAL,
            message: format!("sign: {e}"),
            data: None,
        })?;
        let h = self.store.insert(&env).map_err(store_err)?;
        // Publish so the working-set materializer (task_25) can
        // render the projection file to disk.
        self.notifier.publish(crate::notify::Event::AtomCommitted {
            hash: h.clone(),
            entity: env.entity.clone(),
            predicate: env.predicate.clone(),
        });
        Ok(h)
    }

    /// Reject a quarantined submission. No atoms are authored; the
    /// submission stays in the quarantine for the audit trail with
    /// status `Rejected`.
    async fn ingest_reject(&self, params: Value) -> Result<Value, ApiError> {
        let p: IngestRejectParams = parse_params(params)?;
        self.quarantine
            .reject(&p.submission_id)
            .await
            .map_err(quarantine_err)?;
        to_value(&serde_json::json!({"rejected": p.submission_id}))
    }

    /// `entity.search` v2 (task_40, ADR-030): the resolver's candidate
    /// generator exposed as an RPC. Matches the query against every
    /// string and string-array property the predicate's claim schema
    /// declares (no field names in code), plus a full-text tier over
    /// claim payloads via the store's FTS index. Hits are ranked by
    /// tier (exact canonical name, alias, other field, full text), an
    /// exact-match bonus, and the commonness prior, then newest first.
    /// `entity.same_as` chains are followed so a losing entity's name
    /// returns the winner; a `context_entity`'s `different_from`
    /// assertions exclude candidates; every hit is capability-filtered.
    async fn entity_search(&self, params: Value) -> Result<Value, ApiError> {
        let params = if params.is_null() {
            serde_json::json!({})
        } else {
            params
        };
        let p: EntitySearchParams = parse_params(params)?;
        let needle_raw = p.query.trim().to_string();
        let needle = needle_raw.to_lowercase();
        if needle.is_empty() {
            return to_value(&serde_json::json!({"results": Vec::<Value>::new()}));
        }
        let limit = p.limit.unwrap_or(50).min(1000);
        let now = current_iso8601();
        let index = self.renderer.path_index();
        let excluded: Vec<EntityId> = match p.context_entity.as_deref() {
            Some(ctx) => self
                .store
                .different_from(&EntityId::new(ctx), None)
                .map_err(store_err)?,
            None => Vec::new(),
        };
        let prior_form = ffs_core::resolve::normalized_name_key(&needle_raw);
        let priors: std::collections::HashMap<String, u32> = self
            .store
            .prior_counts(&prior_form)
            .map_err(store_err)?
            .into_iter()
            .map(|(e, c)| (e.as_str().to_string(), c))
            .collect();

        // One candidate row per (winner) entity; the best tier wins.
        let mut rows: std::collections::HashMap<String, SearchRow> =
            std::collections::HashMap::new();

        let families: Vec<ffs_core::predicate::FamilyEntry> = self
            .registry
            .families()
            .into_iter()
            .filter(|f| p.predicate.as_deref().is_none_or(|q| q == f.predicate))
            .filter(|f| p.family.as_deref().is_none_or(|q| q == f.family))
            .collect();

        for family in &families {
            let pred = PredicateName::new(&family.predicate);
            let Some(spec) = self.registry.get(&family.predicate) else {
                continue;
            };
            let (string_fields, array_fields) = schema_string_fields(&spec.claim_schema);
            let path_family = ffs_core::projection::PathFamily::from_entry(family);
            // Newest atom per entity is the head at "now".
            let mut seen_entities: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let atoms = self
                .store
                .list_by_predicate(&pred, None, 10_000)
                .map_err(store_err)?;
            for env in atoms {
                if !seen_entities.insert(env.entity.as_str().to_string()) {
                    continue;
                }
                let Some(display) = env
                    .claim
                    .get(family.name_field.as_str())
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                // Tier 0: canonical name. Tier 1: aliases. Tier 2: any
                // other schema-declared string or string-array field.
                let mut tier: Option<(u8, &str, bool)> = None;
                let dl = display.to_lowercase();
                if dl.contains(&needle) {
                    tier = Some((0, "display_name", dl == needle));
                } else if let Some(aliases) = env.claim.get("aliases").and_then(|v| v.as_array())
                    && array_fields.iter().any(|f| f == "aliases")
                {
                    let mut exact = false;
                    let hit = aliases.iter().any(|v| {
                        v.as_str().is_some_and(|s| {
                            let sl = s.to_lowercase();
                            if sl == needle {
                                exact = true;
                            }
                            sl.contains(&needle)
                        })
                    });
                    if hit {
                        tier = Some((1, "alias", exact));
                    }
                }
                if tier.is_none() {
                    let other_hit = string_fields
                        .iter()
                        .filter(|f| f.as_str() != family.name_field)
                        .any(|f| {
                            env.claim
                                .get(f.as_str())
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| s.to_lowercase().contains(&needle))
                        })
                        || array_fields
                            .iter()
                            .filter(|f| f.as_str() != "aliases")
                            .any(|f| {
                                env.claim
                                    .get(f.as_str())
                                    .and_then(|v| v.as_array())
                                    .is_some_and(|a| {
                                        a.iter().any(|v| {
                                            v.as_str()
                                                .is_some_and(|s| s.to_lowercase().contains(&needle))
                                        })
                                    })
                            });
                    if other_hit {
                        tier = Some((2, "other", false));
                    }
                }
                let Some((tier_no, matched, exact)) = tier else {
                    continue;
                };
                self.push_search_row(
                    &mut rows,
                    &env,
                    family,
                    &path_family,
                    &*index,
                    tier_no,
                    matched,
                    exact,
                    &priors,
                    &excluded,
                    &now,
                )?;
            }

            // Tier 3: full text over claim payloads (FTS5 in SQLite;
            // substring scan in the in-memory store). Plain words go
            // through as-is (FTS5 ANDs the tokens); anything with MATCH
            // syntax characters is quoted as a phrase.
            let fts_query = if needle_raw
                .chars()
                .all(|c| c.is_alphanumeric() || c.is_whitespace())
            {
                needle_raw.clone()
            } else {
                format!("\"{}\"", needle_raw.replace('"', " "))
            };
            for hash in self
                .store
                .search_fts(&fts_query, limit.saturating_mul(4).max(20))
                .map_err(store_err)?
            {
                let Some(atom) = self.store.get(&hash).map_err(store_err)? else {
                    continue;
                };
                if atom.predicate != pred {
                    continue;
                }
                let Some(head) = self
                    .store
                    .head_of_chain(&atom.entity, &pred, None)
                    .map_err(store_err)?
                else {
                    continue;
                };
                self.push_search_row(
                    &mut rows,
                    &head,
                    family,
                    &path_family,
                    &*index,
                    3,
                    "fts",
                    false,
                    &priors,
                    &excluded,
                    &now,
                )?;
            }
        }

        let mut results: Vec<(u8, EntitySearchHit)> =
            rows.into_values().map(|r| (r.tier, r.hit)).collect();
        results.sort_by(|(ta, a), (tb, b)| {
            ta.cmp(tb)
                .then_with(|| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    b.tx_time
                        .as_ref()
                        .map(|t| t.as_str().to_string())
                        .cmp(&a.tx_time.as_ref().map(|t| t.as_str().to_string()))
                })
                .then_with(|| a.entity.as_str().cmp(b.entity.as_str()))
        });
        let results: Vec<EntitySearchHit> =
            results.into_iter().map(|(_, h)| h).take(limit).collect();
        to_value(&serde_json::json!({"results": results}))
    }

    /// Fold one matching head atom into the search rows: follow
    /// `same_as` to the winner, drop excluded entities, capability-
    /// check the winner's head, compute score and path, and keep the
    /// best tier per entity.
    #[allow(clippy::too_many_arguments)]
    fn push_search_row(
        &self,
        rows: &mut std::collections::HashMap<String, SearchRow>,
        env: &ffs_core::AtomEnvelope,
        family: &ffs_core::predicate::FamilyEntry,
        path_family: &ffs_core::projection::PathFamily,
        index: &dyn ffs_core::PathIndex,
        tier: u8,
        matched: &str,
        exact: bool,
        priors: &std::collections::HashMap<String, u32>,
        excluded: &[EntityId],
        now: &Iso8601,
    ) -> Result<(), ApiError> {
        let pred = PredicateName::new(&family.predicate);
        let winner = self
            .store
            .follow_same_as(&env.entity, None)
            .map_err(store_err)?;
        if excluded.contains(&winner) {
            return Ok(());
        }
        // The winner's own head carries the display and classification
        // that the capability check and the hit must reflect.
        let head = if winner == env.entity {
            env.clone()
        } else {
            match self
                .store
                .head_of_chain(&winner, &pred, None)
                .map_err(store_err)?
            {
                Some(h) => h,
                None => env.clone(),
            }
        };
        let target = Target {
            predicate: head.predicate.clone(),
            entity: winner.clone(),
            classification: Some(head.classification.clone()),
            tier: None,
        };
        let decision = capability::evaluate(
            &*self.store,
            &self.owner,
            capability::Action::Read,
            &target,
            now,
        )
        .map_err(eval_err)?;
        if !matches!(decision, Decision::Allow { .. }) {
            return Ok(());
        }
        let key = winner.as_str().to_string();
        if let Some(existing) = rows.get(&key)
            && existing.tier <= tier
        {
            return Ok(());
        }
        let display = head
            .claim
            .get(family.name_field.as_str())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let basename = index.basename_for(&family.family, &winner).unwrap_or(None);
        let path = basename
            .as_deref()
            .and_then(|b| ffs_core::projection::path_for_basename(path_family, b));
        let base = match tier {
            0 => 100.0,
            1 => 80.0,
            2 => 40.0,
            _ => 20.0,
        };
        let prior = priors.get(&key).copied().unwrap_or(0);
        let score = base + if exact { 5.0 } else { 0.0 } + (1.0 + f64::from(prior)).ln();
        rows.insert(
            key,
            SearchRow {
                tier,
                hit: EntitySearchHit {
                    entity: winner,
                    predicate: head.predicate.clone(),
                    display_name: display,
                    basename,
                    path,
                    matched_on: vec![matched.to_string()],
                    score,
                    tx_time: Some(head.tx_time.clone()),
                },
            },
        );
        Ok(())
    }

    async fn capability_evaluate(&self, params: Value) -> Result<Value, ApiError> {
        let p: CapabilityEvaluateParams = parse_params(params)?;
        let target = Target {
            predicate: p.predicate,
            entity: p.entity,
            classification: p.classification,
            tier: p.tier,
        };
        let decision = capability::evaluate(&*self.store, &p.agent, p.action, &target, &p.as_of)
            .map_err(eval_err)?;
        let wire = match decision {
            Decision::Allow { capability } => CapabilityDecisionWire {
                allowed: true,
                capability: Some(capability),
                reason: None,
            },
            Decision::Deny { reason } => CapabilityDecisionWire {
                allowed: false,
                capability: None,
                reason: Some(reason.to_string()),
            },
        };
        to_value(&wire)
    }

    async fn predicate_inspect(&self, params: Value) -> Result<Value, ApiError> {
        let p: PredicateInspectParams = parse_params(params)?;
        let spec = self.registry.get(p.name.as_str()).ok_or_else(|| ApiError {
            code: ERR_NOT_FOUND,
            message: format!("predicate `{}` not loaded", p.name.as_str()),
            data: None,
        })?;
        // Serialize the spec — `PredicateSpec` doesn't derive Serialize, so
        // build a minimal projection of the public fields the client needs.
        let view = serde_json::json!({
            "name": spec.name,
            "version": spec.version,
            "parent_predicate": spec.parent_predicate,
            "claim_schema": spec.claim_schema,
            "rendering": spec.rendering,
            "reverse_map": spec.reverse_map,
            "pagination": spec.pagination,
        });
        Ok(view)
    }

    async fn health_summary(&self) -> Result<Value, ApiError> {
        // Proposals: count of `Pending` submissions in the quarantine
        // — those the scribe has accepted for processing but the user
        // hasn't accepted yet.
        let proposals = self
            .quarantine
            .list(Some(SubmissionStatus::Pending))
            .await
            .len() as u32;
        // Drift flags: count of working-set entries whose stored
        // `last_render_hash` no longer matches the current render
        // (computed lazily on demand). Re-rendering everything for
        // health.summary would be O(N) projections; keep this cheap
        // by reusing the same detect-drift helper.
        let drift_flags = self.compute_drift().await.unwrap_or_default().len() as u32;
        // Questions: a Phase 2 surface (the librarian asks the user
        // about ambiguous extractions). Zero at MVP.
        let summary = HealthSummary {
            proposals,
            questions: 0,
            drift_flags,
            atom_count: self.atom_count_estimate(),
            courier: self.read_courier_status(),
        };
        to_value(&summary)
    }

    /// The courier's last-tick counters from
    /// `$FFS_DATA_DIR/ingest/.courier/last_run.json`, or `None` when
    /// the file is absent, unreadable, or the data dir is unknown.
    fn read_courier_status(&self) -> Option<CourierStatus> {
        let path = self
            .data_dir
            .as_ref()?
            .join("ingest")
            .join(".courier")
            .join("last_run.json");
        let text = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<CourierStatus>(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "courier last_run.json did not parse");
                None
            }
        }
    }

    /// `courier.status`: the same object `health.summary.courier`
    /// carries, or `null`.
    async fn courier_status(&self) -> Result<Value, ApiError> {
        to_value(&self.read_courier_status())
    }

    /// `courier.run { dry_run? }`: invoke one courier tick through the
    /// skills host and return the skill's result verbatim. Until the
    /// daemon scheduler (task_41) drives the courier, this is how
    /// `ffs courier run` triggers a tick by hand.
    async fn courier_run(&self, params: Value) -> Result<Value, ApiError> {
        let params = if params.is_null() {
            serde_json::json!({})
        } else {
            params
        };
        let p: CourierRunParams = parse_params(params)?;
        let Some(invoker) = self.skill_invoker.as_ref() else {
            return Err(ApiError {
                code: ERR_NOT_IMPLEMENTED,
                message: "courier.run: no skills host is wired into this daemon".into(),
                data: None,
            });
        };
        let input = serde_json::json!({"op": "tick", "dry_run": p.dry_run});
        invoker
            .invoke("courier", input)
            .await
            .map_err(|reason| ApiError {
                code: ERR_NOT_IMPLEMENTED,
                message: format!("courier.run: {reason}"),
                data: None,
            })
    }

    fn atom_count_estimate(&self) -> u64 {
        // No total-count method on AtomStore yet; approximate via list_by_predicate
        // of a known predicate (or via 0 if no predicate is registered). At MVP
        // scale the renderer-side stats are sufficient.
        0
    }

    // ---- working_set handlers ----

    async fn working_set_list(&self) -> Result<Value, ApiError> {
        let entries = self.working_set.list_oldest_first().await;
        to_value(&entries)
    }

    async fn working_set_touch(&self, params: Value) -> Result<Value, ApiError> {
        let p: WorkingSetTouchParams = parse_params(params)?;
        self.working_set
            .touch(&p.path, current_iso8601())
            .await
            .map_err(working_set_err)?;
        to_value(&serde_json::json!({"ok": true}))
    }

    async fn working_set_pin(&self, params: Value) -> Result<Value, ApiError> {
        let p: WorkingSetPinParams = parse_params(params)?;
        self.working_set
            .pin(&p.path, p.pinned)
            .await
            .map_err(working_set_err)?;
        to_value(&serde_json::json!({"ok": true}))
    }

    /// Materialize a projection: re-render and record the new
    /// `last_render_hash` in the working set. Does NOT write the
    /// rendered markdown to disk — that's the librarian's
    /// responsibility (it owns the projection root path). The
    /// renderer's `render_hash` field gives the librarian a stable
    /// content hash to write + a value to store here for future
    /// drift checks.
    async fn working_set_materialize(&self, params: Value) -> Result<Value, ApiError> {
        let p: WorkingSetMaterializeParams = parse_params(params)?;
        let req = ProjectionRequest {
            path: p.path.clone(),
            as_of: None,
            agent: self.owner.clone(),
        };
        let resp = self.renderer.render(&req).map_err(render_err)?;
        let render_hash = resp.render_hash.clone();
        self.working_set
            .upsert(p.path.clone(), render_hash.clone(), current_iso8601())
            .await
            .map_err(working_set_err)?;
        to_value(&WorkingSetMaterializeResult {
            path: p.path,
            render_hash,
            markdown: resp.markdown,
        })
    }

    /// Scan every working-set entry, re-render, and return the paths
    /// whose render hash has changed since materialization (drifted).
    /// Does not modify state — pair with `refresh_drifted` to act.
    async fn working_set_detect_drift(&self) -> Result<Value, ApiError> {
        let drifted = self.compute_drift().await.map_err(|e| ApiError {
            code: ERR_RENDER,
            message: e,
            data: None,
        })?;
        to_value(&serde_json::json!({"drifted": drifted}))
    }

    /// Detect-then-refresh: for every drifted entry, re-materialize.
    /// Returns the list of refreshed paths.
    async fn working_set_refresh_drifted(&self) -> Result<Value, ApiError> {
        let drifted = self.compute_drift().await.map_err(|e| ApiError {
            code: ERR_RENDER,
            message: e,
            data: None,
        })?;
        let mut refreshed = Vec::with_capacity(drifted.len());
        for path in drifted {
            let req = ProjectionRequest {
                path: path.clone(),
                as_of: None,
                agent: self.owner.clone(),
            };
            let resp = self.renderer.render(&req).map_err(render_err)?;
            self.working_set
                .upsert(path.clone(), resp.render_hash.clone(), current_iso8601())
                .await
                .map_err(working_set_err)?;
            refreshed.push(WorkingSetRefreshed {
                path,
                render_hash: resp.render_hash,
                markdown: resp.markdown,
            });
        }
        to_value(&serde_json::json!({"refreshed": refreshed}))
    }

    async fn working_set_evict_to_cap(&self, params: Value) -> Result<Value, ApiError> {
        let p: WorkingSetEvictParams = parse_params(params)?;
        let evicted = self.working_set.evict_to_cap(p.cap).await;
        to_value(&serde_json::json!({"evicted": evicted}))
    }

    // ---- federation handlers ----

    /// Register a peer's endpoint + pinned fingerprint locally so the
    /// substrate trusts subsequent inbound mTLS from that cert and
    /// can initiate a handshake to it. Capability-checks `Federate`
    /// on the owner; out-of-band fingerprint exchange happens before
    /// this call (paste into the CLI / plugin).
    async fn federation_peer_add(&self, params: Value) -> Result<Value, ApiError> {
        let p: FederationPeerAddParams = parse_params(params)?;

        let now = current_iso8601();
        let target = Target {
            predicate: PredicateName::new("capability.grant"),
            entity: EntityId::new(p.peer_id_for_target()),
            classification: None,
            tier: None,
        };
        let decision = capability::evaluate(
            &*self.store,
            &self.owner,
            capability::Action::Federate,
            &target,
            &now,
        )
        .map_err(eval_err)?;
        if let Decision::Deny { reason } = decision {
            return Err(capability_denied(&reason));
        }

        let peer = FederationPeer {
            peer_id: p.peer_id.clone(),
            peer_pubkey: p.peer_pubkey.clone(),
            endpoint: p.endpoint,
            cert_fingerprint: p.fingerprint,
            our_capability: None,
            their_capability: None,
            vocab: Vec::new(),
            watermarks: Default::default(),
            established_at: now,
            last_seen_at: None,
        };
        self.federation_peers
            .upsert(peer)
            .await
            .map_err(federation_err)?;
        to_value(&serde_json::json!({"peer_id": p.peer_id}))
    }

    async fn federation_peer_list(&self) -> Result<Value, ApiError> {
        let peers = self.federation_peers.list().await;
        to_value(&peers)
    }

    /// On-demand pull from a specific peer. Calls
    /// `tick_once_for_peer` which: pulls atoms after the stored
    /// watermark, verifies each (signature + content hash), inserts
    /// verified atoms, attributes them in the mount, and advances
    /// the watermark. Returns the pull telemetry so the caller can
    /// surface results (atoms_pulled / revoked / new_watermark).
    async fn federation_pull(&self, params: Value) -> Result<Value, ApiError> {
        let p: FederationPullParams = parse_params(params)?;
        let client = self.federation_client.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "federation.pull requires a configured federation client".into(),
            data: None,
        })?;
        let our_fp = self.our_cert_fingerprint.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "federation.pull requires our_cert_fingerprint to be configured".into(),
            data: None,
        })?;

        let outcome = tick_once_for_peer(
            &p.peer_id,
            &self.federation_peers,
            client,
            our_fp,
            &self.store,
            &self.peer_mounts,
            "default",
        )
        .await
        .map_err(|e| ApiError {
            code: ERR_INTERNAL,
            message: format!("federation.pull: {e}"),
            data: None,
        })?;
        to_value(&outcome)
    }

    /// Initiate the in-band handshake with an already-pinned peer.
    /// Requires `federation_client` to be configured (without it the
    /// daemon can still serve inbound but cannot initiate).
    async fn bridge_establish(&self, params: Value) -> Result<Value, ApiError> {
        let p: BridgeEstablishParams = parse_params(params)?;
        let client = self.federation_client.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "bridge.establish requires a configured federation client".into(),
            data: None,
        })?;
        let our_fp = self.our_cert_fingerprint.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "bridge.establish requires our_cert_fingerprint to be configured".into(),
            data: None,
        })?;

        let peer = self
            .federation_peers
            .get(&p.peer_id)
            .await
            .ok_or_else(|| ApiError {
                code: ERR_NOT_FOUND,
                message: format!("peer not registered: {}", p.peer_id),
                data: None,
            })?;

        let req = HandshakeRequest {
            protocol_version: HANDSHAKE_PROTOCOL_VERSION,
            initiator_pubkey: self.owner.clone(),
            initiator_capability: p.our_capability.clone(),
            initiator_vocab: p.our_vocab.clone(),
            initiator_anchor: current_iso8601(),
        };
        let resp = client
            .handshake(&peer.endpoint, our_fp, req)
            .await
            .map_err(|e| ApiError {
                code: ERR_INTERNAL,
                message: format!("handshake: {e}"),
                data: None,
            })?;

        // Stamp our peer record with the bridge contract.
        let mut updated = peer.clone();
        updated.our_capability = Some(p.our_capability);
        updated.their_capability = Some(resp.responder_capability.clone());
        updated.vocab = resp.responder_vocab.clone();
        updated.last_seen_at = Some(current_iso8601());
        self.federation_peers
            .upsert(updated)
            .await
            .map_err(federation_err)?;
        to_value(&serde_json::json!({
            "peer_id": p.peer_id,
            "their_capability": resp.responder_capability,
            "their_vocab": resp.responder_vocab,
            "their_anchor": resp.responder_anchor,
        }))
    }

    /// Rotate this substrate's TLS certificate with a peer: signs
    /// the new fingerprint with the OLD signing key and ships it.
    /// On peer acceptance, the peer updates its pinned fingerprint.
    async fn bridge_rotate(&self, params: Value) -> Result<Value, ApiError> {
        let p: BridgeRotateParams = parse_params(params)?;
        let client = self.federation_client.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "bridge.rotate requires a configured federation client".into(),
            data: None,
        })?;
        let key = self.signing_key.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "bridge.rotate requires a configured signing key".into(),
            data: None,
        })?;
        let our_fp = self.our_cert_fingerprint.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "bridge.rotate requires our_cert_fingerprint to be configured".into(),
            data: None,
        })?;

        let peer = self
            .federation_peers
            .get(&p.peer_id)
            .await
            .ok_or_else(|| ApiError {
                code: ERR_NOT_FOUND,
                message: format!("peer not registered: {}", p.peer_id),
                data: None,
            })?;

        // Sign over (our_cert_fingerprint, new_fingerprint). The
        // receiver knows our_cert_fingerprint as their pinned fingerprint
        // for us; the (old, new) pair binds the signature to this
        // specific rotation event.
        let signed_bytes = rotation_signing_bytes(our_fp, &p.new_fingerprint);
        let sig = key.sign(&signed_bytes);
        let req = RotateRequest {
            new_fingerprint: p.new_fingerprint,
            old_signature: sig.to_bytes().to_vec(),
        };
        let resp = client
            .rotate(&peer.endpoint, our_fp, req)
            .await
            .map_err(|e| ApiError {
                code: ERR_INTERNAL,
                message: format!("rotate: {e}"),
                data: None,
            })?;
        to_value(&serde_json::json!({"accepted": resp.accepted}))
    }

    // ---- audit handlers ----

    /// Sign and insert an `auditor.daily_summary` atom carrying the
    /// caller-supplied claim. The atom uses entity = `"auditor"` (the
    /// singleton entity per ADR-013) so subsequent atoms supersede
    /// the chain naturally. Tier = `"existence"` so the summary is
    /// visible by default; user can reclassify later if needed.
    async fn audit_publish_summary(&self, params: Value) -> Result<Value, ApiError> {
        let p: AuditPublishParams = parse_params(params)?;
        let key = self.signing_key.as_ref().ok_or_else(|| ApiError {
            code: ERR_NOT_IMPLEMENTED,
            message: "audit.publish_summary requires a configured daemon signing key".into(),
            data: None,
        })?;

        // Capability check: the caller must hold Write on the
        // auditor.daily_summary predicate (auditor identity in
        // production; owner during MVP).
        let now = current_iso8601();
        let target = Target {
            predicate: PredicateName::new("auditor.daily_summary"),
            entity: EntityId::new("auditor"),
            classification: None,
            tier: None,
        };
        let decision = capability::evaluate(
            &*self.store,
            &self.owner,
            capability::Action::Write,
            &target,
            &now,
        )
        .map_err(eval_err)?;
        if let Decision::Deny { reason } = decision {
            return Err(capability_denied(&reason));
        }

        // Chain newest-on-newest: if a previous summary exists, the
        // new one supersedes it. Provides a stable single-entity
        // "current summary" head for `audit.query`.
        let supersedes = self
            .store
            .head_of_chain(
                &EntityId::new("auditor"),
                &PredicateName::new("auditor.daily_summary"),
                None,
            )
            .map_err(store_err)?
            .map(|env| env.content_hash())
            .transpose()
            .map_err(|e| ApiError {
                code: ERR_INTERNAL,
                message: format!("content_hash: {e}"),
                data: None,
            })?;

        let tmpl = AtomTemplate {
            v: 1,
            entity: EntityId::new("auditor"),
            predicate: PredicateName::new("auditor.daily_summary"),
            claim: p.claim,
            valid_from: p.valid_from.unwrap_or_else(|| now.clone()),
            valid_to: None,
            tx_time: now,
            classification: Tier::new("existence"),
            supersedes,
            provenance: vec![],
        };
        let env = tmpl.sign(key).map_err(|e| ApiError {
            code: ERR_INTERNAL,
            message: format!("sign: {e}"),
            data: None,
        })?;
        let hash = self.store.insert(&env).map_err(store_err)?;
        // Publish so subscribers (working-set materializer,
        // Obsidian plugin's daily-summary panel refresh hook) see
        // the commit. The auditor entity has no path-library home
        // so the materializer benignly skips; the plugin still
        // refreshes.
        self.notifier.publish(crate::notify::Event::AtomCommitted {
            hash: hash.clone(),
            entity: env.entity.clone(),
            predicate: env.predicate.clone(),
        });
        to_value(&AuditPublishResult { atom_hash: hash })
    }

    /// Return the most recent `auditor.daily_summary` atom (and the
    /// full chain when no `since` filter narrows it). Read-side
    /// capability check fires on each returned atom.
    async fn audit_query(&self, params: Value) -> Result<Value, ApiError> {
        // Tolerate a null or missing params body; the entire payload
        // is optional (a since-filter).
        let params = if params.is_null() {
            serde_json::json!({})
        } else {
            params
        };
        let p: AuditQueryParams = parse_params(params)?;
        let atoms = self
            .store
            .list_by_entity(
                &EntityId::new("auditor"),
                Some(&PredicateName::new("auditor.daily_summary")),
                p.since.as_ref(),
            )
            .map_err(store_err)?;

        let now = current_iso8601();
        let mut visible = Vec::with_capacity(atoms.len());
        for env in atoms {
            let target = Target {
                predicate: env.predicate.clone(),
                entity: env.entity.clone(),
                classification: Some(env.classification.clone()),
                tier: None,
            };
            let decision = capability::evaluate(
                &*self.store,
                &self.owner,
                capability::Action::Read,
                &target,
                &now,
            )
            .map_err(eval_err)?;
            if matches!(decision, Decision::Allow { .. }) {
                visible.push(env);
            }
        }
        // Most-recent first by tx_time so the daily-health-summary
        // panel can take the head.
        visible.sort_by(|a, b| b.tx_time.as_str().cmp(a.tx_time.as_str()));
        to_value(&visible)
    }

    /// Internal helper: list working-set entries, re-render each,
    /// return paths whose hash no longer matches. On render error
    /// for a single entry, treat it as not-drifted (the librarian
    /// will retry on the next tick). String error so callers can
    /// thread it through both ApiError and serde results.
    async fn compute_drift(&self) -> Result<Vec<String>, String> {
        let entries = self.working_set.list_oldest_first().await;
        let mut drifted = Vec::new();
        for entry in entries {
            let req = ProjectionRequest {
                path: entry.path.clone(),
                as_of: None,
                agent: self.owner.clone(),
            };
            match self.renderer.render(&req) {
                Ok(resp) => {
                    if resp.render_hash != entry.last_render_hash {
                        drifted.push(entry.path);
                    }
                }
                Err(_) => continue, // treat render errors as not-drifted; the librarian retries
            }
        }
        Ok(drifted)
    }
}

// ---- helpers ----

fn stub_not_implemented(implementing_task: &str) -> Result<Value, ApiError> {
    Err(ApiError {
        code: ERR_NOT_IMPLEMENTED,
        message: format!("method not yet implemented; implementing task: {implementing_task}"),
        data: Some(serde_json::json!({ "implementing_task": implementing_task })),
    })
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ApiError> {
    serde_json::from_value(params).map_err(|e| ApiError {
        code: ERR_INVALID_PARAMS,
        message: e.to_string(),
        data: None,
    })
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError {
        code: ERR_INTERNAL,
        message: format!("serialization: {e}"),
        data: None,
    })
}

/// Additive merge of a proposal claim onto an existing head claim
/// (ADR-027: never silently overwrite): scalars present on the head
/// are kept, arrays are unioned (objects with a `display` key are
/// keyed by display so a mention gaining an `entity` replaces its
/// earlier form), and everything the head lacks comes from the
/// proposal. The name field always keeps the head's value; a
/// differing mention text becomes an alias instead.
pub fn merge_additive(head: &Value, proposal: &Value, name_field: &str) -> Value {
    let mut out = head.clone();
    let (Some(out_obj), Some(prop_obj)) = (out.as_object_mut(), proposal.as_object()) else {
        return proposal.clone();
    };
    for (k, v) in prop_obj {
        match out_obj.get_mut(k) {
            None => {
                out_obj.insert(k.clone(), v.clone());
            }
            Some(existing) => match (existing.as_array_mut(), v.as_array()) {
                (Some(arr), Some(add)) => {
                    for item in add {
                        let key_of = |x: &Value| {
                            x.get("display")
                                .and_then(|d| d.as_str())
                                .map(str::to_string)
                        };
                        if let Some(dk) = key_of(item) {
                            if let Some(pos) = arr
                                .iter()
                                .position(|x| key_of(x).as_deref() == Some(dk.as_str()))
                            {
                                arr[pos] = item.clone();
                            } else {
                                arr.push(item.clone());
                            }
                        } else if !arr.contains(item) {
                            arr.push(item.clone());
                        }
                    }
                }
                _ => {
                    if k == name_field || !existing.is_null() {
                        // keep the head's scalar
                    } else {
                        *existing = v.clone();
                    }
                }
            },
        }
    }
    out
}

/// Merge for a re-read of the same record (task_40): the proposal's
/// scalars win, arrays merge as in `merge_additive`, and keys only the
/// head knows are kept.
pub fn merge_refresh(head: &Value, proposal: &Value) -> Value {
    let mut out = merge_additive(head, proposal, "");
    if let (Some(out_obj), Some(prop_obj)) = (out.as_object_mut(), proposal.as_object()) {
        for (k, v) in prop_obj {
            if !v.is_array() && !v.is_null() {
                out_obj.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

fn push_alias(claim: &mut Value, alias: &str) {
    let Some(obj) = claim.as_object_mut() else {
        return;
    };
    let entry = obj
        .entry("aliases")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(arr) = entry.as_array_mut() {
        let dup = arr.iter().any(|v| {
            v.as_str().is_some_and(|s| {
                ffs_core::resolve::normalized_name_key(s)
                    == ffs_core::resolve::normalized_name_key(alias)
            })
        });
        if !dup {
            arr.push(Value::String(alias.to_string()));
        }
    }
}

fn quarantine_err(e: ffs_core::quarantine::QuarantineError) -> ApiError {
    ApiError {
        code: ERR_STORE,
        message: e.to_string(),
        data: None,
    }
}

fn working_set_err(e: ffs_core::working_set::WorkingSetError) -> ApiError {
    ApiError {
        code: ERR_STORE,
        message: e.to_string(),
        data: None,
    }
}

fn federation_err(e: ffs_core::federation_peers::FederationPeerError) -> ApiError {
    ApiError {
        code: ERR_STORE,
        message: e.to_string(),
        data: None,
    }
}

fn store_err(e: ffs_core::store::StoreError) -> ApiError {
    ApiError {
        code: ERR_STORE,
        message: e.to_string(),
        data: None,
    }
}

fn render_err(e: ffs_core::projection::RenderError) -> ApiError {
    use ffs_core::projection::RenderError as R;
    match e {
        R::CapabilityDenied(reason) => capability_denied(&reason),
        R::AtomNotFound { .. } => ApiError {
            code: ERR_NOT_FOUND,
            message: e.to_string(),
            data: None,
        },
        other => ApiError {
            code: ERR_RENDER,
            message: other.to_string(),
            data: None,
        },
    }
}

fn eval_err(e: EvalError) -> ApiError {
    ApiError {
        code: ERR_INTERNAL,
        message: e.to_string(),
        data: None,
    }
}

fn capability_denied(reason: &capability::DenyReason) -> ApiError {
    ApiError {
        code: ERR_CAPABILITY_DENIED,
        message: format!("capability denied: {reason}"),
        data: Some(serde_json::json!({ "reason": reason.to_string() })),
    }
}

fn current_iso8601() -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let now = time::OffsetDateTime::now_utc();
    let s = now
        .format(&Fmt::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    Iso8601::new(s).expect("formatted ISO8601 must parse")
}
