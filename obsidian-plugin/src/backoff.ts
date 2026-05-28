// Exponential-backoff helper for the reconnection loop. Mirrors the
// Rust-side scheduler shape from `crates/ffs-federation/src/scheduler.rs`
// — same doubling, same cap. Task_17 spec caps reconnection backoff
// at 30s, not 60s, because the plugin wants tighter responsiveness
// when the daemon returns.

export const RECONNECT_INITIAL_MS = 1000;
export const RECONNECT_CAP_MS = 30_000;

export function nextBackoffMs(current: number): number {
  const next = Math.min(current * 2, RECONNECT_CAP_MS);
  // Guard against a caller passing 0 or a negative — start fresh.
  if (current <= 0) {
    return RECONNECT_INITIAL_MS;
  }
  return next;
}

/**
 * Produce the deterministic backoff sequence the spec asserts:
 * `1s, 2s, 4s, 8s, 16s, 30s, 30s, ...` (cap at 30s, not exactly the
 * next power of two when that exceeds the cap).
 */
export function backoffSequence(steps: number): number[] {
  const out: number[] = [];
  let cur = RECONNECT_INITIAL_MS;
  for (let i = 0; i < steps; i++) {
    out.push(cur);
    cur = nextBackoffMs(cur);
  }
  return out;
}
