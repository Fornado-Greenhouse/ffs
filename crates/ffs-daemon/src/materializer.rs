//! Working-set materializer.
//!
//! Subscribes to `event.atom.committed` notifications, looks up the
//! affected entity's projection path, renders the canonical markdown
//! via the projection renderer, and writes the file to disk under
//! `$FFS_DATA_DIR/`. The Obsidian plugin's folder view (and any
//! editor that opens a `*.md` file under the data dir) then sees
//! the entity as a real file.
//!
//! Two coordination concerns:
//!
//! 1. **Anti-loop with the fast-path watcher.** The fast-path will
//!    classify any FS event under the data dir; without coordination
//!    the materializer's own writes would trigger fast-path
//!    classifications which would emit further atoms which would
//!    re-fire the materializer. Coordination flows through the
//!    shared `SuppressionRegistry` (recorded before write, checked
//!    by the fast-path watcher on receive; hash-keyed so it self-
//!    cleans).
//! 2. **Idempotence.** Re-materializing an entity whose rendered
//!    content matches the on-disk file is a no-op — no write, no
//!    mtime churn. This matters because the `event.atom.committed`
//!    stream fires on every supersession; without the no-op guard
//!    every atom edit would touch the file mtime regardless of
//!    whether the rendered output actually changed.
//!
//! Capability filtering is delegated to `ProjectionRenderer::render`,
//! which already does the action × scope × bitemporal-window
//! evaluation per ADR-013. When the renderer denies, the
//! materializer treats it as "no readable atoms for this entity" and
//! writes nothing — the file (if it exists from a prior capability
//! state) is intentionally left in place rather than deleted, since
//! capability narrowing shouldn't destroy the user's working set.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use ffs_core::projection::{
    ProjectionRenderer, ProjectionRequest, RenderError,
    path::{PathFamily, family_for_predicate, path_for_entity},
};
use ffs_core::working_set::WorkingSetStore;
use ffs_core::{EntityId, Iso8601, Multihash, PredicateName, PublicKey, SuppressionRegistry};

use crate::notify::EventPublisher;

/// Result of a successful materialization. `None` from
/// [`WorkingSetMaterializer::materialize_entity`] means "rendered
/// successfully, but nothing was written" — either because the
/// projection bytes match what's already on disk (idempotent
/// no-op) or because the renderer denied the read (capability).
#[derive(Debug, Clone)]
pub struct Materialized {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub render_hash: Multihash,
}

/// Materialization handle. Holds the renderer, working-set store,
/// suppression registry, and the data-dir prefix the materializer
/// joins relative paths against.
pub struct WorkingSetMaterializer {
    renderer: Arc<ProjectionRenderer>,
    working_set: Arc<dyn WorkingSetStore>,
    suppression: Arc<SuppressionRegistry>,
    data_dir: PathBuf,
    owner: PublicKey,
}

impl WorkingSetMaterializer {
    pub fn new(
        renderer: Arc<ProjectionRenderer>,
        working_set: Arc<dyn WorkingSetStore>,
        suppression: Arc<SuppressionRegistry>,
        data_dir: PathBuf,
        owner: PublicKey,
    ) -> Self {
        Self {
            renderer,
            working_set,
            suppression,
            data_dir,
            owner,
        }
    }

