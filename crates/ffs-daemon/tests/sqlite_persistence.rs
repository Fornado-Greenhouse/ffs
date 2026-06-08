// Daemon binary spawn + UDS round-trip is Unix-only.
#![cfg(unix)]

//! SQLite atom-store persistence tests for the ffs-daemon binary
//! (task_24).
//!
//! Validates the wiring from `MemAtomStore` → `SqliteAtomStore`:
//!
//! - The daemon binary persists atoms across restarts when the
//!   same `FFS_SQLCIPHER_KEY_HEX` is supplied.
//! - Starting the daemon against an existing `atoms.db` with the
//!   wrong DEK surfaces the failure as a startup error (non-zero
//!   exit) instead of silently masking it.
//!
//! The "writes via the daemon" path is gated by capability checks
//! that the daemon binary doesn't pre-grant — so these tests
//! pre-seed `atoms.db` directly via `SqliteAtomStore`, start the
//! daemon, and verify the daemon reads what was pre-seeded. That
//! covers the substantive persistence claim ("a daemon-restartable
//! SQLite file") without needing a multi-RPC capability bootstrap.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
use ffs_core::store::{AtomStore, SqliteAtomStore};
use ffs_core::{AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, PublicKey, Tier};

const OWNER_KEY_HEX: &str = "0505050505050505050505050505050505050505050505050505050505050505";
const DEK_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const WRONG_DEK_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn owner_seed() -> [u8; 32] {
    [5u8; 32]
}

fn owner_key() -> SigningKey {
    SigningKey::from_bytes(&owner_seed())
}

fn owner_pubkey() -> PublicKey {
    PublicKey::from_verifying(&owner_key().verifying_key())
}

fn dek() -> [u8; 32] {
    [0x11u8; 32]
}

fn repo_root() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace root
    p
}

fn seed_data_dir(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
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
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Pre-seed `atoms.db` with a self-grant capability atom and one
/// `auditor.daily_summary` atom. Returns the test atom's content
/// hash so the daemon-side `atom.get` can confirm it.
fn preseed_db(db_path: &Path) -> Multihash {
    let store = SqliteAtomStore::open_with_key(db_path, &dek()).expect("open seed db");

    // Self-grant: owner can do everything (Read + Write + Supersede)
    // unconditionally. Required so subsequent atom.get reads pass
    // capability evaluation.
    let cap = build_capability_atom(
        &owner_key(),
        owner_pubkey(),
        vec![Action::Read, Action::Write, Action::Supersede],
        CapabilityScope::default(),
        Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
        None,
        Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
        None,
    )
    .expect("sign capability");
    store.insert(&cap).expect("insert capability");

    // Marker atom: an auditor.daily_summary signed by the owner.
    let env = AtomTemplate {
        v: 1,
        entity: EntityId::new("auditor"),
        predicate: PredicateName::new("auditor.daily_summary"),
        claim: serde_json::json!({
            "narrative": "preseeded by sqlite_persistence test",
            "panel": []
        }),
        valid_from: Iso8601::new("2026-05-31T00:00:00Z").unwrap(),
        valid_to: None,
        tx_time: Iso8601::new("2026-05-31T00:00:00Z").unwrap(),
        classification: Tier::new("existence"),
        supersedes: None,
        provenance: vec![],
    }
    .sign(&owner_key())
    .expect("sign marker");
    store.insert(&env).expect("insert marker");
    env.content_hash().expect("hash marker")
}

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
            continue;
        }
        return v;
    }
}

#[tokio::test]
async fn daemon_reads_atoms_persisted_by_a_prior_session() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);
    let db_path = data_dir.join("atoms.db");

    // Pre-seed in a separate session: open SqliteAtomStore, insert
    // atoms, drop. Mirrors the "prior daemon session left atoms
    // behind" state.
    let marker_hash = preseed_db(&db_path);

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", OWNER_KEY_HEX)
        .env("FFS_SQLCIPHER_KEY_HEX", DEK_HEX)
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon binary");

    let socket = data_dir.join("run").join("ffs.sock");
    assert!(
        wait_for(&socket, Duration::from_secs(5)).await,
        "daemon never bound the socket at {}",
        socket.display()
    );

    // atom.get with the marker hash from the pre-seed should return
    // the same envelope — proves the daemon opened the SQLCipher
    // file written by a different process and read it through the
    // capability-evaluator unchanged.
    let resp = rpc(
        &socket,
        "atom.get",
        serde_json::json!({"hash": marker_hash.to_multibase()}),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("atom.get returned error: {resp}"));
    assert_eq!(
        result.get("predicate").and_then(|p| p.as_str()),
        Some("auditor.daily_summary"),
        "wrong atom returned: {result}"
    );
    assert_eq!(
        result
            .get("claim")
            .and_then(|c| c.get("narrative"))
            .and_then(|n| n.as_str()),
        Some("preseeded by sqlite_persistence test"),
    );

    // Clean shutdown.
    let pid = child.id();
    Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("kill");
    let status = child.wait().expect("wait");
    assert!(status.success(), "daemon exited non-zero: {status:?}");
}

