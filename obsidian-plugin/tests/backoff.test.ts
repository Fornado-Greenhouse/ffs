import { describe, expect, it } from "vitest";

import {
  RECONNECT_CAP_MS,
  RECONNECT_INITIAL_MS,
  backoffSequence,
  nextBackoffMs,
} from "../src/backoff.js";

describe("backoff", () => {
  it("doubles until reaching the cap, then clamps", () => {
    const seq = backoffSequence(8);
    expect(seq).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  });

  it("constants match the spec-required values", () => {
    expect(RECONNECT_INITIAL_MS).toBe(1000);
    expect(RECONNECT_CAP_MS).toBe(30000);
  });

  it("nextBackoffMs on a fresh 0 input returns the initial step", () => {
    expect(nextBackoffMs(0)).toBe(RECONNECT_INITIAL_MS);
  });

  it("nextBackoffMs caps at RECONNECT_CAP_MS", () => {
    expect(nextBackoffMs(40000)).toBe(RECONNECT_CAP_MS);
    expect(nextBackoffMs(30000)).toBe(RECONNECT_CAP_MS);
  });
});
