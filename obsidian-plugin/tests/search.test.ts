import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_DEBOUNCE_MS, EntityHit, EntitySearch } from "../src/search.js";

describe("EntitySearch", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("debounces keystrokes: fires once after 200ms of silence", async () => {
    const call = vi.fn(async () => ({
      results: [
        { entity: "Sara", predicate: "contact.person", display_name: "Sara" },
      ],
    }));
    const seen: string[] = [];
    const search = new EntitySearch({ call }, (hits, query) => {
      seen.push(`${query}=${hits.length}`);
    });
    search.pushQuery("S");
    vi.advanceTimersByTime(100);
    search.pushQuery("Sa");
    vi.advanceTimersByTime(100);
    search.pushQuery("Sar");
    // Only 100ms since the last keystroke — call hasn't fired.
    expect(call).not.toHaveBeenCalled();
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS - 1);
    expect(call).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    // Allow the awaited query to resolve.
    await Promise.resolve();
    await Promise.resolve();
    expect(call).toHaveBeenCalledTimes(1);
    expect(call.mock.calls[0][0]).toBe("entity.search");
    expect(seen).toEqual(["Sar=1"]);
  });

  it("discards stale results when a newer query has fired", async () => {
    // Two queries, both in flight at the same time. The first
    // resolves AFTER the second; the first's results should be
    // ignored.
    const responses: Array<Promise<unknown>> = [];
    const call = vi.fn(async (_method: string, _params: unknown) => {
      const p = new Promise<unknown>((resolve) => {
        responses.push(p as unknown as Promise<unknown>);
        // Defer until tests resolve it manually below.
        (p as unknown as { resolve: (v: unknown) => void }).resolve = resolve;
      });
      return p;
    });
    // Custom: capture the resolvers so we can settle them in order.
    const resolvers: Array<(v: unknown) => void> = [];
    const callCapture = vi.fn(
      (_method: string, _params: unknown) =>
        new Promise<unknown>((resolve) => resolvers.push(resolve)),
    );
    void call;
    const seen: EntityHit[][] = [];
    const search = new EntitySearch(
      { call: callCapture },
      (hits) => seen.push(hits),
    );
    search.pushQuery("Sara");
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS);
    await Promise.resolve();
    search.pushQuery("Sarah");
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS);
    await Promise.resolve();
    expect(callCapture).toHaveBeenCalledTimes(2);

    // Resolve the OLDER query first with one result. Its result
    // should be discarded because generation has advanced.
    resolvers[0]({
      results: [
        { entity: "Sara", predicate: "contact.person", display_name: "Sara" },
      ],
    });
    await Promise.resolve();
    expect(seen).toEqual([]);

    // Resolve the NEWER query — its result should land.
    resolvers[1]({
      results: [
        { entity: "Sarah", predicate: "contact.person", display_name: "Sarah" },
      ],
    });
    await Promise.resolve();
    expect(seen).toHaveLength(1);
    expect(seen[0][0].displayName).toBe("Sarah");
  });

  it("empty query clears results without hitting the daemon", async () => {
    const call = vi.fn(async () => ({ results: [] }));
    const seen: EntityHit[][] = [];
    const search = new EntitySearch({ call }, (hits) => seen.push(hits));
    search.pushQuery("   ");
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS);
    await Promise.resolve();
    expect(call).not.toHaveBeenCalled();
    expect(seen).toEqual([[]]);
  });

  it("cancel() stops a pending query from firing", async () => {
    const call = vi.fn(async () => ({ results: [] }));
    const seen: EntityHit[][] = [];
    const search = new EntitySearch({ call }, (hits) => seen.push(hits));
    search.pushQuery("Sara");
    search.cancel();
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS * 2);
    await Promise.resolve();
    expect(call).not.toHaveBeenCalled();
    expect(seen).toEqual([]);
  });

  it("daemon failure logs and clears the result set without throwing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const call = vi.fn(async () => {
      throw new Error("daemon offline");
    });
    const seen: EntityHit[][] = [];
    const search = new EntitySearch({ call }, (hits) => seen.push(hits));
    search.pushQuery("Sara");
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS);
    await Promise.resolve();
    await Promise.resolve();
    expect(seen).toEqual([[]]);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it("respects custom limit via options", async () => {
    const call = vi.fn(async () => ({ results: [] }));
    const search = new EntitySearch(
      { call },
      () => {},
      { limit: 10 },
    );
    search.pushQuery("x");
    vi.advanceTimersByTime(DEFAULT_DEBOUNCE_MS);
    await Promise.resolve();
    expect(call.mock.calls[0][1]).toEqual({ query: "x", limit: 10 });
  });
});
