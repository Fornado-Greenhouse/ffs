import { describe, expect, it, vi } from "vitest";

import { FfsEventEmitter, NotificationFrame } from "../src/events.js";
import {
  BriefingPanelModel,
  DISMISS_DAYS,
  briefingPathFor,
  changeLine,
  eyeCandidateLabel,
  pairKey,
  promoteNote,
} from "../src/briefing.js";
import { SummaryPanelModel } from "../src/summary.js";

function fakeClient(callMap: Record<string, unknown | ((params: unknown) => unknown)>) {
  const events = new FfsEventEmitter();
  return {
    events,
    call: vi.fn(async (method: string, params: unknown) => {
      if (method in callMap) {
        const v = callMap[method];
        return typeof v === "function" ? (v as (p: unknown) => unknown)(params) : v;
      }
      return null;
    }),
  };
}

const BRIEFING_HASH = "zQmBriefing001";

function briefingClaim(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    date: "2026-09-20",
    window: { from: "2026-09-13T06:00:00Z", to: "2026-09-20T06:00:00Z" },
    cadence: "7d",
    generated_at: "2026-09-20T06:00:01Z",
    narrative: "Three new people, one retitle, Acme is trending.",
    truncated: false,
    ceiling: 50,
    new_people: [
      {
        entity: "zP1",
        display: "Sara Chen",
        organization: { entity: "zOAcme", display: "Acme" },
        first_seen_article: { entity: "zArt1", display: "Acme names new CTO" },
      },
    ],
    changes: [
      {
        entity: "zP2",
        display: "Tom Ruiz",
        kind: "retitled",
        organization: { entity: "zOAcme", display: "Acme" },
        from: "VP Eng",
        to: "CTO",
        as_reported: "2026-09-15",
        source_article: { entity: "zArt1", display: "Acme names new CTO" },
      },
    ],
    trending_orgs: [
      { entity: "zOAcme", display: "Acme", mentions_this_window: 4, mentions_prior_window: 1 },
    ],
    events: [
      {
        kind: "funding",
        items: [
          {
            entity: "zE1",
            display: "Acme Series B",
            date: "2026-09-16",
            participants: [{ entity: "zOAcme", display: "Acme", role: "raised" }],
          },
        ],
      },
    ],
    promotion_candidates: [
      {
        entity: "zP1",
        display: "Sara Chen",
        organization: { entity: "zOAcme", display: "Acme" },
        mention_count: 5,
        article_count: 3,
        reason: "mentioned in 3 articles",
      },
      {
        entity: "zP3",
        display: "Lee: Park [Jr]",
        organization: null,
        mention_count: 2,
        article_count: 1,
        reason: "affiliated with a contact's organization",
        aliases: ["L. Park", "Park: Lee"],
      },
    ],
    follow_ups: [
      {
        entity: "zC1",
        display: "Dana Kim",
        organization: { entity: "zOAcme", display: "Acme" },
        triggering_article: { entity: "zArt1", display: "Acme names new CTO" },
      },
    ],
    needs_your_eye: [
      {
        submission_id: "sub-amb-1",
        local_ref: "p0",
        predicate: "person.generic",
        display: "Sara Chen",
        candidates: [
          { entity: "zP1", display: "Sara Chen (Acme)", score: 6.5 },
          { entity: "zP9", display: "Sara Chen (City)", score: 6.04 },
        ],
      },
    ],
    possible_duplicates: [
      {
        family: "people",
        entity_a: { entity: "zP1", display: "Sara Chen" },
        entity_b: { entity: "zP9", display: "S. Chen" },
        shared_aliases: ["S. Chen"],
      },
    ],
    recent_merges: [
      {
        same_as_hash: "zSameOld",
        source: { entity: "zP7", display: "Bob Li" },
        target: { entity: "zP8", display: "Robert Li" },
        tx_time: "2026-09-18T10:00:00Z",
      },
    ],
    filing: { auto_filed_count: 12, reviewed_count: 3 },
    ...overrides,
  };
}

function briefingAtoms(claim: Record<string, unknown> = briefingClaim(), hash = BRIEFING_HASH): unknown {
  return [
    {
      hash,
      entity: "auditor",
      predicate: "auditor.briefing",
      claim,
      tx_time: "2026-09-20T06:00:01Z",
    },
  ];
}

