// Daemon binary spawn + UDS round-trip is Unix-only.
#![cfg(unix)]

//! task_41 scheduler smoke: the real `ffs-daemon` with the auditor
//! bundle installed and `FFS_AUDITOR_BRIEFING_INTERVAL=2s` publishes an
//! `auditor.briefing` atom on its own, reachable through
//! `audit.query {kind: briefing}` and filed at `briefings/<date>.md`.
//! Proves the skill proxy (the auditor's `atom.list` and
//! `audit.publish_summary` queries route through the dispatcher), the
//! scheduler, the flat path family, and the template together.
//! Needs `python3`; skips without it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
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
    let req = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
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

fn seed_data_dir(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let pred = root.join("config").join("predicates");
    let tmpl = root.join("config").join("templates");
    std::fs::create_dir_all(&pred).unwrap();
    std::fs::create_dir_all(&tmpl).unwrap();
    for e in std::fs::read_dir(repo_root().join("starter").join("predicates")).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), pred.join(e.file_name())).unwrap();
        }
    }
    for e in std::fs::read_dir(repo_root().join("starter").join("templates")).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), tmpl.join(e.file_name())).unwrap();
        }
    }
    let dest_skills = root.join("skills");
    std::fs::create_dir_all(&dest_skills).unwrap();
    let src_skills = repo_root().join("skills");
    for sub in ["auditor", "_lib"] {
        let _ = std::os::unix::fs::symlink(src_skills.join(sub), dest_skills.join(sub));
    }
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
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
async fn scheduler_publishes_a_briefing_on_its_interval_in_the_binary() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);

    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let mut child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env(
            "FFS_OWNER_KEY_HEX",
            "4141414141414141414141414141414141414141414141414141414141414141",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "abadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafe",
        )
        .env("FFS_KEYRING_DISABLE", "1")
        .env("FFS_INGEST_STABILITY_MS", "0")
        .env("FFS_AUDITOR_BRIEFING_INTERVAL", "2s")
        .env("FFS_AUDITOR_TICK_INTERVAL", "off")
        .env("FFS_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn daemon");

    let socket = data_dir.join("run").join("ffs.sock");
    assert!(
        wait_for(&socket, Duration::from_secs(5)).await,
        "daemon never bound the socket"
    );

    let start = Instant::now();
    let mut rows = Vec::new();
    while start.elapsed() < Duration::from_secs(30) {
        let resp = rpc(
            &socket,
            "audit.query",
            serde_json::json!({"kind": "briefing"}),
        )
        .await;
        rows = resp
            .get("result")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        !rows.is_empty(),
        "the scheduler never published a briefing; is skills/auditor importable?"
    );
    let claim = &rows[0]["claim"];
    let date = claim["date"].as_str().expect("briefing carries a date");
    assert!(claim["narrative"].is_string(), "{claim}");
    assert!(claim["window"]["from"].is_string(), "{claim}");

    // The working-set materializer filed it as a flat-family page.
    let page = data_dir.join("briefings").join(format!("{date}.md"));
    assert!(
        wait_for(&page, Duration::from_secs(5)).await,
        "briefings/{date}.md was not materialized"
    );
    let text = std::fs::read_to_string(&page).unwrap();
    assert!(text.contains("## Narrative"), "{text}");
    assert!(text.contains("## Follow-ups"), "{text}");

    // `audit.run` on demand publishes a second briefing.
    let ran = rpc(&socket, "audit.run", serde_json::json!({"op": "briefing"})).await;
    assert!(
        ran.get("result").is_some(),
        "audit.run should succeed: {ran}"
    );

    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let status = child.wait().expect("wait");
    if !status.success() {
        let mut stderr_bytes = Vec::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_end(&mut stderr_bytes);
        }
        panic!(
            "daemon exited non-zero: {status:?}; stderr: {}",
            String::from_utf8_lossy(&stderr_bytes)
        );
    }
}
