//! Subprocess host for a single skill. Owns the child process,
//! supervises crashes with exponential backoff, enforces per-call
//! timeouts, and bridges stdio queries to a `SubstrateAccess` proxy.
//!
//! The host is intentionally single-skill: one `SkillProcess` per
//! registered skill. The outer `SkillsHost` (in `lib.rs`) holds many.
//!
//! Concurrency model:
//!
//! - A supervisor task owns the inbound message channel + the child
//!   process. It spawns the child, drives a select loop that drains
//!   `rx_to_child` into the child's stdin, watches `child.wait()` for
//!   exits, and listens for a shutdown notification.
//! - A per-life reader task parses lines from the child's stdout and
//!   either resolves a pending invocation, or — when it sees a
//!   `query` frame from the skill — dispatches that query through
//!   the `SubstrateAccess` proxy and writes the response back via
//!   `tx_to_child` (so the writer loop serializes the reply).
//! - On child exit, the supervisor sleeps for the current backoff
//!   delay (1s, 2s, 4s, ... capped at 60s), then respawns.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::protocol::{HostToSkill, SkillToHost, decode_skill, encode_host};
use crate::registry::SkillManifest;

/// Trait the host calls when a skill issues a substrate-access query.
/// The daemon implements this against its JSON-RPC dispatcher,
/// supplying the skill's identity for capability checks. Tests can
/// inject a stub that returns canned values.
#[async_trait]
pub trait SubstrateAccess: Send + Sync + 'static {
    async fn handle_query(
        &self,
        skill_name: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String>;
}

/// Initial backoff and cap. The supervisor doubles on each restart up
/// to the cap, then clamps.
pub const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);
/// Grace period between a polite `Shutdown` frame and SIGKILL.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill exited mid-invocation")]
    Crashed,
    #[error("invocation timed out after {0:?}")]
    Timeout(Duration),
    #[error("skill returned error: {0}")]
    SkillReported(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("skill is shut down")]
    ShutDown,
}

/// The outcome the supervisor reports back to an awaiting `invoke()`
/// call. Distinguishes a skill-reported error (the skill ran and
/// returned an `error` frame) from a crash (the skill exited mid-
/// invocation without sending any terminal frame).
#[derive(Debug)]
enum InvokeOutcome {
    Result(Value),
    SkillReported(String),
    Crashed,
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<InvokeOutcome>>>>;

pub struct SkillProcess {
    pub manifest: SkillManifest,
    tx_to_child: mpsc::UnboundedSender<HostToSkill>,
    pending: Pending,
    restart_count: Arc<AtomicU32>,
    shutdown: Arc<Notify>,
}

/// A SubstrateAccess implementation that always refuses. The default
/// for hosts constructed without a real proxy; useful in tests.
pub struct RefuseAllProxy;
#[async_trait]
impl SubstrateAccess for RefuseAllProxy {
    async fn handle_query(
        &self,
        _skill: &str,
        method: &str,
        _params: Value,
    ) -> Result<Value, String> {
        Err(format!(
            "substrate-access proxy not configured; rejected {method}"
        ))
    }
}

impl SkillProcess {
    /// Spawn the skill and start the supervisor. Returns immediately;
    /// the first invocation may be sent right away.
    pub fn spawn(manifest: SkillManifest, proxy: Arc<dyn SubstrateAccess>) -> Self {
        let (tx_to_child, rx_to_child) = mpsc::unbounded_channel::<HostToSkill>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let restart_count = Arc::new(AtomicU32::new(0));
        let shutdown = Arc::new(Notify::new());

        tokio::spawn(supervise(
            manifest.clone(),
            proxy,
            rx_to_child,
            tx_to_child.clone(),
            pending.clone(),
            restart_count.clone(),
            shutdown.clone(),
        ));

        Self {
            manifest,
            tx_to_child,
            pending,
            restart_count,
            shutdown,
        }
    }

