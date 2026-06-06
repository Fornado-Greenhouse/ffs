// JSON-RPC 2.0 client over Unix domain socket (Linux/macOS) or
// Windows named pipe.
//
// Wire framing matches `crates/ffs-daemon/src/transport/*`: one JSON
// envelope per line, terminated by `\n`. Requests and notifications
// share the connection; responses are correlated to requests by their
// `id` field, and notifications (no `id`) fan out via the event
// emitter.
//
// Reconnection: when the underlying socket errors or closes, the
// client transitions to a `disconnected` state and schedules a
// reconnect with exponential backoff (1s → 30s per `backoff.ts`).
// Outstanding requests are rejected with a `disconnected` error so
// callers can decide whether to retry.
//
// CLI fallback: if direct UDS / named-pipe connection consistently
// fails (per the TechSpec Known Risks note about Windows named-pipe
// quirks), the client transparently falls back to spawning the `ffs`
// CLI as a subprocess per request. The fallback path doesn't carry
// notifications — the event emitter goes silent until the direct
// connection recovers.

import type { NetModule, NetSocket } from "./platform.js";
import { nextBackoffMs, RECONNECT_INITIAL_MS } from "./backoff.js";
import { FfsEventEmitter, NotificationFrame } from "./events.js";

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "fallback";

export interface DaemonClientOptions {
  /** Filesystem path (UDS) or `\\.\pipe\name` (Windows). */
  socketPath: string;
  /** Optional alternate path to the `ffs` CLI binary for the fallback. */
  cliPath?: string;
  /** Inject a `net` module for tests. Production omits this. */
  netModule?: NetModule;
  /** Spawn function injected for tests. Production omits this. */
  spawnSubprocess?: SpawnFn;
  /** Override the wall-clock timer (vitest fake timers). */
  setTimeoutFn?: typeof setTimeout;
  /** Override clearTimeout (vitest fake timers). */
  clearTimeoutFn?: typeof clearTimeout;
}

export interface SpawnFn {
  (
    bin: string,
    args: string[],
    input: string,
  ): Promise<{ code: number; stdout: string; stderr: string }>;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
}

export class DaemonClient {
  readonly events = new FfsEventEmitter();
  private socket: NetSocket | null = null;
  private state: ConnectionState = "disconnected";
  private pending: Map<number | string, PendingRequest> = new Map();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private currentBackoffMs: number = RECONNECT_INITIAL_MS;
  private inboundBuffer = "";
  private nextRequestId = 1;
  private stateListeners: Array<(s: ConnectionState) => void> = [];
  private consecutiveDirectFailures = 0;
  /**
   * Sticky fallback flag. Once 3 consecutive direct failures have
   * happened (and the spawn function is configured), this flips on
   * and stays on through subsequent reconnect attempts. Cleared
   * when a real connection succeeds — `usingFallback = false` is
   * the signal that direct IPC is healthy again.
   */
  private usingFallback = false;
  /**
   * Whether the client should re-arm the reconnect timer after a
   * disconnect. Set to false by `close()` so `stop` is sticky.
   */
  private wantConnected = false;

  constructor(private opts: DaemonClientOptions) {}

  get connectionState(): ConnectionState {
    return this.state;
  }

  /**
   * Start the connect loop. Idempotent — calling again while
   * connected is a no-op.
   */
  start(): void {
    if (this.state === "connected" || this.state === "connecting") {
      return;
    }
    this.wantConnected = true;
    this.attemptConnect();
  }

  /** Close the connection permanently. Cancels pending reconnects. */
  close(): void {
    this.wantConnected = false;
    if (this.reconnectTimer != null) {
      (this.opts.clearTimeoutFn ?? clearTimeout)(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.socket?.destroy();
    this.socket = null;
    this.failPendingRequests(new Error("client closed"));
    this.setState("disconnected");
  }

  /**
   * Subscribe to state-change notifications. Returns an
   * unsubscribe function — call it from the subscriber's teardown
   * hook (e.g., a view's `onClose`) so listeners don't accumulate
   * across view open/close cycles.
   */
  onStateChange(fn: (s: ConnectionState) => void): () => void {
    this.stateListeners.push(fn);
    return () => {
      const idx = this.stateListeners.indexOf(fn);
      if (idx >= 0) this.stateListeners.splice(idx, 1);
    };
  }

  /**
   * Send a JSON-RPC request. Resolves with `result`; rejects with the
   * `error` payload on protocol errors, or a transport-level Error
   * on disconnect.
   */
  async call(method: string, params: unknown = {}): Promise<unknown> {
    // Fallback is sticky across reconnect attempts: once flipped on
    // it stays on until a direct connection succeeds, so calls keep
    // working even while the background heal cycle is in progress.
    if (this.usingFallback && this.opts.spawnSubprocess) {
      return this.callViaSubprocess(method, params);
    }
    if (this.state !== "connected") {
      throw new Error(`daemon is ${this.state}`);
    }
    const id = this.nextRequestId++;
    const frame = JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n";
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      const socket = this.socket;
      if (!socket) {
        reject(new Error("socket unavailable"));
        return;
      }
      socket.write(frame, (err) => {
        if (err) {
          this.pending.delete(id);
          reject(err);
        }
      });
    });
  }

