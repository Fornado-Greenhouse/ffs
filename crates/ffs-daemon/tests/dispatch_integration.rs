//! Dispatcher + transport integration tests.
//!
//! Coverage:
//! - Method dispatch routes every documented method (or returns
//!   ERR_NOT_IMPLEMENTED for the five tasks-pending stubs) with correct
//!   parameter deserialization.
//! - Capability denial on atom.get returns the structured ERR_CAPABILITY_
//!   DENIED code with a typed reason.
//! - The notification publisher fan-outs an event to multiple subscribers
//!   and the on-the-wire frame is JSON-RPC 2.0 shaped.
//! - Two UDS clients connected to the same daemon both receive a
//!   published event.
//! - The daemon refuses to bind when the parent directory is world-
//!   writable (Unix-only, mirrors the macOS test case in the spec).
//! - Cancellation removes the socket file (SIGTERM equivalent).
//! - health.summary returns counts consistent with the underlying store.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::quarantine::InMemoryQuarantine;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::working_set::InMemoryWorkingSet;
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::api::{
    ApiPayload, ApiRequest, ApiResponse, ERR_CAPABILITY_DENIED, ERR_METHOD_NOT_FOUND,
    ERR_NOT_FOUND, ERR_NOT_IMPLEMENTED,
};
use ffs_daemon::notify::{Event, EventPublisher};
use ffs_daemon::transport;
use ffs_daemon::{Dispatcher, dispatch};

// ---- fixtures ----

const CONTACT_TOML: &str = r#"
name = "contact.person"
version = 1

[claim_schema]
type = "object"
required = ["display_name"]

[claim_schema.properties]
display_name = { type = "string" }
work_email = { type = "string" }

[rendering]
template = "contact-person.md.tera"
frontmatter_fields = ["display_name", "work_email"]

[[reverse_map]]
output = "frontmatter.display_name"
atom_field = "claim.display_name"
edit_kind = "single_line_text"
"#;

const CONTACT_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
{% if claim.work_email %}work_email: {{ claim.work_email }}
{% endif %}---
"#;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[33u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

struct Harness {
    _dir: tempfile::TempDir,
    dispatcher: Arc<Dispatcher>,
    store: Arc<dyn AtomStore>,
}

fn setup() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store: store.clone(),
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: None,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
    });
    Harness {
        _dir: dir,
        dispatcher,
        store,
    }
}

fn grant_full_capability(store: &dyn AtomStore, grantee: &PublicKey) -> Multihash {
    let cap = build_capability_atom(
        &owner_key(),
        grantee.clone(),
        vec![
            Action::Read,
            Action::Write,
            Action::Supersede,
            Action::Erase,
            Action::Classify,
            Action::Federate,
        ],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap()
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

fn req(id: u64, method: &str, params: serde_json::Value) -> ApiRequest {
    ApiRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(id),
        method: method.into(),
        params,
    }
}

fn error_code(resp: &ApiResponse) -> Option<i32> {
    match &resp.payload {
        ApiPayload::Error { error } => Some(error.code),
        _ => None,
    }
}

fn success_result(resp: &ApiResponse) -> Option<&serde_json::Value> {
    match &resp.payload {
        ApiPayload::Success { result } => Some(result),
        _ => None,
    }
}

// ---- unit-style dispatcher tests ----

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let h = setup();
    let resp = h
        .dispatcher
        .handle(req(1, "no.such.method", serde_json::Value::Null))
        .await;
    assert_eq!(error_code(&resp), Some(ERR_METHOD_NOT_FOUND));
}

#[tokio::test]
async fn atom_get_returns_envelope_when_authorized() {
    let h = setup();
    grant_full_capability(&*h.store, &owner_pk());
    let hash = insert_contact(&*h.store, "alice", "Alice", "2026-05-25T08:00:00Z");
    let resp = h
        .dispatcher
        .handle(req(
            2,
            "atom.get",
            serde_json::json!({"hash": hash.to_multibase()}),
        ))
        .await;
    let result = success_result(&resp).expect("expected Success");
    assert_eq!(
        result["entity"], "alice",
        "result should embed the atom envelope; got {result}"
    );
}

#[tokio::test]
async fn atom_get_without_capability_returns_capability_denied() {
    let h = setup();
    let hash = insert_contact(&*h.store, "alice", "Alice", "2026-05-25T08:00:00Z");
    let resp = h
        .dispatcher
        .handle(req(
            3,
            "atom.get",
            serde_json::json!({"hash": hash.to_multibase()}),
        ))
        .await;
    assert_eq!(
        error_code(&resp),
        Some(ERR_CAPABILITY_DENIED),
        "expected capability-denied; got {:?}",
        resp.payload
    );
}

#[tokio::test]
async fn atom_get_for_missing_hash_returns_not_found() {
    let h = setup();
    grant_full_capability(&*h.store, &owner_pk());
    let bogus = Multihash::blake3_of(b"not in store").to_multibase();
    let resp = h
        .dispatcher
        .handle(req(4, "atom.get", serde_json::json!({"hash": bogus})))
        .await;
    assert_eq!(error_code(&resp), Some(ERR_NOT_FOUND));
}