    /// Invoke the skill with `input`. Awaits until the skill produces
    /// a `result` or `error`, the per-call timeout elapses, or the
    /// child exits without responding.
    pub async fn invoke(&self, input: Value) -> Result<Value, SkillError> {
        let id = next_id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        let msg = HostToSkill::Invoke {
            id: id.clone(),
            input,
        };
        self.tx_to_child
            .send(msg)
            .map_err(|_| SkillError::ShutDown)?;

        match tokio::time::timeout(self.manifest.timeout, rx).await {
            Ok(Ok(InvokeOutcome::Result(v))) => Ok(v),
            Ok(Ok(InvokeOutcome::SkillReported(e))) => Err(SkillError::SkillReported(e)),
            Ok(Ok(InvokeOutcome::Crashed)) => Err(SkillError::Crashed),
            Ok(Err(_)) => Err(SkillError::Crashed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(SkillError::Timeout(self.manifest.timeout))
            }
        }
    }

    /// Polite shutdown: signal the supervisor and let it deliver the
    /// `Shutdown` frame + grace + SIGKILL sequence. Idempotent.
    pub fn shutdown(&self) {
        self.shutdown.notify_waiters();
    }

    pub fn restart_count(&self) -> u32 {
        self.restart_count.load(Ordering::Relaxed)
    }
}

impl Drop for SkillProcess {
    fn drop(&mut self) {
        self.shutdown.notify_waiters();
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    manifest: SkillManifest,
    proxy: Arc<dyn SubstrateAccess>,
    mut rx_to_child: mpsc::UnboundedReceiver<HostToSkill>,
    tx_to_child: mpsc::UnboundedSender<HostToSkill>,
    pending: Pending,
    restart_count: Arc<AtomicU32>,
    shutdown: Arc<Notify>,
) {
    let mut delay = BACKOFF_INITIAL;
    let mut buffered: Option<HostToSkill> = None;

    loop {
        let mut child = match spawn_child(&manifest) {
            Ok(c) => c,
            Err(e) => {
                warn!(skill = %manifest.name, error = %e, "skill_spawn_failed");
                if sleep_or_shutdown(delay, &shutdown).await {
                    return;
                }
                delay = (delay * 2).min(BACKOFF_CAP);
                continue;
            }
        };

        let mut stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        let reader_pending = pending.clone();
        let reader_tx = tx_to_child.clone();
        let reader_proxy = proxy.clone();
        let reader_name = manifest.name.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let parsed = match decode_skill(&line) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!(skill = %reader_name, error = %e, line = %line, "skill_bad_frame");
                                continue;
                            }
                        };
                        handle_skill_frame(
                            parsed,
                            &reader_pending,
                            &reader_tx,
                            &reader_proxy,
                            &reader_name,
                        )
                        .await;
                    }
                    Ok(None) => return, // EOF
                    Err(e) => {
                        warn!(skill = %reader_name, error = %e, "skill_stdout_read_failed");
                        return;
                    }
                }
            }
        });

        // Replay any message buffered from a previous life that didn't
        // make it into the previous child's stdin before it died.
        if let Some(msg) = buffered.take()
            && write_one(&mut stdin, &msg).await.is_err()
        {
            buffered = Some(msg);
        }

        let mut shutdown_requested = false;

        // Inline writer/wait loop: pull from rx_to_child, write to
        // stdin, and watch for the child's exit / shutdown signal.
        loop {
            tokio::select! {
                biased;
                _ = shutdown.notified() => {
                    shutdown_requested = true;
                    let _ = write_one(&mut stdin, &HostToSkill::Shutdown {}).await;
                    drop(stdin);
                    kill_with_grace(&mut child).await;
                    break;
                }
                status = child.wait() => {
                    if let Ok(s) = status {
                        debug!(skill = %manifest.name, ?s, "child_exited");
                    }
                    break;
                }
                Some(msg) = rx_to_child.recv() => {
                    if write_one(&mut stdin, &msg).await.is_err() {
                        // Child stdin closed — child is dying. Buffer
                        // and let the child.wait() arm handle it next.
                        buffered = Some(msg);
                    }
                }
            }
        }

        reader.abort();

        // Wake any pending invocations — they will not be answered
        // by this child life.
        let drained: Vec<_> = pending.lock().await.drain().collect();
        for (_, tx) in drained {
            let _ = tx.send(InvokeOutcome::Crashed);
        }

        if shutdown_requested {
            return;
        }

        let n = restart_count.fetch_add(1, Ordering::Relaxed) + 1;
        info!(skill = %manifest.name, restart_count = n, "skill_crashed");
        if sleep_or_shutdown(delay, &shutdown).await {
            return;
        }
        delay = (delay * 2).min(BACKOFF_CAP);
        info!(skill = %manifest.name, next_delay_ms = %delay.as_millis(), "skill_restarted");
    }
}

