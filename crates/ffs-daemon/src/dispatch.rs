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

        let mut hashes: Vec<Multihash> = Vec::with_capacity(sub.proposals.len());
        for proposal in &sub.proposals {
            let tmpl = AtomTemplate {
                v: 1,
                entity: slug_for_proposal(proposal, &sub.id, &now),
                predicate: proposal.predicate.clone(),
                claim: proposal.claim.clone(),
                valid_from: now.clone(),
                valid_to: None,
                tx_time: now.clone(),
                classification: Tier::new("existence"),
                supersedes: None,
                provenance: proposal.provenance.clone(),
            };
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
            hashes.push(h);
        }

        self.quarantine
            .accept(&p.submission_id, hashes.clone())
            .await
            .map_err(quarantine_err)?;
        to_value(&serde_json::json!({"accepted_atom_hashes": hashes}))
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

    /// Search entities by name across loaded predicates. Returns
    /// matches (case-insensitive substring on the canonical name
    /// field) capped at `limit` (default 50). Mirrors the entity-
    /// name search hook in the Obsidian quick-switcher.
    async fn entity_search(&self, params: Value) -> Result<Value, ApiError> {
        let params = if params.is_null() {
            serde_json::json!({})
        } else {
            params
        };
        let p: EntitySearchParams = parse_params(params)?;
        let needle = p.query.trim().to_lowercase();
        if needle.is_empty() {
            return to_value(&serde_json::json!({"results": Vec::<Value>::new()}));
        }
        let limit = p.limit.unwrap_or(50).min(1000);

        // Iterate the registry's known predicates; for each, list
        // atoms and filter on the canonical name field (which
        // depends on the predicate: contact.person + person.generic
        // use `display_name`; note uses `title`).
        let mut results: Vec<EntitySearchHit> = Vec::new();
        let now = current_iso8601();
        for name in self.registry.names() {
            let pred = PredicateName::new(&name);
            let atoms = self
                .store
                .list_by_predicate(&pred, None, limit)
                .map_err(store_err)?;
            for env in atoms {
                if results.len() >= limit {
                    break;
                }
                let name_field = match env.predicate.as_str() {
                    "note" => "title",
                    _ => "display_name",
                };
                let Some(display) = env.claim.get(name_field).and_then(|v| v.as_str()) else {
                    continue;
                };
                if !display.to_lowercase().contains(&needle) {
                    continue;
                }
                // Capability-filter so unauthorized hits don't leak.
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
                if !matches!(decision, Decision::Allow { .. }) {
                    continue;
                }
                results.push(EntitySearchHit {
                    entity: env.entity.clone(),
                    predicate: env.predicate.clone(),
                    display_name: display.to_string(),
                });
            }
            if results.len() >= limit {
                break;
            }
        }
        to_value(&serde_json::json!({"results": results}))
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
        };
        to_value(&summary)
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

/// Derive a human-readable entity ID for a scribe-produced
/// proposal at accept time. Preference order:
///
///   1. `claim.display_name` (slug-ified) — for `contact.person`
///      and `person.generic` proposals.
///   2. `claim.title` (slug-ified) — for `note` proposals carrying
///      a meaningful title (either user-supplied frontmatter or
///      scribe's body-derived first-line slug).
///   3. A tx_time-based fallback (`note-YYYY-MM-DD-HHMM`) — for
///      truly empty notes where scribe couldn't derive a title.
///   4. The last-resort `from-<submission-id>` — for proposals
///      whose claim has no usable text at all (rare; mostly a
///      defensive backstop for malformed scribe output).
///
/// Slugification rules: trim, lowercase only at the boundary
/// (preserve user-supplied casing for navigability —
/// "Sara_Chen.md" reads better than "sara_chen.md" in a file
/// explorer), replace whitespace with underscores, drop characters
/// that aren't alphanumeric / underscore / hyphen / period.
fn slug_for_proposal(
    proposal: &ffs_core::Proposal,
    submission_id: &str,
    now: &Iso8601,
) -> EntityId {
    let predicate = proposal.predicate.as_str();

    // (1) display_name for contact/person predicates.
    if matches!(predicate, "contact.person" | "person.generic")
        && let Some(name) = proposal
            .claim
            .get("display_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
    {
        return EntityId::new(slugify(name));
    }

    // (2) title for note proposals (or anything with a title field).
    if let Some(title) = proposal
        .claim
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty() && *s != "untitled")
    {
        return EntityId::new(slugify(title));
    }

    // (3) tx_time-based slug for unstructured notes (`note-YYYY-MM-DD-HHMM`).
    if predicate == "note"
        && let Some(stamp) = format_tx_time_slug(now)
    {
        return EntityId::new(stamp);
    }

    // (4) Last resort.
    EntityId::new(format!("from-{submission_id}"))
}

/// Convert a human-readable string into a filesystem-safe slug
/// while preserving the original casing.
fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_underscore = false;
    for c in input.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '.' {
            out.push(c);
            last_was_underscore = false;
        } else if (c.is_whitespace() || c == '_') && !last_was_underscore {
            out.push('_');
            last_was_underscore = true;
        }
        // Anything else (commas, parens, slashes, etc.) gets dropped.
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

