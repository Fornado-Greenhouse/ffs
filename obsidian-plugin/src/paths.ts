// Projection-path discrimination shared by folder enumeration,
// projection rendering, and edit routing.
//
// The three MVP path families (per ADR-011) are `contacts/`,
// `people/`, and `notes/`. Anything under those prefixes is a
// substrate projection — the plugin intercepts it; anything else is
// a regular Obsidian vault file and passes through untouched.
//
// Path shapes recognized:
//
//   <family>/recent/                       → recency listing
//   <family>/by-name/<letter>/              → alphabetical listing
//   <family>/by-name/<letter>/<entity>.md   → single-entity render
//
// Other ADR-011 sub-paths (`starred/`, `by-org/`, `from/<peer>/`,
// `intersection/with/<peer>/`) parse to `ParsedPath.Unsupported`
// for MVP — the daemon's renderer is the source of truth on what
// renders; the plugin just discriminates whether a path belongs
// to the substrate at all.

export const PROJECTION_FAMILIES = ["contacts", "people", "notes"] as const;

export type ProjectionFamily = (typeof PROJECTION_FAMILIES)[number];

export type ParsedPath =
  | { kind: "recent"; family: ProjectionFamily }
  | { kind: "alphabetical-letter"; family: ProjectionFamily; letter: string }
  | { kind: "single-entity"; family: ProjectionFamily; entity: string }
  | { kind: "unsupported"; family: ProjectionFamily; raw: string }
  | { kind: "not-projection"; raw: string };

/**
 * Strip any leading `/` or `~/.ffs/` prefix, then normalize. The
 * Obsidian plugin sees vault-relative paths; vault-relative paths
 * inside an FFS-rooted vault start with `contacts/`, `people/`, or
 * `notes/`. We tolerate a leading slash and a `~/.ffs/` prefix to
 * keep callers free of normalization noise.
 */
export function normalizeProjectionPath(path: string): string {
  let p = path.trim();
  if (p.startsWith("~/.ffs/")) p = p.slice("~/.ffs/".length);
  if (p.startsWith(".ffs/")) p = p.slice(".ffs/".length);
  if (p.startsWith("/")) p = p.slice(1);
  // Trim a trailing slash (folder paths) but preserve the empty
  // path so callers can detect the root.
  if (p.endsWith("/") && p.length > 1) p = p.slice(0, -1);
  return p;
}

/**
 * Cheap discriminator: does this vault-relative path live under one
 * of the three MVP projection families? Used by Obsidian event
 * handlers to decide whether to intercept.
 */
export function isProjectionPath(path: string): boolean {
  const p = normalizeProjectionPath(path);
  return PROJECTION_FAMILIES.some(
    (family) => p === family || p.startsWith(family + "/"),
  );
}

/**
 * Discriminator for single-entity projection FILES (versus folders).
 * Plugins use this to gate the projection-render-on-open hook from
 * firing on regular Obsidian notes.
 */
export function isProjectionFile(path: string): boolean {
  const parsed = parseProjectionPath(path);
  return parsed.kind === "single-entity";
}

/**
 * Pure structural parse of a vault-relative path. Mirror of the
 * Rust-side `ffs_core::projection::path::parse` but in TypeScript.
 * Returns `not-projection` for anything outside the three families
 * so callers can short-circuit without throwing.
 */
export function parseProjectionPath(path: string): ParsedPath {
  const p = normalizeProjectionPath(path);
  if (p.length === 0) {
    return { kind: "not-projection", raw: path };
  }
  const parts = p.split("/");
  const family = parts[0];
  if (!isProjectionFamily(family)) {
    return { kind: "not-projection", raw: path };
  }
  // Bare family root, e.g., "contacts" — treat as unsupported
  // listing-of-listings until the renderer adds a top-level view.
  if (parts.length === 1) {
    return { kind: "unsupported", family, raw: p };
  }
  if (parts.length === 2 && parts[1] === "recent") {
    return { kind: "recent", family };
  }
  if (parts.length === 3 && parts[1] === "by-name") {
    const letter = parts[2];
    if (letter.length !== 1) {
      return { kind: "unsupported", family, raw: p };
    }
    return { kind: "alphabetical-letter", family, letter: letter.toUpperCase() };
  }
  if (parts.length === 4 && parts[1] === "by-name") {
    const letter = parts[2];
    const filename = parts[3];
    if (letter.length !== 1 || !filename.endsWith(".md")) {
      return { kind: "unsupported", family, raw: p };
    }
    return {
      kind: "single-entity",
      family,
      entity: filename.slice(0, -3),
    };
  }
  return { kind: "unsupported", family, raw: p };
}

function isProjectionFamily(name: string): name is ProjectionFamily {
  return (PROJECTION_FAMILIES as readonly string[]).includes(name);
}
