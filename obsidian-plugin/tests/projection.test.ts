import { describe, expect, it, vi } from "vitest";

import {
  ProjectionSubscription,
  renderProjection,
} from "../src/projection.js";
import { FfsEventEmitter, NotificationFrame } from "../src/events.js";

function fakeClient(response: unknown) {
  return {
    call: vi.fn(async (_method: string, _params: unknown) => response),
  };
}

describe("projection", () => {
  it("renderProjection calls projection.render and surfaces the markdown + render hash", async () => {
    const client = fakeClient({
      markdown: "---\ndisplay_name: Sara\n---\n",
      render_hash: "zb2rh-hash",
    });
    const rendered = await renderProjection(
      client,
      "~/.ffs/contacts/by-name/S/Sara.md",
    );
    expect(rendered).not.toBeNull();
    expect(client.call).toHaveBeenCalledWith("projection.render", {
      path: "contacts/by-name/S/Sara.md",
    });
    expect(rendered!.markdown).toContain("Sara");
    expect(rendered!.renderHash).toBe("zb2rh-hash");
  });

  it("renderProjection returns null for folder paths and regular notes", async () => {
    const client = fakeClient({ markdown: "x" });
    expect(
      await renderProjection(client, "contacts/by-name/S/"),
    ).toBeNull();
    expect(await renderProjection(client, "daily/today.md")).toBeNull();
    expect(client.call).not.toHaveBeenCalled();
  });

  it("ProjectionSubscription fires onInvalidated when the current path matches", () => {
    const events = new FfsEventEmitter();
    const seen: string[] = [];
    const sub = new ProjectionSubscription(events, (path) => seen.push(path));
    sub.setCurrent("contacts/by-name/S/Sara.md");

    const frame: NotificationFrame = {
      jsonrpc: "2.0",
      method: "event.projection.invalidated",
      params: { path: "contacts/by-name/S/Sara.md" },
    };
    events.emit(frame);
    expect(seen).toEqual(["contacts/by-name/S/Sara.md"]);

    // Different path doesn't fire.
    events.emit({
      ...frame,
      params: { path: "contacts/by-name/B/Bob.md" },
    });
    expect(seen).toEqual(["contacts/by-name/S/Sara.md"]);
  });

  it("ProjectionSubscription clears its listener on dispose()", () => {
    const events = new FfsEventEmitter();
    const sub = new ProjectionSubscription(events, () => {});
    expect(events.listenerCount("event.projection.invalidated")).toBe(1);
    sub.dispose();
    expect(events.listenerCount("event.projection.invalidated")).toBe(0);
  });
});
