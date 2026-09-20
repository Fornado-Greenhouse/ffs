//! The production [`DecisionSink`] (ADR-032): applies inbox decisions
//! parsed by `ffs_fastpath::inbox` through the in-process dispatcher's
//! own JSON-RPC methods, publishes `event.quarantine.changed`, and hands
//! parse warnings to the inbox materializer. Lives in the daemon crate
//! because it holds the `Dispatcher` (task_49 / ADR-036: the fast path
//! no longer depends on the daemon).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use ffs_core::events::{Event, EventPublisher};
use ffs_fastpath::inbox::{
    DecisionAction, DecisionSink, InboxDecision, ParseWarning, decision_rpc,
};

use crate::api::{ApiPayload, ApiRequest};
use crate::inbox::InboxMaterializer;

/// The production sink: calls the in-process dispatcher by JSON-RPC
/// method name (`ingest.accept`, `ingest.reject`,
/// `entity.assert_different`, `entity.merge`, `entity.unmerge`,
/// `ingest.retract`), publishes `event.quarantine.changed`, and hands
/// parse warnings to the inbox materializer.
pub struct DispatcherSink {
    dispatcher: Arc<crate::Dispatcher>,
    publisher: Arc<EventPublisher>,
    inbox: Option<Arc<InboxMaterializer>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl DispatcherSink {
    pub fn new(
        dispatcher: Arc<crate::Dispatcher>,
        publisher: Arc<EventPublisher>,
        inbox: Option<Arc<InboxMaterializer>>,
    ) -> Self {
        Self {
            dispatcher,
            publisher,
            inbox,
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let resp = self
            .dispatcher
            .handle(ApiRequest {
                jsonrpc: "2.0".into(),
                id: json!(id),
                method: method.into(),
                params,
            })
            .await;
        match resp.payload {
            ApiPayload::Success { result } => Ok(result),
            ApiPayload::Error { error } => {
                Err(format!("{method}: {} (code {})", error.message, error.code))
            }
        }
    }
}

#[async_trait]
impl DecisionSink for DispatcherSink {
    async fn apply(&self, decision: &InboxDecision) -> Result<Value, String> {
        let (method, params) = decision_rpc(decision)?;
        // An assertion against "someone new" has no id to compare yet;
        // the accept that minted it did not return the entity id here.
        if let DecisionAction::AssertDifferent { a, .. } = &decision.action
            && a == "new"
        {
            return Err("cannot assert different-from for a person who was just minted; re-tick after the file re-renders".into());
        }
        self.call(method, params).await
    }

    fn report_warnings(&self, warnings: Vec<ParseWarning>) {
        if let Some(inbox) = &self.inbox {
            inbox.set_warnings(warnings);
        }
    }

    async fn finished(&self) {
        self.publisher.publish(Event::QuarantineChanged {
            submission_id: None,
        });
        if let Some(inbox) = &self.inbox
            && let Err(e) = inbox.refresh().await
        {
            tracing::warn!(error = %e, "inbox: re-render after decisions failed");
        }
    }
}
