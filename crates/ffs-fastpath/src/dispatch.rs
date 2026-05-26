//! Apply a `Classification`: either author a supersession atom (fast path)
//! or write a correction notebook entry to the ingest folder (slow path).
//! Fires `event.fastpath.applied` or `event.fastpath.routed_to_ingest` via
//! the daemon's event publisher.

use std::path::Path;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use serde::Serialize;
use tracing::{debug, warn};

use ffs_core::store::AtomStore;
use ffs_core::{AtomEnvelope, AtomTemplate, EntityId, Iso8601, Multihash, PredicateName, Tier};
use ffs_daemon::notify::{Event, EventPublisher};

use crate::classifier::{Classification, SlowPathReason};
use crate::suppress::SuppressionRegistry;

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store: {0}")]
    Store(#[from] ffs_core::store::StoreError),
    #[error("sign: {0}")]
    Sign(String),
    #[error("classification was malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct AppliedReceipt {
    pub atom_hash: Multihash,
    pub projection_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutedReceipt {
    pub submission_path: String,
    pub projection_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub enum Receipt {
    Applied(AppliedReceipt),
    Routed(RoutedReceipt),
}

/// Apply a fast-path classification by authoring a supersession atom and
/// inserting it into the store. Returns the new atom's hash.
#[allow(clippy::too_many_arguments)]
pub fn apply_fast_path(
    store: &dyn AtomStore,
    notifier: &EventPublisher,
    signing_key: &SigningKey,
    head: &AtomEnvelope,
    modified_claim: serde_json::Value,
    projection_path: &str,
    suppression: &SuppressionRegistry,
    new_content_for_suppression: &[u8],
    target_file: &Path,
) -> Result<Multihash, DispatchError> {
    let now = current_iso8601();
    let head_hash = head
        .content_hash()
        .map_err(|e| DispatchError::Sign(e.to_string()))?;
    let tmpl = AtomTemplate {
        v: 1,
        entity: head.entity.clone(),
        predicate: head.predicate.clone(),
        claim: modified_claim,
        valid_from: head.valid_from.clone(),
        valid_to: head.valid_to.clone(),
        tx_time: now,
        classification: head.classification.clone(),
        supersedes: Some(head_hash),
        provenance: vec![],
    };
    let env = tmpl
        .sign(signing_key)
        .map_err(|e| DispatchError::Sign(e.to_string()))?;
    let new_hash = store.insert(&env)?;

    // Record the expected file content so the FS watcher ignores the
    // daemon's own re-render write-back.
    suppression.record(target_file, new_content_for_suppression);

    notifier.publish(Event::FastPathApplied {
        projection_path: projection_path.to_string(),
        atom_hash: new_hash.clone(),
    });
    debug!(
        projection_path,
        new_hash = %new_hash.to_multibase(),
        "fast-path applied"
    );
    Ok(new_hash)
}

/// Route a slow-path edit: write the content under `ingest_dir` with a
/// header that points back to the source projection.
pub fn route_to_ingest(
    notifier: &EventPublisher,
    ingest_dir: &Path,
    projection_path: &str,
    new_content: &[u8],
    reason: &SlowPathReason,
) -> Result<RoutedReceipt, DispatchError> {
    std::fs::create_dir_all(ingest_dir)?;
    let stamp = current_iso8601().as_str().replace(':', "-");
    // Use a hash of the projection path to avoid collisions and keep
    // filenames bounded.
    let path_hash = Multihash::blake3_of(projection_path.as_bytes()).to_multibase();
    let filename = format!(
        "correction-{stamp}-{}.md",
        path_hash.chars().take(8).collect::<String>()
    );
    let dest = ingest_dir.join(&filename);
    let header = format!(
        "---\ncorrection_of: {projection_path}\nsubmitted_at: {stamp}\nreason: {reason:?}\n---\n\n",
    );
    let mut body = Vec::with_capacity(header.len() + new_content.len());
    body.extend_from_slice(header.as_bytes());
    body.extend_from_slice(new_content);
    std::fs::write(&dest, &body)?;

    let receipt = RoutedReceipt {
        submission_path: dest.to_string_lossy().into_owned(),
        projection_path: projection_path.to_string(),
        reason: format!("{reason:?}"),
    };

    // Reuse the projection.invalidated event to signal the slow path
    // landed something for the user to review in the daily summary.
    // (A dedicated `event.fastpath.routed_to_ingest` variant could be
    // added if downstream UIs need to disambiguate; for MVP this is
    // sufficient.)
    notifier.publish(Event::ProjectionInvalidated {
        path: projection_path.to_string(),
    });
    warn!(
        projection_path,
        reason = ?reason,
        dest = ?dest,
        "fast-path routed to ingest"
    );
    Ok(receipt)
}

/// Convenience: drive a `Classification` to the appropriate dispatch.
#[allow(clippy::too_many_arguments)]
pub fn dispatch(
    classification: Classification,
    store: &Arc<dyn AtomStore>,
    notifier: &EventPublisher,
    signing_key: &SigningKey,
    head: &AtomEnvelope,
    projection_path: &str,
    target_file: &Path,
    new_content: &[u8],
    ingest_dir: &Path,
    suppression: &SuppressionRegistry,
) -> Result<Receipt, DispatchError> {
    match classification {
        Classification::Applied { modified_claim, .. } => {
            let new_hash = apply_fast_path(
                &**store,
                notifier,
                signing_key,
                head,
                modified_claim,
                projection_path,
                suppression,
                new_content,
                target_file,
            )?;
            Ok(Receipt::Applied(AppliedReceipt {
                atom_hash: new_hash,
                projection_path: projection_path.to_string(),
            }))
        }
        Classification::RoutedToIngest { reason } => {
            let r = route_to_ingest(notifier, ingest_dir, projection_path, new_content, &reason)?;
            Ok(Receipt::Routed(r))
        }
    }
}

fn current_iso8601() -> Iso8601 {
    use time::format_description::well_known::Iso8601 as Fmt;
    let now = time::OffsetDateTime::now_utc();
    let s = now
        .format(&Fmt::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());
    Iso8601::new(s).expect("formatted ISO8601 must parse")
}

// Marker so unused-warning gates don't prune nominally-unused but conceptually
// load-bearing types from re-exports as the crate grows.
#[allow(dead_code)]
fn _markers(_e: EntityId, _p: PredicateName, _t: Tier) {}