#[tokio::test]
async fn fresh_substrate_gets_owner_self_grant_so_accept_works() {
    // Fresh data dir, no atoms in atoms.db. The daemon's startup
    // bootstrap should sign and insert a self-grant for the owner;
    // an RPC that goes through the capability gate (here,
    // audit.publish_summary, which requires Write on
    // auditor.daily_summary) should succeed instead of being
    // capability-denied. Mirrors the user-facing path:
    // ingest.accept also routes through the capability check.
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", OWNER_KEY_HEX)
        .env("FFS_SQLCIPHER_KEY_HEX", DEK_HEX)
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");

    let socket = data_dir.join("run").join("ffs.sock");
    assert!(
        wait_for(&socket, Duration::from_secs(5)).await,
        "daemon never bound the socket"
    );

    // audit.publish_summary requires Write capability — same gate
    // as ingest.accept. If the self-grant didn't get bootstrapped,
    // this returns the 4001 capability-denied error.
    let resp = rpc(
        &socket,
        "audit.publish_summary",
        serde_json::json!({"claim": {"panel": [], "narrative": "bootstrap test"}}),
    )
    .await;
    if let Some(err) = resp.get("error") {
        panic!("audit.publish_summary failed — owner self-grant likely missing. error: {err}");
    }
    assert!(
        resp.get("result")
            .and_then(|r| r.get("atom_hash"))
            .is_some(),
        "expected atom_hash in result; got: {resp}"
    );

    // Restart the daemon against the same data dir; the bootstrap
    // should detect the existing self-grant and skip — no duplicate
    // capability atoms. We can't directly count atoms here, so just
    // confirm the next audit.publish_summary still succeeds (which
    // it would either way) and that no startup error occurred.
    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let _ = child.wait();

    let mut child2 = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", OWNER_KEY_HEX)
        .env("FFS_SQLCIPHER_KEY_HEX", DEK_HEX)
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn restart");
    assert!(
        wait_for(&socket, Duration::from_secs(5)).await,
        "daemon never re-bound the socket"
    );
    let resp2 = rpc(
        &socket,
        "audit.publish_summary",
        serde_json::json!({"claim": {"panel": [], "narrative": "second boot"}}),
    )
    .await;
    if let Some(err) = resp2.get("error") {
        panic!("second-boot audit.publish_summary failed: {err}");
    }

    Command::new("kill")
        .arg("-TERM")
        .arg(child2.id().to_string())
        .status()
        .expect("kill 2");
    let _ = child2.wait();
}

#[tokio::test]
async fn ffs_keyring_disable_short_circuits_to_env_var_or_generate_fallback() {
    // With FFS_KEYRING_DISABLE=1 and no env-var keys, the daemon
    // must still come up (generating fresh keys with the existing
    // warning). This protects CI / headless installs where there's
    // no session keychain.
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_KEYRING_DISABLE", "1")
        .env_remove("FFS_OWNER_KEY_HEX")
        .env_remove("FFS_SQLCIPHER_KEY_HEX")
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let socket = data_dir.join("run").join("ffs.sock");
    assert!(
        wait_for(&socket, Duration::from_secs(5)).await,
        "daemon never bound the socket; FFS_KEYRING_DISABLE path must boot"
    );

    // health.summary works without capabilities — proves the daemon
    // is past startup and serving RPCs even with generate-and-warn
    // keys.
    let resp = rpc(&socket, "health.summary", serde_json::Value::Null).await;
    assert!(
        resp.get("result").is_some(),
        "health.summary failed under FFS_KEYRING_DISABLE: {resp}"
    );

    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let _ = child.wait();
}

