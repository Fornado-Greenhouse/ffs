// Paginated folder enumeration for the substrate's projection
// paths.
//
// Obsidian's file-explorer would otherwise show a flat directory of
// every projection file under a path family. For substrates with
// hundreds-to-thousands of entries, that's both slow (the plugin
// would have to render every file's tooltip) and unhelpful — users
// navigate by letter or by recency, not by scrolling a giant
// alphabetical list. This module:
//
// 1. Calls the daemon's `path.list` to fetch the markdown listing
//    for a folder path.
// 2. Splits the rendered markdown into entries (one entry per
//    bullet or per `- [name](path)` line).
// 3. Paginates client-side: first page is 100 entries, with a
//    `nextPage` indicator the UI can show.
//
// Pagination happens client-side because the MVP daemon doesn't
// take a `page` param on `path.list` yet — the renderer returns the
// full listing, and the plugin slices it. A future enhancement can
// push pagination into the daemon for huge substrates; the plugin's
// interface stays the same.

import { DaemonClient } from "./client.js";
import { isProjectionPath, normalizeProjectionPath } from "./paths.js";

/** Default entries-per-page for the client-side paginator. */
export const DEFAULT_PAGE_SIZE = 100;

export interface FolderEntry {
  /** Display label as it appears in the file explorer. */
  label: string;
  /** Vault-relative path the entry resolves to, or `null` for headings. */
  path: string | null;
}

export interface PaginatedFolder {
  /** Vault-relative path the listing came from (normalized). */
  path: string;
  /** Visible entries for the requested page. */
  entries: FolderEntry[];
  /** Total entries across all pages (so the UI can show "X of Y"). */
  total: number;
  /** Zero-indexed page number; `null` when there are no entries. */
  page: number;
  /** True if `page + 1` exists. */
  hasNextPage: boolean;
}

/**
 * Enumerate one page of a projection folder.
 *
 * Returns `null` when `path` isn't a projection path — the caller
 * should let Obsidian's default file enumeration take over.
 */
export async function enumerateFolder(
  client: Pick<DaemonClient, "call">,
  path: string,
  page = 0,
  pageSize = DEFAULT_PAGE_SIZE,
): Promise<PaginatedFolder | null> {
  if (!isProjectionPath(path)) return null;
  const normalized = normalizeProjectionPath(path);
  const result = (await client.call("path.list", {
    path: normalized,
    page,
  })) as { markdown?: string };
  const markdown = typeof result?.markdown === "string" ? result.markdown : "";
  const allEntries = parseFolderMarkdown(markdown);
  const start = page * pageSize;
  const end = start + pageSize;
  return {
    path: normalized,
    entries: allEntries.slice(start, end),
    total: allEntries.length,
    page,
    hasNextPage: end < allEntries.length,
  };
}

/**
 * Parse the daemon's path-list markdown into entries. The
 * renderer's convention is one bullet per entry:
 *
 *   - [Sara Chen](contacts/by-name/S/Sara_Chen.md)
 *   - [Sarah Park](contacts/by-name/S/Sarah_Park.md)
 *
 * Heading lines (`## ...`) and blank lines are ignored. Free-form
 * paragraph text becomes a label-only entry (path = null) so the
 * paginator can still surface it.
 */
export function parseFolderMarkdown(markdown: string): FolderEntry[] {
  const entries: FolderEntry[] = [];
  for (const raw of markdown.split("\n")) {
    const line = raw.trimEnd();
    if (line.length === 0) continue;
    if (line.startsWith("## ") || line.startsWith("# ")) continue;
    if (line.startsWith("- ") || line.startsWith("* ")) {
      const body = line.slice(2).trim();
      const linkMatch = body.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      if (linkMatch) {
        entries.push({ label: linkMatch[1], path: linkMatch[2] });
      } else {
        entries.push({ label: body, path: null });
      }
    }
  }
  return entries;
}

/**
 * Decoration class for a single projection file in Obsidian's
 * explorer — production passes this into Obsidian's `setCssClass`
 * helper so projection files render with a distinct icon /
 * background.
 */
export function decorateProjectionFile(path: string): string | null {
  if (!isProjectionPath(path)) return null;
  return "ffs-projection-file";
}
