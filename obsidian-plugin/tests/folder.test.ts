import { describe, expect, it, vi } from "vitest";

import {
  decorateProjectionFile,
  enumerateFolder,
  parseFolderMarkdown,
  DEFAULT_PAGE_SIZE,
} from "../src/folder.js";

function fakeClient(response: unknown) {
  return {
    call: vi.fn(async (_method: string, _params: unknown) => response),
  };
}

describe("folder", () => {
  it("enumerateFolder calls path.list with a normalized path", async () => {
    const client = fakeClient({
      markdown: "- [Sara](contacts/by-name/S/Sara.md)\n",
    });
    const result = await enumerateFolder(client, "~/.ffs/contacts/by-name/S/");
    expect(result).not.toBeNull();
    expect(client.call).toHaveBeenCalledWith("path.list", {
      path: "contacts/by-name/S",
      page: 0,
    });
    expect(result!.entries).toEqual([
      { label: "Sara", path: "contacts/by-name/S/Sara.md" },
    ]);
    expect(result!.total).toBe(1);
    expect(result!.hasNextPage).toBe(false);
  });

  it("enumerateFolder returns null for non-projection paths (pass-through)", async () => {
    const client = fakeClient({ markdown: "" });
    const result = await enumerateFolder(client, "some_other_folder/");
    expect(result).toBeNull();
    expect(client.call).not.toHaveBeenCalled();
  });

  it("paginates a 1000-entry listing into a default 100-entry first page", async () => {
    const lines: string[] = [];
    for (let i = 0; i < 1000; i++) {
      const name = `Person${String(i).padStart(4, "0")}`;
      lines.push(`- [${name}](contacts/by-name/P/${name}.md)`);
    }
    const client = fakeClient({ markdown: lines.join("\n") });
    const page0 = await enumerateFolder(client, "contacts/by-name/P/");
    expect(page0!.entries).toHaveLength(DEFAULT_PAGE_SIZE);
    expect(page0!.total).toBe(1000);
    expect(page0!.hasNextPage).toBe(true);
    expect(page0!.entries[0].label).toBe("Person0000");

    const page1 = await enumerateFolder(client, "contacts/by-name/P/", 1);
    expect(page1!.entries[0].label).toBe("Person0100");
    expect(page1!.hasNextPage).toBe(true);

    const page9 = await enumerateFolder(client, "contacts/by-name/P/", 9);
    expect(page9!.entries[page9!.entries.length - 1].label).toBe("Person0999");
    expect(page9!.hasNextPage).toBe(false);
  });

  it("parseFolderMarkdown extracts bullet labels and links, skipping headings", () => {
    const md = [
      "## Contacts whose name starts with S",
      "",
      "- [Sara Chen](contacts/by-name/S/Sara_Chen.md)",
      "- [Sarah Park](contacts/by-name/S/Sarah_Park.md)",
      "* Standalone label without link",
    ].join("\n");
    const entries = parseFolderMarkdown(md);
    expect(entries).toEqual([
      { label: "Sara Chen", path: "contacts/by-name/S/Sara_Chen.md" },
      { label: "Sarah Park", path: "contacts/by-name/S/Sarah_Park.md" },
      { label: "Standalone label without link", path: null },
    ]);
  });

  it("decorateProjectionFile returns the read-with-care class for projections only", () => {
    expect(decorateProjectionFile("contacts/by-name/S/Sara.md")).toBe(
      "ffs-projection-file",
    );
    expect(decorateProjectionFile("daily/2026-05-28.md")).toBeNull();
  });
});
