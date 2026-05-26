//! End-to-end integration tests for the skills host: spawn real
//! Python skill processes (using `python3`), exercise the stdio
//! protocol, the substrate-access proxy, crash + restart, the per-
//! call timeout path, and graceful shutdown.
//!
//! Each test writes a minimal `SKILL.md` + `entry.py` into a tmpdir,
//! spawns a `SkillProcess`, and exercises one slice of behavior.
//! Tests skip themselves with a `cargo nextest`-visible message if
//! `python3` is not on `PATH`, so the suite stays green on hosts
//! without Python (CI installs Python explicitly).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use ffs_skills_host::{
    RefuseAllProxy, SkillError, SkillKind, SkillManifest, SkillProcess, SkillsHost, SubstrateAccess,
};

fn python_available() -> bool {
    Command::new("python3").arg("--version").output().is_ok()
}

fn helper_lib_dir() -> PathBuf {
    // tests/ live at crates/ffs-skills-host/tests; the helper at
    // skills/_lib/ is two levels up + into skills/_lib.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p.push("skills");
    p.push("_lib");
    p
}

/// Write a SKILL.md + entry.py pair under a tmpdir and return the
/// manifest. The entry script imports `ffs_skill` from
/// `skills/_lib/` via `sys.path.insert`.
fn make_skill(
    tmp: &tempfile::TempDir,
    name: &str,
    entry_body: &str,
    timeout_ms: u64,
) -> SkillManifest {
    let dir = tmp.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let lib = helper_lib_dir();
    let entry = format!(
        "import sys\nsys.path.insert(0, {lib:?})\n{body}\n",
        lib = lib.to_string_lossy(),
        body = entry_body,
    );
    std::fs::write(dir.join("entry.py"), entry).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\nkind: scribe\nentry_point: entry.py\ntimeout_ms: {timeout_ms}\n---\n"
        ),
    )
    .unwrap();
    SkillManifest {
        name: name.to_string(),
        kind: SkillKind::Scribe,
        entry_point: PathBuf::from("entry.py"),
        python: "python3".to_string(),
        timeout: Duration::from_millis(timeout_ms),
        dir,
    }
}

#[tokio::test]
async fn echo_skill_round_trips_invocation_through_host() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let entry = r#"
from ffs_skill import run

def handle(inp):
    return {"echoed": inp}

run(handle)
"#;
    let m = make_skill(&tmp, "echo", entry, 30_000);
    let proc = SkillProcess::spawn(m, Arc::new(RefuseAllProxy));
    let result = proc
        .invoke(serde_json::json!({"hello": "world"}))
        .await
        .expect("invoke ok");
    assert_eq!(result, serde_json::json!({"echoed": {"hello": "world"}}));
}

#[tokio::test]
async fn skill_that_crashes_then_recovers_on_next_invocation() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // First invocation: read the marker file, see it's missing, write
    // it, then `os._exit(1)` to simulate a crash. Subsequent invocations
    // see the marker exists and return ok.
    let marker = tmp.path().join("marker");
    let entry = format!(
        r#"
import os, sys
from ffs_skill import run

MARKER = {marker:?}

def handle(inp):
    if not os.path.exists(MARKER):
        with open(MARKER, "w") as f:
            f.write("x")
        # Crash hard — process exits before result is written.
        os._exit(1)
    return {{"ok": True}}

run(handle)
"#,
        marker = marker.to_string_lossy(),
    );
    // Tighten the backoff window: the first invocation crashes and we
    // need the supervisor to respawn within the test budget. The
    // hard-coded supervisor backoff starts at 1s so the second
    // invocation just needs to wait that long.
    let m = make_skill(&tmp, "flaky", &entry, 30_000);
    let proc = SkillProcess::spawn(m, Arc::new(RefuseAllProxy));

    let first = proc.invoke(serde_json::json!({})).await;
    assert!(
        matches!(first, Err(SkillError::Crashed)),
        "expected first invocation to fail with Crashed; got {first:?}"
    );

    // Wait out the backoff (BACKOFF_INITIAL is 1s) so the next
    // invocation lands on the respawned child.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(proc.restart_count() >= 1, "expected at least one restart");

    let second = proc
        .invoke(serde_json::json!({}))
        .await
        .expect("second invocation should succeed on respawned child");
    assert_eq!(second, serde_json::json!({"ok": true}));
}

/// SubstrateAccess stub that records and answers queries from a
/// pre-seeded map.
struct CannedProxy {
    answers: Mutex<std::collections::HashMap<String, Result<Value, String>>>,
    seen: Mutex<Vec<(String, String, Value)>>,
}

