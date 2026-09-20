---
name: ffs-courier
description: Run the FFS courier loop from an agent host (Hermes, OpenClaw, Claude Code) instead of the daemon-hosted bundle. Use when the owner asks the assistant to file today's newsletter items or feed records into their FFS substrate. Reads the same courier.toml and sources.toml as skills/courier/ and writes byte-identical ingest files.
---

# FFS Courier (agent-hosted alternative)

The courier is normally the deterministic skill bundle at `skills/courier/`, run on the daemon's schedule. An agent host may run the same contract instead. This skill is that statement and a worked example, nothing more. It has no `entry_point`; do not install it under `$FFS_DATA_DIR/skills/`.

## The rule that governs everything

An agent courier reads the same `$FFS_DATA_DIR/config/courier.toml` and `$FFS_DATA_DIR/config/sources.toml` as the bundle and honors them exactly. It does not decide for the owner. A publisher whose `fetch` is `session` gets its articles opened only during the morning read (the `ffs-morning-read` skill, task_48), with the owner present and one at a time on the owner's cue. A publisher whose `fetch` is `off` is never fetched. A publisher whose `fetch` is `scheduled` may be fetched at a human pace, only for URLs decoded from the owner's own digest, under the publisher's `daily_cap`.

What the courier refuses is behavior, not a domain: "open all of today's links", "crawl the section page", "get past the login". Those are refused no matter which publisher they name. Which publishers may be fetched at all is the owner's `sources.toml`.

## The loop

1. **Read the digest email** from the mailbox and folder in `courier.toml`, matching each `[[mailbox.source]]` by sender and subject filter.
2. **Decode tracking links** to canonical article URLs (`link.bizjournals.com/click/<campaign>/<base64url>/...` and the other wrappers listed in `courier.toml`) without clicking them, then normalize each URL (lowercase scheme and host; strip `utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid`; drop the fragment; collapse a trailing slash).
3. **Skip what is already filed**: check the courier ledger (`$FFS_DATA_DIR/ingest/.courier/seen.json`) and the substrate (`ffs_search` with the URL and `predicate: source.article`).
4. **Write one ingest file per new item** per the article intake contract (Convention, Section 13), with `intake` and `fetch` taken from the publisher's `sources.toml` entry: `pointer` writes title, url, date, and an empty body; `clip` also writes the item's body and its `reported_by` attribution when the item names another outlet. Never write an entity id.
5. **Write the digest note** for the publication and day, listing every article filed as `[[<basename>|<title>]]`.
6. **Confirm and report** with the persistence vocabulary: after the stability window, `ffs_search` the URL (or `ffs ls articles/recent/`) and say **submitted** for files dropped, **proposed** once they appear in the quarantine, and never **filed** unless the owner accepted them.

## Worked example

Input: one synthetic digest email from a publisher whose `sources.toml` entry is `intake = "pointer"`, `fetch = "session"`. The bundle at `skills/courier/` writes exactly these two files for that input; the fixtures live at `skills/courier/tests/fixtures/expected/`.

Article file, `example-ledger-2026-09-14-widget-maker-breaks-ground-on-second-plant.md`:

```markdown
---
predicate: source.article
title: Widget maker breaks ground on second plant
url: https://news.example-ledger.test/news/2026/09/14/widget-maker-second-plant.html
publication: Example Ledger
published_at: 2026-09-14
intake: pointer
fetch: session
---
```

Digest note, `example-ledger-digest-2026-09-14.md`:

```markdown
---
predicate: note
title: Example Ledger digest 2026-09-14
tags: [digest, courier]
---

Filed by the courier.

## References
- [[example-ledger-2026-09-14-widget-maker-breaks-ground-on-second-plant|Widget maker breaks ground on second plant]]
- [[example-ledger-2026-09-14-downtown-tower-sells-for-41m|Downtown tower sells for $41M]]
- [[example-ledger-2026-09-14-regional-bank-names-new-chief-lending-officer|Regional bank names new chief lending officer]]
```

A clip item from a newsletter whose policy is `intake = "clip"` differs in two ways: the body carries the blurb, and `reported_by` names the outlet the item attributed, if any.

## Report shape

After a tick, tell the owner: how many items were seen, how many files were submitted (or would be, in a dry run), which were skipped as already filed, and any fetch failures for `scheduled` publishers. Use the words submitted, proposed, filed, and auto-filed as the Convention defines them.

## Related

- `docs/agent-memory/CONVENTION.md`, Section 13 (the article intake contract) and Section 18 (persistence honesty).
- `skills/courier/SKILL.md` for the bundle this skill mirrors.
- ADR-035 (the morning read is a session, not a scrape; publisher policy is the owner's `sources.toml`), ADR-034 (`reported_by` and source independence), ADR-030 (the courier never supplies entity ids).
