// Daemon binary spawn + UDS round-trip is Unix-only.
#![cfg(unix)]

//! task_49 / ADR-036: the fast-path watcher runs in the production
//! daemon binary. Two end-to-end proofs against the real `ffs-daemon`:
//!
//! (a) an editor appends a bullet to an additive section of a
//!     materialized contact and the daemon absorbs it as a supersession
//!     atom without any RPC; the daemon's own re-render of that file
//!     does not produce a second atom (shared suppression registry);
//!
//! (b) the owner ticks `accept` in the rendered `inbox/<date>.md`
//!     (ADR-032) and the daemon commits the atom and re-renders the
//!     block under `## Decided`.
//!
//! Both start from a markdown note dropped in `ingest/`, so they need
//! `python3` on PATH for the scribe skill (symlinked from the repo) and
//! skip when it is missing, matching `ingest_pipeline_e2e.rs`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

struct Daemon {
    child: Child,
    data_dir: PathBuf,
    socket: PathBuf,
    _tmp: tempfile::TempDir,
}

async fn spawn_daemon(owner_key_hex: &str, sqlcipher_hex: &str) -> Daemon {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);
    let bin = env!("CARGO_BIN_EXE_ffs-daemon");
    let child = Command::new(bin)
        .env("FFS_DATA_DIR", &data_dir)
        .env("FFS_OWNER_KEY_HEX", owner_key_hex)
        .env("FFS_SQLCIPHER_KEY_HEX", sqlcipher_hex)
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
    Daemon {
        child,
        data_dir,
        socket,
        _tmp: tmp,
    }
}

impl Daemon {
    fn stop(mut self) {
        Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()
            .expect("kill");
        let status = self.child.wait().expect("wait");
        if !status.success() {
            let mut stderr_bytes = Vec::new();
            if let Some(mut e) = self.child.stderr.take() {
                let _ = e.read_to_end(&mut stderr_bytes);
            }
            panic!(
                "daemon exited non-zero: {status:?}; stderr: {}",
                String::from_utf8_lossy(&stderr_bytes)
            );
        }
    }
}

