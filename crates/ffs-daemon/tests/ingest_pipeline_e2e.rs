// Daemon binary spawn + UDS round-trip is Unix-only.
#![cfg(unix)]

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
        // Opt out of the task_31 stability window so this test's
        // assertions don't have to wait the default 60s.
        .env("FFS_INGEST_STABILITY_MS", "0")
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

/// task_36: opt-in `llm` engine, end to end. Runs only when the
/// test process has `FFS_SCRIBE_ENGINE=llm` set (and, for a real
/// backend, `FFS_SCRIBE_LLM_URL` / `FFS_SCRIBE_LLM_MODEL`); the
/// daemon forwards its environment to the scribe subprocess. The
/// scribe falls back to `heuristic` when the backend is unreachable
/// or its output is schema-invalid, so the assertion is that the
/// `engine` field is present and is one of the two values, with
/// `llm` expected when the backend answered.
#[tokio::test]
async fn llm_engine_e2e_when_backend_configured() {
    if std::env::var("FFS_SCRIBE_ENGINE").as_deref() != Ok("llm") {
        eprintln!("skipping: set FFS_SCRIBE_ENGINE=llm (and FFS_SCRIBE_LLM_URL/MODEL) to run");
        return;
    }
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
            "0909090909090909090909090909090909090909090909090909090909090909",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "abadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafe",
        )
        .env("FFS_KEYRING_DISABLE", "1")
        .env("FFS_INGEST_STABILITY_MS", "0")
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
    let ingest_dir = data_dir.join("ingest");
    let card = "---\nname: Sara Chen\nemail: sara@example.com\nphone: 919-555-0100\n---\n\nMet at the distributed-systems conference.\n";
    std::fs::write(ingest_dir.join("sara-llm.md"), card).unwrap();

    let start = Instant::now();
    let mut submissions = Vec::new();
    // LLM backends are slow; allow up to 90s.
    while start.elapsed() < Duration::from_secs(90) {
        let resp = rpc(&socket, "ingest.list_pending", serde_json::json!({})).await;
        submissions = resp
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
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(!submissions.is_empty(), "no submission surfaced");
    let proposals = submissions[0]
        .get("proposals")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!proposals.is_empty(), "no proposals: {}", submissions[0]);
    let engine = proposals[0]
        .get("engine")
        .and_then(|e| e.as_str())
        .unwrap_or("");
    assert!(
        engine == "llm" || engine == "heuristic",
        "proposal must carry an engine field; got {:?}",
        proposals[0]
    );
    if engine == "heuristic" {
        eprintln!("note: scribe fell back to heuristic (backend unreachable or invalid output)");
    }

    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let _ = child.wait();
}

/// task_32: unstructured body text ("Met Sara Chen at the
/// conference. Phone 919-428-4074.") should produce a
/// `contact.person` proposal with display_name "Sara Chen",
/// not a `note` with title "untitled" and entity ID
/// `from-sub-<id>` (the pre-task_32 behavior).
#[tokio::test]
async fn unstructured_body_text_produces_contact_person_proposal() {
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
            "0707070707070707070707070707070707070707070707070707070707070707",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "decafbaddecafbaddecafbaddecafbaddecafbaddecafbaddecafbaddecafbad",
        )
        .env("FFS_KEYRING_DISABLE", "1")
        .env("FFS_INGEST_STABILITY_MS", "0")
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

    let ingest_dir = data_dir.join("ingest");
    // Unstructured: NO frontmatter, body has the contact signals
    // (capitalized name + phone). Pre-task_32 this got classified
    // as a note with title "untitled".
    let note = "Met Sara Chen at the conference. Phone 919-428-4074.";
    let dropped = ingest_dir.join("sara-rehearsal.md");
    std::fs::write(&dropped, note).unwrap();

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

    // Find the contact.person proposal (there may also be a
    // fallback note proposal — that's fine, but a contact.person
    // MUST exist).
    let contact = proposals
        .iter()
        .find(|p| p.get("predicate").and_then(|v| v.as_str()) == Some("contact.person"));
    assert!(
        contact.is_some(),
        "expected a contact.person proposal from unstructured body; got: {proposals:?}"
    );
    let contact = contact.unwrap();
    let display_name = contact
        .get("claim")
        .and_then(|c| c.get("display_name"))
        .and_then(|v| v.as_str());
    assert_eq!(display_name, Some("Sara Chen"));
    let phone = contact
        .get("claim")
        .and_then(|c| c.get("phone"))
        .and_then(|v| v.as_str());
    assert_eq!(phone, Some("919-428-4074"));

    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let status = child.wait().expect("wait");
    assert!(status.success(), "daemon exited non-zero: {status:?}");
}

