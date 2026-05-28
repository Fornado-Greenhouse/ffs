// Edit routing for in-Obsidian edits to projection files.
//
// The plugin mirrors a tiny subset of the Rust fastpath
// classifier's heuristic (per ADR-014) — just enough to decide
// whether to optimistically apply the edit via `fastpath.submit`
// or route it to `ingest.submit` for the slow path.
//
// The full classifier lives in the Rust daemon
// (`crates/ffs-fastpath`). The plugin's job is twofold:
//
// 1. Make a heuristic call locally so the optimistic UI update
//    fires immediately — the user shouldn't wait for an RPC to
//    learn whether their typo fix is fast-path eligible.
// 2. Submit the edit to the daemon, which runs the authoritative
//    classifier. If the daemon disagrees with the heuristic
//    (returns `kind: "routed_to_ingest"` instead of `applied`),
//    the optimistic update rolls back.

import { DaemonClient } from "./client.js";
import {
  isProjectionFile,
  normalizeProjectionPath,
  parseProjectionPath,
} from "./paths.js";

export type EditShape =
  | "single-line-text"
  | "frontmatter-value"
  | "additive-section"
  | "ambiguous";

export interface ClassifiedEdit {
  shape: EditShape;
  /** True iff the daemon's fastpath is likely to accept this edit. */
  fastPathEligible: boolean;
}

/**
 * Heuristic classifier matching `crates/ffs-fastpath/src/classifier.rs`:
 *
 * - Frontmatter (`---`…`---`) values differing in exactly one key → frontmatter or single-line text.
 * - A single new `- item` appended to a `## Section` body → additive section.
 * - Anything else → ambiguous (route to ingest).
 */
export function classifyEdit(oldText: string, newText: string): ClassifiedEdit {
  if (oldText === newText) {
    return { shape: "ambiguous", fastPathEligible: false };
  }
  const oldFm = parseFrontmatter(oldText);
  const newFm = parseFrontmatter(newText);
  // Compare frontmatter for a single value-only change.
  if (
    oldFm.keys.length === newFm.keys.length &&
    oldFm.keys.every((k, i) => newFm.keys[i] === k)
  ) {
    let changed = 0;
    for (const k of oldFm.keys) {
      if (oldFm.map.get(k) !== newFm.map.get(k)) changed++;
    }
    if (changed === 1 && oldFm.body === newFm.body) {
      return { shape: "single-line-text", fastPathEligible: true };
    }
    if (changed === 0 && oldFm.body !== newFm.body) {
      // Body changed; check for additive-section bullet.
      const bullet = singleAppendedBullet(oldFm.body, newFm.body);
      if (bullet) {
        return { shape: "additive-section", fastPathEligible: true };
      }
    }
  }
  return { shape: "ambiguous", fastPathEligible: false };
}

interface ParsedFm {
  keys: string[];
  map: Map<string, string>;
  body: string;
}

function parseFrontmatter(text: string): ParsedFm {
  const map = new Map<string, string>();
  const keys: string[] = [];
  const lines = text.split("\n");
  if (lines[0]?.trim() !== "---") {
    return { keys, map, body: text };
  }
  let close = -1;
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].trim() === "---") {
      close = i;
      break;
    }
    const m = lines[i].match(/^([^:]+):\s*(.*)$/);
    if (m) {
      keys.push(m[1].trim());
      map.set(m[1].trim(), m[2].trim());
    }
  }
  if (close < 0) return { keys: [], map: new Map(), body: text };
  return { keys, map, body: lines.slice(close + 1).join("\n") };
}

function singleAppendedBullet(oldBody: string, newBody: string): string | null {
  const oldLines = oldBody.split("\n").filter((l) => l.length > 0);
  const newLines = newBody.split("\n").filter((l) => l.length > 0);
  if (newLines.length !== oldLines.length + 1) return null;
  for (let i = 0; i < oldLines.length; i++) {
    if (oldLines[i] !== newLines[i]) return null;
  }
  const last = newLines[newLines.length - 1].trim();
  if (last.startsWith("- ") || last.startsWith("* ")) {
    return last.slice(2).trim();
  }
  return null;
}

// ---- routing ----

export interface RoutedEdit {
  /** Whether the edit went through `fastpath.submit` or `ingest.submit`. */
  via: "fastpath" | "ingest";
  /** Whether the daemon accepted the fast-path classification. */
  applied: boolean;
  /** Atom hash of the supersession, when applied. */
  atomHash?: string;
  /** Daemon response payload for the host to inspect. */
  raw: unknown;
}

/**
 * Route an edit through the daemon. Returns `null` when the path
 * isn't a projection file (no edit routing for regular notes).
 *
 * The caller is expected to have done an optimistic UI update
 * already — use `applyOptimistically()` to wrap the call and get a
 * rollback function back.
 */
export async function routeEdit(
  client: Pick<DaemonClient, "call">,
  path: string,
  oldText: string,
  newText: string,
): Promise<RoutedEdit | null> {
  if (!isProjectionFile(path)) return null;
  const normalized = normalizeProjectionPath(path);
  const classification = classifyEdit(oldText, newText);
  if (classification.fastPathEligible) {
    try {
      const result = (await client.call("fastpath.submit", {
        projection_path: normalized,
        new_content: newText,
      })) as { kind?: string; atom_hash?: string };
      if (result?.kind === "applied") {
        return {
          via: "fastpath",
          applied: true,
          atomHash: result.atom_hash,
          raw: result,
        };
      }
      // Fast-path declined → fall through to ingest.
    } catch (err) {
      // Fast-path RPC error → fall through to ingest so the user's
      // edit still lands somewhere.
      console.warn("[ffs] fastpath.submit failed, routing to ingest:", err);
    }
  }
  const parsed = parseProjectionPath(normalized);
  const sourceUri =
    parsed.kind === "single-entity"
      ? `ffs:${parsed.family}/${parsed.entity}`
      : `ffs:${normalized}`;
  const result = await client.call("ingest.submit", {
    source_uri: sourceUri,
    content: newText,
  });
  return {
    via: "ingest",
    applied: false,
    raw: result,
  };
}

// ---- optimistic update with rollback ----

export interface OptimisticHandle<T> {
  /** The new buffer state to render immediately. */
  next: T;
  /**
   * Await the daemon's response. On rejection, the buffer should
   * roll back to `previous`. Resolves with `next` on success
   * (callers can read `routed.applied` to know whether canonical
   * re-render is needed).
   */
  settle(promise: Promise<RoutedEdit | null>): Promise<OptimisticOutcome<T>>;
  /** The original buffer state — restore this on rollback. */
  previous: T;
}

export interface OptimisticOutcome<T> {
  applied: T;
  routed: RoutedEdit | null;
  rolledBack: boolean;
}

/**
 * Wrap a daemon RPC in an optimistic-update lifecycle. The host
 * renders `next` immediately; awaits `settle()`; if it rejects,
 * the host re-renders `previous`. `outcome.routed.applied` tells
 * the host whether the daemon kept the edit or routed it to ingest.
 */
export function applyOptimistically<T>(
  previous: T,
  next: T,
): OptimisticHandle<T> {
  return {
    previous,
    next,
    async settle(promise) {
      try {
        const routed = await promise;
        return { applied: next, routed, rolledBack: false };
      } catch (err) {
        console.warn("[ffs] edit failed, rolling back optimistic update:", err);
        return { applied: previous, routed: null, rolledBack: true };
      }
    },
  };
}