    /// Materialize an entity by rendering its primary-predicate
    /// projection and writing the result. Returns `Some` on a write,
    /// `None` on a no-op (idempotent, capability-denied, or
    /// path-unmapped).
    pub async fn materialize_entity(
        &self,
        family: PathFamily,
        entity: &EntityId,
    ) -> Result<Option<Materialized>, MaterializeError> {
        let Some(rel_path) = path_for_entity(family, entity) else {
            debug!(entity = entity.as_str(), "no path mapping; skipping");
            return Ok(None);
        };
        let abs_path = self.data_dir.join(&rel_path);

        let request = ProjectionRequest {
            path: rel_path.clone(),
            as_of: None,
            agent: self.owner.clone(),
        };
        let rendered = match self.renderer.render(&request) {
            Ok(r) => r,
            Err(RenderError::CapabilityDenied(reason)) => {
                debug!(
                    entity = entity.as_str(),
                    ?reason,
                    "capability denied; not writing"
                );
                return Ok(None);
            }
            Err(RenderError::AtomNotFound { .. }) => {
                debug!(entity = entity.as_str(), "no head atom; not writing");
                return Ok(None);
            }
            Err(e) => return Err(MaterializeError::Render(Box::new(e))),
        };
        let bytes = rendered.markdown.into_bytes();
        let render_hash = rendered.render_hash;

        // Idempotence: skip the write if the existing file already
        // matches. Catches the common case where an atom supersession
        // didn't actually change the rendered output.
        if let Ok(existing) = std::fs::read(&abs_path)
            && Multihash::blake3_of(&existing) == render_hash
        {
            return Ok(None);
        }

        // Record the suppression BEFORE we write so a watcher that
        // sees the write within the same tick (PollWatcher with a
        // sub-second interval) consults a populated registry. The
        // registry is content-hash-keyed, so the order is correctness-
        // critical only on rare timing edges, but recording first
        // keeps the contract clean.
        self.suppression.record(&abs_path, &bytes);

        atomic_write(&abs_path, &bytes).map_err(|e| MaterializeError::Io(rel_path.clone(), e))?;

        let now = current_iso8601();
        if let Err(e) = self
            .working_set
            .upsert(rel_path.clone(), render_hash.clone(), now)
            .await
        {
            warn!(path = %rel_path, error = %e, "working-set upsert failed");
        }

        Ok(Some(Materialized {
            path: abs_path,
            bytes,
            render_hash,
        }))
    }

    /// Dispatch one parsed `event.atom.committed` notification.
    /// Surfaces any rendering / IO error but treats unmapped
    /// predicates as a benign skip.
    pub async fn handle_commit(
        &self,
        entity: &EntityId,
        predicate: &PredicateName,
    ) -> Result<Option<Materialized>, MaterializeError> {
        let Some(family) = family_for_predicate(predicate) else {
            return Ok(None);
        };
        self.materialize_entity(family, entity).await
    }

    /// Spawn a tokio task that subscribes to `publisher`'s broadcast
    /// channel and calls `handle_commit` for every
    /// `event.atom.committed` frame. Returns the join handle; drop
    /// it to keep the task running, or `await` it for shutdown.
    pub fn spawn(self: Arc<Self>, publisher: Arc<EventPublisher>) -> JoinHandle<()> {
        let mut rx = publisher.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(line) => self.dispatch_line(&line).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            skipped = n,
                            "materializer: broadcast lagged; some commits skipped"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!("materializer: broadcast closed; exiting");
                        return;
                    }
                }
            }
        })
    }

    /// Parse one broadcast line as a JSON-RPC notification frame
    /// and dispatch if it's an `event.atom.committed`. Other frames
    /// (projection.invalidated, fastpath.applied, federation.peer
    /// .changed) are ignored by the materializer.
    async fn dispatch_line(&self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if v.get("method").and_then(Value::as_str) != Some("event.atom.committed") {
            return;
        }
        let params = match v.get("params") {
            Some(p) => p,
            None => return,
        };
        let entity_s = match params.get("entity").and_then(Value::as_str) {
            Some(e) => e,
            None => return,
        };
        let predicate_s = match params.get("predicate").and_then(Value::as_str) {
            Some(p) => p,
            None => return,
        };
        let entity = EntityId::new(entity_s);
        let predicate = PredicateName::new(predicate_s);
        if let Err(e) = self.handle_commit(&entity, &predicate).await {
            warn!(
                entity = entity_s,
                predicate = predicate_s,
                error = %e,
                "materializer: handle_commit failed"
            );
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("render error: {0}")]
    Render(Box<RenderError>),
    #[error("io {0}: {1}")]
    Io(String, std::io::Error),
}

