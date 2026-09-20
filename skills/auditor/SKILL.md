---
name: auditor
kind: auditor
entry_point: audit.py
python: python3
timeout_ms: 30000
---

# Auditor

The substrate's daily-health reporter. Aggregates metrics over a
24-hour window, applies threshold rules to surface anomalies, and
authors an `auditor.daily_summary` atom into the substrate. The
Obsidian plugin's daily-health-summary panel (task 19) renders the
latest one; `ffs health` reads it from the CLI.

## Metrics aggregated

Per TechSpec § Monitoring and Observability:

- `atom_author_rate` — atoms authored in the last 24h.
- `proposals` — pending scribe submissions awaiting user acceptance.
- `drift_flags` — working-set entries whose render hash diverged.
- `capability_denials_per_agent` — counts of denials per agent key.
- `federation_pull_failure_rate_per_peer` — failed-pull ratio per peer.
- `fast_path_vs_slow_path_ratio` — fast-path applies vs slow-path routes.
- `working_set_size` — current materialized projection count.
- `ingest_queue_depth` — backlog of pending scribe submissions.

## Threshold flags

- >10 capability denials per agent per day → "agent X attempted
  out-of-scope writes".
- federation pull failure rate >50% over 24h → "bridge with peer X
  is unhealthy".
- ingest queue depth >100 → "you have a backlog of scribe proposals".
- fast-path / slow-path ratio inversion (slow > fast) → "consider
  predicate-spec coverage".

## Panel limit

The user-visible panel shows the top 5 items by priority. Priorities
(highest first): federation health, capability denials, fast-path
inversion, drift flags, ingest backlog.

## Wire shape

Input from the host (`invoke.input`):

```json
{ "op": "tick" }
```

Returns `{"atom_hash": "..."}` for the published summary, or
`{"atom_hash": null, "reason": "..."}` if publishing is unavailable.

## Morning briefing (`op: briefing`, task_41)

A second, longer output: the movers-and-shakers briefing, published
as an `auditor.briefing` atom (one fresh entity per briefing; the
daemon renders it at `briefings/<date>.md`). The derivations live in
`briefing.py`; `audit.py` only dispatches. The five-item daily
summary is unchanged.

```json
{ "op": "briefing" }
{ "op": "briefing", "window_days": 14 }
```

Returns `{"atom_hash": "...", "reason": null, "counts": {"new_people": 1,
"changes": 2, ...}}` (`counts` is the per-section length; `events`
counts items across kinds), or `{"atom_hash": null, "reason": "..."}`.

### Window and cadence

- `window.from` = the previous briefing's `claim.window.to` (read via
  `audit.query {"kind": "briefing"}`, newest first); first run: now
  minus the cadence. `window.to` = now (UTC). `window_days` overrides
  `from` for a manual run.
- Atoms count as in-window when `window.from < tx_time <= window.to`.
  The prior window (for trending) is the same length immediately
  before `from`.

### Environment

| Variable | Default | Meaning |
|---|---|---|
| `FFS_AUDITOR_BRIEFING_INTERVAL` | `7d` | Cadence; `<n>d`, `<n>h`, `<n>m`, `<n>s`. `1d` for a daily briefing. Recorded in the claim as `cadence`. |
| `FFS_BRIEFING_PROMOTE_MIN_ARTICLES` | `3` | A `person.generic` mentioned in at least this many distinct in-window articles is a promotion candidate. |
| `FFS_BRIEFING_ATOM_CEILING` | `500` | `limit` for every `atom.list` call (clamped to 1..5000). A list that comes back at the limit sets `truncated: true` and the narrative says "lists truncated". |
| `FFS_BRIEFING_LIST_MAX` | `20` | Top-N cap on every output list (events: total items across kinds). |

### What the auditor reads

Only reads, then one write (the briefing atom). No `ingest.accept`,
no `entity.merge`, nothing else.

- `audit.query {"kind": "briefing"}` — the previous briefing.
- `atom.list {"predicate", "since", "limit"}` (entity-less form) for
  the six business predicates plus `entity.same_as` in the window;
  `source.article` and `event.business` again for the prior window;
  `person.generic`, `org.company`, `contact.person`, `affiliation`,
  `entity.same_as`, `entity.different_from` with no `since` (live
  heads, up to the ceiling).
