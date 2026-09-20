// Briefing section data model (task_41).
//
// The auditor publishes `auditor.briefing` atoms — a longer,
// windowed "who moved, what's new, who should I call?" view over
// the business graph — next to the five-item `auditor.daily_summary`.
// This model is a sibling of `SummaryPanelModel`:
//
// 1. Call `audit.query {kind: "briefing"}` to fetch the latest atom.
// 2. Surface every section (narrative, window, counts, and the lists)
//    for the Obsidian view to render.
// 3. Offer the owner actions the briefing asks for:
//    - Promote to contact → `ingest.submit` (a quarantined
//      `contact.person` proposal; nothing reaches the store).
//    - Dismiss → local plugin state, 30-day suppression.
//    - Needs your eye → `ingest.accept` with `choices`, or
//      `ingest.reject`.
//    - Possible duplicates → `entity.merge` / `entity.assert_different`.
//    - Undo merge → `entity.unmerge`.
// 4. Re-fetch when the daemon commits a fresh `auditor.briefing`.
//
// Nothing here imports `obsidian`; production renders the state from
// `main.ts`, tests exercise `BriefingPanelModel` directly.

import { DaemonClient } from "./client.js";
import { NotificationFrame } from "./events.js";

/** How long a dismissed promotion candidate stays hidden. */
export const DISMISS_DAYS = 30;
const DISMISS_MS = DISMISS_DAYS * 24 * 60 * 60 * 1000;

/** An entity reference as the briefing carries it: opaque id (or
 * `null` when the auditor could not resolve one) plus a display
 * string for rendering only (ADR-030). */
export interface EntityRef {
  entity: string | null;
  display: string;
}

export interface NewPerson {
  entity: string;
  display: string;
  organization: EntityRef | null;
  firstSeenArticle: EntityRef | null;
}

export interface Change {
  entity: string;
  display: string;
  kind: string;
  organization: EntityRef | null;
  from: string | null;
  to: string | null;
  /** "as reported <date>" — press reports announcements, not start dates. */
  asReported: string | null;
  sourceArticle: EntityRef | null;
}

export interface TrendingOrg {
  entity: string;
  display: string;
  mentionsThisWindow: number;
  mentionsPriorWindow: number;
}

export interface EventParticipant {
  entity: string | null;
  display: string;
  role: string | null;
}

export interface EventItem {
  entity: string;
  display: string;
  date: string | null;
  participants: EventParticipant[];
}

export interface EventGroup {
  kind: string;
  items: EventItem[];
}

export interface PromotionCandidate {
  entity: string;
  display: string;
  organization: EntityRef | null;
  mentionCount: number;
  articleCount: number;
  reason: string;
  /** Present only when the auditor carried them; the promote note
   * includes them when non-empty. */
  aliases: string[];
  /** Present only when the auditor carried one (the contract has no
   * role field; we never fetch it). */
  role: string | null;
}

export interface FollowUp {
  entity: string;
  display: string;
  organization: EntityRef | null;
  triggeringArticle: EntityRef | null;
}

export interface EyeCandidate {
  entity: string;
  display: string;
  score: number;
}

export interface NeedsYourEyeItem {
  submissionId: string;
  localRef: string;
  predicate: string;
  display: string;
  candidates: EyeCandidate[];
}

export interface PossibleDuplicate {
  family: string;
  entityA: EntityRef;
  entityB: EntityRef;
  sharedAliases: string[];
}

export interface MergeRecord {
  sameAsHash: string;
  source: EntityRef;
  target: EntityRef;
  txTime: string | null;
}

export interface BriefingCounts {
  newPeople: number;
  changes: number;
  trendingOrgs: number;
  events: number;
  promotionCandidates: number;
  followUps: number;
  needsYourEye: number;
  possibleDuplicates: number;
  recentMerges: number;
}

