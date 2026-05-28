//! The six MVP MCP tools. Each tool exposes:
//!
//! - A `name` matching ADR-013 (`ffs_query`, `ffs_render_projection`,
//!   `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`,
//!   `ffs_audit_query`).
//! - A JSON Schema `inputSchema` so MCP-aware clients can validate
//!   arguments before the call.
//! - A translator that turns the MCP `arguments` object into the
//!   matching daemon JSON-RPC method + params and shapes the
//!   response back into an MCP `ToolCallResult`.
//!
//! Capability checks happen entirely on the daemon side (ADR-013) —
//! the MCP server is a thin pass-through. `DaemonError::CapabilityDenied`
//! comes back as a tool-level error with `isError: true`, the
//! canonical MCP shape for "the agent's request was authenticated
//! and well-formed, but the substrate refused it".

use serde_json::Value;

use crate::daemon_client::{DaemonClient, DaemonError};
use crate::protocol::{Tool, ToolCallResult};

/// Build the six-tool catalog the MCP server advertises on
/// `tools/list`. The JSON Schemas are intentionally tolerant — most
/// fields are optional so an agent can call a tool with the
/// minimum required arguments and iterate.
pub fn tool_catalog() -> Vec<Tool> {
    vec![
        Tool {
            name: "ffs_query".into(),
            description: "List atoms about an entity, optionally filtered by predicate and \
                          bitemporal as-of cutoff. Returns capability-filtered envelopes."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["entity"],
                "properties": {
                    "entity": {"type": "string", "description": "Entity id to query."},
                    "predicate": {"type": "string", "description": "Optional predicate filter."},
                    "as_of": {"type": "string", "description": "Optional ISO 8601 bitemporal cutoff."}
                }
            }),
        },
        Tool {
            name: "ffs_render_projection".into(),
            description: "Render a projection path (e.g. contacts/by-name/S/Sara.md) into the \
                          materialized markdown view."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Projection path."},
                    "as_of": {"type": "string", "description": "Optional ISO 8601 bitemporal cutoff."}
                }
            }),
        },
        Tool {
            name: "ffs_resolve_url".into(),
            description: "Resolve an ffs:// URL to its atom, entity, or projection — picks the \
                          right daemon method based on the URL's address mode."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {"type": "string", "description": "An ffs://<graph>/<address> URL."}
                }
            }),
        },
        Tool {
            name: "ffs_author_atom".into(),
            description: "Submit content for scribing into the ingest quarantine. Stamps \
                          provenance with the agent's identity."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["content"],
                "properties": {
                    "source_uri": {"type": "string", "description": "Origin URI of the content."},
                    "content": {"type": "string", "description": "Markdown to ingest."}
                }
            }),
        },
        Tool {
            name: "ffs_inspect_predicate".into(),
            description: "Return the loaded predicate spec (claim schema, rendering convention, \
                          reverse-map rules) for a given predicate name."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {"type": "string", "description": "Predicate name (e.g. contact.person)."}
                }
            }),
        },
        Tool {
            name: "ffs_audit_query".into(),
            description: "Return the most-recent auditor.daily_summary atoms, newest first. \
                          Optional `since` filter limits to atoms after a tx_time watermark."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "since": {"type": "string", "description": "Optional ISO 8601 lower bound."}
                }
            }),
        },
    ]
}

/// Dispatch a `tools/call` payload to the right translator.
pub async fn dispatch_tool_call(
    tool_name: &str,
    arguments: Value,
    daemon: &dyn DaemonClient,
    agent_uri: &str,
) -> ToolCallResult {
    match tool_name {
        "ffs_query" => translate_ffs_query(arguments, daemon).await,
        "ffs_render_projection" => translate_render_projection(arguments, daemon).await,
        "ffs_resolve_url" => translate_resolve_url(arguments, daemon).await,
        "ffs_author_atom" => translate_author_atom(arguments, daemon, agent_uri).await,
        "ffs_inspect_predicate" => translate_inspect_predicate(arguments, daemon).await,
        "ffs_audit_query" => translate_audit_query(arguments, daemon).await,
        other => ToolCallResult::tool_error(
            format!("unknown tool: {other}"),
            Some(
                serde_json::json!({"known_tools": tool_catalog().iter().map(|t| t.name.clone()).collect::<Vec<_>>()}),
            ),
        ),
    }
}

// ---- per-tool translators ----

