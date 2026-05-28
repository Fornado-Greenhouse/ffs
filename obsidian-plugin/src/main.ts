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
import {
  DEFAULT_SETTINGS,
  FfsPluginSettings,
  FfsSettingTab,
} from "./settings.js";

export default class FfsPlugin extends Plugin {
  settings: FfsPluginSettings = DEFAULT_SETTINGS;
  client!: DaemonClient;
  private statusEl: HTMLElement | null = null;

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

    // Register the settings tab using the Obsidian-runtime
    // wrapper. Direct subclassing keeps Obsidian's discovery happy.
    this.addSettingTab(new FfsSettingsTabImpl(this.app, this));
  }

  async onunload(): Promise<void> {
    this.client?.close();
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
