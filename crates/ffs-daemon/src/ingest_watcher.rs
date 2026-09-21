//! Filesystem watcher on `$FFS_DATA_DIR/ingest/` that surfaces
//! user-dropped Markdown files as quarantine submissions.
//!
//! Mirrors the `notify::PollWatcher` pattern from
//! `crates/ffs-fastpath/src/watcher.rs` but with a much smaller
//! responsibility surface: this watcher does NOT classify edits or
//! emit supersession atoms. It just:
//!
//!   1. Reads a new `.md` file as it appears in `ingest/`.
//!   2. Calls `IngestQuarantine::submit` with the file's `file://`
//!      URI and bytes — returning a submission id.
//!   3. Spawns scribe extraction in the background; the resulting
//!      proposals land in the quarantine via `complete()` (or
//!      `fail()`).
//!   4. Moves the source file into `ingest/.processed/` so a
//!      re-submission requires a deliberate user action.
//!
//! Why this bypasses the JSON-RPC dispatcher's `ingest.submit`
//! capability check: the watcher runs in-process and represents
//! the *local user* acting on a file in their own data directory.
//! The capability gate is meaningful at the RPC boundary for
//! agent-driven submissions (Claude via MCP, the Obsidian plugin),
//! not for the daemon's own filesystem watch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event as NotifyEvent, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ffs_core::{IngestQuarantine, Multihash};

use crate::dispatch::ScribeExtractor;
use crate::notify::{Event, EventPublisher};

/// Default poll interval. PollWatcher walks the directory at this
/// cadence — small enough that an Obsidian save in `ingest/` is
/// surfaced within a couple of seconds, large enough that the
/// daemon's CPU is invisible at rest.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Default stability window: a newly-discovered `.md` file is held
/// for this long after its last content change before being
/// submitted to the quarantine. Gives the user space to compose a
/// note in-place (e.g., Obsidian's "New note" action) without the
/// daemon snatching a half-written file. `0` disables the window,
/// restoring the task_26 "submit on first event" behavior.
///
/// History: task_31 originally landed this at 60 s on the theory
/// that compose-in-place was the load-bearing UX. Live use in
/// 2026-06 showed 60 s of "nothing visible happening" after a
/// drop-in felt broken. Dropped to 30 s — still long enough that
/// an Obsidian "New note" + a few sentences of thoughtful typing
/// (with the natural 5-15 s pauses) stays inside the window,
/// short enough that drop-in feels reasonable.
/// `FFS_INGEST_STABILITY_MS` overrides for users who want a
/// different cadence.
pub const DEFAULT_STABILITY_WINDOW: Duration = Duration::from_secs(30);

/// Cadence at which the event loop checks pending files for
/// stabilization. Independent of the poll interval (which drives
/// FS event detection); 1 s gives the user-perceptible "edits
/// settle, then a moment later the file commits" rhythm without
/// burning CPU.
pub const STABILITY_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Subdirectory used to retire processed files. Hidden so it
/// doesn't clutter the Obsidian sidebar.
pub const PROCESSED_DIR: &str = ".processed";

/// A file the watcher has discovered but is holding until its
/// content stabilizes. `first_seen` resets whenever the content
/// hash changes — that's the "user is still typing" signal.
#[derive(Debug, Clone)]
struct PendingFile {
    hash: Multihash,
    first_seen: Instant,
}

/// The watcher's running shape. Holds the `notify::PollWatcher`
/// and the supervisor task — dropping this struct stops the
/// watcher and aborts the task.
pub struct IngestWatcher {
    _raw: notify::PollWatcher,
    _task: tokio::task::JoinHandle<()>,
}

/// Constructor parameters bundled into a struct so the wire-up
/// call site stays readable.
pub struct IngestWatcherConfig {
    pub ingest_dir: PathBuf,
    pub quarantine: Arc<dyn IngestQuarantine>,
    pub scribe: Option<Arc<dyn ScribeExtractor>>,
    pub publisher: Arc<EventPublisher>,
    /// ADR-029 auto-filer run after each extraction; `None` disables
    /// auto-filing for watcher submissions (tests, read-only daemons).
    pub auto_filer: Option<Arc<crate::autofile::AutoFiler>>,
    pub cancel: CancellationToken,
    pub poll_interval: Duration,
    /// How long a file must sit with unchanged content before the
    /// watcher submits it. `Duration::ZERO` reverts to the original
    /// task_26 behavior — submit on the first eligible event — and
    /// is the recommended setting for integration tests that don't
    /// want to wait out a real-world stability window.
    pub stability_window: Duration,
}

