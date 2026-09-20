import { describe, expect, it, vi } from "vitest";

import { FfsEventEmitter, NotificationFrame } from "../src/events.js";
import {
  MAX_PANEL_ITEMS,
  PanelItem,
  SummaryPanelModel,
  engineLabel,
  resolutionLabel,
  candidateLines,
} from "../src/summary.js";

function fakeClient(callMap: Record<string, unknown>) {
  const events = new FfsEventEmitter();
  return {
    events,
    call: vi.fn(async (method: string, _params: unknown) => {
      if (method in callMap) return callMap[method];
      return null;
    }),
  };
}

function summaryAtom(panel: PanelItem[], narrative = "Today's narrative"): unknown {
  return [
    {
      claim: {
        panel,
        narrative,
      },
    },
  ];
}

describe("SummaryPanelModel", () => {
  it("renders all items when fewer than the cap come back", async () => {
    const panel: PanelItem[] = [
      { priority: 1, kind: "federation_unhealthy", message: "fed-A" },
      { priority: 2, kind: "capability_denials", message: "cap-A" },
      { priority: 4, kind: "drift", message: "drift-A" },
    ];
    const client = fakeClient({
      "audit.query": summaryAtom(panel),
      "ingest.list_pending": [],
    });
    const model = new SummaryPanelModel(client);
    const state = await model.refresh();
    expect(state.items).toHaveLength(3);
    expect(state.items[0].message).toBe("fed-A");
  });

  it("truncates to MAX_PANEL_ITEMS (5) when more come back", async () => {
    const panel: PanelItem[] = Array.from({ length: 7 }, (_, i) => ({
      priority: i + 1,
      kind: "drift",
      message: `item-${i}`,
    }));
    const client = fakeClient({
      "audit.query": summaryAtom(panel),
      "ingest.list_pending": [],
    });
    const model = new SummaryPanelModel(client);
    const state = await model.refresh();
    expect(state.items).toHaveLength(MAX_PANEL_ITEMS);
    expect(state.items.map((i) => i.message)).toEqual([
      "item-0",
      "item-1",
      "item-2",
      "item-3",
      "item-4",
    ]);
  });

  it("exposes pending proposals from ingest.list_pending", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [
        {
          id: "sub-001",
          source_uri: "file:///note.md",
          proposals: [{ predicate: "contact.person" }],
        },
      ],
    });
    const model = new SummaryPanelModel(client);
    const state = await model.refresh();
    expect(state.pendingProposals).toEqual([
      {
        submissionId: "sub-001",
        sourceUri: "file:///note.md",
        proposalCount: 1,
        proposals: [{ predicate: "contact.person", claim: {}, rationale: "" }],
      },
    ]);
  });

  it("carries engine and model from ingest.list_pending into the preview (task_36)", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [
        {
          id: "sub-002",
          source_uri: "file:///card.md",
          proposals: [
            { predicate: "contact.person", engine: "llm", model: "claude-sonnet-5" },
            { predicate: "note", engine: "heuristic", model: "" },
            { predicate: "note" },
          ],
        },
      ],
    });
    const model = new SummaryPanelModel(client);
    const state = await model.refresh();
    const previews = state.pendingProposals[0].proposals;
    expect(previews[0].engine).toBe("llm");
    expect(previews[0].model).toBe("claude-sonnet-5");
    expect(previews[1].engine).toBe("heuristic");
    expect(previews[1].model).toBeUndefined();
    expect(previews[2].engine).toBeUndefined();
    expect(engineLabel(previews[0])).toBe("engine: llm (claude-sonnet-5)");
    expect(engineLabel(previews[1])).toBe("engine: heuristic");
    expect(engineLabel(previews[2])).toBe("");
  });

  it("accept() calls ingest.accept with the submission id then refreshes", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [],
      "ingest.accept": { accepted_atom_hashes: ["zhash"] },
    });
    const model = new SummaryPanelModel(client);
    await model.accept("sub-001");
    const accept = client.call.mock.calls.find((c) => c[0] === "ingest.accept");
    expect(accept).toBeDefined();
    expect(accept![1]).toEqual({ submission_id: "sub-001" });
    // refresh ran after accept (audit.query called twice — once
    // implicit in the constructor? No — refresh only inside accept).
    const queries = client.call.mock.calls.filter((c) => c[0] === "audit.query");
    expect(queries.length).toBe(1);
  });

  it("reject() calls ingest.reject with the submission id", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [],
      "ingest.reject": { rejected: "sub-002" },
    });
    const model = new SummaryPanelModel(client);
    await model.reject("sub-002");
    const reject = client.call.mock.calls.find((c) => c[0] === "ingest.reject");
    expect(reject![1]).toEqual({ submission_id: "sub-002" });
  });

  it("re-fetches when event.atom.committed arrives for an auditor.daily_summary atom", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [],
    });
    const model = new SummaryPanelModel(client);
    await model.refresh();
    const beforeCalls = client.call.mock.calls.length;

    const frame: NotificationFrame = {
      jsonrpc: "2.0",
      method: "event.atom.committed",
      params: { hash: "z", entity: "auditor", predicate: "auditor.daily_summary" },
    };
    client.events.emit(frame);
    // Allow the awaited refresh to settle.
    await new Promise((resolve) => setImmediate(resolve));
    expect(client.call.mock.calls.length).toBeGreaterThan(beforeCalls);
  });

  it("ignores event.atom.committed for non-auditor predicates", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [],
    });
    const model = new SummaryPanelModel(client);
    await model.refresh();
    const beforeCalls = client.call.mock.calls.length;

    client.events.emit({
      jsonrpc: "2.0",
      method: "event.atom.committed",
      params: { hash: "z", entity: "Sara", predicate: "contact.person" },
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(client.call.mock.calls.length).toBe(beforeCalls);
  });

  it("dispose() clears the daemon listener", () => {
    const client = fakeClient({});
    const model = new SummaryPanelModel(client);
    expect(client.events.listenerCount("event.atom.committed")).toBe(1);
    model.dispose();
    expect(client.events.listenerCount("event.atom.committed")).toBe(0);
  });

  it("notifies onChange subscribers when refresh completes", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([
        { priority: 1, kind: "drift", message: "drift-A" },
      ]),
      "ingest.list_pending": [],
    });
    const model = new SummaryPanelModel(client);
    const states: number[] = [];
    model.onChange((s) => states.push(s.items.length));
    await model.refresh();
    expect(states).toEqual([1]);
  });

  it("onChange returns an unsubscribe handle that stops future notifications", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([
        { priority: 1, kind: "drift", message: "drift-A" },
      ]),
      "ingest.list_pending": [],
    });
    const model = new SummaryPanelModel(client);
    const states: number[] = [];
    const off = model.onChange((s) => states.push(s.items.length));
    await model.refresh();
    expect(states).toEqual([1]);

    off();
    await model.refresh();
    // The listener was unsubscribed before the second refresh — its
    // callback must not fire again.
    expect(states).toEqual([1]);
  });

  it("event.atom.committed updates lastCommittedAt on every commit", () => {
    const client = fakeClient({});
    const model = new SummaryPanelModel(client);
    expect(model.state.lastCommittedAt).toBeNull();

    // Fire a commit for a non-auditor predicate (which would NOT
    // trigger a refresh) and verify lastCommittedAt still updates.
    client.events.emit({
      jsonrpc: "2.0",
      method: "event.atom.committed",
      params: {
        hash: "zhash",
        entity: "Sara_Chen",
        predicate: "contact.person",
      },
    });
    expect(model.state.lastCommittedAt).toBeInstanceOf(Date);
  });
});


