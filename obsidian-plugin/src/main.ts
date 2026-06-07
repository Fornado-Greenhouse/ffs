// FFS Obsidian plugin entrypoint.
//
// Owns the Obsidian-runtime surface: the daemon client lifecycle,
// the status-bar indicator, the daily-summary ItemView, the
// entity-search SuggestModal, the file-open hook that sets the
// currently-open projection, and the settings tab. The pure-logic
// halves (DaemonClient, SummaryPanelModel, EntitySearch) live in
// their own files so they can be unit-tested against a mocked
// `obsidian` module; only this file imports the real runtime.

import {
  App,
  ItemView,
  Notice,
  Plugin,
  PluginSettingTab,
  Setting,
  SuggestModal,
  WorkspaceLeaf,
  TFile,
  type PluginManifest,
} from "obsidian";
import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { existsSync } from "node:fs";

import { DaemonClient } from "./client.js";
import { enumerateFolder, decorateProjectionFile } from "./folder.js";
import { ProjectionSubscription, renderProjection } from "./projection.js";
import { applyOptimistically, routeEdit } from "./editing.js";
import { EntitySearch, type EntityHit } from "./search.js";
import {
  DEFAULT_SETTINGS,
  FfsPluginSettings,
  FfsSettingTab,
} from "./settings.js";
import {
  SummaryPanelModel,
  type PanelState,
  type ProposalItem,
  type ProposalPreview,
  type PanelItem,
} from "./summary.js";

export const SUMMARY_VIEW_TYPE = "ffs-daily-summary";

export default class FfsPlugin extends Plugin {
  settings: FfsPluginSettings = DEFAULT_SETTINGS;
  client!: DaemonClient;
  private statusEl: HTMLElement | null = null;
  /** Live-update subscription for the currently-open projection. */
  private projectionSub: ProjectionSubscription | null = null;
  /** Daily-health-summary panel model (task_19). */
  summary!: SummaryPanelModel;
  /** Entity-name search backing the suggester modal (task_19). */
  search!: EntitySearch;
  /** Latest results from the entity-search model — drained by the modal. */
  private searchResultsForModal: EntityHit[] = [];
  /** Resolves the next search-results promise so the modal can await it. */
  private resolveSearchResults: ((hits: EntityHit[]) => void) | null = null;
  /**
   * Exposed for downstream task work so plugin subsystems can call
   * into the read/edit pipeline without re-importing the daemon
   * client.
   */
  enumerateFolder = (path: string, page = 0) =>
    enumerateFolder(this.client, path, page);
  renderProjection = (path: string) => renderProjection(this.client, path);
  routeEdit = (path: string, oldText: string, newText: string) =>
    routeEdit(this.client, path, oldText, newText);
  applyOptimistically = applyOptimistically;
  decorateProjectionFile = decorateProjectionFile;

  constructor(app: App, manifest: PluginManifest) {
    super(app, manifest);
  }

  async onload(): Promise<void> {
    await this.loadSettings();

    const resolvedSocket = expandTilde(this.settings.socketPath);
    const resolvedCli = resolveCliPath(this.settings.cliPath);

    this.client = new DaemonClient({
      socketPath: resolvedSocket,
      cliPath: resolvedCli,
      spawnSubprocess: (bin, args, input) => spawnAsync(bin, args, input),
    });

    this.statusEl = this.addStatusBarItem();
    this.renderStatus("disconnected");
    this.client.onStateChange((state) => this.renderStatus(state));
    this.client.start();

    // Live-update wiring: when the daemon publishes a projection
    // invalidation for the currently-open file, re-render the
    // buffer.
    this.projectionSub = new ProjectionSubscription(
      this.client.events,
      (path) => {
        void this.handleInvalidation(path);
      },
    );

    // Summary panel model — wires `audit.query` + `ingest.list_pending`
    // and refreshes on `event.atom.committed` for auditor atoms.
    this.summary = new SummaryPanelModel(this.client);

    // Entity-name search backing the suggester. The callback both
    // updates the in-flight modal (if any) and stores the latest
    // results for next-modal-open.
    this.search = new EntitySearch(this.client, (hits) => {
      this.searchResultsForModal = hits;
      this.resolveSearchResults?.(hits);
    });

    // Register the right-sidebar summary view.
    this.registerView(SUMMARY_VIEW_TYPE, (leaf) => new SummaryView(leaf, this));

    // Ribbon icon to open the summary panel. Obsidian's icon set
    // is Lucide-derived; `lucide-list-checks` reads as a daily-
    // review affordance.
    this.addRibbonIcon("list-checks", "FFS daily summary", () => {
      void this.activateSummaryView();
    });

    // File-open hook: tell the projection-invalidation subscription
    // which path we currently care about, so daemon events for the
    // open file re-render this buffer.
    this.registerEvent(
      this.app.workspace.on("file-open", (file: TFile | null) => {
        this.setOpenProjection(file?.path ?? null);
      }),
    );

    this.addCommand({
      id: "ffs-refresh-summary",
      name: "Refresh daily health summary",
      callback: () => {
        void this.summary
          .refresh()
          .then(() => this.activateSummaryView())
          .catch((err) => {
            console.warn("[ffs] summary refresh failed:", err);
          });
      },
    });
    this.addCommand({
      id: "ffs-open-summary",
      name: "Open daily summary panel",
      callback: () => {
        void this.activateSummaryView();
      },
    });
    this.addCommand({
      id: "ffs-focus-entity-search",
      name: "Search FFS entities by name…",
      callback: () => {
        new EntitySearchModal(this.app, this).open();
      },
    });

    this.addSettingTab(new FfsSettingsTabImpl(this.app, this));

    // On startup, open the summary panel in the right sidebar so
    // the user doesn't have to discover the command first.
    this.app.workspace.onLayoutReady(() => {
      void this.activateSummaryView();
    });
  }

