// Minimal stub for the `obsidian` runtime module so tests that
// transitively import `settings.ts`'s type annotations resolve
// cleanly. Production gets the real module from the Obsidian
// renderer.
export class Plugin {}
export class PluginSettingTab {
  containerEl = new (class {
    empty() {}
  })();
  constructor(public app: unknown, public plugin: unknown) {}
}
export class Setting {
  constructor(public containerEl: unknown) {}
  setName(_n: string) {
    return this;
  }
  setDesc(_d: string) {
    return this;
  }
  addText(_cb: unknown) {
    return this;
  }
}
export interface App {}
export interface PluginManifest {}