async fn handle_skill_frame(
    parsed: SkillToHost,
    pending: &Pending,
    tx_to_child: &mpsc::UnboundedSender<HostToSkill>,
    proxy: &Arc<dyn SubstrateAccess>,
    skill_name: &str,
) {
    match parsed {
        SkillToHost::Result { id, output } => {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(InvokeOutcome::Result(output));
            }
        }
        SkillToHost::Error { id, error } => {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(InvokeOutcome::SkillReported(error));
            }
        }
        SkillToHost::Query { id, method, params } => {
            let proxy = proxy.clone();
            let tx = tx_to_child.clone();
            let skill = skill_name.to_string();
            tokio::spawn(async move {
                let response = match proxy.handle_query(&skill, &method, params).await {
                    Ok(v) => HostToSkill::QueryResponse { id, result: v },
                    Err(e) => HostToSkill::QueryError { id, error: e },
                };
                let _ = tx.send(response);
            });
        }
        SkillToHost::Log { level, message } => {
            debug!(skill = %skill_name, level = %level, %message, "skill_log");
        }
    }
}

async fn write_one<W: AsyncWriteExt + Unpin>(
    stdin: &mut W,
    msg: &HostToSkill,
) -> Result<(), std::io::Error> {
    let line = encode_host(msg).map_err(std::io::Error::other)?;
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await
}

fn spawn_child(manifest: &SkillManifest) -> std::io::Result<Child> {
    Command::new(&manifest.python)
        .arg(manifest.entry_point_abs())
        .current_dir(&manifest.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

/// After sending the polite `Shutdown` frame and dropping stdin,
/// wait up to `SHUTDOWN_GRACE` for the child to exit, then SIGKILL.
async fn kill_with_grace(child: &mut Child) {
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {
                if tokio::time::Instant::now() >= deadline {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(_) => return,
        }
    }
}

/// Sleep for `delay` or return early on shutdown. Returns true if the
/// caller should exit immediately (shutdown was requested).
async fn sleep_or_shutdown(delay: Duration, shutdown: &Notify) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = shutdown.notified() => true,
    }
}

fn next_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("h-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_cap() {
        let mut d = BACKOFF_INITIAL;
        let sequence: Vec<u64> = (0..10)
            .map(|_| {
                let ms = d.as_millis() as u64;
                d = (d * 2).min(BACKOFF_CAP);
                ms
            })
            .collect();
        // 1s, 2s, 4s, 8s, 16s, 32s, 60s, 60s, 60s, 60s
        assert_eq!(
            sequence,
            vec![
                1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000, 60000, 60000
            ]
        );
    }

    #[tokio::test]
    async fn refuse_all_proxy_rejects() {
        let p = RefuseAllProxy;
        let err = p
            .handle_query("scribe", "atom.get", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("rejected"));
    }

    #[test]
    fn next_id_is_unique() {
        let a = next_id();
        let b = next_id();
        assert_ne!(a, b);
    }
}
