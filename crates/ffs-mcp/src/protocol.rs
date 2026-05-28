//! MCP wire types for the three protocol methods this server speaks:
//! `initialize`, `tools/list`, and `tools/call`. Each is JSON-RPC 2.0
//! over line-delimited JSON, mirroring the framing the MCP spec uses
//! over stdio.
//!
//! The server returns:
//!
//! - JSON-RPC errors (`error.code`) for protocol-level problems
//!   (parse errors, unknown methods, bad params).
//! - Tool-level errors as a `tools/call` *result* with
//!   `isError: true` and a structured `content` payload —
//!   this is the MCP-canonical shape so any MCP-aware client can
//!   surface the failure to the user.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version this server speaks. Sent on `initialize`. MCP
/// versions are date-coded; pin a known one so unsupported clients
/// can warn-or-fail cleanly.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Standard JSON-RPC 2.0 error codes plus MCP-specific application
/// codes. Mirrors the daemon's `api` constants for the ones that
/// flow through both layers.
pub const ERR_PARSE: i32 = -32700;
pub const ERR_INVALID_REQUEST: i32 = -32600;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INVALID_PARAMS: i32 = -32602;
pub const ERR_INTERNAL: i32 = -32603;

#[derive(Debug, Clone, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(flatten)]
    pub payload: McpPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum McpPayload {
    Success { result: Value },
    Error { error: McpError },
}

#[derive(Debug, Clone, Serialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl McpResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            payload: McpPayload::Success { result },
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            payload: McpPayload::Error {
                error: McpError {
                    code,
                    message: message.into(),
                    data,
                },
            },
        }
    }
}

// ---- initialize ----

#[derive(Debug, Clone, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    pub capabilities: ServerCapabilities,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    /// MCP signals tool support by presence of this key. The empty
    /// object means "this server has tools but no list-changed
    /// notifications" — the standard shape for static tool sets.
    pub tools: serde_json::Map<String, Value>,
}

// ---- tools/list ----

#[derive(Debug, Clone, Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

// ---- tools/call ----

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text { text: String },
}

impl ToolCallResult {
    /// Build a successful result whose single text block carries the
    /// tool's JSON output. Agents see structured JSON they can
    /// parse; humans see a readable payload.
    pub fn success_json(value: &Value) -> Self {
        let text = serde_json::to_string_pretty(value)
            .unwrap_or_else(|e| format!("<failed to serialize result: {e}>"));
        Self {
            content: vec![ToolContent::Text { text }],
            is_error: false,
        }
    }

    /// Build a tool-level error: still a successful JSON-RPC
    /// response, but with `isError: true` so the MCP client can
    /// surface the failure without treating it as a transport break.
    pub fn tool_error(message: impl Into<String>, extra: Option<Value>) -> Self {
        let body = match extra {
            Some(extra) => serde_json::json!({
                "error": message.into(),
                "details": extra,
            }),
            None => serde_json::json!({"error": message.into()}),
        };
        let text = serde_json::to_string_pretty(&body)
            .unwrap_or_else(|_| "{\"error\":\"<serialization failed>\"}".into());
        Self {
            content: vec![ToolContent::Text { text }],
            is_error: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_result_serializes_with_canonical_keys() {
        let r = InitializeResult {
            protocol_version: PROTOCOL_VERSION.into(),
            server_info: ServerInfo {
                name: "ffs-mcp".into(),
                version: "0.1.0".into(),
            },
            capabilities: ServerCapabilities {
                tools: serde_json::Map::new(),
            },
        };
        let s = serde_json::to_value(&r).unwrap();
        assert_eq!(s["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(s["serverInfo"]["name"], "ffs-mcp");
        assert!(s["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tool_serializes_with_input_schema_key() {
        let t = Tool {
            name: "ffs_query".into(),
            description: "query atoms".into(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let s = serde_json::to_value(&t).unwrap();
        assert_eq!(s["inputSchema"]["type"], "object");
    }

    #[test]
    fn tool_call_result_success_marks_is_error_false() {
        let r = ToolCallResult::success_json(&serde_json::json!({"hello": "world"}));
        assert!(!r.is_error);
        assert_eq!(r.content.len(), 1);
        let ToolContent::Text { text } = &r.content[0];
        assert!(text.contains("hello"));
    }

    #[test]
    fn tool_call_result_tool_error_marks_is_error_true() {
        let r = ToolCallResult::tool_error("capability denied", None);
        assert!(r.is_error);
    }

    #[test]
    fn response_round_trips_as_jsonrpc_envelope() {
        let resp = McpResponse::success(serde_json::json!(1), serde_json::json!({"ok": true}));
        let s = serde_json::to_string(&resp).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["ok"], true);
    }

    #[test]
    fn response_error_serializes_as_jsonrpc_error() {
        let resp = McpResponse::error(serde_json::json!(2), ERR_METHOD_NOT_FOUND, "nope", None);
        let s = serde_json::to_string(&resp).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["error"]["code"], ERR_METHOD_NOT_FOUND);
    }
}
