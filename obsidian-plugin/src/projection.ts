// Projection rendering on file open + live updates from the
// daemon's `event.projection.invalidated` notifications.
//
// Behavior:
//
// - On file-open, if the path resolves to a single-entity
//   projection (per ADR-011), call `projection.render` and replace
//   the in-Obsidian buffer with the rendered markdown. Capture the
//   `render_hash` so the edit-routing layer can diff against the
//   canonical render later.
// - When the daemon publishes `event.projection.invalidated` for a
//   path currently open in Obsidian, re-render and refresh the
//   buffer. (The invalidation flows from federation pulls,
//   librarian refreshes, and fastpath supersessions of OTHER paths
//   that side-affect this one.)

import { DaemonClient } from "./client.js";
import { NotificationFrame } from "./events.js";
import { isProjectionFile, normalizeProjectionPath } from "./paths.js";

export interface RenderedProjection {
  /** Vault-relative path (normalized). */
  path: string;
  /** Rendered markdown the buffer should show. */
  markdown: string;
  /** BLAKE3-multihash of the render — used as the diff baseline. */
  renderHash: string;
}

/**
 * Render a projection by path. Returns `null` when `path` isn't a
 * single-entity projection (callers should leave Obsidian's
 * default open behavior in place for non-projection files).
 */
export async function renderProjection(
  client: Pick<DaemonClient, "call">,
  path: string,
): Promise<RenderedProjection | null> {
  if (!isProjectionFile(path)) return null;
  const normalized = normalizeProjectionPath(path);
  const resp = (await client.call("projection.render", {
    path: normalized,
  })) as { markdown?: string; render_hash?: string };
  return {
    path: normalized,
    markdown: typeof resp?.markdown === "string" ? resp.markdown : "",
    renderHash: typeof resp?.render_hash === "string" ? resp.render_hash : "",
  };
}

/**
 * Live-update subscription wiring. The plugin holds one
 * `ProjectionSubscription` and registers it against the
 * `DaemonClient`'s event emitter; when the open file matches an
 * invalidated path, the subscription fires the supplied
 * `onInvalidated` callback so the host can re-render the buffer.
 */
export class ProjectionSubscription {
  private currentPath: string | null = null;

  constructor(
    private events: {
      on(
        kind: "event.projection.invalidated",
        fn: (frame: NotificationFrame) => void,
      ): void;
      off(
        kind: "event.projection.invalidated",
        fn: (frame: NotificationFrame) => void,
      ): void;
    },
    private onInvalidated: (path: string) => void,
  ) {
    this.handle = this.handle.bind(this);
    this.events.on("event.projection.invalidated", this.handle);
  }

  /** Update the currently-open projection path. `null` clears it. */
  setCurrent(path: string | null): void {
    this.currentPath = path === null ? null : normalizeProjectionPath(path);
  }

  /** Remove the daemon listener (call when the plugin unloads). */
  dispose(): void {
    this.events.off("event.projection.invalidated", this.handle);
  }

  private handle(frame: NotificationFrame): void {
    const invalidatedPath = frame.params?.path;
    if (typeof invalidatedPath !== "string") return;
    if (this.currentPath === null) return;
    const normalized = normalizeProjectionPath(invalidatedPath);
    if (normalized === this.currentPath) {
      this.onInvalidated(normalized);
    }
  }
}