  async onunload(): Promise<void> {
    this.search?.cancel();
    this.summary?.dispose();
    this.projectionSub?.dispose();
    this.projectionSub = null;
    this.client?.close();
  }

  /**
   * Update the currently-open projection so the invalidation
   * subscription knows which path to refresh on.
   */
  setOpenProjection(path: string | null): void {
    this.projectionSub?.setCurrent(path);
  }

  /** Open (or reveal) the summary view in the right sidebar. */
  async activateSummaryView(): Promise<void> {
    const workspace = this.app.workspace;
    let leaf = workspace.getLeavesOfType(SUMMARY_VIEW_TYPE)[0];
    if (!leaf) {
      const right = workspace.getRightLeaf(false);
      if (!right) return;
      await right.setViewState({ type: SUMMARY_VIEW_TYPE, active: true });
      leaf = right;
    }
    workspace.revealLeaf(leaf);
  }

  /**
   * Drain pending search results — the modal awaits this to get
   * the next-arriving callback's payload.
   */
  awaitNextSearchResults(): Promise<EntityHit[]> {
    return new Promise<EntityHit[]>((resolve) => {
      this.resolveSearchResults = resolve;
    });
  }

  private async handleInvalidation(path: string): Promise<void> {
    // Re-render the open buffer by re-querying the projection.
    // Production wiring replaces the active leaf's editor content
    // when the open file matches; for MVP we surface the event so
    // the user knows the substrate noticed an external change.
    console.info("[ffs] projection invalidated, re-rendering:", path);
    try {
      const rendered = await this.renderProjection(path);
      if (!rendered) return;
      const file = this.app.vault.getAbstractFileByPath(path);
      if (file instanceof TFile) {
        await this.app.vault.modify(file, rendered.markdown);
      }
    } catch (err) {
      console.warn("[ffs] re-render failed:", err);
    }
  }

  async loadSettings(): Promise<void> {
    const loaded = (await this.loadData()) as Partial<FfsPluginSettings> | null;
    this.settings = { ...DEFAULT_SETTINGS, ...(loaded ?? {}) };
  }

  async saveSettings(): Promise<void> {
    await this.saveData(this.settings);
  }

  private renderStatus(state: string): void {
    if (!this.statusEl) return;
    // Build the status-bar item as DOM so we can color just the
    // bullet character without colorizing the whole text.
    this.statusEl.empty();
    this.statusEl.addClass("ffs-statusbar");
    this.statusEl.createSpan({ text: "FFS " });
    this.statusEl.createSpan({
      cls: `ffs-conn-dot ffs-conn-dot-${state}`,
      text: connStateBullet(state),
    });
    this.statusEl.createSpan({ text: " " + connStateText(state) });
  }
}

/**
 * The daily-summary right-sidebar view. Owns the DOM rendering;
 * data comes from `plugin.summary` (the model, which talks to the
 * daemon).
 */