export interface BriefingState {
  /** True iff no `auditor.briefing` atom exists yet. */
  empty: boolean;
  /** Multibase content hash of the briefing atom ("" when empty). */
  hash: string;
  date: string;
  window: { from: string; to: string };
  cadence: string;
  generatedAt: string;
  narrative: string;
  truncated: boolean;
  ceiling: number;
  /** Vault-relative path of the rendered page (`briefings/<date>.md`). */
  pagePath: string;
  counts: BriefingCounts;
  newPeople: NewPerson[];
  changes: Change[];
  trendingOrgs: TrendingOrg[];
  events: EventGroup[];
  /** Candidates minus the ones dismissed in the last 30 days and the
   * ones already promoted from this briefing. */
  promotionCandidates: PromotionCandidate[];
  followUps: FollowUp[];
  /** Ambiguous proposals minus the ones resolved from this panel. */
  needsYourEye: NeedsYourEyeItem[];
  /** Pairs minus the ones merged / kept separate from this panel. */
  possibleDuplicates: PossibleDuplicate[];
  /** Merges the auditor listed, minus the ones undone from this panel. */
  recentMerges: MergeRecord[];
  /** Merges performed from this panel since the plugin loaded, so
   * they can be undone before the next briefing lists them. */
  sessionMerges: MergeRecord[];
  filing: { autoFiledCount: number; reviewedCount: number };
}

/** Local plugin state the briefing section persists (task_28 style —
 * plugin data, never an atom). */
export interface BriefingLocalState {
  /** Promotion candidate entity id → ISO timestamp of the dismissal. */
  dismissed: Record<string, string>;
}

export const DEFAULT_BRIEFING_LOCAL_STATE: BriefingLocalState = { dismissed: {} };

export interface BriefingModelOptions {
  /** Restored local state (dismissals). */
  local?: Partial<BriefingLocalState> | null;
  /** Called with the full local state after every change to it. */
  persist?: (state: BriefingLocalState) => void | Promise<void>;
  /** Injectable clock (tests). */
  now?: () => Date;
}

/** Rendered page for a briefing date, per the `briefings/` family. */
export function briefingPathFor(date: string): string {
  return `briefings/${date}.md`;
}

const EMPTY_COUNTS: BriefingCounts = {
  newPeople: 0,
  changes: 0,
  trendingOrgs: 0,
  events: 0,
  promotionCandidates: 0,
  followUps: 0,
  needsYourEye: 0,
  possibleDuplicates: 0,
  recentMerges: 0,
};

const EMPTY_STATE: BriefingState = {
  empty: true,
  hash: "",
  date: "",
  window: { from: "", to: "" },
  cadence: "",
  generatedAt: "",
  narrative: "No briefing yet.",
  truncated: false,
  ceiling: 0,
  pagePath: "",
  counts: EMPTY_COUNTS,
  newPeople: [],
  changes: [],
  trendingOrgs: [],
  events: [],
  promotionCandidates: [],
  followUps: [],
  needsYourEye: [],
  possibleDuplicates: [],
  recentMerges: [],
  sessionMerges: [],
  filing: { autoFiledCount: 0, reviewedCount: 0 },
};

/** The parsed atom before the per-session / per-owner filters. */
interface ParsedBriefing {
  hash: string;
  date: string;
  window: { from: string; to: string };
  cadence: string;
  generatedAt: string;
  narrative: string;
  truncated: boolean;
  ceiling: number;
  newPeople: NewPerson[];
  changes: Change[];
  trendingOrgs: TrendingOrg[];
  events: EventGroup[];
  promotionCandidates: PromotionCandidate[];
  followUps: FollowUp[];
  needsYourEye: NeedsYourEyeItem[];
  possibleDuplicates: PossibleDuplicate[];
  recentMerges: MergeRecord[];
  filing: { autoFiledCount: number; reviewedCount: number };
}

type Raw = Record<string, unknown>;

function str(v: unknown, fallback = ""): string {
  if (typeof v === "string") return v;
  if (typeof v === "number" && Number.isFinite(v)) return String(v);
  return fallback;
}

function strOrNull(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null;
}

function num(v: unknown, fallback = 0): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

function list(v: unknown): Raw[] {
  if (!Array.isArray(v)) return [];
  return v.filter((x): x is Raw => typeof x === "object" && x !== null);
}

function strings(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return v.filter((x): x is string => typeof x === "string" && x.length > 0);
}

function ref(v: unknown): EntityRef | null {
  if (typeof v !== "object" || v === null) return null;
  const r = v as Raw;
  const display = str(r.display ?? r.display_name);
  const entity = strOrNull(r.entity);
  if (!display && !entity) return null;
  return { entity, display: display || (entity ?? "") };
}

function refOrEmpty(v: unknown): EntityRef {
  return ref(v) ?? { entity: null, display: "" };
}

/** Parse one `audit.query {kind: "briefing"}` row. Every list may be
 * missing on older atoms; every field falls back to a neutral value. */