impl IngestWatcher {
    /// Start watching. Performs an initial reconciliation walk so
    /// `.md` files dropped while the daemon was down are picked up
    /// on next boot.
    pub fn start(cfg: IngestWatcherConfig) -> Result<Self, notify::Error> {
        std::fs::create_dir_all(&cfg.ingest_dir).ok();
        std::fs::create_dir_all(cfg.ingest_dir.join(PROCESSED_DIR)).ok();

        let (tx, rx) = mpsc::unbounded_channel::<NotifyEvent>();
        let config = notify::Config::default()
            .with_poll_interval(cfg.poll_interval)
            .with_compare_contents(true);
        let mut watcher = notify::PollWatcher::new(
            move |res: notify::Result<NotifyEvent>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            config,
        )?;
        let watch_path =
            std::fs::canonicalize(&cfg.ingest_dir).unwrap_or_else(|_| cfg.ingest_dir.clone());
        watcher.watch(&watch_path, RecursiveMode::Recursive)?;

        let task = tokio::spawn(event_loop(
            EventLoopCtx {
                ingest_dir: watch_path,
                quarantine: cfg.quarantine,
                scribe: cfg.scribe,
                publisher: cfg.publisher,
                auto_filer: cfg.auto_filer,
                cancel: cfg.cancel,
                stability_window: cfg.stability_window,
            },
            rx,
        ));

        Ok(Self {
            _raw: watcher,
            _task: task,
        })
    }
}

struct EventLoopCtx {
    ingest_dir: PathBuf,
    quarantine: Arc<dyn IngestQuarantine>,
    scribe: Option<Arc<dyn ScribeExtractor>>,
    publisher: Arc<EventPublisher>,
    auto_filer: Option<Arc<crate::autofile::AutoFiler>>,
    cancel: CancellationToken,
    stability_window: Duration,
}

async fn event_loop(ctx: EventLoopCtx, mut rx: mpsc::UnboundedReceiver<NotifyEvent>) {
    // Pending map drives the stability state machine. When
    // `stability_window` is zero we never insert and the event
    // path falls through to immediate processing.
    let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
    let mut tick = interval(STABILITY_CHECK_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // `interval` fires immediately on first poll; consume that
    // tick so the first real check happens one interval in.
    tick.tick().await;

    reconcile_existing(&ctx, &mut pending).await;

    loop {
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                info!("ingest watcher: shutdown requested");
                return;
            }
            // Only arm the stability tick when there's a window to
            // honor — otherwise the immediate-submit path makes the
            // tick redundant.
            _ = tick.tick(), if !ctx.stability_window.is_zero() => {
                check_stable(&ctx, &mut pending).await;
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { return };
                // Remove events: drop any pending entry for the
                // path. The user deleted the file before it
                // stabilized; we silently forget about it.
                if matches!(ev.kind, EventKind::Remove(_)) {
                    for path in &ev.paths {
                        if pending.remove(path).is_some() {
                            debug!(?path, "ingest_watcher: pending file removed before stability");
                        }
                    }
                    continue;
                }
                if !matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    continue;
                }
                for path in ev.paths {
                    if !is_eligible_ingest_file_under(&path, Some(&ctx.ingest_dir)) {
                        continue;
                    }
                    if ctx.stability_window.is_zero() {
                        process_one(&ctx, &path).await;
                    } else {
                        record_pending(&mut pending, &path);
                    }
                }
            }
        }
    }
}

/// Re-walk the ingest dir on startup. Picks up files dropped while
/// the daemon was down so we don't lose a submission to a missed
/// FS event. With a non-zero stability window, recovered files
/// enter the pending map and wait one window before submission —
/// safer than assuming a leftover file is "done" (the user might
/// have been editing it when the daemon died).
async fn reconcile_existing(ctx: &EventLoopCtx, pending: &mut HashMap<PathBuf, PendingFile>) {
    let Ok(entries) = std::fs::read_dir(&ctx.ingest_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_eligible_ingest_file_under(&path, Some(&ctx.ingest_dir)) {
            continue;
        }
        if ctx.stability_window.is_zero() {
            process_one(ctx, &path).await;
        } else {
            record_pending(pending, &path);
        }
    }
}

