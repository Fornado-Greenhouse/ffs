// FFS Obsidian plugin entrypoint.
//
// Bootstraps the JSON-RPC client, surfaces a daemon-offline
// indicator in the status bar, and registers the settings tab.
// Subsequent task work (folder enumeration / projection rendering
// in task_18, daily-summary panel + entity search in task_19)
// layers on top of `this.client` and `this.client.events`.

import {
  App,
  Plugin,
  PluginSettingTab,
  Setting,
  type PluginManifest,
} from "obsidian";
import { spawn } from "node:child_process";

import { DaemonClient } from "./client.js";
import { enumerateFolder, decorateProjectionFile } from "./folder.js";
import { ProjectionSubscription, renderProjection } from "./projection.js";
import { applyOptimistically, routeEdit } from "./editing.js";
import { EntitySearch } from "./search.js";
import {
  DEFAULT_SETTINGS,
  FfsPluginSettings,
  FfsSettingTab,
} from "./settings.js";
import { SummaryPanelModel } from "./summary.js";

export default class FfsPlugin extends Plugin {
  settings: FfsPluginSettings = DEFAULT_SETTINGS;
  client!: DaemonClient;
  private statusEl: HTMLElement | null = null;
  /** Live-update subscription for the currently-open projection. */
  private projectionSub: ProjectionSubscription | null = null;
  /** Daily-health-summary panel model (task_19). */
  summary!: SummaryPanelModel;
  /** Entity-name search backing the quick-switcher hook (task_19). */
  search!: EntitySearch;
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

    this.client = new DaemonClient({
      socketPath: this.settings.socketPath,
      cliPath: this.settings.cliPath || undefined,
      spawnSubprocess: (bin, args, input) => spawnAsync(bin, args, input),
    });

    // Status bar indicator updates as the connection state changes.
    this.statusEl = this.addStatusBarItem();
    this.renderStatus("disconnected");
    this.client.onStateChange((state) => this.renderStatus(state));
    this.client.start();

    // Live-update wiring: when the daemon publishes a projection
    // invalidation for the currently-open file, re-render the
    // buffer. The host (task_19) sets the current path via
    // `projectionSub.setCurrent` on file-open hooks.
    this.projectionSub = new ProjectionSubscription(
      this.client.events,
      (path) => {
        void this.handleInvalidation(path);
      },
    );

    // Summary panel model — wires `audit.query` + `ingest.list_pending`
    // and refreshes on `event.atom.committed` for auditor atoms.
    this.summary = new SummaryPanelModel(this.client);

    // Entity-name search backing the quick-switcher hook. Results
    // arrive in `onResults`; production wires this into Obsidian's
    // suggester. The 200ms debounce default protects the daemon
    // from per-keystroke traffic.
    this.search = new EntitySearch(this.client, (hits, query) => {
      console.info(`[ffs] entity search ${query} → ${hits.length} hits`);
    });

    // Keyboard shortcuts (SHOULD per the task spec). Production
    // binds these to UI commands; the registrations stay in
    // main.ts so the keymap is visible to users in Obsidian's
    // Hotkeys settings.
    this.addCommand({
      id: "ffs-refresh-summary",
      name: "Refresh daily health summary",
      callback: () => {
        void this.summary.refresh().catch((err) => {
          console.warn("[ffs] summary refresh failed:", err);
        });
      },
    });
    this.addCommand({
      id: "ffs-focus-entity-search",
      name: "Search FFS entities by name…",
      callback: () => {
        // The production binding opens a suggester. The plugin's
        // `this.search.pushQuery(input)` drives it.
        console.info("[ffs] entity-search command invoked");
      },
    });

    // Register the settings tab using the Obsidian-runtime
    // wrapper. Direct subclassing keeps Obsidian's discovery happy.
    this.addSettingTab(new FfsSettingsTabImpl(this.app, this));
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
   * subscription knows which path to refresh on. The Obsidian-side
   * file-open hook calls this from the production binding;
   * downstream tasks (task_19) extend the wiring.
   */
  setOpenProjection(path: string | null): void {
    this.projectionSub?.setCurrent(path);
  }

  private async handleInvalidation(path: string): Promise<void> {
    // Production wires this to re-fetch + re-render the open file.
    // For MVP the indication goes to the console; task_19's
    // active-leaf integration replaces it with a buffer refresh.
    console.info("[ffs] projection invalidated, re-render pending:", path);
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
    const label = {
      connected: "FFS ● connected",
      connecting: "FFS ◐ connecting",
      disconnected: "FFS ○ offline",
      fallback: "FFS ◔ CLI fallback",
    }[state] ?? `FFS ${state}`;
    this.statusEl.setText(label);
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
      PluginSettingTab as unknown as ConstructorParameters<typeof FfsSettingTab>[3],
    );
  }
  display(): void {
    this.adapter.display(this.containerEl);
  }
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
