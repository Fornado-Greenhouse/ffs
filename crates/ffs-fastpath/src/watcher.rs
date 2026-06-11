//! Filesystem-watcher orchestrator. Wires `notify` to the classifier and
//! dispatch logic, debouncing rapid editor saves and ignoring
//! daemon-induced writes via the suppression registry.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use ffs_core::predicate::SpecRegistry;
use ffs_core::projection::path as projection_path;
use ffs_core::store::AtomStore;
use ffs_daemon::notify::EventPublisher;

use crate::classifier::{classify, is_federated_path};
use crate::dispatch::dispatch;
use crate::suppress::SuppressionRegistry;

/// Default debounce window for collapsing rapid editor save events. Most
/// editors emit 2-5 events per save (open, write, rename, close); 50ms
/// is enough to absorb the burst without delaying interactive feedback.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(50);

pub struct FastPathWatcher {
    _raw: RecommendedWatcher,
    _task: tokio::task::JoinHandle<()>,
}

/// Variant of `FastPathWatcher` that holds the polling backend instead of
/// the recommended one. Exposed for tests that need deterministic event
/// delivery (macOS FSEvents in particular has substantial latency).
pub struct PollingFastPathWatcher {
    _raw: notify::PollWatcher,
    _task: tokio::task::JoinHandle<()>,
}

impl PollingFastPathWatcher {
    pub fn start(mut ctx: FastPathContext, poll_interval: Duration) -> Result<Self, notify::Error> {
        let (tx, rx) = mpsc::unbounded_channel::<NotifyEvent>();
        // `compare_contents(true)` is essential for sub-second test edits:
        // PollWatcher's mtime is at second granularity, so two writes inside
        // the same second are only distinguishable via content hash.
        let config = notify::Config::default()
            .with_poll_interval(poll_interval)
            .with_compare_contents(true);
        let mut watcher = notify::PollWatcher::new(
            move |res: notify::Result<NotifyEvent>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            config,
        )?;
        // Canonicalize the watched directory so the paths in notify events
        // (which are canonical on macOS, e.g., `/private/var/...`) match
        // `ctx.working_set_dir` for `strip_prefix`. Without this, every
        // event is dropped because the prefix doesn't match.
        let watch_path = std::fs::canonicalize(&ctx.working_set_dir)
            .unwrap_or_else(|_| ctx.working_set_dir.clone());
        ctx.working_set_dir = watch_path.clone();
        watcher.watch(&watch_path, RecursiveMode::Recursive)?;
        let task = tokio::spawn(event_loop(ctx, rx));
        Ok(Self {
            _raw: watcher,
            _task: task,
        })
    }
}

#[derive(Clone)]
pub struct FastPathContext {
    pub store: Arc<dyn AtomStore>,
    pub registry: Arc<SpecRegistry>,
    pub notifier: Arc<EventPublisher>,
    pub signing_key: Arc<SigningKey>,
    pub working_set_dir: PathBuf,
    pub ingest_dir: PathBuf,
    pub suppression: Arc<SuppressionRegistry>,
}

impl FastPathWatcher {
    /// Start watching `working_set_dir` recursively. Returns a handle that
    /// keeps the watcher alive until dropped.
    pub fn start(ctx: FastPathContext) -> Result<Self, notify::Error> {
        let (tx, rx) = mpsc::unbounded_channel::<NotifyEvent>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<NotifyEvent>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?;
        watcher.watch(&ctx.working_set_dir, RecursiveMode::Recursive)?;

        let task = tokio::spawn(event_loop(ctx, rx));
        Ok(Self {
            _raw: watcher,
            _task: task,
        })
    }
}

async fn event_loop(ctx: FastPathContext, mut rx: mpsc::UnboundedReceiver<NotifyEvent>) {
    // On startup, walk the working set and re-process any projection file
    // whose on-disk content has drifted from the rendered head atom.
    // Catches edits made while the daemon was down (subtask 9.7).
    reconcile_working_set(&ctx).await;

    // Simple debouncer: when we see an event for a path, sleep `DEFAULT_DEBOUNCE`
    // and then process whatever's currently on disk (collapsing any
    // intervening writes for the same path).
    let mut pending: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let debounce = DEFAULT_DEBOUNCE;
    loop {
        let Some(ev) = rx.recv().await else { return };
        if !matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            continue;
        }
        for p in ev.paths {
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            pending.insert(p);
        }
        // Drain any immediately-following events into the same batch.
        tokio::time::sleep(debounce).await;
        while let Ok(ev) = rx.try_recv() {
            if !matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                continue;
            }
            for p in ev.paths {
                if p.extension().and_then(|s| s.to_str()) == Some("md") {
                    pending.insert(p);
                }
            }
        }
        let batch: Vec<_> = pending.drain().collect();
        for path in batch {
            if let Err(e) = process_one(&ctx, &path).await {
                warn!(error = %e, ?path, "fast-path processing failed");
            }
        }
    }
}

/// Walk every `.md` file under the working set on startup and dispatch
/// it through the same processing pipeline as a live event. Unchanged
/// files short-circuit at the no-op guard in `process_one`; drifted
/// files produce a supersession or route-to-ingest as appropriate.
async fn reconcile_working_set(ctx: &FastPathContext) {
    let mut stack: Vec<PathBuf> = vec![ctx.working_set_dir.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.file_type() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md")
                && let Err(e) = process_one(ctx, &path).await
            {
                warn!(error = %e, ?path, "reconciliation processing failed");
            }
        }
    }
}