class SummaryView extends ItemView {
  /** Wall-clock of the last successful refresh; displayed in the header. */
  private lastRefreshedAt: Date | null = null;
  /** Current connection state, mirrored from the daemon client. */
  private connState = "disconnected";
  /** Set on the first time we observe a connected client to fire the
   * initial refresh exactly once. */
  private didInitialRefresh = false;
  /** Unsubscribe handles for the listeners this view registers. Both
   * are invoked from `onClose` so listeners don't accumulate across
   * view open/close cycles (task_28). */
  private offSummaryChange: (() => void) | null = null;
  private offStateChange: (() => void) | null = null;

  constructor(leaf: WorkspaceLeaf, private plugin: FfsPlugin) {
    super(leaf);
  }

  getViewType(): string {
    return SUMMARY_VIEW_TYPE;
  }
  getDisplayText(): string {
    return "FFS daily summary";
  }
  getIcon(): string {
    return "list-checks";
  }

  async onOpen(): Promise<void> {
    this.offSummaryChange = this.plugin.summary.onChange((state) =>
      this.render(state),
    );

    // Track the client's connection state. The first time we land
    // on "connected" (or "fallback"), trigger the initial refresh
    // — this dodges the race where onOpen runs before the UDS
    // handshake completes.
    const handleState = (state: string) => {
      this.connState = state;
      if (
        !this.didInitialRefresh &&
        (state === "connected" || state === "fallback")
      ) {
        this.didInitialRefresh = true;
        void this.triggerRefresh();
      } else {
        // Re-render the panel chrome so the connecting/offline
        // state line updates even before the data lands.
        this.render(this.plugin.summary.state);
      }
    };
    this.offStateChange = this.plugin.client.onStateChange(handleState);

    // Seed the panel chrome immediately so the user sees
    // something instead of a blank pane.
    handleState(this.connState);
  }

  async onClose(): Promise<void> {
    this.offSummaryChange?.();
    this.offSummaryChange = null;
    this.offStateChange?.();
    this.offStateChange = null;
  }

  private async triggerRefresh(): Promise<void> {
    try {
      await this.plugin.summary.refresh();
      this.lastRefreshedAt = new Date();
      // The model's setState will have already called this.render
      // via the onChange listener — but call again to update the
      // timestamp line.
      this.render(this.plugin.summary.state);
    } catch (err) {
      new Notice(`FFS summary refresh failed: ${describeError(err)}`);
      this.render(this.plugin.summary.state);
    }
  }

  private render(state: PanelState): void {
    const root = this.containerEl.children[1] as HTMLElement;
    root.empty();
    root.addClass("ffs-summary-root");

    const header = root.createDiv({ cls: "ffs-summary-header" });
    header.createEl("h3", { text: "Daily summary" });
    const refresh = header.createEl("button", {
      text: "↻",
      cls: "ffs-icon-button",
    });
    refresh.setAttr("aria-label", "Refresh");
    refresh.setAttr("title", "Refresh");
    refresh.onclick = () => {
      void this.triggerRefresh();
    };

    // Status line: connection state + last-refreshed timestamp +
    // last-commit timestamp. Without these signals, a click on
    // Refresh against an empty substrate looks like the button is
    // broken; the last-commit line gives the user "yes, the
    // substrate is live" confirmation without opening the dev
    // console.
    const status = root.createDiv({ cls: "ffs-summary-status" });
    const connBadge = status.createSpan({
      cls: `ffs-summary-conn ffs-summary-conn-${this.connState}`,
    });
    connBadge.createSpan({
      cls: `ffs-conn-dot ffs-conn-dot-${this.connState}`,
      text: connStateBullet(this.connState),
    });
    connBadge.appendText(" " + connStateText(this.connState));
    if (this.lastRefreshedAt) {
      status.createSpan({
        cls: "ffs-summary-refreshed",
        text: ` · refreshed ${formatClock(this.lastRefreshedAt)}`,
      });
    }
    if (state.lastCommittedAt) {
      status.createSpan({
        cls: "ffs-summary-last-commit",
        text: ` · last commit ${formatClock(state.lastCommittedAt)}`,
      });
    }

    if (state.empty && state.pendingProposals.length === 0) {
      const empty = root.createDiv({ cls: "ffs-summary-empty" });
      empty.createEl("p", {
        text: "Nothing to review yet. Capture a contact or note in ingest/ to start.",
      });
      return;
    }

    // Narrative paragraph from the latest auditor atom.
    if (state.narrative) {
      root.createEl("p", {
        text: state.narrative,
        cls: "ffs-summary-narrative",
      });
    }

    // Pending proposals — accept / reject inline.
    if (state.pendingProposals.length > 0) {
      root.createEl("h4", { text: `Recent proposals · ${state.pendingProposals.length}` });
      const list = root.createDiv({ cls: "ffs-proposal-list" });
      for (const p of state.pendingProposals) {
        this.renderProposal(list, p);
      }
    }

    // Flags from the auditor's `claim.panel`.
    if (state.items.length > 0) {
      root.createEl("h4", { text: `Flags · ${state.items.length}` });
      const flags = root.createEl("ul", { cls: "ffs-flag-list" });
      for (const item of state.items) {
        this.renderFlag(flags, item);
      }
    }
  }

