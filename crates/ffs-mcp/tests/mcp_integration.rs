//! End-to-end integration: a stub MCP client speaks JSON-RPC through
//! `serve()` to an `McpServer` that's wired to a real `Dispatcher`
//! via an in-process `DaemonClient`. Exercises all four
//! required-by-spec integration scenarios:
//!
//! - `tools/list` returns the six MVP tools.
//! - `ffs_query` returns capability-filtered atoms.
//! - `ffs_author_atom` with an out-of-scope claim returns a
//!   structured capability error.
//! - `ffs_resolve_url` for `ffs://local/atom/<hash>` returns the atom.

use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::{ApiPayload, ApiRequest, Dispatcher, EventPublisher};
use ffs_federation::mount::InMemoryPeerMount;
use ffs_mcp::daemon_client::classify_daemon_error;
use ffs_mcp::{DaemonClient, DaemonError, McpServer, serve};

const CONTACT_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name"]
"#;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[91u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn stranger_pk() -> PublicKey {
    PublicKey::from_verifying(&SigningKey::from_bytes(&[92u8; 32]).verifying_key())
}

fn grant_full(store: &dyn AtomStore) {
    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![
            Action::Read,
            Action::Write,
            Action::Supersede,
            Action::Federate,
        ],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();
}

fn insert_contact(store: &dyn AtomStore, entity: &str, name: &str, tx_time: &str) -> Multihash {
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new(entity),
        predicate: PredicateName::new("contact.person"),
        claim: serde_json::json!({"display_name": name}),
        valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new(tx_time).unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .unwrap();
    store.insert(&env).unwrap()
}

fn make_dispatcher(grant_owner_caps: bool) -> (Arc<Dispatcher>, Arc<dyn AtomStore>) {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        "---\ndisplay_name: {{ claim.display_name }}\n---\n",
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    if grant_owner_caps {
        grant_full(&*store);
    }
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());

    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: if grant_owner_caps {
            owner_pk()
        } else {
            stranger_pk()
        },
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: None,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(InMemoryPeerMount::new()),
    });

    // Leak the tempdir so the test data files stay valid for the
    // duration of the test. The directory is in the OS temp space
    // and will be reclaimed at reboot.
    std::mem::forget(dir);
    (dispatcher, store)
}

/// In-process DaemonClient that wraps a `Dispatcher` directly — what
/// the integration tests use instead of opening a real UDS socket.
struct InProcessDaemonClient {
    dispatcher: Arc<Dispatcher>,
}

#[async_trait]
impl DaemonClient for InProcessDaemonClient {
    async fn call(&self, method: &str, params: Value) -> Result<Value, DaemonError> {
        let req = ApiRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: method.to_string(),
            params,
        };
        let resp = self.dispatcher.handle(req).await;
        match resp.payload {
            ApiPayload::Success { result } => Ok(result),
            ApiPayload::Error { error } => {
                Err(classify_daemon_error(error.code, error.message, error.data))
            }
        }
    }
}

/// Helper: send `requests` line-delimited, return the responses in order.
async fn round_trip(server: McpServer, requests: Vec<Value>) -> Vec<Value> {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (server_read, mut server_write) = tokio::io::split(server_side);
    let (client_read, mut client_write) = tokio::io::split(client_side);

    let handle = tokio::spawn(async move {
        let _ = serve(server, server_read, &mut server_write).await;
    });

    let mut payload = Vec::new();
    for r in &requests {
        payload.extend(serde_json::to_vec(r).unwrap());
        payload.push(b'\n');
    }
    client_write.write_all(&payload).await.unwrap();
    client_write.shutdown().await.unwrap();

    let mut lines = BufReader::new(client_read).lines();
    let mut out = Vec::with_capacity(requests.len());
    while let Some(line) = lines.next_line().await.unwrap() {
        out.push(serde_json::from_str(&line).unwrap());
    }
    handle.await.unwrap();
    out
}

#[tokio::test]
async fn tools_list_returns_six_tools_end_to_end() {
    let (dispatcher, _) = make_dispatcher(true);
    let server = McpServer::new(Arc::new(InProcessDaemonClient { dispatcher }), "test-agent");
    let responses = round_trip(
        server,
        vec![serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
        })],
    )
    .await;
    assert_eq!(responses.len(), 1);
    let tools = responses[0]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"ffs_query"));
    assert!(names.contains(&"ffs_author_atom"));
    assert!(names.contains(&"ffs_resolve_url"));
    assert!(names.contains(&"ffs_audit_query"));
}

#[tokio::test]
async fn ffs_query_returns_capability_filtered_atoms() {
    let (dispatcher, store) = make_dispatcher(true);
    insert_contact(&*store, "Sara_Chen", "Sara", "2026-05-27T08:00:00Z");
    let server = McpServer::new(Arc::new(InProcessDaemonClient { dispatcher }), "test-agent");
    let responses = round_trip(
        server,
        vec![serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "ffs_query",
                "arguments": {"entity": "Sara_Chen"}
            }
        })],
    )
    .await;
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    // The text block carries the daemon's JSON response (a Vec of
    // atom envelopes). Sara's atom should be in there.
    assert!(text.contains("Sara_Chen"), "got: {text}");
    assert!(text.contains("display_name"), "got: {text}");
}

#[tokio::test]
async fn ffs_author_atom_without_capability_returns_structured_mcp_error() {
    // Dispatcher built WITHOUT granting the owner capabilities, so
    // the daemon refuses the ingest.submit call. The MCP layer
    // surfaces this as a tool-level error (isError: true) with the
    // capability_denied kind in the details — not a JSON-RPC error.
    let (dispatcher, _) = make_dispatcher(false);
    let server = McpServer::new(Arc::new(InProcessDaemonClient { dispatcher }), "test-agent");
    let responses = round_trip(
        server,
        vec![serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "ffs_author_atom",
                "arguments": {
                    "content": "# private note an unauthorized agent should not author"
                }
            }
        })],
    )
    .await;
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true, "expected tool-level error");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("capability_denied"), "got: {text}");
}

#[tokio::test]
async fn ffs_resolve_url_atom_address_returns_the_atom() {
    let (dispatcher, store) = make_dispatcher(true);
    let hash = insert_contact(&*store, "Sara", "Sara", "2026-05-27T08:00:00Z");
    let url = format!("ffs://local/atom/{}", hash.to_multibase());
    let server = McpServer::new(Arc::new(InProcessDaemonClient { dispatcher }), "test-agent");
    let responses = round_trip(
        server,
        vec![serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "ffs_resolve_url",
                "arguments": {"url": url}
            }
        })],
    )
    .await;
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], false, "got: {result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    // The atom envelope's claim contains display_name = "Sara".
    assert!(text.contains("Sara"), "got: {text}");
    assert!(text.contains("contact.person"), "got: {text}");
}

#[tokio::test]
async fn initialize_returns_protocol_version_through_stdio_loop() {
    let (dispatcher, _) = make_dispatcher(true);
    let server = McpServer::new(Arc::new(InProcessDaemonClient { dispatcher }), "test-agent");
    let responses = round_trip(
        server,
        vec![serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {}
        })],
    )
    .await;
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "ffs-mcp");
}