  // ---- internal ----

  private attemptConnect(): void {
    if (!this.wantConnected) return;
    // Only emit the connecting state when we're not already in
    // fallback — once fallback is sticky, the indicator should
    // keep showing "fallback" through the heal cycle. Production
    // overwrites this when a real connection succeeds.
    if (!this.usingFallback) {
      this.setState("connecting");
    }
    const net = this.opts.netModule ?? loadNetModule();
    const socket = net.createConnection({ path: this.opts.socketPath });
    this.socket = socket;
    this.inboundBuffer = "";

    socket.on("connect", () => {
      this.consecutiveDirectFailures = 0;
      this.currentBackoffMs = RECONNECT_INITIAL_MS;
      this.usingFallback = false;
      this.setState("connected");
    });
    socket.on("data", (chunk: Buffer | string) => {
      this.inboundBuffer +=
        typeof chunk === "string" ? chunk : chunk.toString("utf8");
      let idx;
      while ((idx = this.inboundBuffer.indexOf("\n")) >= 0) {
        const line = this.inboundBuffer.slice(0, idx);
        this.inboundBuffer = this.inboundBuffer.slice(idx + 1);
        if (line.trim().length === 0) continue;
        this.handleFrame(line);
      }
    });
    socket.on("error", () => {
      this.consecutiveDirectFailures += 1;
      this.failPendingRequests(new Error("socket error"));
      socket.destroy();
    });
    socket.on("close", () => {
      this.failPendingRequests(new Error("socket closed"));
      this.socket = null;
      if (!this.wantConnected) {
        this.setState("disconnected");
        return;
      }
      // Three consecutive direct failures → flip the sticky
      // fallback flag. The plugin keeps a low-rate reconnect
      // attempt going so direct IPC can take over again when the
      // daemon comes back; the next successful `connect` clears the
      // flag (`usingFallback = false` in the `connect` handler).
      if (this.consecutiveDirectFailures >= 3 && this.opts.spawnSubprocess) {
        this.usingFallback = true;
        this.setState("fallback");
      } else {
        this.setState("disconnected");
      }
      this.scheduleReconnect();
    });
  }

  private scheduleReconnect(): void {
    if (!this.wantConnected) return;
    const delay = this.currentBackoffMs;
    this.currentBackoffMs = nextBackoffMs(this.currentBackoffMs);
    this.reconnectTimer = (this.opts.setTimeoutFn ?? setTimeout)(() => {
      this.reconnectTimer = null;
      this.attemptConnect();
    }, delay);
  }

  private handleFrame(line: string): void {
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(line);
    } catch (err) {
      console.warn("[ffs] discarded malformed frame:", line, err);
      return;
    }
    // Notification: no `id`, has `method`.
    if (parsed.id === undefined && typeof parsed.method === "string") {
      this.events.emit(parsed as unknown as NotificationFrame);
      return;
    }
    // Response: has `id`.
    if (parsed.id !== undefined) {
      const id = parsed.id as number | string;
      const pending = this.pending.get(id);
      if (!pending) return;
      this.pending.delete(id);
      if ("error" in parsed && parsed.error) {
        const err = parsed.error as { code?: number; message?: string };
        pending.reject(
          Object.assign(new Error(err.message ?? "rpc error"), {
            code: err.code,
            data: (parsed.error as { data?: unknown }).data,
          }),
        );
      } else {
        pending.resolve((parsed as { result?: unknown }).result);
      }
    }
  }

  private failPendingRequests(err: Error): void {
    for (const pending of this.pending.values()) {
      pending.reject(err);
    }
    this.pending.clear();
  }

  private async callViaSubprocess(
    method: string,
    params: unknown,
  ): Promise<unknown> {
    const spawn = this.opts.spawnSubprocess;
    if (!spawn) {
      throw new Error("CLI fallback not configured");
    }
    const cli = this.opts.cliPath ?? "ffs";
    const frame =
      JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) + "\n";
    const result = await spawn(cli, ["rpc"], frame);
    if (result.code !== 0) {
      throw new Error(`CLI fallback failed: ${result.stderr || result.stdout}`);
    }
    const line = result.stdout.split("\n").find((l) => l.trim().length > 0);
    if (!line) {
      throw new Error("CLI fallback returned no output");
    }
    const parsed = JSON.parse(line) as Record<string, unknown>;
    if ("error" in parsed && parsed.error) {
      const err = parsed.error as { code?: number; message?: string };
      throw Object.assign(new Error(err.message ?? "rpc error"), {
        code: err.code,
      });
    }
    return (parsed as { result?: unknown }).result;
  }

  private setState(s: ConnectionState): void {
    if (s === this.state) return;
    this.state = s;
    for (const fn of this.stateListeners) {
      try {
        fn(s);
      } catch (err) {
        console.error("[ffs] state listener threw:", err);
      }
    }
  }
}

/**
 * Lazy-load Node's `net` module so the file is importable in
 * environments where it isn't available (e.g., type-only contexts).
 */
function loadNetModule(): NetModule {
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const net = require("net") as NetModule;
  return net;
}
