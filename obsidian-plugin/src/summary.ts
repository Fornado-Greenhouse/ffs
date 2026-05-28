// Daily-health-summary panel data model.
//
// The auditor (Rust task_13) publishes `auditor.daily_summary` atoms
// with a structured `claim.panel` field already truncated to ≤5
// items by priority. The plugin's job here is to:
//
// 1. Call `audit.query` to fetch the latest summary atom.
// 2. Surface the panel items (display name, message, kind) for the
//    Obsidian view to render.
// 3. Surface the pending-proposal queue (via `ingest.list_pending`)
//    with accept/reject controls that route through
//    `ingest.accept` / `ingest.reject`.
// 4. Re-fetch when the daemon publishes a fresh auditor atom via
//    `event.atom.committed`.
//
// Production renders the data into an Obsidian custom view; tests
// exercise `SummaryPanelModel` directly.

import { DaemonClient } from "./client.js";
import { NotificationFrame } from "./events.js";

export const MAX_PANEL_ITEMS = 5;

export interface PanelItem {
  /** Priority order — lower is higher priority (1 sorts first). */
  priority: number;
  /** Flag kind: capability_denials, federation_unhealthy, drift, etc. */
  kind: string;
  /** Human-readable message the auditor produced. */
  message: string;
}

export interface ProposalItem {
  submissionId: string;
  sourceUri: string;
  /** Number of proposals in the submission (display only). */
  proposalCount: number;
}

export interface PanelState {
  /** Top-N flags from the latest auditor.daily_summary atom. */
  items: PanelItem[];
  /** Narrative text the auditor produced (single string). */
  narrative: string;
  /** Pending submissions awaiting accept/reject. */
  pendingProposals: ProposalItem[];
  /** True iff the latest fetch surfaced no auditor atom yet. */
  empty: boolean;
}

const EMPTY_STATE: PanelState = {
  items: [],
  narrative: "No auditor summary yet.",
  pendingProposals: [],
  empty: true,
};

export class SummaryPanelModel {
  state: PanelState = EMPTY_STATE;
  private listeners: Array<(s: PanelState) => void> = [];
  private boundOnAtomCommitted: (frame: NotificationFrame) => void;

  constructor(private client: Pick<DaemonClient, "call" | "events">) {
    this.boundOnAtomCommitted = this.onAtomCommitted.bind(this);
    this.client.events.on(
      "event.atom.committed",
      this.boundOnAtomCommitted,
    );
  }

  /**
   * Stop receiving `event.atom.committed` notifications. Call when
   * the panel view is unmounted.
   */
  dispose(): void {
    this.client.events.off(
      "event.atom.committed",
      this.boundOnAtomCommitted,
    );
  }

  /** Subscribe to state-change notifications. */
  onChange(fn: (s: PanelState) => void): void {
    this.listeners.push(fn);
  }

  /**
   * Re-fetch the summary atom + the pending-proposals queue. Returns
   * the new state so callers can render it without waiting on a
   * separate `onChange` callback.
   */
  async refresh(): Promise<PanelState> {
    const atoms = (await this.client.call("audit.query", {})) as Array<{
      claim?: {
        panel?: PanelItem[];
        narrative?: string;
      };
    }>;
    const latest = Array.isArray(atoms) && atoms.length > 0 ? atoms[0] : null;
    const panelItemsRaw = latest?.claim?.panel ?? [];
    const items = (Array.isArray(panelItemsRaw) ? panelItemsRaw : [])
      .slice(0, MAX_PANEL_ITEMS)
      .map((it) => ({
        priority: Number(it?.priority ?? 99),
        kind: String(it?.kind ?? "unknown"),
        message: String(it?.message ?? ""),
      }));

    const pending = (await this.client.call(
      "ingest.list_pending",
      {},
    )) as Array<{ id?: string; source_uri?: string; proposals?: unknown[] }>;
    const pendingProposals: ProposalItem[] = (Array.isArray(pending) ? pending : [])
      .map((sub) => ({
        submissionId: String(sub?.id ?? ""),
        sourceUri: String(sub?.source_uri ?? ""),
        proposalCount: Array.isArray(sub?.proposals) ? sub.proposals.length : 0,
      }))
      .filter((it) => it.submissionId.length > 0);

    const next: PanelState = {
      items,
      narrative: String(latest?.claim?.narrative ?? EMPTY_STATE.narrative),
      pendingProposals,
      empty: latest === null && pendingProposals.length === 0,
    };
    this.setState(next);
    return next;
  }

  /** Accept a pending proposal — calls `ingest.accept`. */
  async accept(submissionId: string): Promise<void> {
    await this.client.call("ingest.accept", { submission_id: submissionId });
    await this.refresh();
  }

  /** Reject a pending proposal — calls `ingest.reject`. */
  async reject(submissionId: string): Promise<void> {
    await this.client.call("ingest.reject", { submission_id: submissionId });
    await this.refresh();
  }

  private setState(next: PanelState): void {
    this.state = next;
    for (const fn of this.listeners) {
      try {
        fn(next);
      } catch (err) {
        console.error("[ffs] panel listener threw:", err);
      }
    }
  }

  /**
   * `event.atom.committed` handler: only re-fetch when the
   * committed atom's predicate is `auditor.daily_summary` so we
   * don't thrash on every fast-path supersession.
   */
  private onAtomCommitted(frame: NotificationFrame): void {
    const predicate = frame.params?.predicate;
    if (predicate === "auditor.daily_summary") {
      void this.refresh().catch((err) => {
        console.warn("[ffs] summary refresh on commit failed:", err);
      });
    }
  }
}
