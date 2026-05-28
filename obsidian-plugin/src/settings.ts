// Settings panel for the FFS plugin. Renders into Obsidian's
// settings UI via the `Setting` builder. The plugin entrypoint
// loads + persists these via the standard `loadData()` /
// `saveData()` Obsidian API.
//
// At MVP we expose three knobs:
//
// - `socketPath` — UDS / named-pipe path the client connects to.
// - `cliPath` — optional alternate path to the `ffs` CLI used by
//   the fallback when direct IPC fails.
// - `identityKeyPath` — optional path to the agent's Ed25519 key,
//   used by the daemon for ffs_author_atom provenance when the
//   plugin authors atoms on the user's behalf.

import type { App, Plugin, PluginSettingTab } from "obsidian";

export interface FfsPluginSettings {
  socketPath: string;
  cliPath: string;
  identityKeyPath: string;
}

export const DEFAULT_SETTINGS: FfsPluginSettings = {
  // Defaults match the path the daemon binds on Linux/macOS by
  // convention. Windows users override to a named pipe like
  // `\\.\pipe\ffs.sock`.
  socketPath: "~/.ffs/run/ffs.sock",
  cliPath: "ffs",
  identityKeyPath: "",
};

/**
 * Build the settings tab. Kept as a factory function (rather than a
 * class) so tests can call it without subclassing Obsidian's
 * `PluginSettingTab`, which Obsidian provides only at runtime.
 *
 * The runtime production binding lives in `main.ts`; this file
 * exports the schema + a renderer that takes the host `Setting`
 * builder so unit tests can stub it.
 */
export interface SettingRenderHost {
  /** Add a labeled string-input setting. Returns the new value on change. */
  addTextSetting(
    name: string,
    description: string,
    initial: string,
    onChange: (value: string) => Promise<void>,
  ): void;
}

export function renderSettings(
  host: SettingRenderHost,
  current: FfsPluginSettings,
  save: (next: FfsPluginSettings) => Promise<void>,
): void {
  host.addTextSetting(
    "Socket path",
    "Filesystem path to the daemon's UDS (Linux/macOS) or named pipe (Windows).",
    current.socketPath,
    async (value) => {
      await save({ ...current, socketPath: value });
    },
  );
  host.addTextSetting(
    "CLI path",
    "Path to the `ffs` CLI for the subprocess fallback when direct IPC fails.",
    current.cliPath,
    async (value) => {
      await save({ ...current, cliPath: value });
    },
  );
  host.addTextSetting(
    "Identity key path",
    "Optional: path to the Ed25519 key file used as the plugin's author identity for atom authoring.",
    current.identityKeyPath,
    async (value) => {
      await save({ ...current, identityKeyPath: value });
    },
  );
}

// ---- Obsidian-runtime PluginSettingTab wrapper ----
//
// This class lives in the same file so the build produces a single
// `main.js`. It is only constructed in production when the Obsidian
// runtime is present; tests exercise `renderSettings()` directly
// against a stub `SettingRenderHost`.

export interface PluginWithSettings extends Plugin {
  settings: FfsPluginSettings;
  saveSettings(): Promise<void>;
}

export class FfsSettingTab {
  constructor(
    private app: App,
    private plugin: PluginWithSettings,
    private SettingCtor: ObsidianSettingCtor,
    private TabSuper: PluginSettingTabCtor,
  ) {}

  /**
   * Build the tab. Obsidian calls this on every settings open;
   * the implementation clears the container, then runs
   * `renderSettings()` against an adapter that proxies into
   * Obsidian's `Setting` builder.
   */
  display(containerEl: HTMLElement): void {
    containerEl.empty();
    const host: SettingRenderHost = {
      addTextSetting: (name, description, initial, onChange) => {
        new this.SettingCtor(containerEl)
          .setName(name)
          .setDesc(description)
          .addText((text) => {
            text.setValue(initial).onChange((value: string) => {
              void onChange(value);
            });
          });
      },
    };
    renderSettings(host, this.plugin.settings, async (next) => {
      this.plugin.settings = next;
      await this.plugin.saveSettings();
    });
  }
}

// Minimal structural types for the Obsidian `Setting` + tab API
// we touch — kept loose so the tests don't have to import obsidian.
interface ObsidianSettingCtor {
  new (containerEl: HTMLElement): {
    setName(name: string): { setDesc(desc: string): {
      addText(cb: (t: {
        setValue(v: string): { onChange(cb: (v: string) => void): void };
      }) => void): void;
    } };
  };
}

interface PluginSettingTabCtor {
  new (app: App, plugin: Plugin): PluginSettingTab;
}