  private renderProposal(parent: HTMLElement, p: ProposalItem): void {
    const row = parent.createDiv({ cls: "ffs-proposal" });

    // Clickable summary header. Toggles the `.is-expanded` class
    // on the card so CSS can show/hide the body. The Accept/Reject
    // buttons live outside the clickable area so a button click
    // doesn't bubble into a card-collapse.
    const summary = row.createDiv({ cls: "ffs-proposal-summary" });
    summary.setAttr("role", "button");
    summary.setAttr("aria-expanded", "false");
    summary.onclick = () => {
      const expanded = row.classList.toggle("is-expanded");
      summary.setAttr("aria-expanded", expanded ? "true" : "false");
    };

    const caret = summary.createSpan({ cls: "ffs-proposal-caret", text: "▸" });
    caret.setAttr("aria-hidden", "true");

    const meta = summary.createDiv({ cls: "ffs-proposal-meta" });
    meta.createEl("div", {
      text: `${p.proposalCount} proposal${p.proposalCount === 1 ? "" : "s"}`,
      cls: "ffs-proposal-count",
    });
    // Truncate to basename so the source URI doesn't overflow the
    // sidebar width. Full URI is on hover via `title=`.
    const sourceEl = meta.createEl("div", {
      text: uriBasename(p.sourceUri),
      cls: "ffs-proposal-source ffs-truncate",
    });
    sourceEl.setAttr("title", p.sourceUri);

    // Expandable body: one section per proposal showing its
    // predicate, claim, and scribe's rationale. The user needs
    // this to make an informed accept/reject decision —
    // accepting signs the claim into the substrate, so they
    // should see exactly what they're committing to.
    const body = row.createDiv({ cls: "ffs-proposal-body" });
    for (const proposal of p.proposals) {
      this.renderProposalDetail(body, proposal);
    }
    if (p.proposals.length === 0) {
      body.createEl("p", {
        text: "(no proposal details available)",
        cls: "ffs-proposal-empty",
      });
    }

    const actions = row.createDiv({ cls: "ffs-proposal-actions" });
    const accept = actions.createEl("button", {
      text: "Accept",
      cls: "mod-cta",
    });
    accept.onclick = () => {
      void this.plugin.summary.accept(p.submissionId).catch((err) => {
        new Notice(`FFS accept failed: ${describeError(err)}`);
      });
    };
    const reject = actions.createEl("button", { text: "Reject" });
    reject.onclick = () => {
      void this.plugin.summary.reject(p.submissionId).catch((err) => {
        new Notice(`FFS reject failed: ${describeError(err)}`);
      });
    };
  }

  /** Render a single proposal's claim inside the expandable body
   * of a proposal card. */
  private renderProposalDetail(
    parent: HTMLElement,
    proposal: ProposalPreview,
  ): void {
    const detail = parent.createDiv({ cls: "ffs-proposal-detail" });
    detail.createEl("div", {
      text: proposal.predicate,
      cls: "ffs-proposal-predicate",
    });
    const claim = detail.createEl("dl", { cls: "ffs-proposal-claim" });
    for (const [key, value] of Object.entries(proposal.claim)) {
      claim.createEl("dt", { text: key });
      const dd = claim.createEl("dd");
      if (Array.isArray(value)) {
        const ul = dd.createEl("ul");
        for (const item of value) {
          ul.createEl("li", { text: String(item) });
        }
      } else if (value && typeof value === "object") {
        dd.setText(JSON.stringify(value));
      } else {
        dd.setText(value == null ? "" : String(value));
      }
    }
    if (proposal.rationale) {
      detail.createEl("div", {
        text: proposal.rationale,
        cls: "ffs-proposal-rationale",
      });
    }
  }