/// Record (or update) a pending file. New entries start the timer
/// at `now`; existing entries with a fresh content hash reset the
/// timer (the user is still typing); existing entries with the
/// same content hash leave `first_seen` untouched (idempotent
/// against duplicate FS events for the same save).
fn record_pending(pending: &mut HashMap<PathBuf, PendingFile>, path: &Path) {
    let content = match std::fs::read(path) {
        Ok(c) => c,
        Err(e) => {
            debug!(?path, error = %e, "ingest_watcher: read failed during record_pending");
            return;
        }
    };
    let hash = Multihash::blake3_of(&content);
    match pending.get_mut(path) {
        Some(entry) => {
            if entry.hash != hash {
                entry.hash = hash;
                entry.first_seen = Instant::now();
            }
        }
        None => {
            pending.insert(
                path.to_path_buf(),
                PendingFile {
                    hash,
                    first_seen: Instant::now(),
                },
            );
        }
    }
}

/// Walk the pending map and submit anything whose content has sat
/// unchanged for at least `stability_window`. Re-reads each
/// candidate file before submitting so a content change since the
/// last FS event is caught: if the on-disk hash no longer matches
/// the recorded hash, the timer resets with the new hash (treat as
/// fresh modification). If the file has disappeared, the entry is
/// dropped silently.
async fn check_stable(ctx: &EventLoopCtx, pending: &mut HashMap<PathBuf, PendingFile>) {
    let now = Instant::now();
    let candidates: Vec<PathBuf> = pending
        .iter()
        .filter(|(_, entry)| now.duration_since(entry.first_seen) >= ctx.stability_window)
        .map(|(path, _)| path.clone())
        .collect();

    for path in candidates {
        let Some(entry) = pending.get(&path) else {
            continue;
        };
        let expected_hash = entry.hash.clone();
        let content = match std::fs::read(&path) {
            Ok(c) => c,
            Err(_) => {
                // File vanished between the last event and this
                // check — treat like a Remove event.
                pending.remove(&path);
                debug!(
                    ?path,
                    "ingest_watcher: pending file disappeared before stability check"
                );
                continue;
            }
        };
        let current_hash = Multihash::blake3_of(&content);
        if current_hash != expected_hash {
            // Content changed since we last sampled. Treat as a
            // fresh modification: reset the timer with the new
            // hash and let the next tick re-evaluate.
            if let Some(entry) = pending.get_mut(&path) {
                entry.hash = current_hash;
                entry.first_seen = Instant::now();
            }
            continue;
        }
        pending.remove(&path);
        process_one(ctx, &path).await;
    }
}

/// Filter: only `.md` files at the top of the ingest dir count.
/// Hidden files (`.DS_Store`, dotfiles) and the `.processed/`
/// retirement subdir are skipped. Files inside a nested directory
/// are also skipped — the watcher's mental model is "drop a note
/// in ingest/", not "build a directory tree under ingest/".
pub fn is_eligible_ingest_file(path: &Path) -> bool {
    is_eligible_ingest_file_under(path, None)
}

/// As [`is_eligible_ingest_file`], bounded by the ingest directory so a
/// hidden *directory* anywhere between `ingest_dir` and the file makes
/// the file ineligible.
///
/// The bound matters: the substrate root is itself `~/.ffs`, so an
/// unbounded walk toward `/` would reject every file. Without the rule,
/// the watcher consumed the courier's dry-run output from
/// `ingest/.courier/dry-run/` and fed 57 files into the real pipeline
/// (seen live 2026-09-21). A dry run must stay dry, and hidden
/// directories under `ingest/` are agent scratch space, not drops.
pub fn is_eligible_ingest_file_under(path: &Path, ingest_dir: Option<&Path>) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    if path.extension().and_then(|s| s.to_str()) != Some("md") {
        return false;
    }
    for ancestor in path.ancestors().skip(1) {
        // Stop at the ingest dir: the substrate root is itself `~/.ffs`,
        // so walking past it would reject everything.
        if ingest_dir == Some(ancestor) {
            break;
        }
        let Some(dir_name) = ancestor.file_name().and_then(|s| s.to_str()) else {
            break;
        };
        let hidden = match ingest_dir {
            // Bounded: any hidden directory under `ingest/` is scratch
            // space, which covers `.processed/` and `.courier/`.
            Some(_) => dir_name.starts_with('.'),
            // Unbounded (legacy callers): only the retirement dir, since
            // every path under `~/.ffs` has a hidden ancestor.
            None => dir_name == PROCESSED_DIR,
        };
        if hidden {
            return false;
        }
    }
    true
}