/// Drop a contact-shaped note in `ingest/` and wait for the scribe to
/// extract it. Returns the submission as `ingest.list_pending` shows it.
async fn submit_contact_note(d: &Daemon) -> serde_json::Value {
    let note = "---\nname: Sara Chen\nemail: sara@example.com\n---\n\n## Notes\n- met at picnic\n";
    std::fs::write(d.data_dir.join("ingest").join("sara.md"), note).unwrap();
    let start = Instant::now();
    let mut last = serde_json::Value::Null;
    while start.elapsed() < Duration::from_secs(15) {
        last = rpc(&d.socket, "ingest.list_pending", serde_json::json!({})).await;
        let subs = last
            .get("result")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if let Some(sub) = subs.first()
            && sub
                .get("status")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("extracted"))
        {
            return sub.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scribe never produced an extracted submission; last: {last}");
}

async fn wait_for_text(path: &Path, needle: &str, max: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < max {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.contains(needle)
        {
            return Some(text);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

async fn atoms_for(d: &Daemon, entity: &str) -> Vec<serde_json::Value> {
    let resp = rpc(
        &d.socket,
        "atom.list",
        serde_json::json!({"entity": entity, "predicate": "contact.person"}),
    )
    .await;
    resp.get("result")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
}

/// The materialized contact file for the accepted `contact.person` atom
/// among `hashes`, with the entity id it renders.
async fn materialized_contact(d: &Daemon, hashes: &[serde_json::Value]) -> (String, PathBuf) {
    let mut entity: Option<String> = None;
    for h in hashes {
        let got = rpc(&d.socket, "atom.get", serde_json::json!({"hash": h})).await;
        let env = got.get("result").cloned().unwrap_or_default();
        if env.get("predicate").and_then(|p| p.as_str()) == Some("contact.person") {
            entity = env
                .get("entity")
                .and_then(|e| e.as_str())
                .map(str::to_string);
        }
    }
    let entity = entity.expect("a contact.person atom was accepted");
    let file = d
        .data_dir
        .join("contacts")
        .join("by-name")
        .join("S")
        .join("Sara_Chen.md");
    assert!(
        wait_for(&file, Duration::from_secs(5)).await,
        "materializer never wrote {}",
        file.display()
    );
    (entity, file)
}

#[tokio::test]
async fn fastpath_absorbs_additive_edit_in_binary() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let d = spawn_daemon(
        "4949494949494949494949494949494949494949494949494949494949494949",
        "abadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafeabadcafe",
    )
    .await;

    let sub = submit_contact_note(&d).await;
    let id = sub.get("id").and_then(|v| v.as_str()).unwrap().to_string();
    let accepted = rpc(
        &d.socket,
        "ingest.accept",
        serde_json::json!({"submission_id": id}),
    )
    .await;
    let hashes = accepted
        .get("result")
        .and_then(|r| r.get("accepted_atom_hashes"))
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(!hashes.is_empty(), "accept committed nothing: {accepted}");
    let (entity, file) = materialized_contact(&d, &hashes).await;
    let before = atoms_for(&d, &entity).await;
    assert_eq!(before.len(), 1, "one head atom after accept: {before:?}");

    // Give the materializer's own write time to settle through the
    // watcher (it is suppressed; this only avoids racing our edit with
    // the daemon's rename).
    tokio::time::sleep(Duration::from_millis(300)).await;
    let original = std::fs::read_to_string(&file).unwrap();
    assert!(
        original.contains("## Notes"),
        "the scribe lifted the note bullet into an additive section: {original}"
    );

    // The editor appends one bullet at the end of the Notes section.
    let edited = append_bullet(&original, "Notes", "brought cookies to the picnic");
    let edit_at = Instant::now();
    std::fs::write(&file, &edited).unwrap();

    // Assert on the atom: a supersession appears on the entity's chain.
    let mut after = Vec::new();
    while edit_at.elapsed() < Duration::from_secs(5) {
        after = atoms_for(&d, &entity).await;
        if after.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let absorbed_in = edit_at.elapsed();
    assert_eq!(
        after.len(),
        2,
        "the edit became a supersession atom; got {after:?}"
    );
    let newest = after
        .iter()
        .find(|a| a.get("supersedes").is_some_and(|s| !s.is_null()))
        .expect("the new atom supersedes the head");
    let notes = newest
        .get("claim")
        .and_then(|c| c.get("notes"))
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str() == Some("brought cookies to the picnic")),
        "the appended bullet is in the superseding claim: {newest}"
    );
    eprintln!("fast path absorbed the edit in {absorbed_in:?}");
    // ARCHITECTURE.md budget: 200 ms after the 50 ms debounce. The
    // atom assertion above is the proof; the wall-clock is reported and
    // held to a generous CI ceiling so a slow runner cannot fail it.
    assert!(
        absorbed_in < Duration::from_secs(2),
        "absorption took {absorbed_in:?}"
    );

    // The daemon re-renders the file from the new head. That write is
    // suppressed (shared registry), so no third atom appears, and the
    // file settles containing the bullet rather than bouncing back.
    let settled = wait_for_text(
        &file,
        "brought cookies to the picnic",
        Duration::from_secs(3),
    )
    .await
    .expect("re-rendered file keeps the bullet");
    assert!(settled.contains("met at picnic"), "{settled}");
    tokio::time::sleep(Duration::from_millis(700)).await;
    let final_atoms = atoms_for(&d, &entity).await;
    assert_eq!(
        final_atoms.len(),
        2,
        "the daemon's own re-render must not re-enter as an edit: {final_atoms:?}"
    );

    d.stop();
}

fn append_bullet(original: &str, section: &str, bullet: &str) -> String {
    let header = format!("## {section}");
    let mut out: Vec<&str> = Vec::new();
    let mut in_section = false;
    let mut inserted = false;
    let new_line = format!("- {bullet}");
    for line in original.lines() {
        if line.starts_with("## ") {
            if in_section && !inserted {
                out.push(&new_line);
                inserted = true;
            }
            in_section = line.trim() == header;
        }
        if in_section && line.trim().is_empty() && !inserted {
            out.push(&new_line);
            inserted = true;
        }
        out.push(line);
    }
    if in_section && !inserted {
        out.push(&new_line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[tokio::test]
async fn inbox_tick_applies_in_binary() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let d = spawn_daemon(
        "5050505050505050505050505050505050505050505050505050505050505050",
        "decafbaddecafbaddecafbaddecafbaddecafbaddecafbaddecafbaddecafbad",
    )
    .await;

    let sub = submit_contact_note(&d).await;
    let id = sub.get("id").and_then(|v| v.as_str()).unwrap().to_string();

    // The inbox materializer renders the submission as a pending source
    // with one accept checkbox per proposal.
    let inbox = d
        .data_dir
        .join("inbox")
        .join(format!("{}.md", ffs_daemon::inbox::today_utc()));
    let source_marker = format!("<!-- source sub:{id} -->");
    let text = wait_for_text(&inbox, &source_marker, Duration::from_secs(5))
        .await
        .expect("inbox rendered the pending submission");
    let accept_prefix = format!("- [ ] accept <!-- sub:{id} ref:");
    let line = text
        .lines()
        .find(|l| l.starts_with(&accept_prefix))
        .unwrap_or_else(|| panic!("no accept checkbox for the submission in:\n{text}"))
        .to_string();

    // The owner ticks accept on the contact block and saves.
    let ticked = text.replacen(&line, &line.replacen("- [ ]", "- [x]", 1), 1);
    // Wait out the materializer's settle so our write is not the one the
    // watcher suppresses.
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(&inbox, &ticked).unwrap();

    // The sink applies the decision, publishes QuarantineChanged, and
    // re-renders: the block now sits under Decided.
    let decided_marker = format!("<!-- decided sub:{id} -->");
    let after = wait_for_text(&inbox, &decided_marker, Duration::from_secs(5))
        .await
        .expect("inbox re-rendered with the submission under Decided");
    assert!(
        !after.contains(&source_marker),
        "no longer a pending section:\n{after}"
    );
    let decided_at = after.find("## Decided").expect("a Decided section");
    let marker_at = after.find(&decided_marker).unwrap();
    assert!(
        marker_at > decided_at,
        "the decided block is under ## Decided"
    );
    assert!(after.contains("accepted ("), "{after}");

    // The atom was committed: the submission left the pending list and
    // the contact materialized from the accepted head.
    let pending = rpc(&d.socket, "ingest.list_pending", serde_json::json!({})).await;
    let still_pending = pending
        .get("result")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .any(|s| s.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        })
        .unwrap_or(false);
    assert!(!still_pending, "submission accepted: {pending}");
    let contact = d
        .data_dir
        .join("contacts")
        .join("by-name")
        .join("S")
        .join("Sara_Chen.md");
    assert!(
        wait_for(&contact, Duration::from_secs(5)).await,
        "the accepted contact materialized"
    );

    d.stop();
}