/// task_31: with a real (non-zero) stability window, the daemon
/// holds a newly-discovered file until its content has been stable
/// for the configured delay. We write an initial draft, modify it
/// once shortly after, and confirm the FINAL content — not the
/// initial draft — is what scribe extracted into the quarantine.
#[tokio::test]
async fn stability_window_waits_for_content_to_settle() {
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
            "0808080808080808080808080808080808080808080808080808080808080808",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "f00dbabef00dbabef00dbabef00dbabef00dbabef00dbabef00dbabef00dbabe",
        )
        .env("FFS_KEYRING_DISABLE", "1")
        // 1000 ms stability window — long enough that a mid-window
        // modification resets the timer in a way we can observe,
        // short enough that the test stays fast.
        .env("FFS_INGEST_STABILITY_MS", "1000")
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

    let ingest_dir = data_dir.join("ingest");
    let dropped = ingest_dir.join("evolving.md");

    // Initial draft. Note distinguishing display_name so we can
    // tell which version made it through.
    let initial = "---\nname: Alpha Initial\nemail: alpha@example.com\n---\n\n## Notes\n- v1\n";
    let final_content = "---\nname: Omega Final\nemail: omega@example.com\n---\n\n## Notes\n- v2\n";
    std::fs::write(&dropped, initial).unwrap();
    // Modify well inside the stability window so the timer resets.
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::write(&dropped, final_content).unwrap();

    // Poll up to 6 s — covers poll interval (500 ms) + stability
    // window (1000 ms) + scribe extraction + slop for slow CI.
    let start = Instant::now();
    let mut display_name: Option<String> = None;
    let mut last_resp: serde_json::Value = serde_json::Value::Null;
    while start.elapsed() < Duration::from_secs(6) {
        last_resp = rpc(&socket, "ingest.list_pending", serde_json::json!({})).await;
        let submissions = last_resp
            .get("result")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if let Some(sub) = submissions.first() {
            let status_str = sub.get("status").and_then(|s| s.as_str()).unwrap_or("");
            if status_str.eq_ignore_ascii_case("extracted") {
                let proposals = sub
                    .get("proposals")
                    .and_then(|p| p.as_array())
                    .cloned()
                    .unwrap_or_default();
                if let Some(contact) = proposals
                    .iter()
                    .find(|p| p.get("predicate").and_then(|v| v.as_str()) == Some("contact.person"))
                {
                    display_name = contact
                        .get("claim")
                        .and_then(|c| c.get("display_name"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(
        display_name.as_deref(),
        Some("Omega Final"),
        "expected the final content's display_name to land; last response: {last_resp}"
    );
    assert!(
        !dropped.exists(),
        "source should have been moved to .processed/ after the window elapsed"
    );

    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill");
    let status = child.wait().expect("wait");
    assert!(status.success(), "daemon exited non-zero: {status:?}");
}

// ---------------------------------------------------------------------
// ADR-035 (task_48): the morning read. A read-filed submission carries
// the read's frontmatter; the scribe turns it into provenance, the
// signer accepts it as the owner's live decision, and a clip lands
// under the `clip` tier with an attestation.
// ---------------------------------------------------------------------

struct ReadDaemon {
    child: std::process::Child,
    socket: PathBuf,
    data_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

async fn spawn_read_daemon() -> Option<ReadDaemon> {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return None;
    }
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);
    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env(
            "FFS_OWNER_KEY_HEX",
            "0707070707070707070707070707070707070707070707070707070707070707",
        )
        .env(
            "FFS_SQLCIPHER_KEY_HEX",
            "abadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafe",
        )
        .env("FFS_INGEST_STABILITY_MS", "0")
        .env("FFS_KEYRING_DISABLE", "1")
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
    Some(ReadDaemon {
        child,
        socket,
        data_dir,
        _tmp: tmp,
    })
}

impl ReadDaemon {
    async fn drop_and_extract(&self, name: &str, content: &str) -> serde_json::Value {
        std::fs::write(self.data_dir.join("ingest").join(name), content).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            let resp = rpc(&self.socket, "ingest.list_pending", serde_json::json!({})).await;
            let subs = resp["result"].as_array().cloned().unwrap_or_default();
            if let Some(s) = subs.iter().find(|s| {
                s["source_uri"]
                    .as_str()
                    .map(|u| u.ends_with(name))
                    .unwrap_or(false)
                    && s["status"]
                        .as_str()
                        .map(|x| x.eq_ignore_ascii_case("extracted"))
                        .unwrap_or(false)
            }) {
                return s.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("submission for {name} never reached extracted");
    }

    async fn accept(&self, submission_id: &str) -> Vec<String> {
        let resp = rpc(
            &self.socket,
            "ingest.accept",
            serde_json::json!({"submission_id": submission_id}),
        )
        .await;
        resp["result"]["accepted_atom_hashes"]
            .as_array()
            .unwrap_or_else(|| panic!("accept failed: {resp}"))
            .iter()
            .map(|h| h.as_str().unwrap().to_string())
            .collect()
    }

    async fn atom(&self, hash: &str) -> serde_json::Value {
        rpc(&self.socket, "atom.get", serde_json::json!({"hash": hash})).await["result"].clone()
    }

    fn stop(mut self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.wait();
    }
}

const READ_FRONTMATTER: &str = "intake: morning_read\nowner_present: true\nsession: 2026-09-21-0814\nactor: mcp:agent/claude-code\n";

fn provenance_kinds(atom: &serde_json::Value) -> Vec<String> {
    atom["provenance"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|p| p["kind"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn morning_read_provenance_survives_to_atom() {
    let Some(d) = spawn_read_daemon().await else {
        return;
    };
    let content = format!(
        "---\npredicate: person.generic\ndisplay_name: Pat Example\nrole: chief executive\n{READ_FRONTMATTER}---\nStated in the article: named chief executive.\n"
    );
    let sub = d.drop_and_extract("pat.md", &content).await;
    let hashes = d.accept(sub["id"].as_str().unwrap()).await;
    assert!(!hashes.is_empty());
    let atom = d.atom(&hashes[0]).await;
    let kinds = provenance_kinds(&atom);
    assert!(
        kinds.contains(&"morning_read".to_string()),
        "kinds: {kinds:?}"
    );
    assert!(kinds.contains(&"session".to_string()), "kinds: {kinds:?}");
    let session = atom["provenance"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "session")
        .unwrap();
    assert_eq!(
        session["uri"],
        "ffs-session://mcp:agent/claude-code/2026-09-21-0814?owner_present=true"
    );
    // The read's keys are provenance, never claim data.
    for k in ["intake", "owner_present", "session", "actor"] {
        assert!(
            atom["claim"].get(k).is_none(),
            "{k} leaked into the claim: {atom}"
        );
    }
    assert_eq!(atom["classification"], "existence", "a note is not a clip");
    d.stop();
}

#[tokio::test]
async fn clip_this_stores_body_under_clip_tier_with_morning_read_provenance_and_attestation() {
    let Some(d) = spawn_read_daemon().await else {
        return;
    };
    let content = format!(
        "---\npredicate: source.article\ntitle: Widget maker breaks ground\nurl: https://news.example-ledger.test/2026/09/21/widget-plant\npublication: Example Ledger\npublished_at: 2026-09-21\n{READ_FRONTMATTER}---\nThe owner clipped this synthetic body during the read.\n"
    );
    let sub = d.drop_and_extract("clip.md", &content).await;
    let article = sub["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["predicate"] == "source.article")
        .expect("article proposal");
    assert_eq!(article["classification_hint"], "clip", "{article}");
    let hashes = d.accept(sub["id"].as_str().unwrap()).await;
    // Find the article atom among the accepted hashes; the accepted hash
    // is the atom's content hash and therefore the attestation's entity.
    let mut article_hash: Option<String> = None;
    let mut article_atom = serde_json::Value::Null;
    let mut seen_atoms = Vec::new();
    for h in &hashes {
        let a = d.atom(h).await;
        if a["predicate"] == "source.article" {
            article_hash = Some(h.clone());
            article_atom = a;
            break;
        }
        seen_atoms.push(a);
    }
    let article_hash =
        article_hash.unwrap_or_else(|| panic!("no article atom among {hashes:?}: {seen_atoms:?}"));
    assert_eq!(article_atom["classification"], "clip");
    let kinds = provenance_kinds(&article_atom);
    assert!(
        kinds.contains(&"morning_read".to_string()) && kinds.contains(&"session".to_string()),
        "{kinds:?}"
    );
    // One owner attestation, basis owner_knowledge (the owner read it live).
    let atts = rpc(
        &d.socket,
        "atom.list",
        serde_json::json!({"entity": article_hash, "predicate": "attestation"}),
    )
    .await["result"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(atts.len(), 1, "expected one attestation: {atts:?}");
    assert_eq!(atts[0]["claim"]["basis"], "owner_knowledge");
    d.stop();
}

#[tokio::test]
async fn read_filed_note_lands_in_vault_with_wikilinks() {
    let Some(d) = spawn_read_daemon().await else {
        return;
    };
    let content = format!(
        "---\npredicate: source.article\ntitle: Harbor and Pine opens third cafe\nurl: https://news.example-ledger.test/2026/09/21/harbor-pine\npublication: Example Ledger\npublished_at: 2026-09-21\n{READ_FRONTMATTER}---\nThe owner's note on the article.\n\n## Mentions\n- Casey Rivera — owner of Harbor and Pine Coffee\n- Harbor and Pine Coffee — a Charlotte cafe group opening its third location\n"
    );
    let sub = d.drop_and_extract("harbor.md", &content).await;
    let preds: Vec<&str> = sub["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["predicate"].as_str().unwrap())
        .collect();
    assert!(
        preds.contains(&"source.article")
            && preds.contains(&"person.generic")
            && preds.contains(&"org.company"),
        "{preds:?}"
    );
    let hashes = d.accept(sub["id"].as_str().unwrap()).await;
    assert!(hashes.len() >= 3, "{hashes:?}");
    // The article materializes with wikilinks to its mentions.
    let articles = d.data_dir.join("articles");
    let start = Instant::now();
    let mut rendered = String::new();
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(files) = walk_md(&articles)
            && let Some(f) = files.first()
        {
            rendered = std::fs::read_to_string(f).unwrap_or_default();
            if rendered.contains("[[") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        rendered.contains("|Casey Rivera]]"),
        "no wikilink to the person: {rendered}"
    );
    assert!(
        rendered.contains("|Harbor and Pine Coffee]]"),
        "no wikilink to the org: {rendered}"
    );
    // Every accepted atom has one attestation.
    for h in &hashes {
        let a = d.atom(h).await;
        if a["predicate"] == "attestation" {
            continue;
        }
        let atts = rpc(
            &d.socket,
            "atom.list",
            serde_json::json!({"entity": h, "predicate": "attestation"}),
        )
        .await["result"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(atts.len(), 1, "atom {h} has {} attestations", atts.len());
    }
    d.stop();
}

fn walk_md(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(root)? {
        let p = entry?.path();
        if p.is_dir() {
            out.extend(walk_md(&p)?);
        } else if p.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(p);
        }
    }
    Ok(out)
}
