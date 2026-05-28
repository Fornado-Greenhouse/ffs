import { describe, expect, it, vi } from "vitest";

import {
  applyOptimistically,
  classifyEdit,
  routeEdit,
} from "../src/editing.js";

function fakeClient(responses: Record<string, unknown>, errors: string[] = []) {
  return {
    call: vi.fn(async (method: string, _params: unknown) => {
      if (errors.includes(method)) {
        throw new Error(`${method} failed`);
      }
      if (method in responses) return responses[method];
      return {};
    }),
  };
}

const PATH = "contacts/by-name/S/Sara.md";

const FRONTMATTER_OLD = "---\ndisplay_name: Sara\nemail: sara@example.com\n---\n";
const FRONTMATTER_NEW = "---\ndisplay_name: Sarah\nemail: sara@example.com\n---\n";

const NOTES_OLD =
  "---\ndisplay_name: Sara\n---\n\n## Notes\n- met at the conference\n";
const NOTES_NEW =
  "---\ndisplay_name: Sara\n---\n\n## Notes\n- met at the conference\n- likes heirloom tomatoes\n";

const AMBIGUOUS_OLD = "---\ndisplay_name: Sara\n---\nLine A\nLine B\n";
const AMBIGUOUS_NEW = "---\ndisplay_name: Sarah\n---\nLine A different\nLine B different\n";

describe("classifyEdit", () => {
  it("classifies a single-frontmatter-value change as fast-path eligible", () => {
    const c = classifyEdit(FRONTMATTER_OLD, FRONTMATTER_NEW);
    expect(c).toEqual({ shape: "single-line-text", fastPathEligible: true });
  });

  it("classifies a single appended-bullet change as additive-section", () => {
    const c = classifyEdit(NOTES_OLD, NOTES_NEW);
    expect(c).toEqual({ shape: "additive-section", fastPathEligible: true });
  });

  it("classifies a multi-edit rewrite as ambiguous", () => {
    const c = classifyEdit(AMBIGUOUS_OLD, AMBIGUOUS_NEW);
    expect(c).toMatchObject({ shape: "ambiguous", fastPathEligible: false });
  });

  it("classifies an identical edit as ambiguous (nothing to submit)", () => {
    const c = classifyEdit(FRONTMATTER_OLD, FRONTMATTER_OLD);
    expect(c.fastPathEligible).toBe(false);
  });
});

describe("routeEdit", () => {
  it("sends a single-line text edit to fastpath.submit", async () => {
    const client = fakeClient({
      "fastpath.submit": { kind: "applied", atom_hash: "zhash" },
    });
    const routed = await routeEdit(client, PATH, FRONTMATTER_OLD, FRONTMATTER_NEW);
    expect(routed).toMatchObject({ via: "fastpath", applied: true, atomHash: "zhash" });
    expect(client.call).toHaveBeenCalledWith("fastpath.submit", {
      projection_path: "contacts/by-name/S/Sara.md",
      new_content: FRONTMATTER_NEW,
    });
  });

  it("routes a multi-paragraph rewrite to ingest.submit", async () => {
    const client = fakeClient({
      "ingest.submit": { submission_id: "sub-001" },
    });
    const routed = await routeEdit(client, PATH, AMBIGUOUS_OLD, AMBIGUOUS_NEW);
    expect(routed).toMatchObject({ via: "ingest", applied: false });
    expect(client.call).toHaveBeenCalledWith("ingest.submit", {
      source_uri: "ffs:contacts/Sara",
      content: AMBIGUOUS_NEW,
    });
  });

  it("falls back to ingest when fastpath.submit declines", async () => {
    const client = fakeClient({
      "fastpath.submit": { kind: "routed_to_ingest" },
      "ingest.submit": { submission_id: "sub-002" },
    });
    const routed = await routeEdit(client, PATH, FRONTMATTER_OLD, FRONTMATTER_NEW);
    expect(routed!.via).toBe("ingest");
  });

  it("falls back to ingest when fastpath.submit throws", async () => {
    const errSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const client = fakeClient(
      { "ingest.submit": { submission_id: "sub-003" } },
      ["fastpath.submit"],
    );
    const routed = await routeEdit(client, PATH, FRONTMATTER_OLD, FRONTMATTER_NEW);
    expect(routed!.via).toBe("ingest");
    errSpy.mockRestore();
  });

  it("returns null for non-projection paths", async () => {
    const client = fakeClient({});
    expect(await routeEdit(client, "daily/today.md", "a", "b")).toBeNull();
    expect(client.call).not.toHaveBeenCalled();
  });
});

describe("applyOptimistically", () => {
  it("returns the new state on RPC success", async () => {
    const handle = applyOptimistically("OLD", "NEW");
    const outcome = await handle.settle(
      Promise.resolve({ via: "fastpath" as const, applied: true, raw: {} }),
    );
    expect(outcome.applied).toBe("NEW");
    expect(outcome.rolledBack).toBe(false);
    expect(outcome.routed?.applied).toBe(true);
  });

  it("rolls back to the previous state on RPC failure", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const handle = applyOptimistically("OLD", "NEW");
    const outcome = await handle.settle(
      Promise.reject(new Error("daemon offline")),
    );
    expect(outcome.applied).toBe("OLD");
    expect(outcome.rolledBack).toBe(true);
    expect(outcome.routed).toBeNull();
    warnSpy.mockRestore();
  });
});
