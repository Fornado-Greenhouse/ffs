// Projection-path discrimination shared by folder enumeration,
// projection rendering, and edit routing.
//
// Path families are no longer a constant. Per ADR-028 the daemon's
// predicate registry declares them (a spec's `[path]` table), and the
// plugin learns the table at runtime from the `path.families` RPC:
//
//   [{ family: "contacts", predicate: "contact.person", name_field: "display_name" }, ...]
//
// The table lives in this module as process state (`setFamilies`).
// It starts EMPTY: with no table nothing is a projection path, so
// Obsidian's default behavior applies until the daemon answers (or
// the last-known table is restored from plugin settings).
//
// Path shapes recognized:
//
//   <family>/recent/                         → recency listing
//   <family>/by-name/<letter>/               → alphabetical listing
//   <family>/by-name/<letter>/<basename>.md  → single-entity render
//
// `basename` is a human-readable file stem, possibly carrying a
// parenthetical qualifier such as `Sara_Chen_(Acme)` (ADR-030). It is
// a projection concern and is NOT the entity id.
//
// Other ADR-011 sub-paths (`starred/`, `by-org/`, `from/<peer>/`,
// `intersection/with/<peer>/`) parse to `ParsedPath.Unsupported`; the
// daemon's renderer is the source of truth on what renders.

/** One row of the daemon's family table (`path.families`). */
export interface FamilyEntry {
  family: string;
  predicate: string;
  name_field: string;
}

export type ProjectionFamily = string;

export type ParsedPath =
  | { kind: "recent"; family: ProjectionFamily }
  | { kind: "alphabetical-letter"; family: ProjectionFamily; letter: string }
  | { kind: "single-entity"; family: ProjectionFamily; entity: string }
  | { kind: "unsupported"; family: ProjectionFamily; raw: string }
  | { kind: "not-projection"; raw: string };

let table: FamilyEntry[] = [];

/**
 * Replace the runtime family table. Entries missing a `family` are
 * dropped; order is normalized to the daemon's (sorted by family).
 */
export function setFamilies(entries: readonly FamilyEntry[] | null | undefined): void {
  const cleaned: FamilyEntry[] = [];
  for (const e of entries ?? []) {
    if (!e || typeof e.family !== "string" || e.family.length === 0) continue;
    cleaned.push({
      family: e.family,
      predicate: typeof e.predicate === "string" ? e.predicate : "",
      name_field: typeof e.name_field === "string" ? e.name_field : "",
    });
  }
  cleaned.sort((a, b) =>
    a.family < b.family ? -1 : a.family > b.family ? 1 : a.predicate < b.predicate ? -1 : 1,
  );
  table = cleaned;
}

/** Current table (a copy). Empty until `setFamilies` is called. */
export function families(): FamilyEntry[] {
  return table.map((e) => ({ ...e }));
}

/** Folder tokens in table order. */
export function familyFolders(): string[] {
  return table.map((e) => e.family);
}

/** Family folder for a predicate, or `null` when the predicate has no folder. */
export function familyForPredicate(predicate: string): string | null {
  const hit = table.find((e) => e.predicate === predicate);
  return hit ? hit.family : null;
}

/**
 * Load the table from the daemon. Never throws: on any failure the
 * current table is left untouched and `false` is returned, so a
 * caller can keep the last-known table (persisted in settings) when
 * the daemon is unreachable. On success the new table is installed
 * and handed to `persist` so a restart with the daemon down still
 * enumerates the last known folders.
 */
export async function loadFamilies(
  client: { call(method: string, params?: unknown): Promise<unknown> },
  persist?: (entries: FamilyEntry[]) => void | Promise<void>,
): Promise<boolean> {
  try {
    const raw = await client.call("path.families", {});
    if (!Array.isArray(raw)) {
      console.debug("[ffs] path.families returned a non-array; keeping last-known table");
      return false;
    }
    setFamilies(raw as FamilyEntry[]);
    if (persist) await persist(families());
    return true;
  } catch (err) {
    console.debug("[ffs] path.families unavailable; keeping last-known table:", err);
    return false;
  }
}

/**
 * Strip any leading `/` or `~/.ffs/` prefix, then normalize. The
 * Obsidian plugin sees vault-relative paths; vault-relative paths
 * inside an FFS-rooted vault start with a family folder. We tolerate
 * a leading slash and a `~/.ffs/` prefix to keep callers free of
 * normalization noise.
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
 * of the declared projection families? Used by Obsidian event
 * handlers to decide whether to intercept. False for everything when
 * the table is empty.
 */
export function isProjectionPath(path: string): boolean {
  const p = normalizeProjectionPath(path);
  return table.some((e) => p === e.family || p.startsWith(e.family + "/"));
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
 * Returns `not-projection` for anything outside the declared families
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

function isProjectionFamily(name: string): boolean {
  return table.some((e) => e.family === name);
}

/** The subset of an entity-search hit this module needs to build paths. */
export interface HitForPath {
  predicate: string;
  displayName: string;
  /** File stem from the daemon's path-to-entity index, when supplied. */
  basename?: string;
}

/**
 * Candidate vault paths for a search hit, most specific first. When
 * the hit's predicate has a family, exactly one candidate is
 * produced; otherwise one per declared family. The stem is the hit's
 * `basename` when the daemon supplies it (qualified names such as
 * `Sara_Chen_(Acme)` come from the path-to-entity index and cannot be
 * derived client-side), else the display name with whitespace
 * replaced by underscores, exactly as before.
 *
 * TODO(task_40): `entity.search` hits will carry a `path` field;
 * prefer it over rebuilding the path here once it exists.
 */
export function hitCandidatePaths(hit: HitForPath): string[] {
  const stem =
    typeof hit.basename === "string" && hit.basename.length > 0
      ? hit.basename
      : hit.displayName.replace(/\s+/g, "_");
  const first = stem.slice(0, 1).toUpperCase();
  const family = familyForPredicate(hit.predicate);
  const folders = family ? [family] : familyFolders();
  return folders.map((f) => `${f}/by-name/${first}/${stem}.md`);
}
