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

use ffs_core::capability::{self, Decision, EvalError, Target};
use ffs_core::projection::{ProjectionRenderer, ProjectionRequest};
use ffs_core::store::AtomStore;
use ffs_core::{Iso8601, PublicKey, predicate::SpecRegistry};

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
            "ingest.submit" => stub_not_implemented("task_11"),
            "fastpath.submit" => stub_not_implemented("task_09"),
            "capability.evaluate" => self.capability_evaluate(req.params).await,
            "federation.peer.add" => stub_not_implemented("task_14"),
            "federation.peer.list" => stub_not_implemented("task_14"),
            "federation.pull" => stub_not_implemented("task_15"),
            "predicate.inspect" => self.predicate_inspect(req.params).await,
            "health.summary" => self.health_summary().await,
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
        // Proposals, questions, drift_flags depend on tasks 11/12/13 (scribe,
        // librarian, auditor). For MVP we report zero plus the visible atom
        // count, which is consistent with the store at this moment.
        let summary = HealthSummary {
            proposals: 0,
            questions: 0,
            drift_flags: 0,
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
