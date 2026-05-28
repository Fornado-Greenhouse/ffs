// Internal event emitter for FFS substrate notifications.
//
// The daemon publishes four notification kinds over the local JSON-RPC
// channel (see `crates/ffs-daemon/src/notify.rs` for the canonical
// definitions):
//
// - `event.atom.committed` — a new atom landed in the store.
// - `event.projection.invalidated` — a projection re-render is needed.
// - `event.fastpath.applied` — fastpath authored a supersession.
// - `event.federation.peer.changed` — bridge handshake / rotation /
//   capability change with a peer.
//
// The plugin's subsystems (folder enumeration, daily-summary panel,
// search) subscribe to the kinds they care about via `on(kind, fn)`.
// `*` is the wildcard channel — useful for logging and the
// reconnection state indicator.

export type EventKind =
  | "event.atom.committed"
  | "event.projection.invalidated"
  | "event.fastpath.applied"
  | "event.federation.peer.changed";

/**
 * Raw notification frame as it arrives on the wire — a JSON-RPC 2.0
 * notification with a daemon-defined `method` and `params`.
 */
export interface NotificationFrame {
  jsonrpc: "2.0";
  method: EventKind | string;
  params: Record<string, unknown>;
}

export type Listener = (frame: NotificationFrame) => void;

/**
 * Tiny typed event emitter. Intentionally not derived from Node's
 * built-in `EventEmitter` — the plugin runs inside Obsidian's
 * Electron renderer, and we want to limit our Node-runtime surface
 * to `net` / `child_process` (the two modules with platform-specific
 * behavior worth testing). The `*` channel receives every frame so
 * the connection-state UI can show a heartbeat.
 */
export class FfsEventEmitter {
  private listeners: Map<string, Set<Listener>> = new Map();

  on(kind: EventKind | "*", fn: Listener): void {
    let set = this.listeners.get(kind);
    if (!set) {
      set = new Set();
      this.listeners.set(kind, set);
    }
    set.add(fn);
  }

  off(kind: EventKind | "*", fn: Listener): void {
    this.listeners.get(kind)?.delete(fn);
  }

  /**
   * Dispatch a frame to all subscribers of its kind plus the `*`
   * wildcard. Listener failures are logged and swallowed so a
   * misbehaving subscriber can't kill the dispatch chain.
   */
  emit(frame: NotificationFrame): void {
    const direct = this.listeners.get(frame.method);
    if (direct) {
      for (const fn of direct) {
        this.safeInvoke(fn, frame);
      }
    }
    const star = this.listeners.get("*");
    if (star) {
      for (const fn of star) {
        this.safeInvoke(fn, frame);
      }
    }
  }

  /** Count of listeners for a given kind. Useful for tests. */
  listenerCount(kind: EventKind | "*"): number {
    return this.listeners.get(kind)?.size ?? 0;
  }

  removeAll(): void {
    this.listeners.clear();
  }

  private safeInvoke(fn: Listener, frame: NotificationFrame): void {
    try {
      fn(frame);
    } catch (err) {
      console.error("[ffs] listener threw on frame:", frame, err);
    }
  }
}