  private renderFlag(parent: HTMLElement, item: PanelItem): void {
    const li = parent.createEl("li", { cls: "ffs-flag" });
    li.createSpan({ text: `[${item.kind}] `, cls: "ffs-flag-kind" });
    li.appendText(item.message);
  }
}

/**
 * Entity-name suggester. Pushes the query string into the
 * `EntitySearch` model and awaits its next callback to populate
 * the suggester list.
 */
class EntitySearchModal extends SuggestModal<EntityHit> {
  constructor(app: App, private plugin: FfsPlugin) {
    super(app);
    this.setPlaceholder("Search FFS entities by name…");
  }

  async getSuggestions(query: string): Promise<EntityHit[]> {
    // SuggestModal calls this on every keystroke; the model
    // debounces internally (200ms), so we await its next
    // delivery rather than firing a fresh daemon call per char.
    if (query.trim().length === 0) return [];
    const promise = this.plugin.awaitNextSearchResults();
    this.plugin.search.pushQuery(query);
    return promise;
  }

  renderSuggestion(hit: EntityHit, el: HTMLElement): void {
    el.createDiv({ text: hit.displayName, cls: "ffs-suggest-name" });
    el.createDiv({
      text: `${hit.predicate} · ${hit.entity}`,
      cls: "ffs-suggest-meta",
    });
  }

  onChooseSuggestion(hit: EntityHit): void {
    void this.openHit(hit);
  }

  /**
   * Resolve a search hit to a projection path, prefer an existing
   * on-disk file, else fall back to a render-on-demand. The
   * resolution heuristic mirrors the materializer's path-library
   * convention (`<family>/by-name/<letter>/<slug>.md`). The
   * family is picked from the hit's predicate; the slug from the
   * display name with spaces replaced by underscores.
   */
  private async openHit(hit: EntityHit): Promise<void> {
    const first = hit.displayName.slice(0, 1).toUpperCase();
    const slug = hit.displayName.replace(/\s+/g, "_");
    const family = familyForPredicate(hit.predicate);
    const candidates = family
      ? [`${family}/by-name/${first}/${slug}.md`]
      : [
          `contacts/by-name/${first}/${slug}.md`,
          `people/by-name/${first}/${slug}.md`,
          `notes/by-name/${first}/${slug}.md`,
        ];

    for (const path of candidates) {
      const file = this.app.vault.getAbstractFileByPath(path);
      if (file instanceof TFile) {
        await this.app.workspace.getLeaf(false).openFile(file);
        return;
      }
    }

    // Nothing on disk. Render-on-demand from the daemon, write the
    // markdown to the vault, and open the new file. The
    // materializer (task_25) writes the canonical version once an
    // atom commits; this path covers the gap where the user
    // searches for an entity before the materializer has caught up
    // (e.g., immediately after federation pull).
    for (const path of candidates) {
      try {
        const rendered = await this.plugin.renderProjection(path);
        if (!rendered) continue;
        const created = await this.app.vault.create(path, rendered.markdown);
        await this.app.workspace.getLeaf(false).openFile(created);
        return;
      } catch (err) {
        if (isCapabilityDenied(err)) {
          new Notice(`FFS: capability denied for ${hit.displayName}`);
          return;
        }
        if (isNotFound(err)) {
          // Try the next candidate path.
          continue;
        }
        new Notice(`FFS: could not open ${hit.displayName}: ${describeError(err)}`);
        return;
      }
    }
    new Notice(`FFS: no projection found for ${hit.displayName} (${hit.predicate})`);
  }
}

/**
 * Map the three MVP predicate names to their path-library family
 * directories. Returns `null` for predicates outside the library so
 * the caller falls back to brute-force-checking all three.
 */
function familyForPredicate(predicate: string): string | null {
  switch (predicate) {
    case "contact.person":
      return "contacts";
    case "person.generic":
      return "people";
    case "note":
      return "notes";
    default:
      return null;
  }
}

/**
 * Last path segment of a `file://` URI or filesystem path. Used
 * to keep the proposals list from overflowing the sidebar width.
 */
function uriBasename(uri: string): string {
  if (!uri) return "";
  const stripped = uri.replace(/^file:\/\//, "");
  const idx = stripped.lastIndexOf("/");
  return idx >= 0 ? stripped.slice(idx + 1) : stripped;
}

/** Pull a human-readable string out of any thrown value. */
function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return String(err);
}

