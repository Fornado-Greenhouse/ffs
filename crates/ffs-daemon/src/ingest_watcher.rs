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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event as NotifyEvent, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
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

/// Subdirectory used to retire processed files. Hidden so it
/// doesn't clutter the Obsidian sidebar.
pub const PROCESSED_DIR: &str = ".processed";

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
    pub cancel: CancellationToken,
    pub poll_interval: Duration,
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
                cancel: cfg.cancel,
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
    cancel: CancellationToken,
}

async fn event_loop(ctx: EventLoopCtx, mut rx: mpsc::UnboundedReceiver<NotifyEvent>) {
    reconcile_existing(&ctx).await;

    loop {
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                info!("ingest watcher: shutdown requested");
                return;
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { return };
                if !matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    continue;
                }
                for path in ev.paths {
                    if !is_eligible_ingest_file(&path) {
                        continue;
                    }
                    process_one(&ctx, &path).await;
                }
            }
        }
    }
}

/// Re-walk the ingest dir on startup. Picks up files dropped while
/// the daemon was down so we don't lose a submission to a missed
/// FS event.
async fn reconcile_existing(ctx: &EventLoopCtx) {
    let Ok(entries) = std::fs::read_dir(&ctx.ingest_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_eligible_ingest_file(&path) {
            continue;
        }
        process_one(ctx, &path).await;
    }
}

/// Filter: only `.md` files at the top of the ingest dir count.
/// Hidden files (`.DS_Store`, dotfiles) and the `.processed/`
/// retirement subdir are skipped. Files inside a nested directory
/// are also skipped — the watcher's mental model is "drop a note
/// in ingest/", not "build a directory tree under ingest/".
pub fn is_eligible_ingest_file(path: &Path) -> bool {
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
    // Skip anything under the .processed/ retirement dir.
    for ancestor in path.ancestors().skip(1) {
        if ancestor.file_name().and_then(|s| s.to_str()) == Some(PROCESSED_DIR) {
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
                    let _ = publisher; // reserved for a future `event.ingest.extracted` channel
                    if let Err(e) = quarantine.complete(&submission_id, proposals).await {
                        warn!(error = %e, id = %submission_id, "ingest_watcher: quarantine_complete_failed");
                    }
                    let _ = hash;
                }
                Err(e) => {
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
            Ok(vec![Proposal {
                predicate: PredicateName::new("note"),
                claim: serde_json::json!({"title": "stub"}),
                provenance: vec![],
                rationale: "stub extractor".into(),
            }])
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

    #[tokio::test]
    async fn process_one_submits_and_moves_to_processed() {
        let tmp = tempfile::tempdir().unwrap();
        let ingest_dir = tmp.path().to_path_buf();
        let quarantine: Arc<dyn IngestQuarantine> = Arc::new(InMemoryQuarantine::new());
        let ctx = EventLoopCtx {
            ingest_dir: ingest_dir.clone(),
            quarantine: quarantine.clone(),
            scribe: Some(Arc::new(StubScribe) as Arc<dyn ScribeExtractor>),
            publisher: Arc::new(EventPublisher::new()),
            cancel: CancellationToken::new(),
        };

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
        let ctx = EventLoopCtx {
            ingest_dir: ingest_dir.clone(),
            quarantine: quarantine.clone(),
            scribe: Some(Arc::new(StubScribe) as Arc<dyn ScribeExtractor>),
            publisher: Arc::new(EventPublisher::new()),
            cancel: CancellationToken::new(),
        };
        // Pre-seed a file as if the user had dropped it while
        // the daemon was offline.
        touch(&ingest_dir, "while_offline.md", "x");
        reconcile_existing(&ctx).await;

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
}