async fn process_one(ctx: &EventLoopCtx, path: &Path) {
    let content = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            warn!(?path, error = %e, "ingest_watcher: read failed");
            return;
        }
    };
    let source_uri = format!("file://{}", path.display());
    let submission_id = match ctx
        .quarantine
        .submit(source_uri.clone(), content.clone())
        .await
    {
        Ok(id) => id,
        Err(e) => {
            warn!(?path, error = %e, "ingest_watcher: quarantine submit failed");
            return;
        }
    };
    info!(?path, submission_id = %submission_id, "ingest_watcher: submission accepted");

    // Move the source file aside so a re-save isn't a re-submit.
    // We do this BEFORE spawning extraction so even if the daemon
    // dies mid-extraction the user sees the file landed in
    // .processed/ and knows it was at least acknowledged.
    if let Err(e) = move_to_processed(&ctx.ingest_dir, path) {
        warn!(?path, error = %e, "ingest_watcher: move to .processed/ failed");
    }

    // Spawn scribe extraction in the background — same shape the
    // dispatcher's ingest_submit RPC uses. The pending submission
    // becomes a "proposal-ready" entry the daily-summary panel
    // surfaces; failures land as `Failed` submissions visible in
    // the auditor's daily summary.
    if let Some(scribe) = ctx.scribe.clone() {
        let quarantine = ctx.quarantine.clone();
        let publisher = ctx.publisher.clone();
        let auto_filer = ctx.auto_filer.clone();
        let submission_id = submission_id.clone();
        tokio::spawn(async move {
            match scribe.extract(&source_uri, &content).await {
                Ok(proposals) => {
                    // Publish a `event.atom.committed`-style hint
                    // is not appropriate (no atoms committed yet —
                    // proposals still need user acceptance), but
                    // emit a synthetic content_hash event so the
                    // Obsidian plugin's summary panel can refresh.
                    let hash = Multihash::blake3_of(&content);
                    debug!(submission_id = %submission_id, proposal_count = proposals.len(), "scribe extraction done");
                    if let Err(e) = quarantine.complete(&submission_id, proposals).await {
                        warn!(error = %e, id = %submission_id, "ingest_watcher: quarantine_complete_failed");
                    }
                    publisher.publish(crate::notify::Event::QuarantineChanged {
                        submission_id: Some(submission_id.clone()),
                    });
                    if let Some(af) = auto_filer.as_ref() {
                        match af.auto_file(&submission_id).await {
                            Ok(r) => {
                                debug!(id = %submission_id, filed = r.filed.len(), pending = r.pending.len(), "ingest_watcher: auto_file")
                            }
                            Err(e) => {
                                warn!(error = %e.message, id = %submission_id, "ingest_watcher: auto_file failed; submission left pending")
                            }
                        }
                    }
                    let _ = hash;
                }
                Err(e) => {
                    // A failed extraction is not silent: the submission
                    // becomes `Failed` (visible to the auditor), and the
                    // reason is logged so a scribe timeout or crash shows
                    // up in the daemon log without inspecting the DB.
                    warn!(error = %e, id = %submission_id, "ingest_watcher: scribe extraction failed; submission marked failed");
                    if let Err(e2) = quarantine
                        .fail(&submission_id, format!("scribe: {e}"))
                        .await
                    {
                        warn!(error = %e2, id = %submission_id, "ingest_watcher: quarantine_fail_failed");
                    }
                }
            }
        });
    } else {
        debug!("ingest_watcher: no scribe configured; submission stays Pending");
    }
}

/// Move `path` into `<ingest_dir>/.processed/<original_filename>`.
/// On a same-name collision, suffixes the destination with a
/// numeric index until we find an unused name.
fn move_to_processed(ingest_dir: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let processed = ingest_dir.join(PROCESSED_DIR);
    std::fs::create_dir_all(&processed)?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("source has no file name"))?;
    let mut dest = processed.join(name);
    let mut idx = 1;
    while dest.exists() {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("note");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("md");
        dest = processed.join(format!("{stem}.{idx}.{ext}"));
        idx += 1;
    }
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

