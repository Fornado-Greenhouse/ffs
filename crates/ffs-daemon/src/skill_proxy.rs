//! Substrate-access proxy for skills (task_41).
//!
//! A skill subprocess reaches the substrate by writing `query` frames
//! that the skills host hands to a `SubstrateAccess` implementation.
//! Until task_41 the daemon installed `RefuseAllProxy`, so the auditor
//! could not read atoms or publish in production; the daily summary
//! and the briefing both need it. This proxy routes a skill's query
//! through the daemon's own dispatcher, in process, with two
//! restrictions:
//!
//! - **Set after boot.** The skills host is built before the dispatcher
//!   (the dispatcher needs the host for the scribe), so the proxy holds
//!   an empty slot at construction and refuses until `main.rs` installs
//!   the dispatcher. Queries during that window are answered with a
//!   clear error instead of a hang.
//! - **Allow-listed methods.** Skills act as the owner (the dispatcher's
//!   local identity; per-skill identities are a later task), so the
//!   proxy exposes only what a skill legitimately needs: reads,
//!   `audit.publish_summary`, `ingest.submit`, and the librarian's
//!   working-set maintenance. Nothing that accepts, merges, retracts,
//!   grants, or federates is reachable from a skill, so the human gate
//!   (ADR-029, ADR-030) cannot be bypassed by a bundle.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use ffs_skills_host::SubstrateAccess;

use crate::api::{ApiPayload, ApiRequest};
use crate::dispatch::Dispatcher;

/// Methods a skill may call through the proxy.
pub const SKILL_ALLOWED_METHODS: &[&str] = &[
    "health.summary",
    "atom.get",
    "atom.list",
    "entity.search",
    "path.families",
    "path.list",
    "projection.render",
    "predicate.inspect",
    "audit.query",
    "audit.publish_summary",
    "ingest.submit",
    "ingest.list_pending",
    "ingest.list_auto_filed",
    "working_set.list",
    "working_set.detect_drift",
    "working_set.refresh_drifted",
    "working_set.evict_to_cap",
    "working_set.materialize",
    "courier.status",
];

#[derive(Default)]
pub struct DispatcherProxy {
    slot: OnceLock<Arc<Dispatcher>>,
}

impl DispatcherProxy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the dispatcher. Returns `false` (and changes nothing) if
    /// one was already installed.
    pub fn install(&self, dispatcher: Arc<Dispatcher>) -> bool {
        self.slot.set(dispatcher).is_ok()
    }

    pub fn is_allowed(method: &str) -> bool {
        SKILL_ALLOWED_METHODS.contains(&method)
    }
}

#[async_trait]
impl SubstrateAccess for DispatcherProxy {
    async fn handle_query(
        &self,
        skill: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        if !Self::is_allowed(method) {
            return Err(format!(
                "skill `{skill}` may not call `{method}` (not in the skill allow-list)"
            ));
        }
        let Some(dispatcher) = self.slot.get() else {
            return Err(format!(
                "skill `{skill}` called `{method}` before the daemon finished starting"
            ));
        };
        debug!(skill, method, "skill query");
        let req = ApiRequest {
            jsonrpc: "2.0".into(),
            id: Value::String(format!("skill:{skill}")),
            method: method.to_string(),
            params,
        };
        match dispatcher.handle(req).await.payload {
            ApiPayload::Success { result } => Ok(result),
            ApiPayload::Error { error } => Err(format!("{} ({})", error.message, error.code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refuses_before_install_and_outside_the_allow_list() {
        let proxy = DispatcherProxy::new();
        let err = proxy
            .handle_query("auditor", "health.summary", Value::Null)
            .await
            .unwrap_err();
        assert!(err.contains("before the daemon finished starting"), "{err}");
        for denied in [
            "ingest.accept",
            "entity.merge",
            "capability.grant",
            "ingest.retract",
        ] {
            let err = proxy
                .handle_query("auditor", denied, Value::Null)
                .await
                .unwrap_err();
            assert!(err.contains("allow-list"), "{denied}: {err}");
        }
    }
}