describe("BriefingPanelModel", () => {
  it("briefing_section_renders_latest_briefing", async () => {
    const client = fakeClient({
      "audit.query": (params: unknown) => {
        expect(params).toEqual({ kind: "briefing" });
        return briefingAtoms();
      },
    });
    const model = new BriefingPanelModel(client);
    expect(model.state.empty).toBe(true);
    const s = await model.refresh();

    expect(s.empty).toBe(false);
    expect(s.hash).toBe(BRIEFING_HASH);
    expect(s.date).toBe("2026-09-20");
    expect(s.pagePath).toBe("briefings/2026-09-20.md");
    expect(briefingPathFor("2026-09-20")).toBe("briefings/2026-09-20.md");
    expect(s.window).toEqual({ from: "2026-09-13T06:00:00Z", to: "2026-09-20T06:00:00Z" });
    expect(s.cadence).toBe("7d");
    expect(s.narrative).toMatch(/Acme is trending/);
    expect(s.counts).toEqual({
      newPeople: 1,
      changes: 1,
      trendingOrgs: 1,
      events: 1,
      promotionCandidates: 2,
      followUps: 1,
      needsYourEye: 1,
      possibleDuplicates: 1,
      recentMerges: 1,
    });
    expect(s.newPeople[0]).toEqual({
      entity: "zP1",
      display: "Sara Chen",
      organization: { entity: "zOAcme", display: "Acme" },
      firstSeenArticle: { entity: "zArt1", display: "Acme names new CTO" },
    });
    expect(s.changes[0].kind).toBe("retitled");
    expect(changeLine(s.changes[0])).toBe("Tom Ruiz at Acme: VP Eng → CTO — as reported 2026-09-15");
    expect(s.trendingOrgs[0]).toEqual({
      entity: "zOAcme",
      display: "Acme",
      mentionsThisWindow: 4,
      mentionsPriorWindow: 1,
    });
    expect(s.events[0].kind).toBe("funding");
    expect(s.events[0].items[0].participants[0]).toEqual({ entity: "zOAcme", display: "Acme", role: "raised" });
    expect(s.promotionCandidates.map((c) => c.display)).toEqual(["Sara Chen", "Lee: Park [Jr]"]);
    expect(s.followUps[0].organization?.display).toBe("Acme");
    expect(s.needsYourEye[0].candidates).toHaveLength(2);
    expect(eyeCandidateLabel(s.needsYourEye[0].candidates[1])).toBe("Accept as Sara Chen (City) (6.0)");
    expect(s.possibleDuplicates[0].sharedAliases).toEqual(["S. Chen"]);
    expect(s.recentMerges[0].sameAsHash).toBe("zSameOld");
    expect(s.filing).toEqual({ autoFiledCount: 12, reviewedCount: 3 });
  });

  it("renders defensively when every list is missing (older atoms)", async () => {
    const client = fakeClient({
      "audit.query": briefingAtoms({ narrative: "Quiet week." }, "zOld"),
    });
    const model = new BriefingPanelModel(client);
    const s = await model.refresh();
    expect(s.empty).toBe(false);
    expect(s.narrative).toBe("Quiet week.");
    // `date` falls back to the atom's tx_time day.
    expect(s.date).toBe("2026-09-20");
    expect(s.newPeople).toEqual([]);
    expect(s.promotionCandidates).toEqual([]);
    expect(s.needsYourEye).toEqual([]);
    expect(s.possibleDuplicates).toEqual([]);
    expect(s.recentMerges).toEqual([]);
    expect(s.counts.events).toBe(0);
    expect(s.filing).toEqual({ autoFiledCount: 0, reviewedCount: 0 });
  });

  it("stays empty when no briefing atom exists", async () => {
    const client = fakeClient({ "audit.query": [] });
    const model = new BriefingPanelModel(client);
    const s = await model.refresh();
    expect(s.empty).toBe(true);
    expect(s.narrative).toBe("No briefing yet.");
  });

  it("promote_to_contact_submits_quarantined_proposal_with_briefing_provenance", async () => {
    const client = fakeClient({
      "audit.query": briefingAtoms(),
      "ingest.submit": { submission_id: "sub-promo-1" },
    });
    const model = new BriefingPanelModel(client);
    await model.refresh();

    const id = await model.promote("zP1");
    expect(id).toBe("sub-promo-1");

    const submit = client.call.mock.calls.find((c) => c[0] === "ingest.submit");
    expect(submit).toBeDefined();
    const params = submit![1] as { source_uri: string; content: string };
    expect(Object.keys(params).sort()).toEqual(["content", "source_uri"]);
    expect(params.source_uri.startsWith(`ffs://briefing/${BRIEFING_HASH}/promote/`)).toBe(true);
    expect(params.source_uri).toBe(`ffs://briefing/${BRIEFING_HASH}/promote/zP1`);
    expect(params.content).toContain("predicate: contact.person");
    expect(params.content).toContain("name: Sara Chen");
    expect(params.content).toContain("organization: Acme");
    expect(params.content).not.toContain("role:");
    expect(params.content).not.toContain("aliases:");
    expect(params.content).toContain("Promoted from the briefing of 2026-09-20.");
    expect(params.content.startsWith("---\n")).toBe(true);

    // The human gate is preserved: nothing but ingest.submit was
    // called; the proposal lands in the quarantine, not the store.
    const methods = client.call.mock.calls.map((c) => c[0]);
    expect(methods.filter((m) => m !== "audit.query")).toEqual(["ingest.submit"]);

    // The promoted candidate leaves the list; the other stays.
    expect(model.state.promotionCandidates.map((c) => c.entity)).toEqual(["zP3"]);
    expect(model.state.counts.promotionCandidates).toBe(1);
  });

  it("promote note quotes YAML-unsafe values and lists aliases when present", () => {
    const note = promoteNote(
      {
        entity: "zP3",
        display: "Lee: Park [Jr]",
        organization: { entity: null, display: "Acme, Inc" },
        mentionCount: 2,
        articleCount: 1,
        reason: "",
        aliases: ["L. Park", "Park: Lee"],
        role: "CFO",
      },
      "2026-09-20",
    );
    expect(note).toContain('name: "Lee: Park [Jr]"');
    expect(note).toContain('organization: "Acme, Inc"');
    expect(note).toContain("role: CFO");
    expect(note).toContain('aliases: [L. Park, "Park: Lee"]');
  });

  it("dismiss_suppresses_candidate_for_thirty_days", async () => {
    const day0 = new Date("2026-09-20T09:00:00Z");
    let now = day0;
    const persisted: unknown[] = [];
    const client = fakeClient({ "audit.query": briefingAtoms() });
    const model = new BriefingPanelModel(client, {
      now: () => now,
      persist: (state) => {
        persisted.push(state);
      },
    });
    await model.refresh();
    expect(model.state.promotionCandidates.map((c) => c.entity)).toEqual(["zP1", "zP3"]);

    await model.dismiss("zP1");
    expect(persisted).toEqual([{ dismissed: { zP1: day0.toISOString() } }]);
    expect(model.state.promotionCandidates.map((c) => c.entity)).toEqual(["zP3"]);
    expect(model.state.counts.promotionCandidates).toBe(1);

    // Day 29: still hidden (also across a re-fetch).
    now = new Date(day0.getTime() + 29 * 24 * 60 * 60 * 1000);
    await model.refresh();
    expect(model.isDismissed("zP1")).toBe(true);
    expect(model.state.promotionCandidates.map((c) => c.entity)).toEqual(["zP3"]);

    // Day 31: visible again.
    now = new Date(day0.getTime() + 31 * 24 * 60 * 60 * 1000);
    await model.refresh();
    expect(model.isDismissed("zP1")).toBe(false);
    expect(model.state.promotionCandidates.map((c) => c.entity)).toEqual(["zP1", "zP3"]);
    expect(DISMISS_DAYS).toBe(30);

    // A restored dismissal (task_28-style plugin data) is honored on
    // a fresh model.
    const restored = new BriefingPanelModel(client, {
      now: () => day0,
      local: { dismissed: { zP3: day0.toISOString() } },
    });
    await restored.refresh();
    expect(restored.state.promotionCandidates.map((c) => c.entity)).toEqual(["zP1"]);
    // No dismissal was made on this model, so nothing was persisted.
    expect(restored.localState()).toEqual({ dismissed: { zP3: day0.toISOString() } });
  });

  it("needs_your_eye_picker_resolves_ambiguous_proposal_to_chosen_candidate_or_new", async () => {
    const client = fakeClient({
      "audit.query": briefingAtoms(),
      "ingest.accept": { accepted_atom_hashes: ["zh"] },
      "ingest.reject": { rejected: "sub-amb-1" },
    });
    const model = new BriefingPanelModel(client);
    await model.refresh();
    expect(model.state.needsYourEye).toHaveLength(1);

    // Choose a candidate.
    await model.resolveNeedsYourEye("sub-amb-1", "zP9");
    let accept = client.call.mock.calls.filter((c) => c[0] === "ingest.accept");
    expect(accept).toHaveLength(1);
    expect(accept[0][1]).toEqual({ submission_id: "sub-amb-1", choices: { p0: "zP9" } });
    // The resolved item leaves the list.
    expect(model.state.needsYourEye).toEqual([]);
    expect(model.state.counts.needsYourEye).toBe(0);

    // "Someone new" on a fresh model (the item is back after a refresh
    // of a model that has not resolved it).
    const model2 = new BriefingPanelModel(client);
    await model2.refresh();
    await model2.resolveNeedsYourEye("sub-amb-1", "new");
    accept = client.call.mock.calls.filter((c) => c[0] === "ingest.accept");
    expect(accept).toHaveLength(2);
    expect(accept[1][1]).toEqual({ submission_id: "sub-amb-1", choices: { p0: "new" } });
    expect(model2.state.needsYourEye).toEqual([]);

    // Reject.
    const model3 = new BriefingPanelModel(client);
    await model3.refresh();
    await model3.rejectNeedsYourEye("sub-amb-1");
    const reject = client.call.mock.calls.find((c) => c[0] === "ingest.reject");
    expect(reject![1]).toEqual({ submission_id: "sub-amb-1" });
    expect(model3.state.needsYourEye).toEqual([]);

    // Unknown submission: no RPC.
    await expect(model3.resolveNeedsYourEye("nope", "new")).rejects.toThrow(/not in the current briefing/);
  });

  it("merge_possible_duplicate_writes_same_as_and_is_undoable", async () => {
    const client = fakeClient({
      "audit.query": briefingAtoms(),
      "entity.merge": (params: unknown) => {
        const p = params as { source: string; target: string };
        return { same_as_hash: "zSameNew", source: p.source, target: p.target };
      },
      "entity.unmerge": { unmerged: "zSameNew", superseded_by: "zSup" },
    });
    const model = new BriefingPanelModel(client);
    await model.refresh();
    const pair = model.state.possibleDuplicates[0];

    // Default direction: B into A (A is the winner).
    const record = await model.merge(pair);
    const merge = client.call.mock.calls.find((c) => c[0] === "entity.merge");
    expect(merge![1]).toEqual({
      source: "zP9",
      target: "zP1",
      reason: "briefing",
      criterion: "alias_overlap",
    });
    expect(record.sameAsHash).toBe("zSameNew");
    expect(record.source).toEqual({ entity: "zP9", display: "S. Chen" });
    expect(record.target).toEqual({ entity: "zP1", display: "Sara Chen" });

    // The pair leaves the list; the merge is offered for undo.
    expect(model.state.possibleDuplicates).toEqual([]);
    expect(model.state.counts.possibleDuplicates).toBe(0);
    expect(model.state.sessionMerges.map((m) => m.sameAsHash)).toEqual(["zSameNew"]);

    // Undo calls entity.unmerge with the returned hash.
    await model.undoMerge("zSameNew");
    const unmerge = client.call.mock.calls.find((c) => c[0] === "entity.unmerge");
    expect(unmerge![1]).toEqual({ same_as_hash: "zSameNew" });
    expect(model.state.sessionMerges).toEqual([]);
    // Still not relisted until the next briefing.
    expect(model.state.possibleDuplicates).toEqual([]);

    // A merge the auditor listed is undoable too, and leaves the list.
    await model.undoMerge("zSameOld");
    const unmerges = client.call.mock.calls.filter((c) => c[0] === "entity.unmerge");
    expect(unmerges[1][1]).toEqual({ same_as_hash: "zSameOld" });
    expect(model.state.recentMerges).toEqual([]);

    // The other direction: A into B.
    const model2 = new BriefingPanelModel(client);
    await model2.refresh();
    await model2.merge(model2.state.possibleDuplicates[0], "a_into_b");
    const merges = client.call.mock.calls.filter((c) => c[0] === "entity.merge");
    expect(merges[1][1]).toEqual({
      source: "zP1",
      target: "zP9",
      reason: "briefing",
      criterion: "alias_overlap",
    });
  });

  it("keep_separate_writes_different_from_and_pair_is_not_relisted", async () => {
    const client = fakeClient({
      "audit.query": briefingAtoms(),
      "entity.assert_different": { atom_hash: "zDiff" },
    });
    const model = new BriefingPanelModel(client);
    await model.refresh();
    const pair = model.state.possibleDuplicates[0];

    await model.keepSeparate(pair);
    const call = client.call.mock.calls.find((c) => c[0] === "entity.assert_different");
    expect(call![1]).toEqual({ a: "zP1", b: "zP9", criterion: "briefing:keep_separate" });
    expect(model.state.possibleDuplicates).toEqual([]);
    expect(model.state.sessionMerges).toEqual([]);

    // A re-fetch of the same briefing does not relist the pair.
    await model.refresh();
    expect(model.state.possibleDuplicates).toEqual([]);

    // A new briefing (different hash) that still lists it does — the
    // auditor excludes different_from pairs, so this only happens if
    // the write failed, and the owner should see it again.
    client.call.mockImplementation(async (method: string) =>
      method === "audit.query" ? briefingAtoms(briefingClaim(), "zQmBriefing002") : null,
    );
    await model.refresh();
    expect(model.state.hash).toBe("zQmBriefing002");
    expect(model.state.possibleDuplicates).toHaveLength(1);
    expect(pairKey(pair.entityA, pair.entityB)).toBe(pairKey(pair.entityB, pair.entityA));
  });

  it("refuses to merge or keep separate a pair without both ids, without calling the daemon", async () => {
    const client = fakeClient({
      "audit.query": briefingAtoms(
        briefingClaim({
          possible_duplicates: [
            {
              family: "orgs",
              entity_a: { entity: "zO1", display: "Acme" },
              entity_b: { entity: null, display: "ACME Corp" },
              shared_aliases: [],
            },
          ],
        }),
      ),
    });
    const model = new BriefingPanelModel(client);
    await model.refresh();
    const pair = model.state.possibleDuplicates[0];
    await expect(model.merge(pair)).rejects.toThrow(/both entity ids/);
    await expect(model.keepSeparate(pair)).rejects.toThrow(/both entity ids/);
    expect(client.call.mock.calls.map((c) => c[0])).toEqual(["audit.query"]);
  });

  it("briefing_refreshes_on_auditor_briefing_commit", async () => {
    const client = fakeClient({ "audit.query": briefingAtoms() });
    const model = new BriefingPanelModel(client);
    await model.refresh();
    const before = client.call.mock.calls.length;

    // A daily-summary commit is the other model's business.
    client.events.emit({
      jsonrpc: "2.0",
      method: "event.atom.committed",
      params: { hash: "z", entity: "auditor", predicate: "auditor.daily_summary" },
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(client.call.mock.calls.length).toBe(before);

    const states: string[] = [];
    model.onChange((s) => states.push(s.hash));
    const frame: NotificationFrame = {
      jsonrpc: "2.0",
      method: "event.atom.committed",
      params: { hash: BRIEFING_HASH, entity: "auditor", predicate: "auditor.briefing" },
    };
    client.events.emit(frame);
    await new Promise((resolve) => setImmediate(resolve));
    const queries = client.call.mock.calls.filter((c) => c[0] === "audit.query");
    expect(queries.length).toBe(2);
    expect(queries[1][1]).toEqual({ kind: "briefing" });
    expect(states).toEqual([BRIEFING_HASH]);

    expect(client.events.listenerCount("event.atom.committed")).toBe(1);
    model.dispose();
    expect(client.events.listenerCount("event.atom.committed")).toBe(0);
  });

  it("daily_summary_model_still_calls_audit_query_without_kind", async () => {
    const client = fakeClient({
      "audit.query": (params: unknown) => {
        const p = params as Record<string, unknown>;
        // The daemon defaults `kind` to daily_summary; the daily
        // summary model must not send one.
        return p.kind === "briefing"
          ? briefingAtoms()
          : [{ hash: "zDaily", claim: { panel: [], narrative: "Daily narrative" } }];
      },
      "ingest.list_pending": [],
    });
    const summary = new SummaryPanelModel(client);
    const briefing = new BriefingPanelModel(client);
    const s = await summary.refresh();
    const b = await briefing.refresh();

    const queries = client.call.mock.calls.filter((c) => c[0] === "audit.query");
    expect(queries).toHaveLength(2);
    expect(queries[0][1]).toEqual({});
    expect("kind" in (queries[0][1] as Record<string, unknown>)).toBe(false);
    expect(queries[1][1]).toEqual({ kind: "briefing" });
    expect(s.narrative).toBe("Daily narrative");
    expect(b.narrative).toMatch(/Acme is trending/);
  });
});
