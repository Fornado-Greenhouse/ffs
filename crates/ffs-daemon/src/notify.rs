//! Server-to-client notification publisher.
//!
//! Uses `tokio::sync::broadcast` for fan-out: one sender, N subscribers
//! (one per active client connection). Each event is serialized once and
//! delivered to all subscribers. Subscribers that fall behind beyond the
//! channel capacity get a `Lagged` indication from broadcast, which the
//! transport layer translates into an `event.resync` hint per ADR-019.

use serde::Serialize;
use tokio::sync::broadcast;

use ffs_core::{EntityId, Multihash, PredicateName};

/// Channel capacity. Set above the 1000-event backpressure threshold the
/// task spec calls out so the resync hint only fires under genuine stall.
pub const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Event {
    #[serde(rename = "event.atom.committed")]
    AtomCommitted {
        hash: Multihash,
        entity: EntityId,
        predicate: PredicateName,
    },
    #[serde(rename = "event.projection.invalidated")]
    ProjectionInvalidated { path: String },
    #[serde(rename = "event.fastpath.applied")]
    FastPathApplied {
        projection_path: String,
        atom_hash: Multihash,
    },
    #[serde(rename = "event.federation.peer.changed")]
    FederationPeerChanged { peer: String },
}

/// On-the-wire frame: JSON-RPC notification envelope with the event flattened in.
#[derive(Debug, Clone, Serialize)]
struct NotificationFrame<'a> {
    jsonrpc: &'a str,
    #[serde(flatten)]
    event: &'a Event,
}

/// Fan-out publisher. Cheap to clone (Arc-wrapped at the dispatcher level).
pub struct EventPublisher {
    sender: broadcast::Sender<String>,
}

impl Default for EventPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl EventPublisher {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self { sender }
    }

    /// Publish an event to every active subscriber. Events are serialized
    /// once here so subscribers don't repeat the work. Returns the number
    /// of subscribers that received the event (zero is fine — means no
    /// active clients).
    pub fn publish(&self, event: Event) -> usize {
        let frame = NotificationFrame {
            jsonrpc: "2.0",
            event: &event,
        };
        let Ok(line) = serde_json::to_string(&frame) else {
            return 0;
        };
        self.sender.send(line).unwrap_or(0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }

    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}