async fn translate_ffs_query(args: Value, daemon: &dyn DaemonClient) -> ToolCallResult {
    let entity = match args.get("entity").and_then(|v| v.as_str()) {
        Some(e) => e.to_string(),
        None => {
            return ToolCallResult::tool_error("missing required argument: entity", None);
        }
    };
    let mut params = serde_json::json!({"entity": entity});
    if let Some(p) = args.get("predicate").and_then(|v| v.as_str()) {
        params["predicate"] = serde_json::json!(p);
    }
    if let Some(a) = args.get("as_of").and_then(|v| v.as_str()) {
        params["as_of"] = serde_json::json!(a);
    }
    forward(daemon, "atom.list", params).await
}

async fn translate_render_projection(args: Value, daemon: &dyn DaemonClient) -> ToolCallResult {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return ToolCallResult::tool_error("missing required argument: path", None),
    };
    let mut params = serde_json::json!({"path": path});
    if let Some(a) = args.get("as_of").and_then(|v| v.as_str()) {
        params["as_of"] = serde_json::json!(a);
    }
    forward(daemon, "projection.render", params).await
}

async fn translate_resolve_url(args: Value, daemon: &dyn DaemonClient) -> ToolCallResult {
    let url = match args.get("url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return ToolCallResult::tool_error("missing required argument: url", None),
    };
    let parsed = match parse_ffs_url(&url) {
        Ok(p) => p,
        Err(e) => return ToolCallResult::tool_error(format!("invalid ffs:// url: {e}"), None),
    };
    match parsed {
        FfsUrlKind::Atom { hash } => {
            forward(daemon, "atom.get", serde_json::json!({"hash": hash})).await
        }
        FfsUrlKind::Entity { id } => {
            forward(daemon, "atom.list", serde_json::json!({"entity": id})).await
        }
        FfsUrlKind::Path { path } => {
            forward(
                daemon,
                "projection.render",
                serde_json::json!({"path": path}),
            )
            .await
        }
    }
}

async fn translate_author_atom(
    args: Value,
    daemon: &dyn DaemonClient,
    agent_uri: &str,
) -> ToolCallResult {
    let content = match args.get("content").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => return ToolCallResult::tool_error("missing required argument: content", None),
    };
    // Provenance stamping: when the caller omits `source_uri`, fall
    // back to the configured agent identity URI so the daemon
    // records *who* authored the proposal even when the agent has
    // no upstream source to point at.
    let source_uri = args
        .get("source_uri")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("mcp:agent/{agent_uri}"));
    let params = serde_json::json!({
        "source_uri": source_uri,
        "content": content,
    });
    forward(daemon, "ingest.submit", params).await
}

async fn translate_inspect_predicate(args: Value, daemon: &dyn DaemonClient) -> ToolCallResult {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return ToolCallResult::tool_error("missing required argument: name", None),
    };
    forward(
        daemon,
        "predicate.inspect",
        serde_json::json!({"name": name}),
    )
    .await
}

async fn translate_audit_query(args: Value, daemon: &dyn DaemonClient) -> ToolCallResult {
    let mut params = serde_json::Map::new();
    if let Some(s) = args.get("since").and_then(|v| v.as_str()) {
        params.insert("since".into(), serde_json::json!(s));
    }
    forward(daemon, "audit.query", Value::Object(params)).await
}

// ---- helpers ----

async fn forward(daemon: &dyn DaemonClient, method: &str, params: Value) -> ToolCallResult {
    match daemon.call(method, params).await {
        Ok(v) => ToolCallResult::success_json(&v),
        Err(DaemonError::CapabilityDenied { reason }) => ToolCallResult::tool_error(
            format!("capability denied: {reason}"),
            Some(serde_json::json!({"kind": "capability_denied", "reason": reason})),
        ),
        Err(DaemonError::NotFound(msg)) => {
            ToolCallResult::tool_error(msg, Some(serde_json::json!({"kind": "not_found"})))
        }
        Err(DaemonError::InvalidParams(msg)) => {
            ToolCallResult::tool_error(msg, Some(serde_json::json!({"kind": "invalid_params"})))
        }
        Err(DaemonError::Other { code, message }) => ToolCallResult::tool_error(
            message,
            Some(serde_json::json!({"kind": "daemon_error", "code": code})),
        ),
        Err(DaemonError::Transport(msg)) => ToolCallResult::tool_error(
            format!("transport: {msg}"),
            Some(serde_json::json!({"kind": "transport"})),
        ),
    }
}