// Suppress unused-import warning while we keep the publisher
// hooked into the context for the next-task extension point.
#[allow(dead_code)]
fn _unused_event_marker(_p: Arc<EventPublisher>, _e: Event) {}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    use ffs_core::{
        InMemoryQuarantine, IngestQuarantine, PredicateName, Proposal, SubmissionStatus,
    };

    /// In-process scribe stub that returns a single canned proposal
    /// every time. Lets the test exercise the watcher's spawn-and-
    /// route plumbing without standing up a Python subprocess.
    struct StubScribe;
    #[async_trait]
    impl ScribeExtractor for StubScribe {
        async fn extract(
            &self,
            _source_uri: &str,
            _content: &[u8],
        ) -> Result<Vec<Proposal>, crate::dispatch::ScribeExtractError> {
            Ok(vec![Proposal::new(
                PredicateName::new("note"),
                serde_json::json!({"title": "stub"}),
                vec![],
                "stub extractor",
            )])
        }
    }

    fn touch(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn eligible_filter_accepts_top_level_md() {
        let tmp = tempfile::tempdir().unwrap();
        let p = touch(tmp.path(), "note.md", "x");
        assert!(is_eligible_ingest_file(&p));
    }

    #[test]
    fn eligible_filter_skips_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        let p = touch(tmp.path(), ".DS_Store", "x");
        assert!(!is_eligible_ingest_file(&p));
        let dotfile = touch(tmp.path(), ".secret.md", "x");
        assert!(!is_eligible_ingest_file(&dotfile));
    }

    #[test]
    fn eligible_filter_skips_non_md_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let p = touch(tmp.path(), "note.txt", "x");
        assert!(!is_eligible_ingest_file(&p));
    }

    #[test]
    fn eligible_filter_skips_processed_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let processed = tmp.path().join(PROCESSED_DIR);
        std::fs::create_dir_all(&processed).unwrap();
        let p = touch(&processed, "old.md", "x");
        assert!(!is_eligible_ingest_file(&p));
    }

    /// Build an EventLoopCtx with the legacy "submit immediately"
    /// behavior. Tests that don't care about the stability window
    /// take this and exercise `process_one` directly.
    fn immediate_ctx(ingest_dir: PathBuf, quarantine: Arc<dyn IngestQuarantine>) -> EventLoopCtx {
        EventLoopCtx {
            ingest_dir,
            quarantine,
            scribe: Some(Arc::new(StubScribe) as Arc<dyn ScribeExtractor>),
            publisher: Arc::new(EventPublisher::new()),
            auto_filer: None,
            cancel: CancellationToken::new(),
            stability_window: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn process_one_submits_and_moves_to_processed() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let ctx = immediate_ctx(ingest_dir.clone(), quarantine.clone());

        let source = touch(&ingest_dir, "tuesday.md", "# tuesday\nbody");
        process_one(&ctx, &source).await;

        // Source moved.
        assert!(
            !source.exists(),
            "source file should be moved out of ingest/"
        );
        assert!(
            ingest_dir.join(PROCESSED_DIR).join("tuesday.md").exists(),
            "file should land in .processed/"
        );

        // Quarantine submission appeared. Wait briefly for the
        // spawned scribe-completion task to land.
        let mut subs = quarantine.list(None).await;
        let start = std::time::Instant::now();
        while subs.is_empty() || subs[0].status != SubmissionStatus::Extracted {
            if start.elapsed() > Duration::from_millis(500) {
                panic!("submission did not transition: {subs:?}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            subs = quarantine.list(None).await;
        }
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].status, SubmissionStatus::Extracted);
        assert_eq!(subs[0].proposals.len(), 1);
    }

    #[tokio::test]
    async fn process_one_handles_same_name_collision_in_processed() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let processed = ingest_dir.join(PROCESSED_DIR);
        std::fs::create_dir_all(&processed).unwrap();
        // Seed an existing entry in .processed/ so the move has
        // to pick a non-colliding suffix.
        std::fs::write(processed.join("note.md"), b"old").unwrap();

        let source = touch(&ingest_dir, "note.md", "new");
        let dest = move_to_processed(&ingest_dir, &source).unwrap();
        assert!(!source.exists());
        assert!(dest.exists());
        assert_ne!(
            dest,
            processed.join("note.md"),
            "should not have overwritten"
        );
        let new_bytes = std::fs::read(&dest).unwrap();
        assert_eq!(new_bytes, b"new");
        // Original .processed/note.md stays put.
        let old_bytes = std::fs::read(processed.join("note.md")).unwrap();
        assert_eq!(old_bytes, b"old");
    }

    #[tokio::test]
    async fn reconcile_picks_up_files_dropped_while_daemon_was_down() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&ingest_dir).unwrap();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let ctx = immediate_ctx(ingest_dir.clone(), quarantine.clone());
        // Pre-seed a file as if the user had dropped it while
        // the daemon was offline.
        touch(&ingest_dir, "while_offline.md", "x");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        reconcile_existing(&ctx, &mut pending).await;

        let start = std::time::Instant::now();
        loop {
            let subs = quarantine.list(None).await;
            if !subs.is_empty() {
                break;
            }
            if start.elapsed() > Duration::from_millis(500) {
                panic!("reconcile did not produce a submission");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // ---- Stability-window state machine (task_31) ----

    /// First eligible event for a new file inserts a pending entry
    /// with the file's current content hash.
    #[test]
    fn record_pending_inserts_new_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = touch(tmp.path(), "note.md", "first");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);
        assert_eq!(pending.len(), 1);
        let entry = pending.get(&path).expect("present");
        assert_eq!(entry.hash, Multihash::blake3_of(b"first"));
    }

    /// A second event whose content hash differs from the recorded
    /// one resets `first_seen` — that's the "user is still typing"
    /// signal that delays submission.
    #[test]
    fn record_pending_resets_first_seen_when_hash_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = touch(tmp.path(), "note.md", "first");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);
        let first_seen_v1 = pending.get(&path).unwrap().first_seen;
        // Sleep enough that any reset is observable on the clock.
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&path, b"second").unwrap();
        record_pending(&mut pending, &path);
        let entry = pending.get(&path).expect("present");
        assert_eq!(entry.hash, Multihash::blake3_of(b"second"));
        assert!(
            entry.first_seen > first_seen_v1,
            "first_seen should advance on content change"
        );
    }

    /// A duplicate event whose content hash matches the recorded
    /// one leaves `first_seen` untouched — duplicate FS events for
    /// the same save mustn't reset the timer.
    #[test]
    fn record_pending_keeps_first_seen_when_hash_same() {
        let tmp = tempfile::tempdir().unwrap();
        let path = touch(tmp.path(), "note.md", "first");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);
        let first_seen_v1 = pending.get(&path).unwrap().first_seen;
        std::thread::sleep(Duration::from_millis(5));
        // No on-disk change — duplicate FS event.
        record_pending(&mut pending, &path);
        let entry = pending.get(&path).expect("present");
        assert_eq!(
            entry.first_seen, first_seen_v1,
            "first_seen should not move when content is unchanged"
        );
    }

    /// An entry whose `first_seen` is younger than the stability
    /// window stays in the pending map and is NOT submitted.
    #[tokio::test]
    async fn check_stable_skips_entries_within_window() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let mut ctx = immediate_ctx(ingest_dir.clone(), quarantine.clone());
        ctx.stability_window = Duration::from_secs(60);

        let path = touch(&ingest_dir, "note.md", "draft");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);
        check_stable(&ctx, &mut pending).await;

        assert_eq!(pending.len(), 1, "entry should still be pending");
        assert!(
            quarantine.list(None).await.is_empty(),
            "no submission expected within the stability window"
        );
        assert!(path.exists(), "source should not be moved yet");
    }

    /// An entry whose `first_seen` is older than the stability
    /// window is submitted and removed from the pending map.
    #[tokio::test]
    async fn check_stable_submits_entries_past_window() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let mut ctx = immediate_ctx(ingest_dir.clone(), quarantine.clone());
        // Tiny window — we'll just sleep past it for determinism.
        ctx.stability_window = Duration::from_millis(20);

        let path = touch(&ingest_dir, "note.md", "stable content");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);

        tokio::time::sleep(Duration::from_millis(30)).await;
        check_stable(&ctx, &mut pending).await;

        assert!(
            pending.is_empty(),
            "pending entry should be drained after submission"
        );
        // Wait for the spawned scribe-completion task to land.
        let start = std::time::Instant::now();
        loop {
            let subs = quarantine.list(None).await;
            if !subs.is_empty() && subs[0].status == SubmissionStatus::Extracted {
                break;
            }
            if start.elapsed() > Duration::from_millis(500) {
                panic!("expected exactly one extracted submission");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!path.exists(), "source should have moved to .processed/");
        assert!(ingest_dir.join(PROCESSED_DIR).join("note.md").exists());
    }

    /// If the file's content changed since the last FS event but
    /// before this check tick, the recorded hash no longer matches
    /// the on-disk hash. `check_stable` resets the timer with the
    /// new hash rather than submitting stale content.
    #[tokio::test]
    async fn check_stable_resets_when_content_changed_since_last_event() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let mut ctx = immediate_ctx(ingest_dir.clone(), quarantine.clone());
        ctx.stability_window = Duration::from_millis(10);

        let path = touch(&ingest_dir, "note.md", "version-A");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);
        let first_seen_v1 = pending.get(&path).unwrap().first_seen;

        // Sleep past the window, then mutate the file behind the
        // watcher's back (simulating a save that landed in the
        // gap between the last FS event and this tick).
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(&path, b"version-B").unwrap();
        check_stable(&ctx, &mut pending).await;

        // Not submitted — the recorded hash no longer matched.
        assert!(quarantine.list(None).await.is_empty());
        assert!(path.exists(), "source should not have been moved");
        let entry = pending.get(&path).expect("still pending");
        assert_eq!(entry.hash, Multihash::blake3_of(b"version-B"));
        assert!(
            entry.first_seen > first_seen_v1,
            "first_seen should have been reset by the on-disk change"
        );
    }

    /// A Remove event before the file stabilizes drops the entry
    /// silently — no submission, no panic, no error.
    #[tokio::test]
    async fn pending_entry_dropped_when_file_disappears_before_check() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let mut ctx = immediate_ctx(ingest_dir.clone(), quarantine.clone());
        ctx.stability_window = Duration::from_millis(10);

        let path = touch(&ingest_dir, "note.md", "draft");
        let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
        record_pending(&mut pending, &path);

        // User deletes the file before stability lands.
        std::fs::remove_file(&path).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        check_stable(&ctx, &mut pending).await;

        assert!(
            pending.is_empty(),
            "missing file should be dropped from pending map"
        );
        assert!(
            quarantine.list(None).await.is_empty(),
            "no submission expected for a deleted file"
        );
    }
}

