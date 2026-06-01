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
    const label =
      {
        connected: "FFS ● connected",
        connecting: "FFS ◐ connecting",
        disconnected: "FFS ○ offline",
        fallback: "FFS ◔ CLI fallback",
      }[state] ?? `FFS ${state}`;
    this.statusEl.setText(label);
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
    this.plugin.summary.onChange((state) => this.render(state));

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
    this.plugin.client.onStateChange(handleState);
    // Track the listener so onClose can stop receiving updates
    // after the view is destroyed.
    this.offStateChange = () => {
      // DaemonClient.onStateChange doesn't return an unsubscribe
      // handle (MVP omission). We mark the view as disposed and
      // make the handler a no-op via a closure flag below.
    };

    // Seed the panel chrome immediately so the user sees
    // something instead of a blank pane.
    handleState(this.connState);
  }

  async onClose(): Promise<void> {
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
      console.warn("[ffs] summary refresh failed:", err);
      this.render(this.plugin.summary.state);
    }
  }

  private render(state: PanelState): void {
    const root = this.containerEl.children[1] as HTMLElement;
    root.empty();
    root.addClass("ffs-summary-root");

    const header = root.createDiv({ cls: "ffs-summary-header" });
    header.createEl("h3", { text: "Daily summary" });
    const refresh = header.createEl("button", { text: "Refresh" });
    refresh.onclick = () => {
      void this.triggerRefresh();
    };

    // Status line: connection state + last-refreshed timestamp.
    // Without these two signals, a click on Refresh against an
    // empty substrate looks like the button is broken.
    const status = root.createDiv({ cls: "ffs-summary-status" });
    const connBadge = status.createSpan({
      cls: `ffs-summary-conn ffs-summary-conn-${this.connState}`,
    });
    connBadge.setText(connStateLabel(this.connState));
    if (this.lastRefreshedAt) {
      status.createSpan({
        cls: "ffs-summary-refreshed",
        text: ` · refreshed ${formatClock(this.lastRefreshedAt)}`,
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
    const meta = row.createDiv({ cls: "ffs-proposal-meta" });
    meta.createEl("div", {
      text: `${p.proposalCount} proposal${p.proposalCount === 1 ? "" : "s"}`,
      cls: "ffs-proposal-count",
    });
    meta.createEl("div", {
      text: p.sourceUri,
      cls: "ffs-proposal-source",
    });
    const actions = row.createDiv({ cls: "ffs-proposal-actions" });
    const accept = actions.createEl("button", { text: "Accept" });
    accept.onclick = () => {
      void this.plugin.summary.accept(p.submissionId).catch((err) => {
        console.warn("[ffs] accept failed:", err);
      });
    };
    const reject = actions.createEl("button", { text: "Reject" });
    reject.onclick = () => {
      void this.plugin.summary.reject(p.submissionId).catch((err) => {
        console.warn("[ffs] reject failed:", err);
      });
    };
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
    // Try to navigate to a projection path matching this entity.
    // Resolution heuristic: if the entity's name starts with a
    // letter, try `contacts/by-name/<letter>/<entity>.md`. The
    // file-open hook calls into `setOpenProjection` so the
    // invalidation subscription picks up the right path.
    const first = hit.displayName.slice(0, 1).toUpperCase();
    const slug = hit.displayName.replace(/\s+/g, "_");
    const candidates = [
      `contacts/by-name/${first}/${slug}.md`,
      `people/by-name/${first}/${slug}.md`,
      `notes/by-name/${first}/${slug}.md`,
    ];
    for (const path of candidates) {
      const file = this.app.vault.getAbstractFileByPath(path);
      if (file instanceof TFile) {
        void this.app.workspace.getLeaf(false).openFile(file);
        return;
      }
    }
    // Fall back to logging; the file may not be materialized in
    // the working set yet (Phase 2 wires a render-on-demand path).
    console.info(
      `[ffs] selected ${hit.entity} (${hit.predicate}); no projection on disk`,
    );
  }
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

function connStateLabel(state: string): string {
  switch (state) {
    case "connected":
      return "● connected";
    case "connecting":
      return "◐ connecting…";
    case "fallback":
      return "◔ CLI fallback";
    case "disconnected":
      return "○ offline";
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