/// task_29: a pending submission with extracted proposals must
/// survive a daemon restart. Pre-task_29 the `InMemoryQuarantine`
/// lost everything on restart even though the watcher had already
/// moved the source file into `.processed/`. Now both submission
/// and its proposals persist to the same SQLCipher-encrypted
/// atoms.db that the atom store uses.
#[tokio::test]
async fn quarantine_submission_survives_daemon_restart() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);
    seed_skills_symlinks(&data_dir);

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");

    // === First daemon boot: drop a markdown file, let scribe
    // extract it, then SIGTERM.
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", OWNER_KEY_HEX)
        .env("FFS_SQLCIPHER_KEY_HEX", DEK_HEX)
        .env("FFS_KEYRING_DISABLE", "1")
        // task_31: opt out of the ingest stability window so the
        // dropped file is submitted near-immediately.
        .env("FFS_INGEST_STABILITY_MS", "0")
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn first boot");

    let socket = data_dir.join("run").join("ffs.sock");
    assert!(wait_for(&socket, Duration::from_secs(5)).await);

    let ingest_dir = data_dir.join("ingest");
    let note = "---\nname: Sara Chen\nemail: sara@example.com\n---\n\n## Notes\n- met at picnic\n";
    std::fs::write(ingest_dir.join("survives.md"), note).unwrap();

    // Wait for the submission to land + transition to Extracted.
    let first_id = {
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(10) {
                panic!("submission never reached Extracted");
            }
            let resp = rpc(&socket, "ingest.list_pending", serde_json::json!({})).await;
            if let Some(subs) = resp.get("result").and_then(|r| r.as_array())
                && let Some(first) = subs.first()
                && first.get("status").and_then(|s| s.as_str()) == Some("extracted")
            {
                break first
                    .get("id")
                    .and_then(|v| v.as_str())
                    .expect("id")
                    .to_string();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let _ = child.wait();

    // === Second daemon boot: SAME data dir, SAME DEK. The
    // submission must still be there.
    let mut child2 = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", OWNER_KEY_HEX)
        .env("FFS_SQLCIPHER_KEY_HEX", DEK_HEX)
        .env("FFS_KEYRING_DISABLE", "1")
        .env("FFS_INGEST_STABILITY_MS", "0")
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn second boot");
    assert!(wait_for(&socket, Duration::from_secs(5)).await);

    let resp = rpc(&socket, "ingest.list_pending", serde_json::json!({})).await;
    let subs = resp
        .get("result")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        subs.iter()
            .any(|s| s.get("id").and_then(|v| v.as_str()) == Some(first_id.as_str())),
        "submission {first_id} should survive the daemon restart; got: {subs:?}"
    );
    let surviving = subs
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(first_id.as_str()))
        .unwrap();
    assert_eq!(
        surviving.get("status").and_then(|s| s.as_str()),
        Some("extracted"),
        "post-restart status should remain Extracted"
    );
    let proposals = surviving
        .get("proposals")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !proposals.is_empty(),
        "post-restart proposals should be present"
    );

    Command::new("kill")
        .arg("-TERM")
        .arg(child2.id().to_string())
        .status()
        .expect("kill 2");
    let _ = child2.wait();
}

/// Symlink the scribe skill bundle into the data dir so the daemon
/// can spawn it. Used by tests that exercise the full ingest
/// pipeline (which needs scribe to produce proposals before the
/// quarantine has anything to persist).
fn seed_skills_symlinks(data_dir: &Path) {
    let src_skills = repo_root().join("skills");
    let dst_skills = data_dir.join("skills");
    std::fs::create_dir_all(&dst_skills).unwrap();
    for sub in ["scribe", "_lib"] {
        let _ = std::os::unix::fs::symlink(src_skills.join(sub), dst_skills.join(sub));
    }
}

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn daemon_refuses_to_open_existing_db_with_wrong_dek() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);

    // Create atoms.db with the canonical DEK.
    let db_path = data_dir.join("atoms.db");
    let _ = preseed_db(&db_path);

    // Now start the daemon with the WRONG DEK. The SQLite store
    // verifies the key on its first read (the migration-version
    // SELECT inside `migrations::apply`); a mismatch is surfaced
    // by `StartupError::Store` and the binary exits non-zero.
    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let output = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", OWNER_KEY_HEX)
        .env("FFS_SQLCIPHER_KEY_HEX", WRONG_DEK_HEX)
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn daemon binary");

    assert!(
        !output.status.success(),
        "daemon should reject wrong DEK, but exited 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("atom store") || stderr.contains("sqlite"),
        "stderr should reference the store error; got: {stderr}"
    );
}