export function parseBriefingAtom(atom: unknown): ParsedBriefing | null {
  if (typeof atom !== "object" || atom === null) return null;
  const a = atom as Raw;
  const claim = (typeof a.claim === "object" && a.claim !== null ? a.claim : {}) as Raw;
  const window = (typeof claim.window === "object" && claim.window !== null ? claim.window : {}) as Raw;
  const filing = (typeof claim.filing === "object" && claim.filing !== null ? claim.filing : {}) as Raw;
  const generatedAt = str(claim.generated_at, str(a.tx_time));
  const date = str(claim.date, generatedAt.slice(0, 10));
  return {
    hash: str(a.hash),
    date,
    window: { from: str(window.from), to: str(window.to) },
    cadence: str(claim.cadence),
    generatedAt,
    narrative: str(claim.narrative),
    truncated: claim.truncated === true,
    ceiling: num(claim.ceiling),
    newPeople: list(claim.new_people)
      .map((p) => ({
        entity: str(p.entity),
        display: str(p.display ?? p.display_name, str(p.entity)),
        organization: ref(p.organization),
        firstSeenArticle: ref(p.first_seen_article),
      }))
      .filter((p) => p.entity.length > 0 || p.display.length > 0),
    changes: list(claim.changes).map((c) => ({
      entity: str(c.entity),
      display: str(c.display ?? c.display_name, str(c.entity)),
      kind: str(c.kind, "changed"),
      organization: ref(c.organization),
      from: strOrNull(c.from),
      to: strOrNull(c.to),
      asReported: strOrNull(c.as_reported),
      sourceArticle: ref(c.source_article),
    })),
    trendingOrgs: list(claim.trending_orgs).map((o) => ({
      entity: str(o.entity),
      display: str(o.display ?? o.display_name, str(o.entity)),
      mentionsThisWindow: num(o.mentions_this_window),
      mentionsPriorWindow: num(o.mentions_prior_window),
    })),
    events: list(claim.events).map((g) => ({
      kind: str(g.kind, "event"),
      items: list(g.items).map((it) => ({
        entity: str(it.entity),
        display: str(it.display ?? it.display_name, str(it.entity)),
        date: strOrNull(it.date),
        participants: list(it.participants).map((p) => ({
          entity: strOrNull(p.entity),
          display: str(p.display ?? p.display_name),
          role: strOrNull(p.role),
        })),
      })),
    })),
    promotionCandidates: list(claim.promotion_candidates)
      .map((c) => ({
        entity: str(c.entity),
        display: str(c.display ?? c.display_name, str(c.entity)),
        organization: ref(c.organization),
        mentionCount: num(c.mention_count),
        articleCount: num(c.article_count),
        reason: str(c.reason),
        aliases: strings(c.aliases),
        role: strOrNull(c.role),
      }))
      .filter((c) => c.entity.length > 0),
    followUps: list(claim.follow_ups).map((f) => ({
      entity: str(f.entity),
      display: str(f.display ?? f.display_name, str(f.entity)),
      organization: ref(f.organization),
      triggeringArticle: ref(f.triggering_article),
    })),
    needsYourEye: list(claim.needs_your_eye)
      .map((n) => ({
        submissionId: str(n.submission_id),
        localRef: str(n.local_ref),
        predicate: str(n.predicate),
        display: str(n.display ?? n.display_name),
        candidates: list(n.candidates).map((c) => ({
          entity: str(c.entity),
          display: str(c.display ?? c.display_name, str(c.entity)),
          score: num(c.score),
        })),
      }))
      .filter((n) => n.submissionId.length > 0),
    possibleDuplicates: list(claim.possible_duplicates).map((d) => ({
      family: str(d.family),
      entityA: refOrEmpty(d.entity_a),
      entityB: refOrEmpty(d.entity_b),
      sharedAliases: strings(d.shared_aliases),
    })),
    recentMerges: list(claim.recent_merges)
      .map((m) => ({
        sameAsHash: str(m.same_as_hash),
        source: refOrEmpty(m.source),
        target: refOrEmpty(m.target),
        txTime: strOrNull(m.tx_time),
      }))
      .filter((m) => m.sameAsHash.length > 0),
    filing: {
      autoFiledCount: num(filing.auto_filed_count ?? claim.auto_filed_count),
      reviewedCount: num(filing.reviewed_count ?? claim.reviewed_count),
    },
  };
}

