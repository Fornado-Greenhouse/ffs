import { beforeEach, describe, expect, it } from "vitest";

import {
  isProjectionFile,
  isProjectionPath,
  normalizeProjectionPath,
  parseProjectionPath,
  setFamilies,
} from "../src/paths.js";

const MVP_FAMILIES = [
  { family: "contacts", predicate: "contact.person", name_field: "display_name" },
  { family: "people", predicate: "person.generic", name_field: "display_name" },
  { family: "notes", predicate: "note", name_field: "title" },
];

describe("paths", () => {
  // The table is runtime state now (ADR-028); seed the three MVP
  // families so every assertion below reads exactly as it did when
  // they were a constant.
  beforeEach(() => setFamilies(MVP_FAMILIES));

  it("isProjectionPath matches the three MVP families", () => {
    expect(isProjectionPath("contacts/by-name/S/Sara.md")).toBe(true);
    expect(isProjectionPath("people/recent/")).toBe(true);
    expect(isProjectionPath("notes/by-name/T/tuesday.md")).toBe(true);
  });

  it("isProjectionPath does not match non-projection paths", () => {
    expect(isProjectionPath("some_other_folder/")).toBe(false);
    expect(isProjectionPath("README.md")).toBe(false);
    expect(isProjectionPath("daily/2026-05-28.md")).toBe(false);
  });

  it("isProjectionFile gates the projection-render hook to single-entity paths", () => {
    expect(isProjectionFile("contacts/by-name/S/Sara.md")).toBe(true);
    // Folders are not single-entity files.
    expect(isProjectionFile("contacts/by-name/S/")).toBe(false);
    expect(isProjectionFile("contacts/recent/")).toBe(false);
    // Regular notes don't match.
    expect(isProjectionFile("daily/2026-05-28.md")).toBe(false);
  });

  it("normalizeProjectionPath strips ~/.ffs/ and leading/trailing slashes", () => {
    expect(normalizeProjectionPath("~/.ffs/contacts/recent/")).toBe(
      "contacts/recent",
    );
    expect(normalizeProjectionPath("/contacts/recent/")).toBe("contacts/recent");
    expect(normalizeProjectionPath(".ffs/contacts/")).toBe("contacts");
  });

  it("parseProjectionPath returns the right shape for each MVP form", () => {
    expect(parseProjectionPath("contacts/recent")).toMatchObject({
      kind: "recent",
      family: "contacts",
    });
    expect(parseProjectionPath("contacts/by-name/S")).toMatchObject({
      kind: "alphabetical-letter",
      family: "contacts",
      letter: "S",
    });
    expect(parseProjectionPath("contacts/by-name/s")).toMatchObject({
      kind: "alphabetical-letter",
      letter: "S",
    });
    expect(parseProjectionPath("contacts/by-name/S/Sara.md")).toMatchObject({
      kind: "single-entity",
      family: "contacts",
      entity: "Sara",
    });
    expect(parseProjectionPath("contacts/starred/")).toMatchObject({
      kind: "unsupported",
      family: "contacts",
    });
    expect(parseProjectionPath("random/path.md")).toMatchObject({
      kind: "not-projection",
    });
  });
});
