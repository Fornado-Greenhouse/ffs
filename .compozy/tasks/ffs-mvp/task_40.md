---
status: pending
title: Courier intake contract + `ffs_search` v2 + URL dedup (the newspaper comes in)
type: backend
complexity: medium
dependencies:
  - task_36
  - task_37
  - task_38
---

# Task 40: Courier intake contract + `ffs_search` v2 + URL dedup (the newspaper comes in)

## Overview
The north-star workflow is "read the paper end to end every morning, then keep the filing cabinets current." A courier agent (Hermes, OpenClaw, Claude Code — any host) receives the Charlotte Business Journal email, opens every link with the owner's logged-in session, and hands each article to FFS. **None of the fetching is FFS.** What FFS owns is the *contract* the courier must satisfy so that an article lands as a `source.article` entity with its people, organizations, and events cross-linked into the vault, and so that reading the same article twice updates one record instead of filing a duplicate.

Three gaps block that today. First, there is no published ingest shape for an article: the scribe guesses from free text, and task_36's engines need a stable `predicate:` hint plus a `## Mentions` section to do multi-entity extraction reliably. Second, nothing deduplicates by URL — a re-read (the email arrives twice, the courier retries) would create a second article entity with the same title. Third, `entity.search` matches only the hardcoded `display_name` / `title` field, so `ffs_search "Acme"` cannot find an org filed under its alias, a person by their organization, or an article by its publication — which makes the convention's search-before-write loop (ADR-027) too weak for a graph with aliases. This task publishes the contract, ships a reference courier skill, adds URL dedup, and upgrades search to match every string field the predicate spec declares.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST publish the article ingest contract in `docs/agent-memory/CONVENTION.md` (new section) and in the courier skill: one markdown file per article in `$FFS_DATA_DIR/ingest/` with YAML frontmatter `predicate: source.article`, `title`, `url` (both required), `publication`, `published_at` (ISO 8601 date), `byline`, `tags` (list); a body that is the agent-written summary (not the article text); a `## Mentions` section of bullets `- <Name> — <one-line role/org context>` for people and organizations; an optional `## Events` section of bullets `- <kind>: <one-line description>` using the `event.business` kinds from ADR-028. The contract MUST state that the courier never includes full article text (copyright and bulk), and the scribe MUST truncate any body over a configurable limit (default 4 000 characters) and attach a `parse-warning` rationale so the truncation is visible in the quarantine.
- MUST make the scribe honor an explicit `predicate:` frontmatter hint: when present and registered, the primary proposal uses that predicate (validated against its `claim_schema` as today); an unregistered hint falls back to the current inference path with a warning. `## Mentions` bullets become `person.generic` / `org.company` proposals (task_36's multi-entity path); `## Events` bullets become `event.business` proposals; every secondary proposal carries the article's URL in its provenance and the article's entity id in its `mentions[]` / `sources` field per ADR-028.
- MUST deduplicate articles by normalized URL. Normalization (stdlib only, in `skills/_lib/`): lowercase scheme and host, strip `utm_*` / `fbclid` / `gclid` / `mc_cid` / `mc_eid` query params, drop fragments, collapse a trailing slash. The article entity id is derived deterministically as `<publication-slug>-<YYYY-MM-DD>-<title-slug>` so the same article resolves to the same entity; task_36's daemon-side entity resolution MUST additionally match on normalized `url` so a changed title on re-read still updates the existing entity (a supersession) rather than creating a sibling. Both submissions' provenance MUST survive on the chain.
- MUST specify the daily digest: the courier drops one `note` per run titled `CBJ digest YYYY-MM-DD` (publication-prefixed, date-suffixed) whose `## References` section lists every article filed that day as a wikilink to the article's projection. The scribe MUST populate the note's `references[]` from those bullets (the field exists in `starter/predicates/note.toml`; today the scribe does not fill it). The digest note is how "did I read the paper end to end?" is answered from the vault.
- MUST upgrade `entity.search` (daemon) to match across every string and string-array property declared in the predicate's `claim_schema` (e.g. `display_name`, `title`, `aliases`, `organization`, `tags`, `publication`), not only the canonical name field. Parameters gain optional `predicate` (exact predicate filter) and `family` (path-family filter per ADR-028). Each hit gains `path` (from `path_for_entity`) and `matched_on` (list of field names that matched). Results keep the lightweight shape (no claim bodies) and remain capability-filtered per hit. Ordering: exact canonical-name match first, then alias match, then any other field; ties by `tx_time` descending.
- MUST update the `ffs_search` MCP tool schema and translator for the new optional `predicate` and `family` arguments and the enriched hit shape; `SEARCH_DEFAULT_LIMIT` / `SEARCH_MAX_LIMIT` unchanged. `ffs_query` and `ffs_render_projection` are unchanged.
- MUST ship `docs/agent-memory/skill/ffs-courier/SKILL.md`: an instructional, host-agnostic skill (no `entry_point`; not installed under `$FFS_DATA_DIR/skills/`) that describes the courier loop — read the email, enumerate links, open each with the owner's session, write one ingest file per article per the contract, write the digest note, then confirm filing with `ffs_search` (or `ffs ls articles/recent/`) and report per the convention's persistence honesty (a dropped file is "submitted", a quarantined proposal is "proposed", only an accepted atom is "filed"). It MUST state plainly that credentials, session handling, and the publication's terms of use are the owner's responsibility and that no fetch, browser, or email logic lives in FFS. It MUST include one complete worked example ingest file and one digest note.
- MUST NOT add any pip dependency to `skills/` and MUST NOT change the skills-host wire protocol. URL normalization and slugging live in `skills/_lib/` (stdlib) and, where the daemon needs the same normalization for entity resolution, in `ffs-core` (Rust) with a shared fixture file so both implementations agree.
- SHOULD add a "Read the paper for me" section to `docs/onboarding/first-use-guide.md` (what to install on the agent side, what shows up in the vault, how to tell it worked) and list the courier skill in `docs/agent-memory/README.md`.
</requirements>

## Subtasks
- [ ] 40.1 Article ingest contract written into `CONVENTION.md` (new section) with the frontmatter fields, `## Mentions` / `## Events` bullet grammar, the no-full-text rule, and the digest-note shape.
- [ ] 40.2 Scribe: `predicate:` frontmatter hint honored; body truncation with `parse-warning`; `## Mentions` → `person.generic` / `org.company` proposals, `## Events` → `event.business` proposals, article id threaded into their `mentions[]`; `## References` bullets populate `note.references[]`.
- [ ] 40.3 URL normalization + article id slugging in `skills/_lib/` (stdlib) and the matching Rust helper in `ffs-core`; shared fixture of (raw url → normalized url) pairs consumed by both test suites.
- [ ] 40.4 Daemon-side entity resolution extended to match `source.article` by normalized `url` (task_36's resolver); second submission of the same URL supersedes the head and keeps both provenance entries.
- [ ] 40.5 `entity.search` v2: schema-declared string fields, `predicate` / `family` filters, `path` + `matched_on` on hits, documented ordering; `EntitySearchParams` / `EntitySearchHit` updated in `api.rs`.
- [ ] 40.6 `ffs_search` MCP tool: schema + translator for `predicate` / `family`; unit tests; integration test asserting an alias match returns `matched_on: ["aliases"]` and a `path`.
- [ ] 40.7 `docs/agent-memory/skill/ffs-courier/SKILL.md`, `docs/agent-memory/README.md` listing, first-use-guide section; live validation by hand-dropping two CBJ-shaped article files plus a digest note.

## Implementation Details
Current structure: `crates/ffs-daemon/src/ingest_watcher.rs` submits any eligible `.md` under `ingest/` after the stability window and moves it to `processed/`; `crates/ffs-daemon/src/scribe.rs` translates skill proposals into quarantine `Proposal`s; `skills/scribe/extraction.py` parses frontmatter tolerantly (`parse_markdown`) but only consults `name` / `title` / `email` / `phone` / `org` keys and never a `predicate` hint. `entity.search` in `crates/ffs-daemon/src/dispatch.rs` iterates `registry.names()`, lists atoms per predicate, and substring-matches one hardcoded field (`title` for `note`, else `display_name`); the predicate registry already exposes each spec's `claim_schema`, which is where the v2 field list comes from.

The contract is deliberately a superset of what the heuristic engine can consume: with `FFS_SCRIBE_ENGINE=heuristic` the `predicate:` hint plus structured `## Mentions` bullets yield correct proposals without an LLM, so the courier path works on a fresh install; the `llm` engine improves extraction quality for the summary body but is not required.

Persistence-honesty vocabulary for the courier (mirrors ADR-027): *submitted* (file dropped, watcher will pick it up), *proposed* (in quarantine), *filed* (accepted atom, visible in the vault), *auto-filed* (accepted by an ADR-029 policy). The skill uses exactly these words.

### Relevant Files
- `skills/scribe/extraction.py` — `predicate:` hint, truncation, `## Mentions` / `## Events` / `## References` handling.
- `skills/_lib/ffs_skill.py` or new `skills/_lib/urlnorm.py` — URL normalization + slug helpers (stdlib).
- `crates/ffs-core/src/` — Rust URL normalization helper for entity resolution (new small module).
- `crates/ffs-daemon/src/scribe.rs` — resolver: match `source.article` by normalized url.
- `crates/ffs-daemon/src/dispatch.rs`, `crates/ffs-daemon/src/api.rs` — `entity.search` v2 params, hits, ordering.
- `crates/ffs-mcp/src/tools.rs` — `ffs_search` schema/translator; tests.
- `docs/agent-memory/CONVENTION.md`, `docs/agent-memory/README.md`, `docs/agent-memory/skill/ffs-courier/SKILL.md` — contract and skill.
- `starter/predicates/source.article.toml`, `starter/predicates/note.toml` — read-only inputs (ADR-028 shapes).

### Dependent Files
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — URL dedup e2e; article + mentions e2e.
- `crates/ffs-mcp/tests/mcp_integration.rs` — alias-match integration test.
- `skills/scribe/tests/corpus/` — CBJ-shaped fixtures added to task_36's golden corpus.
- `docs/onboarding/first-use-guide.md` — "Read the paper for me" section.

### Related ADRs
- [ADR-028] — business-graph predicates and registry-declared path families (the shapes this contract targets).
- [ADR-027: Adopt the OKF Agent Memory Convention](adrs/adr-027.md) — search-before-write and persistence honesty the courier skill enforces.
- [ADR-026: Scribe v2](adrs/adr-026.md) — engine seam and schema-driven prompts; the `predicate:` hint is an input to both engines.
- [ADR-009: Claw integration](adrs/adr-009.md) — stdlib-only skills; the courier skill is instructional, host-agnostic.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — `ffs_search` stays a thin pass-through; filtering happens daemon-side.

## Deliverables
- Article ingest contract (convention section) + `ffs-courier` skill with worked examples **(REQUIRED)**.
- Scribe support for `predicate:` hint, `## Mentions` / `## Events` / `## References`, body truncation **(REQUIRED)**.
- URL normalization (Python + Rust, shared fixtures) and article dedup via entity resolution **(REQUIRED)**.
- `entity.search` v2 + `ffs_search` schema update with tests **(REQUIRED)**.
- Onboarding + `docs/agent-memory/README.md` updates.
- Unit tests with 80%+ coverage on new Python and Rust modules **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `test_normalize_url_strips_tracking_params_and_fragment` — `utm_*`, `fbclid`, `gclid`, `#frag` removed; host lowercased; trailing slash collapsed.
  - [ ] `test_normalize_url_is_idempotent` — normalizing twice equals once.
  - [ ] `test_article_entity_id_is_deterministic` — same publication/date/title → same id; differs on any change.
  - [ ] `test_predicate_hint_selects_source_article` — fixture with `predicate: source.article` yields a primary `source.article` proposal with `title`, `url`, `publication`, `published_at`.
  - [ ] `test_mentions_section_yields_person_and_org_proposals` — each bullet becomes a `person.generic` or `org.company` proposal carrying the article id in `mentions[]`.
  - [ ] `test_events_section_yields_event_business_proposals` — kind parsed from the bullet prefix.
  - [ ] `test_body_over_limit_is_truncated_with_parse_warning`.
  - [ ] `test_digest_note_references_are_populated_from_bullets`.
  - [ ] Rust `url_normalization_matches_shared_fixtures` — the Rust helper agrees with every pair in the shared fixture file.
  - [ ] `entity_search_matches_alias_field` / `entity_search_matches_tags_field` / `entity_search_filters_by_predicate` / `entity_search_orders_exact_name_first` — dispatcher unit tests.
  - [ ] `ffs_search_passes_predicate_and_family_filters` / `ffs_search_hit_shape_includes_path_and_matched_on` — MCP translator tests.
- Integration tests:
  - [ ] `same_url_submitted_twice_yields_one_article_entity_with_two_provenance_entries` — `ingest_pipeline_e2e`.
  - [ ] `article_with_mentions_files_person_org_and_event_entities` — after accept, `articles/`, `people/`, `orgs/` projections exist and the article's rendered markdown contains wikilinks to each mention.
  - [ ] `ffs_search_finds_org_by_alias_end_to_end` — `mcp_integration`.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- Two hand-dropped CBJ-shaped article files plus one digest note produce `articles/`, `people/`, and `orgs/` files in the vault, with wikilinks from the article to each mention and from the digest to each article.
- Dropping the same article again (same URL, tweaked title) updates the existing article rather than creating a second one; both provenance entries are visible via `ffs_query`.
- `ffs_search "Acme"` finds an organization filed under the alias "Acme Corp" with `matched_on: ["aliases"]` and a usable `path`.
- The courier skill, the convention section, and the scribe agree byte-for-byte on the frontmatter keys and section names; a reader of the skill alone can produce a valid ingest file.
- Default install privacy is unchanged: no network calls from FFS; the courier's fetching happens entirely in the agent host.
