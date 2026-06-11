// Windows-only end-to-end smoke for the `ffs-daemon` binary's
// named-pipe transport. Sibling of `binary_end_to_end.rs` (gated
// `#![cfg(unix)]`), exists because Windows had zero integration
// coverage of the named-pipe transport pre-task_34 and the
// `transport/windows.rs` module's only proof was "it compiles."
#![cfg(windows)]

//! End-to-end smoke test for the `ffs-daemon` binary on Windows
//! (task_34). Spawns the compiled daemon, waits for the named pipe
//! to become available, round-trips one `health.summary` JSON-RPC
//! call, asserts the result shape, then terminates the process.
//!
//! Unlike the Unix sibling, this test does NOT assert a clean
//! shutdown: Windows daemons don't trap SIGTERM (the Unix concept
//! doesn't exist there), and the daemon's signal handler on
//! Windows only listens for `ctrl_c()` — sending CTRL-C to a
//! detached child process requires console attachment plumbing
//! that's overkill for an integration smoke. We `taskkill /F`
//! the child instead and trust the RPC round-trip as the
//! transport-works signal.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::time::timeout;

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

/// Compute the named-pipe name the daemon binds. Mirrors
/// `transport::windows::default_socket_path()`.
fn pipe_name_for_current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    format!(r"\\.\pipe\ffs-{user}")
}

/// Poll the pipe until a client connect succeeds, or give up.
async fn connect_with_backoff(
    pipe_name: &str,
    max: Duration,
) -> tokio::net::windows::named_pipe::NamedPipeClient {
    let start = Instant::now();
    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => return client,
            Err(_) => {
                if start.elapsed() > max {
                    panic!("named pipe never became available: {pipe_name}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[tokio::test]
async fn daemon_binary_starts_and_serves_health_summary_via_named_pipe() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        // Pin the signing key and SQLCipher DEK so warn-level logs
        // are suppressed and the daemon comes up deterministically.
        .env(
            "FFS_OWNER_KEY_HEX",
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        // Keychain path isn't yet wired for Windows Credential
        // Manager testing; force the env-var route to keep the
        // surface tight.
        .env("FFS_KEYRING_DISABLE", "1")
        // Stability window must be zero so the ingest watcher
        // doesn't sit on an empty inbox indefinitely. The e2e
        // round-trip doesn't drop a file in ingest/ but a future
        // expansion might, and 0 is the documented test value.
        .env("FFS_INGEST_STABILITY_MS", "0")
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon binary");

    let pipe_name = pipe_name_for_current_user();
    let client = connect_with_backoff(&pipe_name, Duration::from_secs(10)).await;

    // Round-trip one `health.summary` JSON-RPC call. The pipe is a
    // duplex stream — `tokio::io::split` to address the two halves
    // independently so we can write the request and stream the
    // response without owning the whole thing twice.
    let (read_half, mut write_half) = tokio::io::split(client);
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "health.summary",
        "params": {},
    });
    let mut line = serde_json::to_vec(&req).unwrap();
    line.push(b'\n');
    write_half.write_all(&line).await.expect("write");
    write_half.flush().await.expect("flush");

    let mut reader = BufReader::new(read_half).lines();
    let response: serde_json::Value = loop {
        let next = timeout(Duration::from_secs(5), reader.next_line())
            .await
            .expect("read timed out")
            .expect("read")
            .expect("response line");
        let v: serde_json::Value = serde_json::from_str(&next).expect("valid json");
        // Skip notification frames; only the response carries an `id`.
        if v.get("id").is_some() {
            break v;
        }
    };

    assert_eq!(
        response.get("id").and_then(|v| v.as_u64()),
        Some(1),
        "response id should echo the request: {response}"
    );
    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("health response missing `result`: {response}"));
    assert!(
        result.get("atom_count").is_some(),
        "health summary missing atom_count: {result:?}"
    );

    // Best-effort terminate. The daemon's Windows signal handler
    // only listens for ctrl_c; sending CTRL-C to a detached child
    // requires console attachment we don't want for an integration
    // smoke. Force-kill via taskkill and let the OS reap.
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/F"])
        .status();
    let _ = child.wait();
}