#[tokio::test]
async fn ingest_submit_returns_submission_id_without_scribe_configured() {
    // task_11 implemented ingest.submit. With no scribe wired in, the
    // call still succeeds (capability-checked) and stores a Pending
    // submission. The full pipeline (scribe → Extracted) is exercised
    // in scribe_integration.rs.
    let h = setup();
    grant_full_capability(&*h.store, &owner_pk());
    let resp = h
        .dispatcher
        .handle(req(
            5,
            "ingest.submit",
            serde_json::json!({"source_uri": "file:///x", "content": "anything"}),
        ))
        .await;
    let id = match &resp.payload {
        ffs_daemon::ApiPayload::Success { result } => result["submission_id"]
            .as_str()
            .expect("submission_id")
            .to_string(),
        ffs_daemon::ApiPayload::Error { error } => {
            panic!("expected success; got error: {error:?}");
        }
    };
    assert!(id.starts_with("sub-"), "id should be hash-tagged: {id}");
}

#[tokio::test]
async fn federation_pull_without_client_returns_not_implemented() {
    // task_15 implemented federation.pull. Without a configured
    // federation_client on the dispatcher (read-only test harness),
    // the handler degrades to ERR_NOT_IMPLEMENTED rather than
    // panicking — the full pull is exercised in
    // federation_pull_integration.rs.
    let h = setup();
    let resp = h
        .dispatcher
        .handle(req(
            6,
            "federation.pull",
            serde_json::json!({"peer_id": "alice"}),
        ))
        .await;
    assert_eq!(error_code(&resp), Some(ERR_NOT_IMPLEMENTED));
}

#[tokio::test]
async fn predicate_inspect_returns_loaded_spec() {
    let h = setup();
    let resp = h
        .dispatcher
        .handle(req(
            7,
            "predicate.inspect",
            serde_json::json!({"name": "contact.person"}),
        ))
        .await;
    let result = success_result(&resp).expect("expected Success");
    assert_eq!(result["name"], "contact.person");
    assert_eq!(result["version"], 1);
}

#[tokio::test]
async fn predicate_inspect_unknown_returns_not_found() {
    let h = setup();
    let resp = h
        .dispatcher
        .handle(req(
            8,
            "predicate.inspect",
            serde_json::json!({"name": "no.such"}),
        ))
        .await;
    assert_eq!(error_code(&resp), Some(ERR_NOT_FOUND));
}

#[tokio::test]
async fn health_summary_returns_zeroed_counts_at_mvp() {
    let h = setup();
    let resp = h
        .dispatcher
        .handle(req(9, "health.summary", serde_json::Value::Null))
        .await;
    let result = success_result(&resp).expect("expected Success");
    assert_eq!(result["proposals"], 0);
    assert_eq!(result["questions"], 0);
    assert_eq!(result["drift_flags"], 0);
}

#[tokio::test]
async fn capability_evaluate_returns_decision() {
    let h = setup();
    grant_full_capability(&*h.store, &owner_pk());
    let resp = h
        .dispatcher
        .handle(req(
            10,
            "capability.evaluate",
            serde_json::json!({
                "agent": owner_pk(),
                "action": "read",
                "predicate": "contact.person",
                "entity": "alice",
                "as_of": "2026-12-31T23:59:59Z",
            }),
        ))
        .await;
    let result = success_result(&resp).expect("expected Success");
    assert_eq!(result["allowed"], true);
    assert!(result["capability"].is_string());
}

#[tokio::test]
async fn jsonrpc_version_must_be_two_point_zero() {
    let h = setup();
    let bad = ApiRequest {
        jsonrpc: "1.0".into(),
        id: serde_json::json!(1),
        method: "health.summary".into(),
        params: serde_json::Value::Null,
    };
    let resp = h.dispatcher.handle(bad).await;
    assert!(matches!(resp.payload, ApiPayload::Error { .. }));
}

// ---- notification publisher tests ----

#[tokio::test]
async fn publish_event_reaches_multiple_subscribers() {
    let pub_ = EventPublisher::new();
    let mut sub1 = pub_.subscribe();
    let mut sub2 = pub_.subscribe();
    let n = pub_.publish(Event::AtomCommitted {
        hash: Multihash::blake3_of(b"x"),
        entity: EntityId::new("e"),
        predicate: PredicateName::new("p"),
    });
    assert_eq!(n, 2, "expected 2 subscribers to receive");
    let line1 = sub1.recv().await.unwrap();
    let line2 = sub2.recv().await.unwrap();
    assert_eq!(line1, line2);
    let v: serde_json::Value = serde_json::from_str(&line1).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["method"], "event.atom.committed");
}

#[tokio::test]
async fn published_event_is_jsonrpc_notification_shaped() {
    let pub_ = EventPublisher::new();
    let mut sub = pub_.subscribe();
    pub_.publish(Event::ProjectionInvalidated {
        path: "contacts/recent/".into(),
    });
    let line = sub.recv().await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["method"], "event.projection.invalidated");
    assert_eq!(v["params"]["path"], "contacts/recent/");
}

