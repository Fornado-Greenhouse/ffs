import { describe, expect, it, vi } from "vitest";

import {
  DEFAULT_SETTINGS,
  FfsPluginSettings,
  renderSettings,
  SettingRenderHost,
} from "../src/settings.js";

describe("settings", () => {
  it("defaults are populated for socket + cli paths", () => {
    expect(DEFAULT_SETTINGS.socketPath).toMatch(/\.ffs/);
    expect(DEFAULT_SETTINGS.cliPath).toBe("ffs");
    expect(DEFAULT_SETTINGS.identityKeyPath).toBe("");
  });

  it("renderSettings adds one text setting per user-tunable knob", () => {
    const added: { name: string; description: string; initial: string }[] = [];
    const host: SettingRenderHost = {
      addTextSetting: (name, description, initial, _onChange) => {
        added.push({ name, description, initial });
      },
    };
    const save = vi.fn(async () => {});
    renderSettings(host, DEFAULT_SETTINGS, save);
    expect(added.map((s) => s.name)).toEqual([
      "Socket path",
      "CLI path",
      "Identity key path",
    ]);
    expect(added[0].initial).toBe(DEFAULT_SETTINGS.socketPath);
  });

  it("settings updates flow through to save() via onChange callbacks", async () => {
    let captured: ((value: string) => Promise<void>) | null = null;
    const host: SettingRenderHost = {
      addTextSetting: (name, _description, _initial, onChange) => {
        if (name === "Socket path") captured = onChange;
      },
    };
    const save = vi.fn(async (_next: FfsPluginSettings) => {});
    renderSettings(host, DEFAULT_SETTINGS, save);
    expect(captured).toBeTypeOf("function");
    await captured!("/custom/path.sock");
    expect(save).toHaveBeenCalledWith({
      ...DEFAULT_SETTINGS,
      socketPath: "/custom/path.sock",
    });
  });
});
