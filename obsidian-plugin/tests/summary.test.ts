import { describe, expect, it, vi } from "vitest";

import { FfsEventEmitter, NotificationFrame } from "../src/events.js";
import {
  MAX_PANEL_ITEMS,
  PanelItem,
  SummaryPanelModel,
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
      },
    ]);
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