describe("resolution on the proposal card (task_45)", () => {
  it("carries resolution, entity, and candidates from ingest.list_pending", async () => {
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [
        {
          id: "sub-1",
          source_uri: "file:///ingest/a.md",
          proposals: [
            {
              predicate: "person.generic",
              claim: { display_name: "Sara Chen" },
              rationale: "r",
              resolution: "ambiguous",
              candidates: [
                { entity: "zA", display: "Sara Chen (Acme)", score: 6.5, matched_on: ["display_name"] },
                { entity: "zB", display: "Sara Chen (City)", score: 6.0, matched_on: ["display_name"] },
              ],
            },
            { predicate: "org.company", claim: { display_name: "Acme" }, rationale: "r", resolution: "new" },
          ],
        },
      ],
    });
    const model = new SummaryPanelModel(client);
    const state = await model.refresh();
    const previews = state.pendingProposals[0].proposals;
    expect(previews[0].resolution).toBe("ambiguous");
    expect(previews[0].candidates?.length).toBe(2);
    expect(previews[0].candidates?.[0].matchedOn).toEqual(["display_name"]);
    expect(previews[1].resolution).toBe("new");
    expect(previews[1].candidates).toBeUndefined();
  });

  it("labels the three outcomes and lists candidates", () => {
    expect(resolutionLabel({})).toBe("");
    expect(resolutionLabel({ resolution: "new" })).toBe("resolution: new");
    expect(resolutionLabel({ resolution: "ambiguous" })).toBe("resolution: ambiguous");
    expect(
      resolutionLabel({
        resolution: "existing",
        candidates: [{ entity: "zA", display: "Sara Chen", score: 12.5, matchedOn: [] }],
      }),
    ).toBe("resolution: existing (Sara Chen)");
    expect(
      candidateLines({
        candidates: [
          { entity: "zA", display: "Sara Chen (Acme)", score: 6.5, matchedOn: [] },
          { entity: "zB", display: "Sara Chen (City)", score: 6.04, matchedOn: [] },
        ],
      }),
    ).toEqual(["Sara Chen (Acme) (6.5)", "Sara Chen (City) (6.0)"]);
  });
});

describe("inbox count line and link (ADR-032, task_39)", () => {
  it("counts pending proposals and the ones that need the owner's eye", async () => {
    const { countLine, needsEye, inboxPathFor } = await import("../src/summary.js");
    const client = fakeClient({
      "audit.query": summaryAtom([]),
      "ingest.list_pending": [
        {
          id: "sub-1",
          source_uri: "file:///ingest/a.md",
          proposals: [
            { predicate: "source.article", claim: { title: "A" }, resolution: "new" },
            { predicate: "person.generic", claim: { display_name: "P" }, resolution: "ambiguous", candidates: [] },
            { predicate: "affiliation", claim: { title: "CEO" }, resolution: "existing" },
            { predicate: "affiliation", claim: { title: "Chair" }, resolution: "new", ends_role: true },
          ],
        },
      ],
    });
    const model = new SummaryPanelModel(client);
    const state = await model.refresh();
    expect(state.pendingCount).toBe(4);
    expect(state.needYourEye).toBe(3);
    expect(countLine(state)).toBe("4 pending, 3 need your eye");
    expect(countLine({ pendingCount: 2, needYourEye: 0 })).toBe("2 pending");
    expect(needsEye({ predicate: "note", resolution: "new" })).toBe(false);
    expect(state.inboxPath).toBe(inboxPathFor());
    expect(inboxPathFor(new Date("2026-09-21T15:00:00Z"))).toBe("inbox/2026-09-21.md");
  });
});
