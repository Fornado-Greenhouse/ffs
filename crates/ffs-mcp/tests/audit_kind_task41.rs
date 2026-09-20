//! task_41: `ffs_audit_query` forwards the optional `kind` so an agent
//! can read the morning briefing; omitted, the daemon's default
//! (`daily_summary`) applies and the call is byte-identical to before.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use ffs_mcp::{DaemonClient, DaemonError, dispatch_tool_call, tool_catalog};

struct Recorder {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl DaemonClient for Recorder {
    async fn call(&self, method: &str, params: Value) -> Result<Value, DaemonError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params));
        Ok(json!([]))
    }
}

#[tokio::test]
async fn ffs_audit_query_passes_kind_filter() {
    let daemon = Arc::new(Recorder {
        calls: Mutex::new(Vec::new()),
    });
    let r = dispatch_tool_call(
        "ffs_audit_query",
        json!({"kind": "briefing", "since": "2026-09-01T00:00:00Z"}),
        daemon.as_ref(),
        "test-agent",
    )
    .await;
    assert!(!r.is_error);
    let r = dispatch_tool_call("ffs_audit_query", json!({}), daemon.as_ref(), "test-agent").await;
    assert!(!r.is_error);

    let calls = daemon.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "audit.query");
    assert_eq!(
        calls[0].1,
        json!({"kind": "briefing", "since": "2026-09-01T00:00:00Z"})
    );
    assert_eq!(calls[1].1, json!({}), "no kind means the daemon default");

    let tool = tool_catalog()
        .into_iter()
        .find(|t| t.name == "ffs_audit_query")
        .unwrap();
    assert_eq!(
        tool.input_schema["properties"]["kind"]["enum"],
        json!(["daily_summary", "briefing"])
    );
}