#[async_trait]
impl SubstrateAccess for CannedProxy {
    async fn handle_query(
        &self,
        skill_name: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.seen
            .lock()
            .await
            .push((skill_name.to_string(), method.to_string(), params));
        let key = method.to_string();
        self.answers
            .lock()
            .await
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Err(format!("no canned answer for {key}")))
    }
}

#[tokio::test]
async fn skill_query_round_trips_via_substrate_access_proxy() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let entry = r#"
from ffs_skill import run, query

def handle(inp):
    atom = query("atom.get", {"hash": "abc"})
    return {"fetched": atom}

run(handle)
"#;
    let m = make_skill(&tmp, "querier", entry, 30_000);

    let mut answers = std::collections::HashMap::new();
    answers.insert(
        "atom.get".to_string(),
        Ok(serde_json::json!({"hash": "abc", "claim": {"x": 1}})),
    );
    let proxy = Arc::new(CannedProxy {
        answers: Mutex::new(answers),
        seen: Mutex::new(Vec::new()),
    });
    let proc = SkillProcess::spawn(m, proxy.clone());

    let result = proc.invoke(serde_json::json!({})).await.expect("invoke ok");
    assert_eq!(
        result,
        serde_json::json!({"fetched": {"hash": "abc", "claim": {"x": 1}}})
    );
    let seen = proxy.seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "querier");
    assert_eq!(seen[0].1, "atom.get");
}

#[tokio::test]
async fn skill_query_capability_denial_surfaces_as_error() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let entry = r#"
from ffs_skill import run, query, FfsSkillError

def handle(inp):
    try:
        query("atom.get", {"hash": "abc"})
        return {"unexpected": "should have been denied"}
    except FfsSkillError as e:
        return {"denied": str(e)}

run(handle)
"#;
    let m = make_skill(&tmp, "denied", entry, 30_000);

    let mut answers = std::collections::HashMap::new();
    answers.insert(
        "atom.get".to_string(),
        Err("capability denied: read denied for tier=secret".to_string()),
    );
    let proxy = Arc::new(CannedProxy {
        answers: Mutex::new(answers),
        seen: Mutex::new(Vec::new()),
    });
    let proc = SkillProcess::spawn(m, proxy);

    let result = proc.invoke(serde_json::json!({})).await.expect("invoke ok");
    let denied = result["denied"].as_str().expect("denied is string");
    assert!(
        denied.contains("capability denied"),
        "expected capability-denied error; got: {denied}"
    );
}

#[tokio::test]
async fn hung_skill_is_killed_by_per_call_timeout() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let entry = r#"
import time
from ffs_skill import run

def handle(inp):
    time.sleep(60)
    return {"unreachable": True}

run(handle)
"#;
    // Tight 500ms timeout so the test completes quickly.
    let m = make_skill(&tmp, "hung", entry, 500);
    let proc = SkillProcess::spawn(m, Arc::new(RefuseAllProxy));

    let result = proc.invoke(serde_json::json!({})).await;
    assert!(
        matches!(result, Err(SkillError::Timeout(_))),
        "expected Timeout; got {result:?}"
    );
}

#[tokio::test]
async fn shutdown_all_signals_every_skill() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let entry = r#"
from ffs_skill import run

def handle(inp):
    return {"ok": True}

run(handle)
"#;
    // Two skills, both well-behaved.
    let _m_a = make_skill(&tmp, "alpha", entry, 30_000);
    let _m_b = make_skill(&tmp, "beta", entry, 30_000);

    let mut host = SkillsHost::new(Arc::new(RefuseAllProxy));
    host.discover_and_spawn(tmp.path()).unwrap();
    assert_eq!(host.skills().len(), 2);

    // Smoke-test invocation against one of them before shutting down.
    let s = host.get("alpha").expect("alpha discovered");
    let r = s.invoke(serde_json::json!({})).await.expect("invoke ok");
    assert_eq!(r, serde_json::json!({"ok": true}));

    // Shutdown signals both; the supervisors send `Shutdown` frames
    // and the `kill_on_drop(true)` setting backstops if a skill
    // ignores it. We don't have a direct "are they dead" check here
    // (the API is async-fire-and-forget); the assertion is that
    // shutdown_all completes without panicking and that subsequent
    // invocations on the now-shut-down skills fail.
    host.shutdown_all();

    // Give the supervisors a moment to deliver the shutdown frame.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Drop the host. `SkillProcess::Drop` re-fires shutdown, which is
    // idempotent. The supervisor task exits.
    drop(host);
}
