// Daemon binary spawn + UDS round-trip is Unix-only.
#![cfg(unix)]

//! End-to-end smoke test for the `ffs-daemon` binary (task_22).
//!
//! This is the "now it actually runs" test: it spawns the compiled
//! daemon binary, hands it a temp `FFS_DATA_DIR` seeded with the
//! starter library, waits for the socket to appear, runs two
//! JSON-RPC methods through it, and asserts the daemon shuts down
//! cleanly on SIGTERM (socket file removed, exit code 0).
//!
//! Why this lives in `tests/`, not as a unit test:
//!
//! - `cargo test` for a binary crate can't call `main()` directly.
//!   It exposes the binary path through `env!("CARGO_BIN_EXE_<name>")`
//!   instead. Integration tests run with `cargo nextest` automatically
//!   build the bin first, so the env var is populated.
//! - The binary's startup wiring (env-var config, predicate dir,
//!   templates dir, signing-key bootstrap, UDS bind, signal handling)
//!   only matters when exercised as a whole.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Wait for `path` to appear (the daemon creates the UDS once bound).
async fn wait_for(path: &Path, max: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < max {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Wait for `path` to disappear (the daemon removes it on graceful
/// shutdown).
async fn wait_for_removal(path: &Path, max: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < max {
        if !path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn rpc(socket: &Path, method: &str, params: serde_json::Value) -> serde_json::Value {
    let stream = UnixStream::connect(socket).await.expect("connect");
    let (read_half, mut write_half) = stream.into_split();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_vec(&req).unwrap();
    line.push(b'\n');
    write_half.write_all(&line).await.unwrap();
    write_half.flush().await.unwrap();
    let mut reader = BufReader::new(read_half).lines();
    loop {
        let next = timeout(Duration::from_secs(2), reader.next_line())
            .await
            .expect("read timed out")
            .expect("read")
            .expect("response line");
        let v: serde_json::Value = serde_json::from_str(&next).unwrap();
        if v.get("id").is_none() {
            // skip notification frames
            continue;
        }
        return v;
    }
}

fn repo_root() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace root
    p
}

fn seed_data_dir(root: &Path) {
    let predicates_src = repo_root().join("starter").join("predicates");
    let templates_src = repo_root().join("starter").join("templates");
    let predicates_dst = root.join("config").join("predicates");
    let templates_dst = root.join("config").join("templates");
    std::fs::create_dir_all(&predicates_dst).unwrap();
    std::fs::create_dir_all(&templates_dst).unwrap();
    for entry in std::fs::read_dir(&predicates_src).unwrap() {
        let e = entry.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), predicates_dst.join(e.file_name())).unwrap();
        }
    }
    for entry in std::fs::read_dir(&templates_src).unwrap() {
        let e = entry.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), templates_dst.join(e.file_name())).unwrap();
        }
    }
}

#[tokio::test]
async fn daemon_binary_starts_serves_rpc_then_shuts_down_on_sigterm() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);
    // Ensure parent of the run dir isn't world-writable (the daemon's
    // permission check on macOS refuses to bind into 0o777 trees).
    std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        // Pin the signing key and the SQLCipher DEK so warn logs
        // are suppressed and the test is reproducible — without
        // these envs the daemon generates fresh values per boot.
        .env(
            "FFS_OWNER_KEY_HEX",
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon binary");

    let socket = data_dir.join("run").join("ffs.sock");
    assert!(
        wait_for(&socket, Duration::from_secs(5)).await,
        "daemon never created the socket at {}",
        socket.display()
    );

    // health.summary should always succeed (no caps required).
    let health = rpc(&socket, "health.summary", serde_json::Value::Null).await;
    let result = health
        .get("result")
        .unwrap_or_else(|| panic!("health response: {health}"));
    assert!(
        result.get("atom_count").is_some(),
        "health summary missing atom_count: {result:?}"
    );

    // predicate.inspect for a known starter predicate.
    let pred = rpc(
        &socket,
        "predicate.inspect",
        serde_json::json!({"name": "contact.person"}),
    )
    .await;
    let pred_result = pred.get("result").expect("predicate result");
    assert_eq!(
        pred_result
            .get("rendering")
            .and_then(|r| r.get("template"))
            .and_then(|t| t.as_str()),
        Some("contact-person.md.tera")
    );

    // SIGTERM via the `kill` utility — Tokio's signal handler in the
    // daemon traps this and triggers the CancellationToken path. We
    // shell out to `kill` to avoid pulling `libc` in as a dep just
    // for one signal call.
    let pid = child.id();
    let kill_status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("invoke kill");
    assert!(kill_status.success(), "kill -TERM failed: {kill_status:?}");

    // Wait for socket removal as a positive shutdown signal (the
    // daemon explicitly removes it before returning).
    assert!(
        wait_for_removal(&socket, Duration::from_secs(5)).await,
        "socket not removed after SIGTERM"
    );

    let status = child.wait().expect("wait");
    assert!(status.success(), "daemon exited non-zero: {status:?}");
    let mut stderr_bytes = Vec::new();
    if let Some(mut e) = child.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_end(&mut stderr_bytes);
    }
    // Surface stderr if we ever see a failure investigating this test.
    if !status.success() {
        std::io::stderr().write_all(&stderr_bytes).ok();
    }
}