- `atom.get {"hash"}` for the parent of an in-window `affiliation` or
  `org.company` supersession not already fetched.
- `ingest.list_pending {}` for the resolver's `ambiguous` proposals.

A query the host refuses degrades its section to empty with a warn
log; the briefing publishes anyway.

### RPC check

A windowed list-by-predicate could not be expressed with the RPCs
that existed: `atom.list` required an `entity`, `audit.query` reads
auditor atoms only, and `entity.search` is ranked and limited (a
resolver surface, not an enumeration). Rather than add a method, the
daemon extended `atom.list` with an entity-less
`{"predicate", "since"?, "limit"?}` form: every atom of that
predicate with `tx_time > since`, newest first, capability-filtered,
each row carrying `hash`, `entity`, `supersedes`, `valid_from`,
`valid_to`, `tx_time`, and `provenance`. The entity-scoped form is
unchanged. `audit.query` gained `kind` and `audit.publish_summary`
gained `predicate` (restricted to the two auditor predicates).

### Claim contract (summary)

Every entity reference is a ref `{"entity": "<opaque id>" | null,
"display": "<text>"}`; ids are opaque and permanent (ADR-030) and
nothing is keyed by name. Fields:

- `date` (`YYYY-MM-DD` of `window.to`; the file basename), `window`
  `{from, to}`, `cadence`, `generated_at`, `narrative` (one plain
  paragraph with the counts), `truncated`, `ceiling`.
- `new_people[]` `{entity, display, organization: ref|null,
  first_seen_article: ref|null}` — ids whose root `person.generic`
  atom (`supersedes == null`) landed in the window. A renamed person
  keeps her id and is not new.
- `changes[]` `{entity, display, kind, organization, from, to,
  as_reported, source_article}` with `kind` one of `joined` (root
  `affiliation` atom), `left` (supersession setting `valid_to`),
  `retitled` (supersession changing `title`), `org_changed`
  (`org.company` supersession changing `display_name` or
  `location`). A person scalar supersession (location, role) is not
  a change; a new `mentions[]` entry is a mention.
- `trending_orgs[]` `{entity, display, mentions_this_window,
  mentions_prior_window}` — article mentions plus event participants
  by org id; listed when this window >= 2 and > prior.
- `events[]` `{kind, items: [{entity, display, date, participants:
  [{entity|null, display, role|null}]}]}` — newest head per event,
  grouped by kind in first-seen order, items newest first.
- `promotion_candidates[]` `{entity, display, organization,
  mention_count, article_count, reason}` — live `person.generic` ids
  without a `contact.person` head, by the article threshold or by
  sharing an organization (open `affiliation` atom, or the
  `organization` scalar equal to an org id) with an existing contact.
- `follow_ups[]` `{entity, display, organization, triggering_article}`
  — contacts with an open affiliation (fallback: `organization`
  scalar equal to an org id) whose organization was mentioned.
- `needs_your_eye[]` `{submission_id, local_ref, predicate, display,
  candidates: [{entity, display, score}]}` — pending proposals with
  `resolution == "ambiguous"`.
- `possible_duplicates[]` `{family, entity_a, entity_b,
  shared_aliases}` — live heads in one family (`people`, `orgs`,
  `contacts`) sharing a lowercase-trimmed display name or alias, not
  already related by a live `entity.same_as` / `entity.different_from`
  atom; ordered by overlap.
- `recent_merges[]` `{same_as_hash, source, target, tx_time}`.
- `filing` `{auto_filed_count, reviewed_count}` over the in-window
  atoms of the six business predicates (`auto_accept` provenance vs
  the rest).

Every list is sorted deterministically, so two runs over the same
atoms produce the same claim.

### "As reported"

`changes[].as_reported` is the affiliation atom's `valid_from` date
(`valid_to` for `left`), which the resolver set from the article's
publication or the announcement. The press reports announcements,
not start dates: render it as "as reported <date>", never as a start
date. Known gap carried from the 2026-09-14 plan review.

## ADRs

- ADR-013 — MCP server in MVP. `ffs_audit_query` MCP tool reads
  auditor summary atoms (`kind: briefing` for the latest briefing).
- ADR-030 — opaque permanent ids: what "new" means; `ambiguous`
  outcomes surface under "Needs your eye".
- ADR-031 — `affiliation` is its own role atom; joins, departures,
  and retitles are read from affiliation activity.
