import { beforeEach, describe, expect, it, vi } from "vitest";

import { enumerateFolder } from "../src/folder.js";
import {
  familyFolders,
  familyForPredicate,
  families,
  hitCandidatePaths,
  isProjectionPath,
  loadFamilies,
  setFamilies,
} from "../src/paths.js";

const FOUR = [
  { family: "contacts", predicate: "contact.person", name_field: "display_name" },
  { family: "people", predicate: "person.generic", name_field: "display_name" },
  { family: "notes", predicate: "note", name_field: "title" },
  { family: "orgs", predicate: "org.company", name_field: "display_name" },
];

describe("runtime family table", () => {
  beforeEach(() => setFamilies([]));

  it("starts empty: nothing is a projection path and enumeration yields no folders", async () => {
    expect(families()).toEqual([]);
    expect(familyFolders()).toEqual([]);
    expect(isProjectionPath("contacts/by-name/S/Sara.md")).toBe(false);
    expect(isProjectionPath("orgs/by-name/A/")).toBe(false);
    const client = { call: vi.fn(async () => ({ markdown: "- [x](orgs/by-name/A/x.md)" })) };
    expect(await enumerateFolder(client, "orgs/by-name/A/")).toBeNull();
    expect(client.call).not.toHaveBeenCalled();
  });

  it("loads four families from a mocked path.families response, sorted by family", async () => {
    const client = { call: vi.fn(async (m: string) => (m === "path.families" ? [...FOUR].reverse() : null)) };
    const persisted: unknown[] = [];
    const ok = await loadFamilies(client, (e) => {
      persisted.push(e);
    });
    expect(ok).toBe(true);
    expect(client.call).toHaveBeenCalledWith("path.families", {});
    expect(familyFolders()).toEqual(["contacts", "notes", "orgs", "people"]);
    expect(familyForPredicate("org.company")).toBe("orgs");
    expect(familyForPredicate("capability.grant")).toBeNull();
    expect(persisted).toHaveLength(1);
    expect(isProjectionPath("orgs/by-name/A/")).toBe(true);
    const enumerated = await enumerateFolder(
      { call: vi.fn(async () => ({ markdown: "- [Acme](orgs/by-name/A/Acme.md)\n" })) },
      "orgs/by-name/A/",
    );
    expect(enumerated?.entries).toEqual([{ label: "Acme", path: "orgs/by-name/A/Acme.md" }]);
  });

  it("keeps the last-known table and does not throw when the daemon is unreachable", async () => {
    setFamilies(FOUR);
    const client = {
      call: vi.fn(async () => {
        throw new Error("ECONNREFUSED");
      }),
    };
    const persist = vi.fn();
    await expect(loadFamilies(client, persist)).resolves.toBe(false);
    expect(persist).not.toHaveBeenCalled();
    expect(familyFolders()).toEqual(["contacts", "notes", "orgs", "people"]);
    expect(isProjectionPath("people/recent/")).toBe(true);
  });

  it("ignores a malformed response and drops entries without a family", async () => {
    setFamilies(FOUR);
    await expect(loadFamilies({ call: vi.fn(async () => ({ nope: 1 })) })).resolves.toBe(false);
    expect(familyFolders()).toHaveLength(4);
    setFamilies([{ family: "", predicate: "x", name_field: "y" }, FOUR[3]] as never);
    expect(familyFolders()).toEqual(["orgs"]);
  });

  it("openHit path: uses the daemon-supplied basename with its qualifier", () => {
    setFamilies(FOUR);
    expect(
      hitCandidatePaths({ predicate: "org.company", displayName: "Sara Chen", basename: "Sara_Chen_(Acme)" }),
    ).toEqual(["orgs/by-name/S/Sara_Chen_(Acme).md"]);
  });

  it("openHit path: falls back to the slugified display name and to every family when the predicate has none", () => {
    setFamilies(FOUR);
    expect(hitCandidatePaths({ predicate: "person.generic", displayName: "Sara Chen" })).toEqual([
      "people/by-name/S/Sara_Chen.md",
    ]);
    expect(hitCandidatePaths({ predicate: "unknown.thing", displayName: "Zed" })).toEqual([
      "contacts/by-name/Z/Zed.md",
      "notes/by-name/Z/Zed.md",
      "orgs/by-name/Z/Zed.md",
      "people/by-name/Z/Zed.md",
    ]);
    setFamilies([]);
    expect(hitCandidatePaths({ predicate: "unknown.thing", displayName: "Zed" })).toEqual([]);
  });
});