/// Atomic file write: write to a sibling temp file, then rename
/// over the destination. Rename on the same filesystem is atomic
/// per POSIX, so an editor reading the destination either sees the
/// previous content or the new content — never half-written bytes.
fn atomic_write(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = match dest.file_name() {
        Some(name) => dest.with_file_name(format!(".{}.tmp", name.to_string_lossy())),
        None => return Err(std::io::Error::other("destination has no file name")),
    };
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

fn current_iso8601() -> Iso8601 {
    let now = time::OffsetDateTime::now_utc();
    let formatted = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    Iso8601::new(&formatted).unwrap_or_else(|_| Iso8601::new("1970-01-01T00:00:00Z").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use ffs_core::capability::{Action, CapabilityScope, build_capability_atom};
    use ffs_core::predicate::SpecRegistry;
    use ffs_core::store::{AtomStore, MemAtomStore};
    use ffs_core::working_set::InMemoryWorkingSet;
    use ffs_core::{AtomTemplate, Tier};

    fn repo_root() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.pop();
        p
    }

    fn owner_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }
    fn owner_pk() -> PublicKey {
        PublicKey::from_verifying(&owner_key().verifying_key())
    }

    fn grant_full_caps(store: &dyn AtomStore) {
        let cap = build_capability_atom(
            &owner_key(),
            owner_pk(),
            vec![Action::Read, Action::Write, Action::Supersede],
            CapabilityScope::default(),
            Iso8601::new("2026-01-01T00:00:00Z").unwrap(),
            None,
            Iso8601::new("2026-01-01T00:00:01Z").unwrap(),
            None,
        )
        .unwrap();
        store.insert(&cap).unwrap();
    }

    async fn build_mat(data_dir: PathBuf) -> (Arc<MemAtomStore>, Arc<WorkingSetMaterializer>) {
        let store = Arc::new(MemAtomStore::new());
        grant_full_caps(&*store);
        let registry = Arc::new(SpecRegistry::new());
        let predicates_dir = repo_root().join("starter").join("predicates");
        let templates_dir = repo_root().join("starter").join("templates");
        registry.load_dir(&predicates_dir).unwrap();
        let renderer = Arc::new(
            ProjectionRenderer::new(
                store.clone() as Arc<dyn AtomStore>,
                registry,
                &templates_dir,
            )
            .unwrap(),
        );
        let ws = Arc::new(InMemoryWorkingSet::new());
        let suppression = Arc::new(SuppressionRegistry::new());
        let mat = Arc::new(WorkingSetMaterializer::new(
            renderer,
            ws,
            suppression,
            data_dir,
            owner_pk(),
        ));
        (store, mat)
    }

    fn sign_contact(name: &str, work_email: &str) -> ffs_core::AtomEnvelope {
        AtomTemplate {
            v: 1,
            entity: EntityId::new(name),
            predicate: PredicateName::new("contact.person"),
            claim: serde_json::json!({
                "display_name": name.replace('_', " "),
                "work_email": work_email,
            }),
            valid_from: Iso8601::new("2026-05-31T00:00:00Z").unwrap(),
            valid_to: None,
            tx_time: Iso8601::new("2026-05-31T00:00:00Z").unwrap(),
            classification: Tier::new("existence"),
            supersedes: None,
            provenance: vec![],
        }
        .sign(&owner_key())
        .unwrap()
    }

    #[tokio::test]
    async fn materializes_a_contact_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, mat) = build_mat(tmp.path().to_path_buf()).await;
        store
            .insert(&sign_contact("Sara_Chen", "sara@example.com"))
            .unwrap();

        let result = mat
            .materialize_entity(PathFamily::Contacts, &EntityId::new("Sara_Chen"))
            .await
            .expect("materialize");
        let m = result.expect("materialization should produce a write");

        assert_eq!(m.path, tmp.path().join("contacts/by-name/S/Sara_Chen.md"));
        assert!(m.path.exists(), "file should be written");
        let on_disk = std::fs::read_to_string(&m.path).unwrap();
        assert!(on_disk.contains("display_name: Sara Chen"));
        assert!(on_disk.contains("work_email: sara@example.com"));
    }

    #[tokio::test]
    async fn re_materializing_is_a_noop_without_mtime_churn() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, mat) = build_mat(tmp.path().to_path_buf()).await;
        store
            .insert(&sign_contact("Alex_Kim", "alex@example.com"))
            .unwrap();

        let first = mat
            .materialize_entity(PathFamily::Contacts, &EntityId::new("Alex_Kim"))
            .await
            .unwrap()
            .expect("first materialization writes");
        let mtime_first = std::fs::metadata(&first.path).unwrap().modified().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let second = mat
            .materialize_entity(PathFamily::Contacts, &EntityId::new("Alex_Kim"))
            .await
            .unwrap();
        assert!(second.is_none(), "second materialization should be a no-op");
        let mtime_second = std::fs::metadata(&first.path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_first, mtime_second,
            "mtime should not change on no-op"
        );
    }

    #[tokio::test]
    async fn missing_entity_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (_store, mat) = build_mat(tmp.path().to_path_buf()).await;
        // No atoms in the store, so this should be a benign no-op.
        let r = mat
            .materialize_entity(PathFamily::Contacts, &EntityId::new("Ghost"))
            .await
            .unwrap();
        assert!(r.is_none());
        assert!(
            !tmp.path().join("contacts/by-name/G/Ghost.md").exists(),
            "no file should be created"
        );
    }

    #[tokio::test]
    async fn write_records_suppression_for_fastpath_anti_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(MemAtomStore::new());
        grant_full_caps(&*store);
        let registry = Arc::new(SpecRegistry::new());
        registry
            .load_dir(&repo_root().join("starter").join("predicates"))
            .unwrap();
        let renderer = Arc::new(
            ProjectionRenderer::new(
                store.clone() as Arc<dyn AtomStore>,
                registry,
                &repo_root().join("starter").join("templates"),
            )
            .unwrap(),
        );
        let ws = Arc::new(InMemoryWorkingSet::new());
        let suppression = Arc::new(SuppressionRegistry::new());
        let mat = Arc::new(WorkingSetMaterializer::new(
            renderer,
            ws,
            suppression.clone(),
            tmp.path().to_path_buf(),
            owner_pk(),
        ));
        store
            .insert(&sign_contact("Wes_F", "wes@example.com"))
            .unwrap();

        let result = mat
            .materialize_entity(PathFamily::Contacts, &EntityId::new("Wes_F"))
            .await
            .unwrap()
            .expect("write");

        // After the materializer wrote, the suppression registry
        // should have a pending entry for that path. A fast-path
        // event loop receiving the file's bytes would call
        // `check(path, bytes)` and consume the entry — proving the
        // anti-loop guard.
        assert!(
            suppression.has_pending(&result.path),
            "suppression entry should be pending after write"
        );
        // Simulate the watcher: reading the bytes and checking.
        let on_disk = std::fs::read(&result.path).unwrap();
        assert!(
            suppression.check(&result.path, &on_disk),
            "watcher's check with the same bytes should hit"
        );
    }

    #[tokio::test]
    async fn handle_commit_skips_predicates_outside_the_path_library() {
        let tmp = tempfile::tempdir().unwrap();
        let (_store, mat) = build_mat(tmp.path().to_path_buf()).await;
        // capability.grant has no path-library mapping; should be a benign Ok(None).
        let r = mat
            .handle_commit(
                &EntityId::new("z6Mkr..."),
                &PredicateName::new("capability.grant"),
            )
            .await
            .unwrap();
        assert!(r.is_none());
    }
}