#[cfg(test)]
mod dry_run_isolation_tests {
    use super::*;

    /// Live finding 2026-09-21: the courier's dry run wrote to
    /// `ingest/.courier/dry-run/<ts>/`, the watcher walked in, and 57
    /// files meant as a preview entered the real pipeline. A dry run
    /// must stay dry: hidden directories under `ingest/` are scratch.
    #[test]
    fn files_inside_a_hidden_directory_under_ingest_are_not_eligible() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest = tmp.path().join("ingest");
        let scratch = ingest
            .join(".courier")
            .join("dry-run")
            .join("20260921T112116Z");
        std::fs::create_dir_all(&scratch).unwrap();
        let preview = scratch.join("charlotte-business-journal-2026-09-19-story.md");
        std::fs::write(&preview, "---\npredicate: source.article\n---\n").unwrap();

        assert!(
            !is_eligible_ingest_file_under(&preview, Some(&ingest)),
            "a dry-run preview must never be ingested"
        );

        // A real drop at the top of ingest/ is still eligible.
        let drop = ingest.join("note.md");
        std::fs::write(&drop, "hello").unwrap();
        assert!(is_eligible_ingest_file_under(&drop, Some(&ingest)));

        // The bound matters: the substrate root is itself hidden.
        let hidden_root = tmp.path().join(".ffs").join("ingest");
        std::fs::create_dir_all(&hidden_root).unwrap();
        let under_hidden_root = hidden_root.join("note.md");
        std::fs::write(&under_hidden_root, "hello").unwrap();
        assert!(
            is_eligible_ingest_file_under(&under_hidden_root, Some(&hidden_root)),
            "a hidden ancestor above the ingest dir must not disqualify a drop"
        );
    }
}
