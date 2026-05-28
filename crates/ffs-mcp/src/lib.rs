//! `ffs-mcp` library: MCP protocol types, daemon-client abstraction,
//! and the six MVP tool translators.
//!
//! The MCP server is structured as a library so unit + integration
//! tests can drive `dispatch_request` directly, plus a thin
//! `main.rs` binary that owns the stdio I/O loop and wires a real
//! `DaemonClient` to the daemon's UDS / named pipe.
//!
//! Architecture:
//!
//! - `protocol.rs` — MCP wire types (`McpRequest`/`McpResponse`,
//!   `Tool`, `ToolCallResult`) and the three protocol methods this
//!   server implements: `initialize`, `tools/list`, `tools/call`.
//! - `daemon_client.rs` — `DaemonClient` async trait. Production
//!   implementation lives in the daemon binary's onboarding scripts
//!   (task_22); tests inject the in-process variant in
//!   `tests/mcp_integration.rs`.
//! - `tools.rs` — the six tools with JSON schemas and translators
//!   that map MCP arguments → daemon JSON-RPC params and the
//!   responses back.
//! - `transport.rs` — line-delimited JSON-RPC over an async reader
//!   / writer (stdin/stdout in production; in-memory pipes in
//!   tests).
//!
//! See ADR-013 (MCP server in MVP) and ADR-008 (speak MCP at the
//! boundary).

pub mod daemon_client;
pub mod protocol;
pub mod tools;
pub mod transport;

use std::sync::Arc;

use serde_json::Value;

pub use daemon_client::{DaemonClient, DaemonError, classify_daemon_error};
pub use protocol::{
    InitializeResult, McpError, McpPayload, McpRequest, McpResponse, PROTOCOL_VERSION,
    ServerCapabilities, ServerInfo, Tool, ToolCallParams, ToolCallResult, ToolContent,
    ToolsListResult,
};
pub use tools::{dispatch_tool_call, tool_catalog};
pub use transport::{TransportError, serve, serve_stdio};

/// Workspace marker exposed so smoke tests can confirm the crate links.
pub const CRATE_NAME: &str = "ffs-mcp";

/// Server-wide context the dispatcher carries across requests.
pub struct McpServer {
    pub daemon: Arc<dyn DaemonClient>,
    /// Identity URI stamped onto `ffs_author_atom` provenance when
    /// the caller does not supply an explicit `source_uri`. The
    /// production wiring sets this to the agent's configured
    /// FFS author key multibase; tests use a stable label.
    pub agent_identity_uri: String,
    /// Server metadata returned on `initialize`.
    pub server_info: ServerInfo,
}

impl McpServer {
    pub fn new(daemon: Arc<dyn DaemonClient>, agent_identity_uri: impl Into<String>) -> Self {
        Self {
            daemon,
            agent_identity_uri: agent_identity_uri.into(),
            server_info: ServerInfo {
                name: "ffs-mcp".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        }
    }

    /// Handle one MCP request — used by the stdio loop and by tests.
    /// Returns the corresponding `McpResponse` (a protocol-level
    /// error becomes `McpResponse::error`; a tool-level error stays
    /// inside `result.isError`).
    pub async fn handle(&self, req: McpRequest) -> McpResponse {
        if req.jsonrpc != "2.0" {
            return McpResponse::error(
                req.id,
                protocol::ERR_INVALID_REQUEST,
                format!("jsonrpc must be \"2.0\", got {:?}", req.jsonrpc),
                None,
            );
        }
        let id = req.id.clone();
        match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req.params).await,
            "notifications/initialized" => {
                // Per MCP spec: notification, no response. Return a
                // success envelope so the stdio loop knows nothing
                // went wrong; the caller filters notifications.
                McpResponse::success(id, serde_json::json!({}))
            }
            other => McpResponse::error(
                id,
                protocol::ERR_METHOD_NOT_FOUND,
                format!("unknown method: {other}"),
                None,
            ),
        }
    }

    fn handle_initialize(&self, id: Value) -> McpResponse {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.into(),
            server_info: self.server_info.clone(),
            capabilities: ServerCapabilities {
                tools: serde_json::Map::new(),
            },
        };
        match serde_json::to_value(&result) {
            Ok(v) => McpResponse::success(id, v),
            Err(e) => McpResponse::error(id, protocol::ERR_INTERNAL, e.to_string(), None),
        }
    }

    fn handle_tools_list(&self, id: Value) -> McpResponse {
        let result = ToolsListResult {
            tools: tool_catalog(),
        };
        match serde_json::to_value(&result) {
            Ok(v) => McpResponse::success(id, v),
            Err(e) => McpResponse::error(id, protocol::ERR_INTERNAL, e.to_string(), None),
        }
    }

    async fn handle_tools_call(&self, id: Value, params: Value) -> McpResponse {
        let p: ToolCallParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return McpResponse::error(
                    id,
                    protocol::ERR_INVALID_PARAMS,
                    format!("tools/call params: {e}"),
                    None,
                );
            }
        };
        let result = dispatch_tool_call(
            &p.name,
            p.arguments,
            &*self.daemon,
            &self.agent_identity_uri,
        )
        .await;
        match serde_json::to_value(&result) {
            Ok(v) => McpResponse::success(id, v),
            Err(e) => McpResponse::error(id, protocol::ERR_INTERNAL, e.to_string(), None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct PanicClient;
    #[async_trait]
    impl DaemonClient for PanicClient {
        async fn call(&self, method: &str, _: Value) -> Result<Value, DaemonError> {
            panic!("daemon should not be called for protocol-level methods: {method}");
        }
    }

    fn server() -> McpServer {
        McpServer::new(Arc::new(PanicClient), "test-agent")
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version_and_server_info() {
        let resp = server()
            .handle(McpRequest {
                jsonrpc: "2.0".into(),
                id: serde_json::json!(1),
                method: "initialize".into(),
                params: serde_json::json!({}),
            })
            .await;
        match resp.payload {
            McpPayload::Success { result } => {
                assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
                assert_eq!(result["serverInfo"]["name"], "ffs-mcp");
                assert!(result["capabilities"]["tools"].is_object());
            }
            other => panic!("expected Success; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tools_list_returns_the_six_mvp_tools() {
        let resp = server()
            .handle(McpRequest {
                jsonrpc: "2.0".into(),
                id: serde_json::json!(2),
                method: "tools/list".into(),
                params: serde_json::Value::Null,
            })
            .await;
        match resp.payload {
            McpPayload::Success { result } => {
                let tools = result["tools"].as_array().expect("tools array");
                let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
                assert!(names.contains(&"ffs_query"));
                assert!(names.contains(&"ffs_author_atom"));
                assert_eq!(names.len(), 6);
            }
            other => panic!("expected Success; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let resp = server()
            .handle(McpRequest {
                jsonrpc: "2.0".into(),
                id: serde_json::json!(3),
                method: "no.such.method".into(),
                params: serde_json::Value::Null,
            })
            .await;
        match resp.payload {
            McpPayload::Error { error } => {
                assert_eq!(error.code, protocol::ERR_METHOD_NOT_FOUND);
            }
            other => panic!("expected error; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wrong_jsonrpc_version_rejected_with_invalid_request() {
        let resp = server()
            .handle(McpRequest {
                jsonrpc: "1.0".into(),
                id: serde_json::json!(4),
                method: "initialize".into(),
                params: serde_json::json!({}),
            })
            .await;
        match resp.payload {
            McpPayload::Error { error } => {
                assert_eq!(error.code, protocol::ERR_INVALID_REQUEST);
            }
            other => panic!("expected error; got {other:?}"),
        }
    }
}
