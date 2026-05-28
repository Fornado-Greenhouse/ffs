// Entity-name search for the Obsidian quick-switcher / file-finder.
//
// The plugin debounces keystrokes (200ms default) and streams
// results: each completed search dispatches into the onResults
// callback so the view can update incrementally as the user types.
// In-flight searches whose keystroke has been superseded are
// discarded (their results never call back). The "stream" here is
// at the keystroke level — the daemon returns a complete result
// array per query, but the UI surfaces them one query at a time so
// the user always sees results for what they last typed.
//
// `entity.search` lives daemon-side (task_19 Rust addition); it
// returns `{results: [{entity, predicate, display_name}]}` already
// capped at `limit` (default 50).

import { DaemonClient } from "./client.js";

export const DEFAULT_DEBOUNCE_MS = 200;
export const DEFAULT_RESULT_LIMIT = 50;

export interface EntityHit {
  entity: string;
  predicate: string;
  displayName: string;
}

export interface EntitySearchOptions {
  /** Keystroke → query debounce. */
  debounceMs?: number;
  /** Maximum results per query. */
  limit?: number;
  /** Inject a custom timer for tests (vitest fake timers). */
  setTimeoutFn?: typeof setTimeout;
  clearTimeoutFn?: typeof clearTimeout;
}

export class EntitySearch {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private generation = 0;
  /** Most-recent results delivered to onResults. */
  results: EntityHit[] = [];

  constructor(
    private client: Pick<DaemonClient, "call">,
    private onResults: (results: EntityHit[], query: string) => void,
    private opts: EntitySearchOptions = {},
  ) {}

  /**
   * Push a keystroke. The query fires `debounceMs` after the LAST
   * keystroke; earlier in-flight queries whose generation has been
   * superseded are discarded.
   */
  pushQuery(query: string): void {
    const setT = this.opts.setTimeoutFn ?? setTimeout;
    const clearT = this.opts.clearTimeoutFn ?? clearTimeout;
    if (this.timer != null) {
      clearT(this.timer);
      this.timer = null;
    }
    this.generation += 1;
    const myGen = this.generation;
    this.timer = setT(() => {
      this.timer = null;
      void this.runQuery(query, myGen);
    }, this.opts.debounceMs ?? DEFAULT_DEBOUNCE_MS);
  }

  /** Cancel any pending query without running it. */
  cancel(): void {
    if (this.timer != null) {
      const clearT = this.opts.clearTimeoutFn ?? clearTimeout;
      clearT(this.timer);
      this.timer = null;
    }
    this.generation += 1;
  }

  private async runQuery(query: string, generation: number): Promise<void> {
    const trimmed = query.trim();
    if (trimmed.length === 0) {
      // Empty query clears results immediately.
      this.deliver([], trimmed, generation);
      return;
    }
    try {
      const response = (await this.client.call("entity.search", {
        query: trimmed,
        limit: this.opts.limit ?? DEFAULT_RESULT_LIMIT,
      })) as { results?: Array<{ entity: string; predicate: string; display_name: string }> };
      const hits: EntityHit[] = (response?.results ?? []).map((r) => ({
        entity: String(r.entity ?? ""),
        predicate: String(r.predicate ?? ""),
        displayName: String(r.display_name ?? ""),
      }));
      this.deliver(hits, trimmed, generation);
    } catch (err) {
      console.warn("[ffs] entity.search failed:", err);
      this.deliver([], trimmed, generation);
    }
  }

  /**
   * Deliver `hits` to the callback only if this query is still the
   * latest — older generations get discarded so a slow query
   * doesn't overwrite a fast newer one.
   */
  private deliver(hits: EntityHit[], query: string, generation: number): void {
    if (generation !== this.generation) return;
    this.results = hits;
    try {
      this.onResults(hits, query);
    } catch (err) {
      console.error("[ffs] entity-search listener threw:", err);
    }
  }
}