/**
 * Detect the daemon's capability-denied error. The DaemonClient
 * decorates rejected promises with `code = 4001` (per
 * `crates/ffs-daemon/src/api.rs::ERR_CAPABILITY_DENIED`).
 */
function isCapabilityDenied(err: unknown): boolean {
  return typeof err === "object" && err !== null && (err as { code?: number }).code === 4001;
}

/**
 * Detect the daemon's not-found error (`ERR_NOT_FOUND = 4040`).
 * The render-on-demand path tries multiple candidate paths; a 4040
 * on one means "try the next" rather than a hard failure.
 */
function isNotFound(err: unknown): boolean {
  return typeof err === "object" && err !== null && (err as { code?: number }).code === 4040;
}

/**
 * Obsidian's `PluginSettingTab` is only available at runtime; this
 * thin subclass exists to satisfy `addSettingTab`'s type while the
 * real rendering work lives in `FfsSettingTab` (testable without
 * Obsidian).
 */
class FfsSettingsTabImpl extends PluginSettingTab {
  private adapter: FfsSettingTab;
  constructor(app: App, private plugin: FfsPlugin) {
    super(app, plugin);
    this.adapter = new FfsSettingTab(
      app,
      plugin,
      Setting as unknown as ConstructorParameters<typeof FfsSettingTab>[2],
      PluginSettingTab as unknown as ConstructorParameters<
        typeof FfsSettingTab
      >[3],
    );
  }
  display(): void {
    this.adapter.display(this.containerEl);
  }
}

/** Connection-state bullet character. Split from the label text so
 * the bullet can be wrapped in its own span and colored via CSS
 * (green for connected, gray for offline, etc.) while the label
 * stays neutral. */
function connStateBullet(state: string): string {
  switch (state) {
    case "connected":
      return "●";
    case "connecting":
      return "◐";
    case "fallback":
      return "◔";
    case "disconnected":
      return "○";
    default:
      return "•";
  }
}

function connStateText(state: string): string {
  switch (state) {
    case "connected":
      return "connected";
    case "connecting":
      return "connecting…";
    case "fallback":
      return "CLI fallback";
    case "disconnected":
      return "offline";
    default:
      return state;
  }
}

function formatClock(d: Date): string {
  const h = String(d.getHours()).padStart(2, "0");
  const m = String(d.getMinutes()).padStart(2, "0");
  const s = String(d.getSeconds()).padStart(2, "0");
  return `${h}:${m}:${s}`;
}

/**
 * Expand a leading `~/` or `~` to the user's home directory. Plain
 * `~user` syntax (without slash) isn't handled — the only intended
 * use is `~/.ffs/run/ffs.sock`-style paths.
 */
function expandTilde(p: string): string {
  if (p === "~") return homedir();
  if (p.startsWith("~/")) return homedir() + p.slice(1);
  return p;
}

/**
 * Resolve the CLI path used by the JSON-RPC subprocess fallback.
 * Obsidian-launched processes on macOS inherit a minimal PATH
 * (no `~/.local/bin`, no `/opt/homebrew/bin`), so a bare `ffs`
 * configured in settings yields ENOENT at spawn time. This
 * function turns a configured value of `ffs` into an absolute
 * path by probing the known install locations.
 */
function resolveCliPath(configured: string | undefined): string {
  const value = (configured ?? "").trim();
  if (value.length === 0) return resolveCliPath("ffs");
  if (value.includes("/")) {
    return expandTilde(value);
  }
  // Bare program name — probe known install prefixes.
  const candidates = [
    `${homedir()}/.local/bin/${value}`,
    `/usr/local/bin/${value}`,
    `/opt/homebrew/bin/${value}`,
    `/usr/bin/${value}`,
  ];
  for (const c of candidates) {
    if (existsSync(c)) return c;
  }
  // Nothing resolved — return the bare name so the error from
  // spawn() is the actual surface, not a silent miss.
  return value;
}

/**
 * Promise wrapper around `child_process.spawn` for the CLI fallback.
 * Returns the subprocess's exit code, captured stdout, and stderr.
 */
function spawnAsync(
  bin: string,
  args: string[],
  input: string,
): Promise<{ code: number; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d: Buffer) => (stdout += d.toString("utf8")));
    child.stderr.on("data", (d: Buffer) => (stderr += d.toString("utf8")));
    child.on("error", reject);
    child.on("close", (code: number | null) => {
      resolve({ code: code ?? -1, stdout, stderr });
    });
    child.stdin.write(input);
    child.stdin.end();
  });
}