async fn process_one(ctx: &FastPathContext, path: &std::path::Path) -> std::io::Result<()> {
    let new_content = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    if ctx.suppression.check(path, &new_content) {
        debug!(?path, "suppressed daemon-induced write");
        return Ok(());
    }
    let rel = match path.strip_prefix(&ctx.working_set_dir) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    // Normalize OS-native backslashes to forward slashes so the
    // string we hand to the path parser + every downstream event
    // payload is substrate-canonical on every host. Without this,
    // Windows ships `contacts\\by-name\\S\\Sarah_Chen.md` into
    // event.projection.invalidated.params.path and the fast-path
    // classifier silently falls through to slow-path because
    // `parse()` can't decompose a `\`-separated string.
    // See projection::path::normalize_separators + task_34.
    let rel_str = projection_path::normalize_separators(&rel.to_string_lossy()).into_owned();

    if is_federated_path(&rel_str) {
        let _ = crate::dispatch::route_to_ingest(
            &ctx.notifier,
            &ctx.ingest_dir,
            &rel_str,
            &new_content,
            &crate::classifier::SlowPathReason::FederatedProjection,
        );
        return Ok(());
    }

    // Parse path into (family, entity); listings + unsupported subpaths
    // route to ingest.
    let parsed = match projection_path::parse(&rel_str) {
        Ok(p) => p,
        Err(_) => {
            let _ = crate::dispatch::route_to_ingest(
                &ctx.notifier,
                &ctx.ingest_dir,
                &rel_str,
                &new_content,
                &crate::classifier::SlowPathReason::PathOrHeadUnavailable,
            );
            return Ok(());
        }
    };
    let (family, entity) = match parsed {
        projection_path::ParsedPath::SingleEntity { family, entity } => (family, entity),
        _ => {
            let _ = crate::dispatch::route_to_ingest(
                &ctx.notifier,
                &ctx.ingest_dir,
                &rel_str,
                &new_content,
                &crate::classifier::SlowPathReason::PathOrHeadUnavailable,
            );
            return Ok(());
        }
    };
    let predicate = family.primary_predicate();
    let Some(spec) = ctx.registry.get(predicate.as_str()) else {
        let _ = crate::dispatch::route_to_ingest(
            &ctx.notifier,
            &ctx.ingest_dir,
            &rel_str,
            &new_content,
            &crate::classifier::SlowPathReason::NoReverseMapRules,
        );
        return Ok(());
    };

    // Fetch head atom + render its current projection for diffing.
    let head = match ctx.store.head_of_chain(&entity, &predicate, None) {
        Ok(Some(h)) => h,
        _ => {
            let _ = crate::dispatch::route_to_ingest(
                &ctx.notifier,
                &ctx.ingest_dir,
                &rel_str,
                &new_content,
                &crate::classifier::SlowPathReason::PathOrHeadUnavailable,
            );
            return Ok(());
        }
    };

    // Compose old markdown by rendering the head atom against the spec's
    // template. We render directly via tera to avoid going through the
    // projection renderer's capability check (the fastpath is running
    // server-side as the owner).
    let old_markdown = render_via_template(&spec, &head);

    let new_str = String::from_utf8_lossy(&new_content);
    // No-op guard: if the on-disk content already matches the rendered
    // head, there's nothing to author. Required for restart
    // reconciliation, which walks every projection file and would
    // otherwise route unchanged files to ingest as `AmbiguousDiff`.
    if old_markdown == new_str {
        return Ok(());
    }
    let classification = classify(&spec, &head.claim, &old_markdown, &new_str);

    let _ = dispatch(
        classification,
        &ctx.store,
        &ctx.notifier,
        &ctx.signing_key,
        &head,
        &rel_str,
        path,
        &new_content,
        &ctx.ingest_dir,
        &ctx.suppression,
    );
    Ok(())
}

/// Render a single atom into the diff-baseline markdown shape used by the
/// classifier. The classifier only needs frontmatter keys and additive
/// sections to render in the *same shape* as the on-disk file — exact
/// layout fidelity belongs to the projection renderer, which a future
/// task will plumb through here. This direct builder avoids depending on
/// the predicate spec's template, which may live outside this crate.
fn render_via_template(
    spec: &ffs_core::predicate::PredicateSpec,
    head: &ffs_core::AtomEnvelope,
) -> String {
    let mut out = String::from("---\n");
    if let Some(obj) = head.claim.as_object() {
        for k in &spec.rendering.frontmatter_fields {
            if let Some(v) = obj.get(k).and_then(|x| x.as_str()) {
                out.push_str(k);
                out.push_str(": ");
                out.push_str(v);
                out.push('\n');
            }
        }
    }
    out.push_str("---\n");
    // Emit each additive section as a `## Name` header followed by its
    // `- item` bullets. Only emit the section when the underlying claim
    // field is a non-empty array — matches the baseline shape used in
    // tests and the renderer's convention of omitting empty sections.
    if let Some(obj) = head.claim.as_object() {
        for section in &spec.rendering.additive_sections {
            // Convention: claim field for a section is the section name
            // lowercased (e.g., `Notes` → `notes`). The reverse-map rule's
            // `atom_field` of `claim.notes[]` agrees with this convention.
            let field = section.to_ascii_lowercase();
            if let Some(arr) = obj.get(&field).and_then(|v| v.as_array())
                && !arr.is_empty()
            {
                out.push('\n');
                out.push_str("## ");
                out.push_str(section);
                out.push('\n');
                for item in arr {
                    if let Some(s) = item.as_str() {
                        out.push_str("- ");
                        out.push_str(s);
                        out.push('\n');
                    }
                }
            }
        }
    }
    out.push('\n');
    out
}
