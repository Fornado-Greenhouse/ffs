//! End-to-end test for the ingest pipeline wired in task_26:
//! spawn the real daemon binary, drop a markdown file under
//! `$FFS_DATA_DIR/ingest/`, wait, and assert
//! `ingest.list_pending` returns a submission with parsed scribe
//! proposals. Proves the full path:
//!
//!   filesystem event → ingest watcher → quarantine.submit →
//!   scribe (Python subprocess) → quarantine.complete → RPC
//!
//! Requires `python3` on PATH (the scribe skill is Python) and
//! the workspace's `starter/predicates/` + `starter/templates/`
//! tree (seeded into the test's temp data dir). The skill bundle
//! at `skills/scribe/` is symlinked into the data dir's
//! `skills/scribe/`.

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
    // Symlink the scribe bundle (and _lib helper it imports) into
    // the data dir's skills/ folder. Symlinks rather than copies
    // so an edit to skills/scribe/extraction.py in the repo is
    // picked up on next test run.
    let dest_skills = root.join("skills");
    std::fs::create_dir_all(&dest_skills).unwrap();
    let src_skills = repo_root().join("skills");
    for sub in ["scribe", "_lib"] {
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
async fn drop_markdown_in_ingest_produces_a_proposal_via_real_scribe() {
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
            "0606060606060606060606060606060606060606060606060606060606060606",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "abadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafe",
        )
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

    // Sanity check: ingest dir exists.
    let ingest_dir = data_dir.join("ingest");
    assert!(ingest_dir.exists(), "ingest dir should be created on boot");

    // Drop a contact-shaped markdown note. Scribe should produce a
    // contact.person proposal (display_name + work_email lifted
    // from the frontmatter).
    let note = "---\nname: Sara Chen\nemail: sara@example.com\n---\n\n## Notes\n- met at picnic\n";
    let dropped = ingest_dir.join("sara.md");
    std::fs::write(&dropped, note).unwrap();

    // Wait up to 10s for the watcher to pick the file up, scribe
    // to extract, and the submission to appear in `ingest.list_pending`.
    let start = Instant::now();
    let mut last_resp: serde_json::Value = serde_json::Value::Null;
    let mut submissions = Vec::new();
    while start.elapsed() < Duration::from_secs(10) {
        last_resp = rpc(&socket, "ingest.list_pending", serde_json::json!({})).await;
        submissions = last_resp
            .get("result")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if !submissions.is_empty()
            && submissions[0]
                .get("status")
                .and_then(|s| s.as_str())
                .map(|s| s.eq_ignore_ascii_case("extracted"))
                .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Source file should have moved into .processed/.
    assert!(
        !dropped.exists(),
        "source file should be moved out of ingest/"
    );
    assert!(
        ingest_dir.join(".processed").join("sara.md").exists(),
        ".processed/ should contain the source"
    );

    assert!(
        !submissions.is_empty(),
        "ingest.list_pending should surface the submission; last response: {last_resp}"
    );
    let sub = &submissions[0];
    let proposals = sub
        .get("proposals")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !proposals.is_empty(),
        "scribe should have produced at least one proposal: {sub}"
    );
    let predicate = proposals[0]
        .get("predicate")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    assert_eq!(
        predicate, "contact.person",
        "expected a contact.person proposal; got: {proposals:?}"
    );

    // Clean shutdown.
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
