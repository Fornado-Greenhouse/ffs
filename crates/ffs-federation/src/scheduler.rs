//! Pull scheduler: drives `pull_atoms` against each registered peer
//! on a heartbeat plus on demand. Per ADR-020 the heartbeat default
//! is 60s; this module exposes the timing knob plus an exponential-
//! backoff state machine so a failing peer doesn't starve the
//! scheduler.
//!
//! For MVP the heartbeat loop is intentionally minimal: it walks the
//! peer store, calls `tick_once_for_peer` for each peer, and uses
//! `tokio::time::sleep` between rounds. A future iteration can wire
//! `tokio::time::Interval` with jitter; the public `tick_once_for_peer`
//! function stays the same so on-demand triggers via
//! `federation.pull` reuse the exact code path the heartbeat runs.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use ffs_core::store::AtomStore;
use ffs_core::{AtomEnvelope, Iso8601, Multihash};

use crate::client::{FederationClient, FederationClientError};
use crate::mount::PeerMountStore;
use ffs_core::federation_peers::FederationPeerStore;

/// Default heartbeat cadence per ADR-020.
pub const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(60);

/// Exponential-backoff bounds from the requirements file.
pub const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// What `tick_once_for_peer` returns. `Pulled` carries the per-pull
/// telemetry the dispatcher / scheduler use to update watermarks
/// and surface results to the caller of `federation.pull`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PullOutcome {
    pub peer_id: String,
    pub atoms_pulled: usize,
    pub atoms_rejected: usize,
    pub revoked: bool,
    pub new_watermark: Option<Iso8601>,
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("peer not found: {0}")]
    PeerNotFound(String),
    #[error("peer has no their_capability — handshake incomplete")]
    NoCapabilityRecord,
    #[error("client: {0}")]
    Client(#[from] FederationClientError),
    #[error("store: {0}")]
    Store(String),
    #[error("peer store: {0}")]
    PeerStore(#[from] ffs_core::federation_peers::FederationPeerError),
}

/// One pull against one peer: pull atoms after the stored watermark
/// using the peer's pinned capability, verify each envelope, insert
/// verified atoms into the local store, attribute them in the
/// `PeerMountStore`, and advance the watermark to the newest
/// successfully-inserted atom's `tx_time`.
///
/// Returns `revoked = true` when the peer's previous pull yielded at
/// least one atom and this one yielded zero — the canonical
/// revocation signal per ADR-020. The caller (scheduler) then drops
/// the peer's mount via `mount.unmount(peer_id)`.
#[allow(clippy::too_many_arguments)]
pub async fn tick_once_for_peer(
    peer_id: &str,
    peers: &Arc<dyn FederationPeerStore>,
    client: &Arc<dyn FederationClient>,
    our_cert_fingerprint: &Multihash,
    local_store: &Arc<dyn AtomStore>,
    mount: &Arc<dyn PeerMountStore>,
    watermark_capability_key: &str,
) -> Result<PullOutcome, SchedulerError> {
    let peer = peers
        .get(peer_id)
        .await
        .ok_or_else(|| SchedulerError::PeerNotFound(peer_id.to_string()))?;
    let cap_hash = peer
        .their_capability
        .clone()
        .ok_or(SchedulerError::NoCapabilityRecord)?;

    let prior_watermark = peer.watermarks.get(watermark_capability_key).cloned();
    let previously_mounted = mount.count(peer_id).await > 0;

    let pulled: Vec<AtomEnvelope> = client
        .pull_atoms(
            &peer.endpoint,
            our_cert_fingerprint,
            prior_watermark.as_ref(),
            &cap_hash,
        )
        .await?;

    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut max_tx: Option<Iso8601> = prior_watermark.clone();
    for env in pulled.into_iter() {
        // Insert delegates verification (signature + content hash) to
        // the store. On failure, drop the atom and keep going; the
        // watermark stays where it was for that one.
        match local_store.insert(&env) {
            Ok(hash) => {
                mount.record(peer_id, hash).await;
                accepted += 1;
                match &max_tx {
                    Some(cur) if cur.as_str() >= env.tx_time.as_str() => {}
                    _ => {
                        max_tx = Some(env.tx_time.clone());
                    }
                }
            }
            Err(e) => {
                warn!(peer_id = %peer_id, error = %e, "federation_pull_rejected_atom");
                rejected += 1;
            }
        }
    }

    let revoked = previously_mounted && accepted == 0 && rejected == 0;
    if revoked {
        info!(peer_id = %peer_id, "federation_revocation_detected_unmounting");
        mount.unmount(peer_id).await;
    }

    // Persist the new watermark if it advanced.
    let new_watermark = if max_tx != prior_watermark {
        let new = max_tx.clone();
        let mut updated_peer = peer.clone();
        if let Some(ts) = &new {
            updated_peer
                .watermarks
                .insert(watermark_capability_key.to_string(), ts.clone());
        }
        updated_peer.last_seen_at = Some(current_iso8601());
        peers.upsert(updated_peer).await?;
        new
    } else {
        prior_watermark
    };

    debug!(
        peer_id = %peer_id,
        accepted,
        rejected,
        revoked,
        ?new_watermark,
        "federation_tick_complete"
    );
    Ok(PullOutcome {
        peer_id: peer_id.to_string(),
        atoms_pulled: accepted,
        atoms_rejected: rejected,
        revoked,
        new_watermark,
    })
}

/// Compute the next backoff given the current value. Doubles, caps,
/// resets to `BACKOFF_INITIAL` on success.
pub fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(BACKOFF_CAP)
}

/// Long-running scheduler: walks every peer per heartbeat and pulls.
/// Returns a handle that aborts the loop when dropped. For MVP this
/// is intentionally simple — production deployments can add per-peer
/// independent timers; the on-demand `federation.pull` RPC bypasses
/// this loop entirely by calling `tick_once_for_peer` directly.
pub struct PullScheduler {
    _task: tokio::task::JoinHandle<()>,
    cancel: Arc<Mutex<bool>>,
}

#[allow(clippy::too_many_arguments)]
impl PullScheduler {
    pub fn start(
        peers: Arc<dyn FederationPeerStore>,
        client: Arc<dyn FederationClient>,
        our_cert_fingerprint: Multihash,
        local_store: Arc<dyn AtomStore>,
        mount: Arc<dyn PeerMountStore>,
        watermark_capability_key: String,
        heartbeat: Duration,
    ) -> Self {
        let cancel = Arc::new(Mutex::new(false));
        let cancel_loop = cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                if *cancel_loop.lock().await {
                    return;
                }
                let snapshot = peers.list().await;
                for peer in snapshot {
                    if peer.their_capability.is_none() {
                        continue;
                    }
                    if let Err(e) = tick_once_for_peer(
                        &peer.peer_id,
                        &peers,
                        &client,
                        &our_cert_fingerprint,
                        &local_store,
                        &mount,
                        &watermark_capability_key,
                    )
                    .await
                    {
                        warn!(peer_id = %peer.peer_id, error = %e, "federation_tick_failed");
                    }
                }
                tokio::time::sleep(heartbeat).await;
            }
        });
        Self {
            _task: task,
            cancel,
        }
    }

    pub async fn stop(&self) {
        *self.cancel.lock().await = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_cap() {
        let mut d = BACKOFF_INITIAL;
        let seq: Vec<u64> = (0..10)
            .map(|_| {
                let v = d.as_millis() as u64;
                d = next_backoff(d);
                v
            })
            .collect();
        assert_eq!(
            seq,
            vec![
                1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000, 60000, 60000
            ]
        );
    }
}
