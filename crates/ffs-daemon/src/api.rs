//! JSON-RPC 2.0 wire types and per-method parameter / result definitions.
//!
//! The wire framing is newline-delimited JSON: one request per line in,
//! one response per line out. Notifications (server-to-client events)
//! share the same line framing and the same `jsonrpc: "2.0"` field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use ffs_core::capability::Action;
use ffs_core::{EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};

// ---- JSON-RPC 2.0 envelope types ----

#[derive(Debug, Clone, Deserialize)]
pub struct ApiRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(flatten)]
    pub payload: ApiPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ApiPayload {
    Success { result: Value },
    Error { error: ApiError },
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ApiResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            payload: ApiPayload::Success { result },
        }
    }

    pub fn error(id: Value, error: ApiError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            payload: ApiPayload::Error { error },
        }
    }
}

// ---- error codes ----

// JSON-RPC 2.0 standard codes
pub const ERR_PARSE: i32 = -32700;
pub const ERR_INVALID_REQUEST: i32 = -32600;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INVALID_PARAMS: i32 = -32602;
pub const ERR_INTERNAL: i32 = -32603;

// FFS-specific application codes
pub const ERR_CAPABILITY_DENIED: i32 = 4001;
pub const ERR_NOT_FOUND: i32 = 4040;
pub const ERR_STORE: i32 = 5001;
pub const ERR_RENDER: i32 = 5002;
pub const ERR_NOT_IMPLEMENTED: i32 = 5003;

// ---- per-method params ----

#[derive(Debug, Clone, Deserialize)]
pub struct AtomGetParams {
    pub hash: Multihash,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AtomListParams {
    #[serde(default)]
    pub entity: Option<EntityId>,
    #[serde(default)]
    pub predicate: Option<PredicateName>,
    #[serde(default)]
    pub as_of: Option<Iso8601>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectionRenderParams {
    pub path: String,
    #[serde(default)]
    pub as_of: Option<Iso8601>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathListParams {
    pub path: String,
    #[serde(default)]
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestSubmitParams {
    pub source_uri: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FastpathSubmitParams {
    pub projection_path: String,
    pub new_content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityEvaluateParams {
    pub agent: PublicKey,
    pub action: Action,
    pub predicate: PredicateName,
    pub entity: EntityId,
    #[serde(default)]
    pub classification: Option<Tier>,
    #[serde(default)]
    pub tier: Option<Tier>,
    pub as_of: Iso8601,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FederationPeerAddParams {
    pub endpoint: String,
    pub fingerprint: Multihash,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FederationPullParams {
    pub peer: PublicKey,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PredicateInspectParams {
    pub name: PredicateName,
}

// ---- results ----

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityDecisionWire {
    pub allowed: bool,
    pub capability: Option<Multihash>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    pub proposals: u32,
    pub questions: u32,
    pub drift_flags: u32,
    pub atom_count: u64,
}
