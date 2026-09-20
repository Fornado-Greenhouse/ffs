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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(flatten)]
    pub payload: ApiPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApiPayload {
    Success { result: Value },
    Error { error: ApiError },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
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
    pub peer_id: String,
    pub peer_pubkey: PublicKey,
    pub endpoint: String,
    pub fingerprint: Multihash,
}

impl FederationPeerAddParams {
    pub fn peer_id_for_target(&self) -> &str {
        &self.peer_id
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BridgeEstablishParams {
    pub peer_id: String,
    pub our_capability: Multihash,
    pub our_vocab: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BridgeRotateParams {
    pub peer_id: String,
    pub new_fingerprint: Multihash,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FederationPullParams {
    /// Local peer-id (matches `federation.peer.add`'s `peer_id`).
    pub peer_id: String,
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
pub struct IngestSubmitResult {
    pub submission_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestAcceptParams {
    pub submission_id: String,
    /// The owner's picks for `ambiguous` proposals (ADR-030), keyed by
    /// the proposal's `local_ref` (or `#<index>` when it has none):
    /// an entity id to bind, or the literal `"new"` to mint. Required
    /// for every ambiguous proposal; accept refuses otherwise.
    #[serde(default)]
    pub choices: std::collections::HashMap<String, String>,
    /// Shorthand for the common single-ambiguity case (task_39): an
    /// entity id or `"new"` applied to the submission's one ambiguous
    /// proposal. With more than one ambiguous proposal, use `choices`.
    #[serde(default)]
    pub resolved_entity: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestRejectParams {
    pub submission_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntitySearchParams {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// Exact predicate filter (task_40, `entity.search` v2).
    #[serde(default)]
    pub predicate: Option<String>,
    /// Path-family filter (`contacts`, `orgs`, ...; ADR-028).
    #[serde(default)]
    pub family: Option<String>,
    /// An entity id whose `entity.different_from` assertions exclude
    /// candidates (ADR-030 hard block).
    #[serde(default)]
    pub context_entity: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntitySearchHit {
    pub entity: EntityId,
    pub predicate: PredicateName,
    pub display_name: String,
    /// File basename in the entity's projection family (ADR-030), when
    /// the path index has a row for it. Lets the plugin open
    /// `Sara_Chen_(Acme).md` rather than guessing from the display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basename: Option<String>,
    /// Projection path (`<family>/by-name/<L>/<basename>.md`) when the
    /// entity has a family and a basename (task_40).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The tier that produced the hit, first: `display_name`, `alias`,
    /// `fts`, or `other` (any other schema-declared string field).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_on: Vec<String>,
    /// Ranking score: tier base plus exact-match bonus plus the
    /// commonness prior `ln(1 + accepted resolutions)` (ADR-030).
    #[serde(default)]
    pub score: f64,
    /// Head atom's `tx_time`, the final tie-breaker (newest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_time: Option<Iso8601>,
}

/// What the courier skill wrote after its last tick
/// (`$FFS_DATA_DIR/ingest/.courier/last_run.json`, task_40). Surfaced
/// unchanged by `health.summary.courier` and `courier.status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CourierStatus {
    #[serde(default)]
    pub last_run: Option<String>,
    #[serde(default)]
    pub items_seen: u64,
    #[serde(default)]
    pub files_written: u64,
    #[serde(default)]
    pub fetch_failures: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CourierRunParams {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkingSetTouchParams {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkingSetPinParams {
    pub path: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkingSetMaterializeParams {
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkingSetMaterializeResult {
    pub path: String,
    pub render_hash: Multihash,
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkingSetRefreshed {
    pub path: String,
    pub render_hash: Multihash,
    pub markdown: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkingSetEvictParams {
    pub cap: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuditPublishParams {
    /// Structured claim payload the auditor produced. Embedded
    /// verbatim into the signed `auditor.daily_summary` atom.
    pub claim: Value,
    /// Optional bitemporal anchor. Defaults to "now" if omitted.
    #[serde(default)]
    pub valid_from: Option<Iso8601>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuditQueryParams {
    /// Optional lower-bound tx_time filter. When omitted, returns
    /// every auditor.daily_summary atom in the store.
    #[serde(default)]
    pub since: Option<Iso8601>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditPublishResult {
    pub atom_hash: Multihash,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    pub proposals: u32,
    pub questions: u32,
    pub drift_flags: u32,
    pub atom_count: u64,
    /// Courier's last-tick counters, or `null` when it has never run
    /// (task_40). Serialized as null rather than omitted so the
    /// auditor and the plugin can tell "no courier" from "old daemon".
    #[serde(default)]
    pub courier: Option<CourierStatus>,
    /// What the quarantine auto-filed in the last 24 hours (ADR-029).
    #[serde(default)]
    pub auto_filed: AutoFiledSummary,
}

// ---- task_39: auto-filing, retraction, identity assertions, capability admin ----

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IngestListAutoFiledParams {
    /// Optional lower bound on the submission's `tx_time`.
    #[serde(default)]
    pub since: Option<Iso8601>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestRetractParams {
    /// Multibase content hash of the atom to retract; it must be the
    /// current head of its chain.
    pub atom_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityAssertDifferentParams {
    pub a: EntityId,
    pub b: EntityId,
    #[serde(default)]
    pub criterion: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityMergeParams {
    /// The losing entity: its atoms stay in place and render under `target`.
    pub source: EntityId,
    pub target: EntityId,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub criterion: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityUnmergeParams {
    /// Multibase content hash of the `entity.same_as` atom to undo.
    pub same_as_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityGrantParams {
    /// One of `read | write | supersede | accept | erase | classify | federate`.
    pub action: String,
    /// An Ed25519 multibase key, or an agent identity string such as
    /// `mcp:agent/courier` (mapped to a deterministic key-shaped id).
    pub grantee: String,
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(default)]
    pub classifications: Vec<String>,
    /// Daily cap on auto-filed atoms (ADR-029). Required for `accept`
    /// unless `unlimited` is true.
    #[serde(default)]
    pub max_per_day: Option<u32>,
    #[serde(default)]
    pub unlimited: bool,
    #[serde(default)]
    pub valid_to: Option<Iso8601>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityRevokeParams {
    pub grant_hash: String,
}

/// One active grant as `capability.list` reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityListEntry {
    pub grant_hash: String,
    pub grantee: String,
    pub actions: Vec<String>,
    pub predicates: Option<Vec<String>>,
    pub classifications: Option<Vec<String>>,
    pub max_per_day: Option<u32>,
    pub used_today: u32,
    pub valid_from: Iso8601,
    pub valid_to: Option<Iso8601>,
}

/// One auto-filed (or merged / unmerged) atom as `ingest.list_auto_filed`
/// and `health.summary.auto_filed` report it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoFiledItem {
    pub hash: String,
    pub entity: EntityId,
    pub predicate: PredicateName,
    pub source_uri: String,
    pub tx_time: Iso8601,
    #[serde(default)]
    pub submission_id: Option<String>,
    /// `auto_accept`, `merge`, or `unmerge`.
    pub kind: String,
}

/// The `auto_filed` section of `health.summary` (ADR-029): what the
/// quarantine filed on its own in the window.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AutoFiledSummary {
    pub count: u32,
    pub by_predicate: std::collections::BTreeMap<String, u32>,
    pub items: Vec<AutoFiledItem>,
}