// ---- transport (UDS) integration ----

async fn spawn_server() -> (
    std::path::PathBuf,
    Arc<Dispatcher>,
    Arc<EventPublisher>,
    CancellationToken,
    tokio::task::JoinHandle<std::io::Result<()>>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let predicates_dir = dir.path().join("predicates");
    let templates_dir = dir.path().join("templates");
    let run_dir = dir.path().join("run");
    std::fs::create_dir_all(&predicates_dir).unwrap();
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(predicates_dir.join("contact.person.toml"), CONTACT_TOML).unwrap();
    std::fs::write(
        templates_dir.join("contact-person.md.tera"),
        CONTACT_TEMPLATE,
    )
    .unwrap();

    let registry = Arc::new(SpecRegistry::new());
    registry.load_dir(&predicates_dir).unwrap();
    let store: Arc<dyn AtomStore> = Arc::new(MemAtomStore::new());
    grant_full_capability(&*store, &owner_pk());
    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store,
        registry,
        renderer,
        notifier: notifier.clone(),
        owner: owner_pk(),
        quarantine: Arc::new(InMemoryQuarantine::new()),
        scribe: None,
        working_set: Arc::new(InMemoryWorkingSet::new()),
        signing_key: None,
        federation_peers: Arc::new(ffs_core::federation_peers::InMemoryFederationPeerStore::new()),
        federation_client: None,
        our_cert_fingerprint: None,
        peer_mounts: Arc::new(ffs_federation::mount::InMemoryPeerMount::new()),
    });

    let socket = run_dir.join("ffs.sock");
    let cancel = CancellationToken::new();
    let server_dispatcher = dispatcher.clone();
    let server_cancel = cancel.clone();
    let sock = socket.clone();
    let handle =
        tokio::spawn(
            async move { transport::serve(&sock, server_dispatcher, server_cancel).await },
        );
    // Allow the listener to bind.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (socket, dispatcher, notifier, cancel, handle, dir)
}

async fn rpc_call(stream: &mut UnixStream, line: &str) -> String {
    stream.write_all(line.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    let (read, _) = stream.split();
    let mut reader = BufReader::new(read).lines();
    timeout(Duration::from_secs(2), reader.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn uds_round_trip_health_summary() {
    let (socket, _dispatcher, _notifier, cancel, server, _dir) = spawn_server().await;
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    let line = rpc_call(
        &mut stream,
        r#"{"jsonrpc":"2.0","id":1,"method":"health.summary","params":null}"#,
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    assert!(v["result"].is_object());

    cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server).await;
    // Socket should be removed.
    assert!(!socket.exists(), "socket should be removed on shutdown");
}

#[tokio::test]
async fn uds_two_clients_both_receive_published_event() {
    let (socket, _dispatcher, notifier, cancel, server, _dir) = spawn_server().await;

    // Two long-lived connections.
    let mut s1 = UnixStream::connect(&socket).await.unwrap();
    let mut s2 = UnixStream::connect(&socket).await.unwrap();
    // Wait briefly so the server-side event subscriptions get installed.
    tokio::time::sleep(Duration::from_millis(50)).await;

    notifier.publish(Event::ProjectionInvalidated {
        path: "contacts/recent/".into(),
    });

    // Each client should read the published event line. (Note: clients may
    // also be sending requests in parallel; for this test we only read.)
    async fn read_line(s: &mut UnixStream) -> String {
        let (read, _) = s.split();
        let mut reader = BufReader::new(read).lines();
        timeout(Duration::from_secs(2), reader.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }
    let l1 = read_line(&mut s1).await;
    let l2 = read_line(&mut s2).await;
    let v1: serde_json::Value = serde_json::from_str(&l1).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&l2).unwrap();
    assert_eq!(v1["method"], "event.projection.invalidated");
    assert_eq!(v2["method"], "event.projection.invalidated");

    cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server).await;
}

#[tokio::test]
async fn refuses_to_bind_when_parent_dir_is_world_writable() {
    let dir = tempfile::tempdir().unwrap();
    let unsafe_run = dir.path().join("run");
    std::fs::create_dir_all(&unsafe_run).unwrap();
    std::fs::set_permissions(&unsafe_run, std::fs::Permissions::from_mode(0o777)).unwrap();
    let socket = unsafe_run.join("ffs.sock");

    let h = setup();
    let cancel = CancellationToken::new();
    let result = transport::serve(&socket, h.dispatcher.clone(), cancel).await;
    let err = result.expect_err("expected bind to fail under world-writable parent");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied,
        "expected PermissionDenied; got {err}"
    );
}

#[tokio::test]
async fn cancellation_removes_the_socket_file() {
    let (socket, _, _, cancel, server, _dir) = spawn_server().await;
    assert!(socket.exists());
    cancel.cancel();
    let _ = timeout(Duration::from_secs(2), server).await;
    assert!(!socket.exists());
}

// Marker reference so the dispatch module isn't pruned by unused-warning
// gates as the crate grows.
#[allow(dead_code)]
fn _marker() -> Option<dispatch::Dispatcher> {
    None
}
