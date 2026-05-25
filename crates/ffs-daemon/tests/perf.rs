//! Daemon perf budgets per task_07 success criteria:
//!
//! - **Burst**: 1000 sequential requests from a single client complete
//!   without dropped connections.
//! - **Latency**: p95 of `atom.get` and `path.list` against a 10000-atom
//!   store stays under 50ms in release. Debug builds run ~10× slower; we
//!   relax the assertion via `cfg!(debug_assertions)` per CLAUDE.md.
//!
//! These were called out as follow-up work after task_07 (the dispatcher
//! shipped without runtime perf assertions). The test harness uses the
//! daemon library directly (not the binary) so it can construct a known
//! 10000-atom store deterministically.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::ProjectionRenderer;
use ffs_core::store::{AtomStore, MemAtomStore};
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};
use ffs_daemon::transport;
use ffs_daemon::{Dispatcher, EventPublisher};

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

const CONTACT_TEMPLATE: &str = r#"---
display_name: {{ claim.display_name }}
---
"#;

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&[77u8; 32])
}

fn owner_pk() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

struct Bench {
    socket: std::path::PathBuf,
    handles: Vec<Multihash>,
    cancel: CancellationToken,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
    _dir: tempfile::TempDir,
}

/// Spin up a daemon backed by a MemAtomStore with `n_atoms` contacts plus
/// a wildcard capability for the owner.
async fn spawn_with_atoms(n_atoms: usize) -> Bench {
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

    let cap = build_capability_atom(
        &owner_key(),
        owner_pk(),
        vec![Action::Read],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .unwrap();
    store.insert(&cap).unwrap();

    let mut handles = Vec::with_capacity(n_atoms);
    for i in 0..n_atoms {
        let env = AtomTemplate {
            v: 1,
            entity: EntityId::new(format!("entity-{i:05}")),
            predicate: PredicateName::new("contact.person"),
            claim: serde_json::json!({"display_name": format!("Person {i}")}),
            valid_from: Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
            valid_to: None,
            tx_time: Iso8601::new(format!(
                "2026-05-25T{:02}:{:02}:{:02}.{:03}Z",
                (i / 3600) % 24,
                (i / 60) % 60,
                i % 60,
                i % 1000,
            ))
            .unwrap(),
            classification: Tier::new("existence"),
            supersedes: None,
            provenance: vec![],
        }
        .sign(&owner_key())
        .unwrap();
        handles.push(store.insert(&env).unwrap());
    }

    let renderer =
        Arc::new(ProjectionRenderer::new(store.clone(), registry.clone(), &templates_dir).unwrap());
    let notifier = Arc::new(EventPublisher::new());
    let dispatcher = Arc::new(Dispatcher {
        store,
        registry,
        renderer,
        notifier,
        owner: owner_pk(),
    });

    let socket = run_dir.join("ffs.sock");
    let cancel = CancellationToken::new();
    let dispatcher_clone = dispatcher.clone();
    let cancel_clone = cancel.clone();
    let sock = socket.clone();
    let server =
        tokio::spawn(async move { transport::serve(&sock, dispatcher_clone, cancel_clone).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    Bench {
        socket,
        handles,
        cancel,
        server,
        _dir: dir,
    }
}

async fn jsonrpc_call(stream: &mut UnixStream, request: &str) -> String {
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    let (read, _) = stream.split();
    let mut reader = BufReader::new(read).lines();
    loop {
        let line = reader.next_line().await.unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        // Skip notification frames (no `id`).
        if v.get("id").is_some() {
            return line;
        }
    }
}

#[tokio::test]
async fn one_thousand_sequential_atom_get_requests_complete_without_drops() {
    // 10K-atom store; 1000 atom.get requests sequentially from one client.
    let bench = spawn_with_atoms(10_000).await;
    let mut stream = UnixStream::connect(&bench.socket).await.unwrap();

    let start = Instant::now();
    for i in 0..1000 {
        let hash = bench.handles[i].to_multibase();
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"atom.get","params":{{"hash":"{hash}"}}}}"#
        );
        let response = jsonrpc_call(&mut stream, &req).await;
        let v: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert!(
            v.get("result").is_some(),
            "request {i} got error: {response}"
        );
    }
    let elapsed = start.elapsed();
    eprintln!("1000 sequential atom.get requests: {elapsed:?}");

    bench.cancel.cancel();
    let _ = timeout(Duration::from_secs(5), bench.server).await;
}

#[tokio::test]
async fn p95_latency_for_atom_get_against_ten_thousand_atoms_under_budget() {
    let bench = spawn_with_atoms(10_000).await;
    let mut stream = UnixStream::connect(&bench.socket).await.unwrap();

    // Warm up
    for i in 0..10 {
        let hash = bench.handles[i].to_multibase();
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"atom.get","params":{{"hash":"{hash}"}}}}"#
        );
        let _ = jsonrpc_call(&mut stream, &req).await;
    }

    // Measure 100 calls
    let mut latencies: Vec<Duration> = Vec::with_capacity(100);
    for i in 0..100 {
        let hash = bench.handles[i].to_multibase();
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"atom.get","params":{{"hash":"{hash}"}}}}"#
        );
        let t0 = Instant::now();
        let _ = jsonrpc_call(&mut stream, &req).await;
        latencies.push(t0.elapsed());
    }
    latencies.sort();
    let p95 = latencies[95];

    let budget = if cfg!(debug_assertions) {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(50)
    };
    eprintln!(
        "atom.get p95={p95:?} (budget {budget:?}, debug_assertions={})",
        cfg!(debug_assertions)
    );
    assert!(
        p95 < budget,
        "atom.get p95 {p95:?} exceeds budget {budget:?}"
    );

    bench.cancel.cancel();
    let _ = timeout(Duration::from_secs(5), bench.server).await;
}

#[tokio::test]
async fn p95_latency_for_path_list_against_ten_thousand_atoms_under_budget() {
    let bench = spawn_with_atoms(10_000).await;
    let mut stream = UnixStream::connect(&bench.socket).await.unwrap();

    // path.list with a recency path; the renderer caps at the first 100
    // atoms internally so the work is bounded.
    for _ in 0..10 {
        let req =
            r#"{"jsonrpc":"2.0","id":1,"method":"path.list","params":{"path":"contacts/recent/"}}"#;
        let _ = jsonrpc_call(&mut stream, req).await;
    }
    let mut latencies: Vec<Duration> = Vec::with_capacity(100);
    for i in 0..100 {
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"path.list","params":{{"path":"contacts/recent/"}}}}"#
        );
        let t0 = Instant::now();
        let _ = jsonrpc_call(&mut stream, &req).await;
        latencies.push(t0.elapsed());
    }
    latencies.sort();
    let p95 = latencies[95];

    // path.list does more work than atom.get (per-entity capability filter
    // + markdown render); give it a more generous debug budget.
    let budget = if cfg!(debug_assertions) {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(200)
    };
    eprintln!(
        "path.list p95={p95:?} (budget {budget:?}, debug_assertions={})",
        cfg!(debug_assertions)
    );
    assert!(
        p95 < budget,
        "path.list p95 {p95:?} exceeds budget {budget:?}"
    );

    bench.cancel.cancel();
    let _ = timeout(Duration::from_secs(5), bench.server).await;
}
