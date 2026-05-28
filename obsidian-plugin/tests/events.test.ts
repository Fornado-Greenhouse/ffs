import { describe, expect, it, vi } from "vitest";

import { FfsEventEmitter, NotificationFrame } from "../src/events.js";

function frame(method: string, params: Record<string, unknown> = {}): NotificationFrame {
  return { jsonrpc: "2.0", method, params };
}

describe("FfsEventEmitter", () => {
  it("delivers event.atom.committed to subscribed listeners", () => {
    const ee = new FfsEventEmitter();
    const seen: NotificationFrame[] = [];
    ee.on("event.atom.committed", (f) => seen.push(f));
    ee.emit(frame("event.atom.committed", { hash: "abc" }));
    expect(seen).toHaveLength(1);
    expect(seen[0].params.hash).toBe("abc");
  });

  it("delivers a single emit to every subscriber of that kind", () => {
    const ee = new FfsEventEmitter();
    const a = vi.fn();
    const b = vi.fn();
    ee.on("event.fastpath.applied", a);
    ee.on("event.fastpath.applied", b);
    ee.emit(frame("event.fastpath.applied"));
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });

  it("delivers every emit to the `*` wildcard channel", () => {
    const ee = new FfsEventEmitter();
    const star = vi.fn();
    ee.on("*", star);
    ee.emit(frame("event.atom.committed"));
    ee.emit(frame("event.projection.invalidated"));
    expect(star).toHaveBeenCalledTimes(2);
  });

  it("off() removes a listener so subsequent emits skip it", () => {
    const ee = new FfsEventEmitter();
    const fn = vi.fn();
    ee.on("event.atom.committed", fn);
    ee.emit(frame("event.atom.committed"));
    ee.off("event.atom.committed", fn);
    ee.emit(frame("event.atom.committed"));
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("isolates a throwing listener so other subscribers still fire", () => {
    const ee = new FfsEventEmitter();
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const bad = vi.fn(() => {
      throw new Error("boom");
    });
    const good = vi.fn();
    ee.on("event.atom.committed", bad);
    ee.on("event.atom.committed", good);
    ee.emit(frame("event.atom.committed"));
    expect(bad).toHaveBeenCalledTimes(1);
    expect(good).toHaveBeenCalledTimes(1);
    expect(errSpy).toHaveBeenCalled();
    errSpy.mockRestore();
  });

  it("listenerCount tracks adds and removes", () => {
    const ee = new FfsEventEmitter();
    expect(ee.listenerCount("event.atom.committed")).toBe(0);
    const fn = vi.fn();
    ee.on("event.atom.committed", fn);
    expect(ee.listenerCount("event.atom.committed")).toBe(1);
    ee.off("event.atom.committed", fn);
    expect(ee.listenerCount("event.atom.committed")).toBe(0);
  });
});
