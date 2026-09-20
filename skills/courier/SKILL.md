---
name: courier
kind: courier
entry_point: courier.py
python: python3
timeout_ms: 300000
schedule: "0 7,17 * * *"
---

# Courier

The deterministic, scheduled half of the newspaper goal (ADR-035,
task_40). Reads the owner's own mailbox and the configured public
record or permitted feeds, and writes one ingest file per new item
under the article contract plus the day's digest note. Extraction is
the scribe's job (task_36), identity is the resolver's (task_45). The
courier has no LLM client and no browser driver.

## What the owner decides

`$FFS_DATA_DIR/config/sources.toml` sets, per publisher, `intake =
"pointer" | "clip"` and `fetch = "off" | "session" | "scheduled"`.
The bundle applies those values as written and carries no domain
list of its own (ADR-035 as amended 2026-09-15). The starter file
keeps the terms-restricted publishers at `pointer` / `session`, so
their articles are read in the owner-present morning read (task_48).

`$FFS_DATA_DIR/config/courier.toml` names the mailbox, its sources,
the feeds, the EDGAR declaration, output options, and the
tracking-wrapper table. Secrets never live in TOML: the mailbox
secret is the OS keychain item `ffs.courier.<host>`; owner-exported
cookies for a `scheduled` publisher are `ffs.courier.cookies.<domain>`.

## The article contract

One file per item in `ingest/`, named
`<publication-slug>-<YYYY-MM-DD>-<title-slug>.md`, frontmatter keys in
this order: `predicate: source.article`, `title`, `url`, `publication`,
`published_at`, `byline`?, `tags`?, `intake`, `reported_by`?,
`content_hash`?, `fetch`. Body: empty for `pointer`, the blurb or
article text for `clip`, the record's summary for `feed`. Then
`## Mentions` bullets `- <Name> — <context>` and `## Events` bullets
`- <kind>: <description> | <Name> (<role>), ...` only when non-empty.
The courier never writes an entity id. The digest note is
`<publication-slug>-digest-<date>.md` with `## References` wikilinks.

## Wire shape

Input: `{"op": "tick", "dry_run": false, "ticks": ["mailbox", "feeds"]}`
(all optional) or `{"op": "status"}`.

Output per tick: `{tick, dry_run, items_seen, files_written, files,
would_submit, fetch_requests, fetch_failures, skipped_seen, warnings,
last_error}`. A real tick says "submitted"; a dry run says "would
submit"; the courier never says "filed". After a real tick it writes
`ingest/.courier/last_run.json`, which `health.summary.courier` reads.

## Schedule

`schedule` in the frontmatter is advisory until the daemon scheduler
(task_41) lands; `ffs courier run [--dry-run]` invokes one tick by
hand, and `python courier.py run --dry-run` works without a host.

## ADRs

- ADR-009 — skill bundles are stdlib-only, `SKILL.md`-shaped.
- ADR-035 — the morning read is a session; publisher policy is the
  owner's `sources.toml`.
- ADR-027 — persistence honesty: submitted, proposed, filed, auto-filed.
