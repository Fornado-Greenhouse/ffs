import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DaemonClient, ConnectionState, SpawnFn } from "../src/client.js";
import type { NetModule, NetSocket } from "../src/platform.js";

/**
 * Fake socket the test drives manually. Captures writes for
 * inspection and exposes `fire*` helpers to simulate the server
 * pushing data, errors, or closing the connection.
 */
class FakeSocket implements NetSocket {
  writes: string[] = [];
  destroyed = false;
  private handlers: Record<string, Array<(arg?: any) => void>> = {};

  on(event: "connect", fn: () => void): void;
  on(event: "data", fn: (chunk: Buffer | string) => void): void;
  on(event: "error", fn: (err: Error) => void): void;
  on(event: "close", fn: (hadError: boolean) => void): void;
  on(event: string, fn: (arg?: any) => void): void {
    (this.handlers[event] ||= []).push(fn);
  }

  write(data: string, cb?: (err?: Error | null) => void): boolean {
    this.writes.push(data);
    cb?.(null);
    return true;
  }

  destroy(): void {
    this.destroyed = true;
  }

  fireConnect(): void {
    this.handlers["connect"]?.forEach((fn) => fn());
  }

  fireData(line: string): void {
    this.handlers["data"]?.forEach((fn) => fn(Buffer.from(line, "utf8")));
  }

  fireError(err: Error): void {
    this.handlers["error"]?.forEach((fn) => fn(err));
  }

  fireClose(hadError = false): void {
    this.handlers["close"]?.forEach((fn) => fn(hadError));
  }
}

function buildNetModule(sockets: FakeSocket[]): NetModule {
  return {
    createConnection: () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    },
  };
}

describe("DaemonClient", () => {
  let sockets: FakeSocket[];
  let net: NetModule;

  beforeEach(() => {
    sockets = [];
    net = buildNetModule(sockets);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("serializes a request to a single line of JSON terminated by \\n", async () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    client.start();
    sockets[0].fireConnect();

    void client.call("atom.list", { entity: "Sara" });
    // The client writes synchronously after the connect event fires.
    expect(sockets[0].writes).toHaveLength(1);
    const line = sockets[0].writes[0];
    expect(line.endsWith("\n")).toBe(true);
    const parsed = JSON.parse(line.trim());
    expect(parsed).toMatchObject({
      jsonrpc: "2.0",
      method: "atom.list",
      params: { entity: "Sara" },
    });
    expect(typeof parsed.id).toBe("number");
  });

  it("demultiplexes a response and a notification arriving on the same line stream", async () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    client.start();
    sockets[0].fireConnect();

    const notified = vi.fn();
    client.events.on("event.atom.committed", notified);

    const pending = client.call("atom.get", { hash: "abc" });
    // Server pushes a notification THEN the response — both on the
    // same socket. The demux should route each correctly.
    sockets[0].fireData(
      JSON.stringify({
        jsonrpc: "2.0",
        method: "event.atom.committed",
        params: { hash: "abc" },
      }) + "\n",
    );
    sockets[0].fireData(
      JSON.stringify({ jsonrpc: "2.0", id: 1, result: { ok: true } }) + "\n",
    );

    const result = await pending;
    expect(result).toEqual({ ok: true });
    expect(notified).toHaveBeenCalledTimes(1);
  });

  it("schedules reconnects with exponential backoff after a disconnect", async () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    client.start();
    sockets[0].fireConnect();

    // Disconnect — first reconnect should fire after 1s.
    sockets[0].fireError(new Error("eof"));
    sockets[0].fireClose();

    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(999);
    expect(sockets).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(2);

    // Second disconnect — 2s.
    sockets[1].fireError(new Error("eof"));
    sockets[1].fireClose();
    vi.advanceTimersByTime(1999);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(3);

    // Third disconnect — 4s.
    sockets[2].fireError(new Error("eof"));
    sockets[2].fireClose();
    vi.advanceTimersByTime(4000);
    expect(sockets).toHaveLength(4);
  });

  it("transitions to fallback state after 3 consecutive direct failures", async () => {
    const spawn: SpawnFn = vi.fn(async () => ({
      code: 0,
      stdout: JSON.stringify({ jsonrpc: "2.0", id: 1, result: { fallback: true } }) + "\n",
      stderr: "",
    }));
    const client = new DaemonClient({
      socketPath: "/tmp/x.sock",
      netModule: net,
      spawnSubprocess: spawn,
    });

    const states: ConnectionState[] = [];
    client.onStateChange((s) => states.push(s));
    client.start();

    // Three consecutive error+close cycles.
    for (let i = 0; i < 3; i++) {
      sockets[i].fireError(new Error("eof"));
      sockets[i].fireClose();
      vi.advanceTimersByTime(30_000); // advance past whatever backoff is now
    }
    expect(states).toContain("fallback");
    // A call in fallback dispatches via spawn instead of the socket.
    await expect(client.call("predicate.inspect", { name: "note" })).resolves.toEqual({
      fallback: true,
    });
    expect(spawn).toHaveBeenCalled();
  });

  it("close() cancels pending reconnects and marks state disconnected", () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    client.start();
    sockets[0].fireConnect();
    sockets[0].fireError(new Error("eof"));
    sockets[0].fireClose();
    expect(sockets).toHaveLength(1);
    client.close();
    vi.advanceTimersByTime(60_000);
    // No new socket created after close().
    expect(sockets).toHaveLength(1);
    expect(client.connectionState).toBe("disconnected");
  });

  it("rejects pending requests with a transport error when the socket dies mid-call", async () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    client.start();
    sockets[0].fireConnect();
    const pending = client.call("atom.get", { hash: "x" });
    sockets[0].fireError(new Error("eof"));
    sockets[0].fireClose();
    await expect(pending).rejects.toThrow(/socket|closed/i);
  });

  it("rejects with a structured error when the daemon returns an RPC error", async () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    client.start();
    sockets[0].fireConnect();
    const pending = client.call("atom.get", { hash: "missing" });
    sockets[0].fireData(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        error: { code: 4040, message: "atom not found" },
      }) + "\n",
    );
    await expect(pending).rejects.toThrow(/not found/);
  });

  it("emits state changes the offline indicator can observe", () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    const states: ConnectionState[] = [];
    client.onStateChange((s) => states.push(s));
    client.start();
    sockets[0].fireConnect();
    expect(states).toEqual(["connecting", "connected"]);
    sockets[0].fireError(new Error("eof"));
    sockets[0].fireClose();
    expect(states).toContain("disconnected");
  });

  it("onStateChange returns an unsubscribe handle that stops future notifications", () => {
    const client = new DaemonClient({ socketPath: "/tmp/x.sock", netModule: net });
    const states: ConnectionState[] = [];
    const off = client.onStateChange((s) => states.push(s));
    client.start();
    sockets[0].fireConnect();
    expect(states).toEqual(["connecting", "connected"]);

    off();
    sockets[0].fireError(new Error("eof"));
    sockets[0].fireClose();
    // After unsubscribe the listener must not observe the
    // disconnected transition that follows.
    expect(states).toEqual(["connecting", "connected"]);
  });
});