// ---- ffs:// URL minimal parser ----
//
// ffs-cli's parser is the canonical impl; replicating just the
// address-mode discrimination here avoids pulling in ffs-cli (which
// depends on ffs-daemon and would tangle the dep graph). Format:
//
//   ffs://<graph>/atom/<hash>
//   ffs://<graph>/entity/<id>
//   ffs://<graph>/<path...>

#[derive(Debug, PartialEq, Eq)]
enum FfsUrlKind {
    Atom { hash: String },
    Entity { id: String },
    Path { path: String },
}

fn parse_ffs_url(url: &str) -> Result<FfsUrlKind, String> {
    let after = url
        .strip_prefix("ffs://")
        .ok_or_else(|| "missing `ffs://` scheme".to_string())?;
    let body = after.split('?').next().unwrap_or(after);
    let (_graph, rest) = body
        .split_once('/')
        .ok_or_else(|| "missing address after graph".to_string())?;
    if let Some(hash) = rest.strip_prefix("atom/") {
        return Ok(FfsUrlKind::Atom {
            hash: hash.to_string(),
        });
    }
    if let Some(id) = rest.strip_prefix("entity/") {
        return Ok(FfsUrlKind::Entity { id: id.to_string() });
    }
    Ok(FfsUrlKind::Path {
        path: rest.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct RecorderClient {
        responses: Mutex<std::collections::HashMap<String, Result<Value, DaemonError>>>,
        seen: Mutex<Vec<(String, Value)>>,
    }

    impl RecorderClient {
        fn new() -> Self {
            Self {
                responses: Mutex::new(std::collections::HashMap::new()),
                seen: Mutex::new(Vec::new()),
            }
        }
        fn set_ok(&self, method: &str, response: Value) {
            self.responses
                .lock()
                .unwrap()
                .insert(method.to_string(), Ok(response));
        }
        fn set_err(&self, method: &str, err: DaemonError) {
            self.responses
                .lock()
                .unwrap()
                .insert(method.to_string(), Err(err));
        }
    }

    #[async_trait]
    impl DaemonClient for RecorderClient {
        async fn call(&self, method: &str, params: Value) -> Result<Value, DaemonError> {
            self.seen
                .lock()
                .unwrap()
                .push((method.to_string(), params.clone()));
            match self.responses.lock().unwrap().remove(method) {
                Some(r) => r,
                None => Err(DaemonError::Other {
                    code: 5001,
                    message: format!("no canned response for {method}"),
                }),
            }
        }
    }

    fn extract_text(r: &ToolCallResult) -> &str {
        match &r.content[0] {
            crate::protocol::ToolContent::Text { text } => text,
        }
    }

    // -- catalog --

    #[test]
    fn catalog_contains_the_six_mvp_tools() {
        let names: Vec<_> = tool_catalog().into_iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "ffs_query",
                "ffs_render_projection",
                "ffs_resolve_url",
                "ffs_author_atom",
                "ffs_inspect_predicate",
                "ffs_audit_query",
            ]
        );
    }

    #[test]
    fn every_tool_declares_input_schema_of_type_object() {
        for tool in tool_catalog() {
            assert_eq!(
                tool.input_schema["type"], "object",
                "tool {} missing object schema",
                tool.name
            );
        }
    }

    // -- translators --

    #[tokio::test]
    async fn ffs_query_translates_to_atom_list_with_passed_params() {
        let c = RecorderClient::new();
        c.set_ok("atom.list", serde_json::json!([]));
        dispatch_tool_call(
            "ffs_query",
            serde_json::json!({
                "entity": "Sara_Chen",
                "predicate": "contact.person",
                "as_of": "2026-05-27T08:00:00Z",
            }),
            &c,
            "test-agent",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "atom.list");
        assert_eq!(seen[0].1["entity"], "Sara_Chen");
        assert_eq!(seen[0].1["predicate"], "contact.person");
        assert_eq!(seen[0].1["as_of"], "2026-05-27T08:00:00Z");
    }

    #[tokio::test]
    async fn ffs_author_atom_translates_to_ingest_submit_with_agent_provenance() {
        let c = RecorderClient::new();
        c.set_ok(
            "ingest.submit",
            serde_json::json!({"submission_id": "sub-001"}),
        );
        dispatch_tool_call(
            "ffs_author_atom",
            serde_json::json!({"content": "# hello"}),
            &c,
            "claude-code",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen[0].0, "ingest.submit");
        // Default provenance stamps the agent identity.
        assert_eq!(seen[0].1["source_uri"], "mcp:agent/claude-code");
        assert_eq!(seen[0].1["content"], "# hello");
    }

    #[tokio::test]
    async fn ffs_author_atom_respects_explicit_source_uri() {
        let c = RecorderClient::new();
        c.set_ok("ingest.submit", serde_json::json!({"submission_id": "sub"}));
        dispatch_tool_call(
            "ffs_author_atom",
            serde_json::json!({"source_uri": "file:///note.md", "content": "x"}),
            &c,
            "agent",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen[0].1["source_uri"], "file:///note.md");
    }

    #[tokio::test]
    async fn capability_denial_returns_tool_level_error_not_jsonrpc_error() {
        let c = RecorderClient::new();
        c.set_err(
            "atom.list",
            DaemonError::CapabilityDenied {
                reason: "read denied for tier=secret".into(),
            },
        );
        let r =
            dispatch_tool_call("ffs_query", serde_json::json!({"entity": "x"}), &c, "agent").await;
        assert!(r.is_error, "expected isError: true");
        let text = extract_text(&r);
        assert!(text.contains("capability_denied"), "got: {text}");
        assert!(text.contains("read denied for tier=secret"), "got: {text}");
    }

    #[tokio::test]
    async fn missing_required_argument_returns_tool_error() {
        let c = RecorderClient::new();
        let r = dispatch_tool_call("ffs_query", serde_json::json!({}), &c, "agent").await;
        assert!(r.is_error);
        let text = extract_text(&r);
        assert!(text.contains("missing required argument: entity"));
        // The daemon was never called.
        assert!(c.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_tool_returns_tool_error_with_catalog_hint() {
        let c = RecorderClient::new();
        let r = dispatch_tool_call("ffs_nope", serde_json::json!({}), &c, "agent").await;
        assert!(r.is_error);
        let text = extract_text(&r);
        assert!(text.contains("unknown tool"));
        assert!(text.contains("ffs_query"));
    }

    // -- resolve_url --

    #[tokio::test]
    async fn ffs_resolve_url_atom_address_dispatches_to_atom_get() {
        let c = RecorderClient::new();
        c.set_ok("atom.get", serde_json::json!({"hash": "abc"}));
        dispatch_tool_call(
            "ffs_resolve_url",
            serde_json::json!({"url": "ffs://local/atom/zb2rh..."}),
            &c,
            "agent",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen[0].0, "atom.get");
        assert_eq!(seen[0].1["hash"], "zb2rh...");
    }

    #[tokio::test]
    async fn ffs_resolve_url_entity_address_dispatches_to_atom_list() {
        let c = RecorderClient::new();
        c.set_ok("atom.list", serde_json::json!([]));
        dispatch_tool_call(
            "ffs_resolve_url",
            serde_json::json!({"url": "ffs://local/entity/Sara_Chen"}),
            &c,
            "agent",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen[0].0, "atom.list");
        assert_eq!(seen[0].1["entity"], "Sara_Chen");
    }

    #[tokio::test]
    async fn ffs_resolve_url_path_address_dispatches_to_projection_render() {
        let c = RecorderClient::new();
        c.set_ok(
            "projection.render",
            serde_json::json!({"markdown": "# ..."}),
        );
        dispatch_tool_call(
            "ffs_resolve_url",
            serde_json::json!({"url": "ffs://local/contacts/by-name/S/Sara.md"}),
            &c,
            "agent",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen[0].0, "projection.render");
        assert_eq!(seen[0].1["path"], "contacts/by-name/S/Sara.md");
    }

    #[tokio::test]
    async fn ffs_resolve_url_rejects_non_ffs_scheme() {
        let c = RecorderClient::new();
        let r = dispatch_tool_call(
            "ffs_resolve_url",
            serde_json::json!({"url": "https://example.com/"}),
            &c,
            "agent",
        )
        .await;
        assert!(r.is_error);
        let text = extract_text(&r);
        assert!(text.contains("invalid ffs:// url"));
    }

    #[tokio::test]
    async fn ffs_audit_query_passes_optional_since_filter() {
        let c = RecorderClient::new();
        c.set_ok("audit.query", serde_json::json!([]));
        dispatch_tool_call(
            "ffs_audit_query",
            serde_json::json!({"since": "2026-05-27T00:00:00Z"}),
            &c,
            "agent",
        )
        .await;
        let seen = c.seen.lock().unwrap();
        assert_eq!(seen[0].1["since"], "2026-05-27T00:00:00Z");
    }
}