/// Format an `Iso8601` as a `note-YYYY-MM-DD-HHMM` slug suitable
/// for use as an entity ID. Returns `None` if the input doesn't
/// parse into the expected shape.
fn format_tx_time_slug(now: &Iso8601) -> Option<String> {
    // Iso8601 stores something like "2026-06-07T11:23:45.123456789Z".
    // We want just the YYYY-MM-DD and HHMM bits.
    let s = now.as_str();
    let date = s.get(0..10)?;
    let time_seg = s.get(11..16)?; // "HH:MM"
    let hhmm: String = time_seg.chars().filter(|c| *c != ':').collect();
    if hhmm.len() != 4 {
        return None;
    }
    Some(format!("note-{date}-{hhmm}"))
}

#[cfg(test)]
mod slug_tests {
    use super::*;
    use ffs_core::Proposal;

    fn now_iso() -> Iso8601 {
        Iso8601::new("2026-06-07T11:23:00Z").unwrap()
    }

    fn proposal(predicate: &str, claim: serde_json::Value) -> Proposal {
        Proposal {
            predicate: PredicateName::new(predicate),
            claim,
            provenance: vec![],
            rationale: "test".into(),
        }
    }

    #[test]
    fn contact_person_uses_display_name_with_spaces_to_underscores() {
        let p = proposal(
            "contact.person",
            serde_json::json!({"display_name": "Sara Chen"}),
        );
        let slug = slug_for_proposal(&p, "sub-1", &now_iso());
        assert_eq!(slug.as_str(), "Sara_Chen");
    }

    #[test]
    fn person_generic_uses_display_name() {
        let p = proposal(
            "person.generic",
            serde_json::json!({"display_name": "Alex Kim", "role": "engineer"}),
        );
        let slug = slug_for_proposal(&p, "sub-2", &now_iso());
        assert_eq!(slug.as_str(), "Alex_Kim");
    }

    #[test]
    fn note_uses_title_when_present() {
        let p = proposal(
            "note",
            serde_json::json!({"title": "Tuesday standup notes", "body": "..."}),
        );
        let slug = slug_for_proposal(&p, "sub-3", &now_iso());
        assert_eq!(slug.as_str(), "Tuesday_standup_notes");
    }

    #[test]
    fn note_with_untitled_falls_back_to_tx_time_slug() {
        let p = proposal("note", serde_json::json!({"title": "untitled", "body": ""}));
        let slug = slug_for_proposal(&p, "sub-4", &now_iso());
        assert_eq!(slug.as_str(), "note-2026-06-07-1123");
    }

    #[test]
    fn note_with_missing_title_falls_back_to_tx_time_slug() {
        let p = proposal("note", serde_json::json!({"body": "x"}));
        let slug = slug_for_proposal(&p, "sub-5", &now_iso());
        assert_eq!(slug.as_str(), "note-2026-06-07-1123");
    }

    #[test]
    fn unknown_predicate_falls_back_to_submission_id() {
        let p = proposal("capability.grant", serde_json::json!({}));
        let slug = slug_for_proposal(&p, "sub-99", &now_iso());
        assert_eq!(slug.as_str(), "from-sub-99");
    }

    #[test]
    fn slugify_drops_punctuation_and_collapses_underscores() {
        // Punctuation gone, multiple whitespace → single underscore.
        assert_eq!(slugify("Sara,  (Chen)"), "Sara_Chen");
        // Hyphens and periods preserved (file-extension shapes survive).
        assert_eq!(slugify("Doc 2.1 — draft"), "Doc_2.1_draft");
        // Empty-after-strip falls back to "untitled".
        assert_eq!(slugify("!!!"), "untitled");
    }

    #[test]
    fn slugify_preserves_casing_for_navigability() {
        // The dispatcher slug becomes the filename in the path
        // library, where reading "Sara_Chen.md" beats reading
        // "sara_chen.md".
        assert_eq!(slugify("Sara Chen"), "Sara_Chen");
        assert_eq!(slugify("sara chen"), "sara_chen");
        assert_eq!(slugify("SARA CHEN"), "SARA_CHEN");
    }

    #[test]
    fn format_tx_time_slug_extracts_date_and_hhmm() {
        let stamp = format_tx_time_slug(&Iso8601::new("2026-06-07T11:23:45.123Z").unwrap());
        assert_eq!(stamp.as_deref(), Some("note-2026-06-07-1123"));
    }

    #[test]
    fn note_with_body_derived_title_uses_it() {
        // Scribe's body-derived title path: title carries a
        // first-line slug rather than literal "untitled".
        let p = proposal(
            "note",
            serde_json::json!({"title": "Random thought about federation", "body": "..."}),
        );
        let slug = slug_for_proposal(&p, "sub-7", &now_iso());
        assert_eq!(slug.as_str(), "Random_thought_about_federation");
    }
}