/** Quote a YAML scalar when a bare value would be misread. */
function yamlScalar(v: string): string {
  const needsQuote =
    v.length === 0 ||
    /[:\[\]{}#&*!|>'"%@`,]/.test(v) ||
    /^\s|\s$/.test(v) ||
    /^(true|false|null|yes|no|~)$/i.test(v) ||
    /^[-+]?\d/.test(v);
  if (!needsQuote) return v;
  return `"${v.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/**
 * The markdown note submitted for a promotion: a frontmatter note the
 * scribe turns into a `contact.person` proposal. `role:` is written
 * only when the candidate carries one (the briefing contract has none
 * and we never fetch it); `aliases:` only when non-empty.
 */
export function promoteNote(candidate: PromotionCandidate, briefingDate: string): string {
  const lines = ["---", "predicate: contact.person", `name: ${yamlScalar(candidate.display)}`];
  const org = candidate.organization?.display;
  if (org) lines.push(`organization: ${yamlScalar(org)}`);
  if (candidate.role) lines.push(`role: ${yamlScalar(candidate.role)}`);
  if (candidate.aliases.length > 0) {
    lines.push(`aliases: [${candidate.aliases.map(yamlScalar).join(", ")}]`);
  }
  lines.push("---", "", `Promoted from the briefing of ${briefingDate || "today"}.`, "");
  return lines.join("\n");
}

/** Provenance URI for a promotion: `ffs://briefing/<hash>/promote/<entity>`. */
export function promoteSourceUri(briefingHash: string, entity: string): string {
  return `ffs://briefing/${briefingHash}/promote/${entity}`;
}

/** Order-independent key for a duplicate pair. */
export function pairKey(a: EntityRef, b: EntityRef): string {
  const x = a.entity ?? a.display;
  const y = b.entity ?? b.display;
  return x < y ? `${x}|${y}` : `${y}|${x}`;
}

/** Human-readable line for a change: "Sara Chen joined Acme (CTO) — as reported 2026-09-12". */
export function changeLine(c: Change): string {
  const org = c.organization?.display;
  let text: string;
  switch (c.kind) {
    case "joined":
      text = `${c.display} joined${org ? ` ${org}` : ""}${c.to ? ` as ${c.to}` : ""}`;
      break;
    case "left":
      text = `${c.display} left${org ? ` ${org}` : ""}${c.from ? ` (${c.from})` : ""}`;
      break;
    case "retitled":
      text = `${c.display}${org ? ` at ${org}` : ""}: ${c.from ?? "?"} → ${c.to ?? "?"}`;
      break;
    case "org_changed":
      text = `${c.display}: ${c.from ?? "?"} → ${c.to ?? "?"}`;
      break;
    default:
      text = `${c.display} ${c.kind}${org ? ` (${org})` : ""}`;
  }
  return c.asReported ? `${text} — as reported ${c.asReported}` : text;
}

/** Label for a candidate button in the "Needs your eye" picker. */
export function eyeCandidateLabel(c: EyeCandidate): string {
  return `Accept as ${c.display} (${c.score.toFixed(1)})`;
}

export class BriefingPanelModel {
  state: BriefingState = EMPTY_STATE;
  private listeners: Array<(s: BriefingState) => void> = [];
  private boundOnAtomCommitted: (frame: NotificationFrame) => void;
  private local: BriefingLocalState;
  private now: () => Date;
  private persist?: (state: BriefingLocalState) => void | Promise<void>;
  /** The most recently parsed atom (before filters). */
  private parsed: ParsedBriefing | null = null;
  /** Per-briefing bookkeeping: which items the owner already acted on
   * from this panel. Reset when a new briefing hash arrives. */
  private handledHash = "";
  private promoted = new Set<string>();
  private resolvedSubmissions = new Set<string>();
  private handledPairs = new Set<string>();
  private undoneMerges = new Set<string>();
  private sessionMerges: MergeRecord[] = [];

  constructor(
    private client: Pick<DaemonClient, "call" | "events">,
    opts: BriefingModelOptions = {},
  ) {
    this.local = {
      dismissed: { ...(opts.local?.dismissed ?? {}) },
    };
    this.persist = opts.persist;
    this.now = opts.now ?? (() => new Date());
    this.boundOnAtomCommitted = this.onAtomCommitted.bind(this);
    this.client.events.on("event.atom.committed", this.boundOnAtomCommitted);
  }

  /** Stop receiving `event.atom.committed` notifications. */
  dispose(): void {
    this.client.events.off("event.atom.committed", this.boundOnAtomCommitted);
  }

  onChange(fn: (s: BriefingState) => void): () => void {
    this.listeners.push(fn);
    return () => {
      const idx = this.listeners.indexOf(fn);
      if (idx >= 0) this.listeners.splice(idx, 1);
    };
  }

  /** Snapshot of the persisted local state (dismissals). */
  localState(): BriefingLocalState {
    return { dismissed: { ...this.local.dismissed } };
  }

  /** Re-fetch the latest briefing atom (`audit.query {kind: "briefing"}`). */
  async refresh(): Promise<BriefingState> {
    const atoms = await this.client.call("audit.query", { kind: "briefing" });
    const latest = Array.isArray(atoms) && atoms.length > 0 ? atoms[0] : null;
    this.parsed = latest === null ? null : parseBriefingAtom(latest);
    if (this.parsed && this.parsed.hash !== this.handledHash) {
      // A new briefing: the per-briefing action bookkeeping starts
      // over. Merges done in this session stay undoable until the
      // next briefing lists them itself.
      this.handledHash = this.parsed.hash;
      this.promoted.clear();
      this.resolvedSubmissions.clear();
      this.handledPairs.clear();
      this.undoneMerges.clear();
      this.sessionMerges = this.sessionMerges.filter(
        (m) => !this.parsed!.recentMerges.some((r) => r.sameAsHash === m.sameAsHash),
      );
    }
    this.rebuild();
    return this.state;
  }

  /**
   * Promote to contact: submit a `contact.person` proposal built from
   * the candidate into the ingest quarantine with provenance pointing
   * at the briefing atom. Returns the submission id. The proposal
   * shows up as a pending card in the daily summary; nothing reaches
   * the store until the owner accepts it there.
   */
  async promote(entity: string): Promise<string> {
    const candidate = this.parsed?.promotionCandidates.find((c) => c.entity === entity);
    if (!candidate) throw new Error(`no promotion candidate ${entity} in the current briefing`);
    const result = (await this.client.call("ingest.submit", {
      source_uri: promoteSourceUri(this.parsed?.hash ?? "", entity),
      content: promoteNote(candidate, this.parsed?.date ?? ""),
    })) as { submission_id?: string } | null;
    this.promoted.add(entity);
    this.rebuild();
    return String(result?.submission_id ?? "");
  }

  /** Dismiss a promotion candidate for 30 days (local state, not an atom). */
  async dismiss(entity: string): Promise<void> {
    this.local.dismissed[entity] = this.now().toISOString();
    this.pruneDismissals();
    await this.persist?.(this.localState());
    this.rebuild();
  }

  /** True iff the entity was dismissed less than 30 days ago. */
  isDismissed(entity: string): boolean {
    const at = this.local.dismissed[entity];
    if (!at) return false;
    const t = Date.parse(at);
    if (Number.isNaN(t)) return false;
    return this.now().getTime() - t < DISMISS_MS;
  }

  /**
   * Needs your eye: resolve an ambiguous proposal to a chosen
   * candidate (`choice` = entity id) or to "someone new"
   * (`choice` = "new"). Calls `ingest.accept` with `choices`.
   */
  async resolveNeedsYourEye(submissionId: string, choice: string | "new"): Promise<void> {
    const item = this.parsed?.needsYourEye.find((n) => n.submissionId === submissionId);
    if (!item) throw new Error(`submission ${submissionId} is not in the current briefing`);
    await this.client.call("ingest.accept", {
      submission_id: submissionId,
      choices: { [item.localRef]: choice },
    });
    this.resolvedSubmissions.add(submissionId);
    this.rebuild();
  }

  /** Needs your eye: reject the ambiguous proposal outright. */
  async rejectNeedsYourEye(submissionId: string): Promise<void> {
    await this.client.call("ingest.reject", { submission_id: submissionId });
    this.resolvedSubmissions.add(submissionId);
    this.rebuild();
  }

  /**
   * Merge a possible-duplicate pair: `loser` redirects to `winner`
   * through an owner-signed `entity.same_as` atom (`entity.merge`).
   * The returned `same_as_hash` is kept so the merge can be undone
   * from this panel. Returns the merge record.
   */
  async merge(pair: PossibleDuplicate, direction: "b_into_a" | "a_into_b" = "b_into_a"): Promise<MergeRecord> {
    const [source, target] =
      direction === "b_into_a" ? [pair.entityB, pair.entityA] : [pair.entityA, pair.entityB];
    if (!source.entity || !target.entity) {
      throw new Error("cannot merge a pair without both entity ids");
    }
    const result = (await this.client.call("entity.merge", {
      source: source.entity,
      target: target.entity,
      reason: "briefing",
      criterion: "alias_overlap",
    })) as { same_as_hash?: string; source?: string; target?: string } | null;
    const record: MergeRecord = {
      sameAsHash: String(result?.same_as_hash ?? ""),
      source: { entity: String(result?.source ?? source.entity), display: source.display },
      target: { entity: String(result?.target ?? target.entity), display: target.display },
      txTime: this.now().toISOString(),
    };
    this.handledPairs.add(pairKey(pair.entityA, pair.entityB));
    if (record.sameAsHash) this.sessionMerges.unshift(record);
    this.rebuild();
    return record;
  }

  /** Keep separate: owner-signed `entity.different_from` so the pair
   * is never suggested again and the resolver never auto-links them. */
  async keepSeparate(pair: PossibleDuplicate): Promise<void> {
    if (!pair.entityA.entity || !pair.entityB.entity) {
      throw new Error("cannot keep separate a pair without both entity ids");
    }
    await this.client.call("entity.assert_different", {
      a: pair.entityA.entity,
      b: pair.entityB.entity,
      criterion: "briefing:keep_separate",
    });
    this.handledPairs.add(pairKey(pair.entityA, pair.entityB));
    this.rebuild();
  }

  /** Undo a merge by superseding its `entity.same_as` atom (`entity.unmerge`). */
  async undoMerge(sameAsHash: string): Promise<void> {
    await this.client.call("entity.unmerge", { same_as_hash: sameAsHash });
    this.undoneMerges.add(sameAsHash);
    this.sessionMerges = this.sessionMerges.filter((m) => m.sameAsHash !== sameAsHash);
    this.rebuild();
  }

  // ---- internal ----

  private pruneDismissals(): void {
    for (const [entity, at] of Object.entries(this.local.dismissed)) {
      const t = Date.parse(at);
      if (Number.isNaN(t) || this.now().getTime() - t >= DISMISS_MS) {
        delete this.local.dismissed[entity];
      }
    }
  }

  /** Derive the visible state from the parsed atom plus the filters. */
  private rebuild(): void {
    const p = this.parsed;
    if (!p) {
      this.setState({ ...EMPTY_STATE, sessionMerges: [...this.sessionMerges] });
      return;
    }
    const promotionCandidates = p.promotionCandidates.filter(
      (c) => !this.promoted.has(c.entity) && !this.isDismissed(c.entity),
    );
    const needsYourEye = p.needsYourEye.filter((n) => !this.resolvedSubmissions.has(n.submissionId));
    const possibleDuplicates = p.possibleDuplicates.filter(
      (d) => !this.handledPairs.has(pairKey(d.entityA, d.entityB)),
    );
    const recentMerges = p.recentMerges.filter((m) => !this.undoneMerges.has(m.sameAsHash));
    this.setState({
      empty: false,
      hash: p.hash,
      date: p.date,
      window: p.window,
      cadence: p.cadence,
      generatedAt: p.generatedAt,
      narrative: p.narrative,
      truncated: p.truncated,
      ceiling: p.ceiling,
      pagePath: p.date ? briefingPathFor(p.date) : "",
      counts: {
        newPeople: p.newPeople.length,
        changes: p.changes.length,
        trendingOrgs: p.trendingOrgs.length,
        events: p.events.reduce((n, g) => n + g.items.length, 0),
        promotionCandidates: promotionCandidates.length,
        followUps: p.followUps.length,
        needsYourEye: needsYourEye.length,
        possibleDuplicates: possibleDuplicates.length,
        recentMerges: recentMerges.length,
      },
      newPeople: p.newPeople,
      changes: p.changes,
      trendingOrgs: p.trendingOrgs,
      events: p.events,
      promotionCandidates,
      followUps: p.followUps,
      needsYourEye,
      possibleDuplicates,
      recentMerges,
      sessionMerges: [...this.sessionMerges],
      filing: p.filing,
    });
  }

  private setState(next: BriefingState): void {
    this.state = next;
    for (const fn of this.listeners) {
      try {
        fn(next);
      } catch (err) {
        console.error("[ffs] briefing listener threw:", err);
      }
    }
  }

  /** Refresh only on `auditor.briefing` commits — the daily summary
   * has its own model with its own hook. */
  private onAtomCommitted(frame: NotificationFrame): void {
    if (frame.params?.predicate === "auditor.briefing") {
      void this.refresh().catch((err) => {
        console.warn("[ffs] briefing refresh on commit failed:", err);
      });
    }
  }
}
