---
status: pending
title: Courier — deterministic email-to-ingest skill bundle + `ffs_search` v2 + URL dedup (Hermes optional)
type: backend
complexity: medium
dependencies:
  - task_37
  - task_38
  - task_43
  - task_45
---

# Task 40: Courier — deterministic email-to-ingest skill bundle + `ffs_search` v2 + URL dedup (Hermes optional)

## Overview
The north-star workflow is "read the paper end to end every morning, then keep the filing cabinets current." The first link in that chain is moving the Charlotte Business Journal digest email into `$FFS_DATA_DIR/ingest/` as one markdown file per article plus a daily digest note. Reshaped 2026-09-14: that link does not need an agent. An LLM is not needed to move text from an email into a folder; extraction is the scribe's job (task_36), identity is the resolver's (task_45), and the courier's job is deterministic. So the courier is an FFS skill bundle, `skills/courier/`, the fourth agent ADR-009 named and the only one that had no home. It is stdlib-only Python like the auditor: `imaplib` and `email` for the mailbox, `urllib` with an optional cookie jar for full-page fetch, credentials from the OS keychain per task_27, run on a daemon schedule like the auditor, writing files that satisfy the intake contract below.

A Hermes, OpenClaw, or Claude Code agent remains supported as an alternative courier through the same contract: the existing `docs/agent-memory/skill/ffs-courier/SKILL.md` stays, reframed as the agent-hosted alternative for owners who want an agent to read the paper, and for publications whose email carries links but no text. The contract is what FFS owns; the bundle is the reference implementation of it.

Full fetch of article pages is an opt-in decided by spike task_43. The default is email-only intake: the digest's headline and blurb per item. Spike 43 must show that email-only covers at least 70 percent of the people and organizations a full read would yield; if it does, most owners never turn on fetch, and the bot-wall and terms-of-use questions the spike raises stay the owner's opt-in choice rather than the pipeline's default.

Three gaps this task closes are unchanged from the original scope. There is no published ingest shape for an article, so the scribe guesses from free text and the engines have no stable `predicate:` hint or `## Mentions` section to do multi-entity extraction reliably. Nothing deduplicates by URL, so a re-read files a second article. And `entity.search` matches only the hardcoded `display_name` / `title` field, so `ffs_search "Acme"` cannot find an org filed under its alias, which makes the convention's search-before-write loop (ADR-027) too weak for a graph with aliases.

Amended 2026-09-14 (ADR-030, ADR-031). Entity ids are opaque and permanent (ADR-030), so the courier never supplies ids and the article's `<publication>-<date>-<title>` slug is a projection basename and a resolver blocking key, not the identity; the dedup keys are the normalized `url` and an optional `content_hash`. Mentions and event participants are entity references, not names (ADR-031): `## Mentions` bullets become `{entity, display, context}` items and `## Events` bullets carry participants with a role in the event, with `entity` filled in by the resolver. `entity.search` v2 is the resolver's candidate generator with the Wikidata and Fellegi-Sunter lessons built in: it ranks by exact name, alias, prior count, then full-text rank; it follows `entity.same_as` chains; it excludes anything the caller's context is `different_from`; and it exposes the reconciliation shape (query, candidates with score and `matched_on`) that a W3C Reconciliation Service facade could wrap later.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST ship the courier as a daemon-hosted skill bundle `skills/courier/` per ADR-009: `SKILL.md` with frontmatter `name: courier`, `kind: courier`, `entry_point: courier.py`, `python: python3`, `timeout_ms`, and a `schedule` (cron-style or interval; the daemon's scheduler from task_41 subtask 41.3 drives it, and until that lands `ffs courier run` invokes one tick by hand); `definition.atom.json`; `courier.py`; `tests/`. Zero pip dependencies: `imaplib`, `email`, `urllib.request`, `http.cookiejar`, `json`, `hashlib`, `tomllib`, and the `ffs_skill` helper only. The bundle MUST NOT contain an LLM client; extraction is the scribe's.
- MUST read configuration from `$FFS_DATA_DIR/config/courier.toml` (seeded by the installer from `starter/config/courier.toml`) with environment overrides: `[mailbox]` host, port, folder, `sender_filter` (list of from-addresses or domains), `subject_filter` (optional regex), `mark_seen` (bool); `[fetch]` mode `off | cookies` (default `off`), `cookie_jar` path (Netscape format, owner-provided, only read when mode is `cookies`), `timeout_s`, `max_bytes`; `[output]` publication name, digest title prefix. Secrets MUST NOT live in TOML: the mailbox password or app token and any session cookies come from the OS keychain (service `ffs.courier.<host>`, via the task_27 keyring helpers exposed through `ffs_skill`) or from `FFS_COURIER_MAIL_PASSWORD` for tests, never from config or argv.
- MUST implement one tick as: connect to the mailbox, select the folder, find unseen messages matching the sender and subject filters, and for each message parse the HTML or text body into items (headline, blurb, link) using `email` and `html.parser`; normalize every link per the URL rule below; skip any item whose normalized URL already appears in the courier's local ledger (`$FFS_DATA_DIR/ingest/.courier/seen.json`, keyed by normalized URL with the date first seen) or in the substrate (an `entity.search` call with `predicate: source.article` and the URL); write one ingest file per new item per the intake contract; write or append the day's digest note; update the ledger; mark the message seen only when every item's file was written. A second tick over the same message MUST write zero new files.
- MUST support `fetch.mode = "cookies"` as an explicit opt-in gated on spike task_43: fetch each item's normalized URL with the owner's cookie jar, respect `max_bytes` and `timeout_s`, extract the main text with `html.parser` heuristics (title, byline, paragraphs), compute `content_hash` (BLAKE3 is not in stdlib; use `hashlib.blake2b` with 32-byte digest, multibase base58btc, and document that the daemon's Rust side accepts either `blake3` or `blake2b` prefixed hashes for `content_hash`), and write the summary body from the first N paragraphs. A fetch failure (HTTP error, bot wall, timeout) MUST fall back to email-only for that item and record `fetch: failed <reason>` in the ingest file's frontmatter so the quarantine shows what the courier could not read. The bundle MUST never include full article text in the ingest file regardless of mode; the contract's no-full-text rule holds.
- MUST provide a dry-run mode (`FFS_COURIER_DRY_RUN=1` or `ffs courier run --dry-run`) that performs every step but writes ingest files and the digest to a scratch directory (`$FFS_DATA_DIR/ingest/.courier/dry-run/<timestamp>/`) and does not mark messages seen or update the ledger; the tick's result JSON lists the files it would have written.
- MUST publish the article ingest contract in `docs/agent-memory/CONVENTION.md` (new section) and in both courier skills: one markdown file per article in `$FFS_DATA_DIR/ingest/` with YAML frontmatter `predicate: source.article`, `title`, `url` (both required), `publication`, `published_at` (ISO 8601 date), `byline`, `tags` (list), optional `content_hash` (multibase-encoded digest of the fetched page bytes, supplied only when the courier fetched; ADR-031's information-bearer key), and optional `fetch` (`email-only | fetched | failed <reason>`); a body that is the summary (the email blurb in email-only mode, the courier's first paragraphs in fetch mode, never the article text); a `## Mentions` section of bullets `- <Name> — <one-line role/org context>` for people and organizations, where each bullet becomes one `mentions[]` item `{entity, display, context}` with `display` and `context` taken verbatim from the bullet and `entity` filled in by task_45's resolver (the courier never supplies entity ids, per ADR-030); an optional `## Events` section of bullets `- <kind>: <one-line description> | <Name> (<role-in-event>), <Name> (<role-in-event>)` using the `event.business` kinds from ADR-028 and the participant roles from ADR-031, where each named participant becomes a `participants[]` item `{entity, display, role}` resolved the same way. In email-only mode the courier leaves `## Mentions` empty and the scribe's engine fills it from the blurb; in fetch mode the courier MAY pre-fill it from byline and capitalized-phrase heuristics, marked with a `courier-heuristic` tag so the scribe treats them as hints. The scribe MUST truncate any body over a configurable limit (default 4 000 characters) and attach a `parse-warning` rationale so the truncation is visible in the quarantine.
- MUST make the scribe honor an explicit `predicate:` frontmatter hint: when present and registered, the primary proposal uses that predicate (validated against its `claim_schema` as today); an unregistered hint falls back to the current inference path with a warning. `## Mentions` bullets become `person.generic` / `org.company` proposals (task_36's multi-entity path); `## Events` bullets become `event.business` proposals with `participants[]` objects; every secondary proposal carries the article's URL in its provenance and the article's entity id in its `mentions[]` / `sources` field per ADR-028, and the article proposal's `mentions[]` and the event proposals' `participants[]` carry the resolver's chosen entity id per item (or no `entity` when the resolver answered `new` or `ambiguous`, in which case the proposal routes to review per ADR-029 and the id is back-filled on accept).
- MUST deduplicate articles by normalized URL. Normalization (stdlib only, in `skills/_lib/urlnorm.py`, shared by courier and scribe): lowercase scheme and host, strip `utm_*` / `fbclid` / `gclid` / `mc_cid` / `mc_eid` query params, drop fragments, collapse a trailing slash. Per ADR-030 the article's entity id is opaque and minted once; `<publication-slug>-<YYYY-MM-DD>-<title-slug>` is the projection basename and the resolver's blocking key, never the identity. task_45's resolver MUST match `source.article` on normalized `url` first and on `content_hash` second (when both submissions carry one), so a changed title on re-read still updates the existing entity (a supersession) rather than creating a sibling, and a re-fetch of the same page is a second bearer of the same content, not a second article. Both submissions' provenance MUST survive on the chain.
- MUST write the daily digest: one `note` per publication per day titled `<publication> digest YYYY-MM-DD` whose `## References` section lists every article filed that day as a `[[target|display]]` wikilink (target is the article's unique projection basename, display is its title, per ADR-030's naming rule); a second tick on the same day appends to the existing digest file in `ingest/` if it has not been consumed yet, or writes a new note that the resolver merges into the day's digest entity by title. The scribe MUST populate the note's `references[]` from those bullets (the field exists in `starter/predicates/note.toml`; today the scribe does not fill it). The digest note is how "did I read the paper end to end?" is answered from the vault.
- MUST upgrade `entity.search` (daemon) into the resolver's candidate generator (ADR-030): it matches across every string and string-array property declared in the predicate's `claim_schema` (e.g. `display_name`, `title`, `aliases`, `organization`, `tags`, `publication`, `url`), not only the canonical name field, and adds a full-text tier over claim payloads using the store's existing `search_fts` (FTS5, `crates/ffs-core/src/store/mod.rs`, unexposed today). Parameters gain optional `predicate` (exact predicate filter), `family` (path-family filter per ADR-028), and `context_entity` (an entity id whose `entity.different_from` assertions exclude candidates). Each hit gains `path` (from `path_for_entity`), `matched_on` (one of `display_name` | `alias` | `fts` | `other`, the tier that produced the hit), and `score`. Results keep the lightweight shape (no claim bodies) and remain capability-filtered per hit. Ordering: exact canonical-name match, then alias match, then prior count (the number of accepted resolutions of this surface form to each candidate, computed from the store; ADR-030's commonness prior), then FTS rank; ties by `tx_time` descending. The search MUST follow `entity.same_as` chains so a hit on a merged (losing) entity's name or alias returns the winning entity id and path, and MUST never return an entity that a supplied `context_entity` is `different_from`.
- MUST update the `ffs_search` MCP tool schema and translator for the new optional `predicate`, `family`, and `context_entity` arguments and the enriched hit shape (`path`, `matched_on`, `score`); the tool description MUST document the result as a reconciliation shape (query in, ranked candidates with score and `matched_on` out) and note that a W3C Reconciliation Service API facade over this tool is a later option, not part of this task. `SEARCH_DEFAULT_LIMIT` / `SEARCH_MAX_LIMIT` unchanged. `ffs_query` and `ffs_render_projection` are unchanged.
- MUST report the courier in the daily summary: `health.summary` gains `courier` (`last_run`, `items_seen`, `files_written`, `fetch_failures`, `last_error`) and `skills/auditor/audit.py` renders one "courier ran" line (or "courier has not run since <date>" as a flag when the schedule was missed by more than a day). A `ffs courier status` CLI verb reads the same fields.
- MUST keep `docs/agent-memory/skill/ffs-courier/SKILL.md` as the agent-hosted alternative (instructional, no `entry_point`, not installed under `$FFS_DATA_DIR/skills/`), reframed: the loop (read the email, enumerate links, open each with the owner's session where the owner permits it, write one ingest file per article per the contract, write the digest note, then confirm filing with `ffs_search` or `ffs ls articles/recent/` and report per the convention's persistence honesty), an explicit statement that credentials, session handling, and the publication's terms of use are the owner's responsibility and that no fetch, browser, or email logic lives in FFS beyond the deterministic bundle, and one complete worked example ingest file plus one digest note that is byte-identical to what `skills/courier/` writes for the same input.
- MUST NOT add any pip dependency to `skills/` and MUST NOT change the skills-host wire protocol beyond what task_41's scheduler adds. URL normalization and slugging live in `skills/_lib/` (stdlib) and, where the daemon needs the same normalization for entity resolution, in `ffs-core` (Rust) with a shared fixture file so both implementations agree.
- MUST NOT commit any real publication text: test fixtures are synthetic `.eml` files with invented publication names, headlines, and blurbs.
- SHOULD add a "Read the paper for me" section to `docs/onboarding/first-use-guide.md` (what to put in `courier.toml`, how to store the mailbox token in the keychain, what the dry run prints, what shows up in the vault, when to turn on fetch and what it means) and list both courier skills in `docs/agent-memory/README.md`.
</requirements>

## Subtasks
- [ ] 40.1 Article ingest contract written into `CONVENTION.md` (new section) with the frontmatter fields (including `fetch` and `content_hash`), `## Mentions` / `## Events` bullet grammar, the no-full-text rule, and the digest-note shape.
- [ ] 40.2 `skills/_lib/urlnorm.py` (normalize, article basename slug) and the matching Rust helper in `ffs-core`; shared fixture of (raw url → normalized url) pairs consumed by both test suites; `content_hash` accepts `blake3` or `blake2b` multihash prefixes on the Rust side.
- [ ] 40.3 `skills/courier/` bundle: `SKILL.md`, `definition.atom.json`, `courier.py` with mailbox connect, filter, parse (HTML and text digests), ledger, ingest-file and digest writing, `mark_seen`; `starter/config/courier.toml`; keychain lookup through `ffs_skill`; dry-run mode; unit tests over a synthetic `.eml` fixture (N items → N files + 1 digest; second run → 0 files; dry run → 0 files in `ingest/`, N in scratch).
- [ ] 40.4 `fetch.mode = "cookies"` path: cookie-jar fetch, `max_bytes` / `timeout_s`, main-text heuristics, `content_hash`, per-item fallback to email-only with `fetch: failed <reason>`; tests with a stubbed `urllib` (success, 403, timeout, oversize).
- [ ] 40.5 Scribe: `predicate:` frontmatter hint honored; body truncation with `parse-warning`; `## Mentions` → `person.generic` / `org.company` proposals plus `mentions[]` items `{entity, display, context}` on the article proposal; `## Events` → `event.business` proposals with `participants[]` items `{entity, display, role}`; `courier-heuristic` tagged bullets treated as hints; article id threaded into secondary proposals' `mentions[]`; `## References` bullets populate `note.references[]`; `content_hash` and `fetch` frontmatter round-trip into the `source.article` claim.
- [ ] 40.6 Resolver (task_45) extended to match `source.article` by normalized `url` and then `content_hash`; second submission of the same URL supersedes the head and keeps both provenance entries; digest notes for the same publication and day resolve to one entity; resolver fills `entity` on `mentions[]` and `participants[]` items.
- [ ] 40.7 `entity.search` v2: schema-declared string fields, FTS tier via `search_fts`, `predicate` / `family` / `context_entity` filters, `path` + `matched_on` + `score` on hits, documented four-tier ordering with the prior-count computation; `EntitySearchParams` / `EntitySearchHit` updated in `api.rs`; `entity.same_as` chain following and `entity.different_from` exclusion.
- [ ] 40.8 `ffs_search` MCP tool: schema + translator for `predicate` / `family` / `context_entity`; reconciliation-shape description; unit tests; integration test asserting an alias match returns `matched_on: "alias"` and a `path`.
- [ ] 40.9 Daemon and CLI: `health.summary.courier` fields, auditor "courier ran" line and missed-schedule flag, `ffs courier run [--dry-run]` and `ffs courier status`; installer seeds `courier.toml` and copies the bundle (`installer/install.sh`, `install.ps1`).
- [ ] 40.10 Docs: `docs/agent-memory/skill/ffs-courier/SKILL.md` reframed as the agent-hosted alternative with the worked example matching the bundle's output byte for byte; `docs/agent-memory/README.md` lists both; first-use-guide "Read the paper for me"; live validation: one real digest email, dry run, then a real run.

## Implementation Details
Current structure: `crates/ffs-daemon/src/ingest_watcher.rs` submits any eligible `.md` under `ingest/` after the stability window and moves it to `processed/`; `crates/ffs-daemon/src/scribe.rs` translates skill proposals into quarantine `Proposal`s; `skills/scribe/extraction.py` parses frontmatter tolerantly (`parse_markdown`) but only consults `name` / `title` / `email` / `phone` / `org` keys and never a `predicate` hint. `skills/auditor/` is the shape to copy for the courier: `SKILL.md` frontmatter with `kind`, `entry_point`, `python`, `timeout_ms`; `audit.py` importing `query`, `log`, `run` from `skills/_lib/ffs_skill.py`; `tests/conftest.py` doing its own `sys.path` setup. The skills host discovers bundles under `$FFS_DATA_DIR/skills/` and the daemon spawns them by name (`crates/ffs-daemon/src/main.rs`); the auditor has no periodic driver today, which task_41 subtask 41.3 adds and this task reuses.

`entity.search` in `crates/ffs-daemon/src/dispatch.rs` iterates `registry.names()`, lists atoms per predicate, and substring-matches one hardcoded field (`title` for `note`, else `display_name`); the predicate registry already exposes each spec's `claim_schema`, which is where the v2 field list comes from. The `AtomStore` trait already has `search_fts(query, limit)` (FTS5 MATCH over claim payloads, returning content hashes) that no RPC exposes; the FTS tier of v2 is that method plus a hash-to-entity lookup and a capability filter. Prior counts come from accepted proposals: each accept that resolved a surface form to an entity is visible as an atom whose provenance carries the mention text, so the count is a store query, not new state.

Because entity ids are opaque under ADR-030, the resolver's blocking key for articles is the `<publication>-<date>-<title>` slug and the dedup keys are normalized `url` and `content_hash`; the same slug with a different `url` is two articles, the same `url` with a different title is one.

The contract is deliberately a superset of what the heuristic engine can consume: with `FFS_SCRIBE_ENGINE=heuristic` the `predicate:` hint plus structured `## Mentions` bullets yield correct proposals without an LLM, so the courier path works on a fresh install; the `llm` engine improves extraction quality for the summary body and fills `## Mentions` in email-only mode.

Why a bundle and not an agent: the mailbox, the filters, and the files are deterministic, and a deterministic step is testable with a fixture `.eml` in CI, restartable, and auditable in the daily summary. An agent doing the same job adds a model call, a browser, and a host process to the first link of a daily chain, and none of them make the files better. The agent-hosted alternative stays for owners who already run one and for publications that need judgment to read.

Persistence-honesty vocabulary for both couriers (mirrors ADR-027): *submitted* (file dropped, watcher will pick it up), *proposed* (in quarantine), *filed* (accepted atom, visible in the vault), *auto-filed* (accepted by an ADR-029 policy). The bundle's tick result and the skill use exactly these words.

### Relevant Files
- `skills/courier/SKILL.md`, `definition.atom.json`, `courier.py`, `tests/` — new bundle.
- `skills/_lib/urlnorm.py` (new), `skills/_lib/ffs_skill.py` (keychain lookup helper) — shared stdlib helpers.
- `starter/config/courier.toml` — seeded configuration; no secrets.
- `skills/scribe/extraction.py` — `predicate:` hint, truncation, `## Mentions` / `## Events` / `## References` handling, `courier-heuristic` hints.
- `crates/ffs-core/src/` — Rust URL normalization helper and `content_hash` prefix acceptance (new small module).
- `crates/ffs-daemon/src/scribe.rs` — resolver hooks: match `source.article` by normalized url and `content_hash`; fill `entity` on `mentions[]` / `participants[]` (task_45 owns the resolver itself).
- `crates/ffs-core/src/store/mod.rs` — `search_fts` (existing, unexposed) backs the FTS tier.
- `crates/ffs-daemon/src/dispatch.rs`, `crates/ffs-daemon/src/api.rs` — `entity.search` v2, `health.summary.courier`.
- `crates/ffs-mcp/src/tools.rs` — `ffs_search` schema/translator; tests.
- `crates/ffs-cli/src/commands.rs`, `lib.rs` — `ffs courier run|status`.
- `skills/auditor/audit.py` — "courier ran" line and missed-schedule flag.
- `installer/install.sh`, `installer/install.ps1` — copy the bundle, seed `courier.toml`.
- `docs/agent-memory/CONVENTION.md`, `docs/agent-memory/README.md`, `docs/agent-memory/skill/ffs-courier/SKILL.md` — contract and agent-hosted alternative.
- `starter/predicates/source.article.toml`, `starter/predicates/note.toml`, `starter/predicates/event.business.toml` — read-only inputs (ADR-028 shapes as amended by ADR-031).

### Dependent Files
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — URL dedup e2e; article + mentions e2e; a courier-written fixture through the pipeline.
- `crates/ffs-mcp/tests/mcp_integration.rs` — alias-match integration test.
- `skills/scribe/tests/corpus/` — synthetic business-press fixtures (task_36's golden corpus).
- `docs/onboarding/first-use-guide.md` — "Read the paper for me" section.
- `crates/ffs-daemon/src/main.rs` — scheduler hook shared with task_41.

### Related ADRs
- [ADR-009: Claw integration](adrs/adr-009.md) — names the courier; stdlib-only, `SKILL.md`-shaped bundles; the reason the courier is a bundle.
- [ADR-030](adrs/adr-030.md): opaque permanent entity ids, the three-outcome resolver, `entity.same_as` / `entity.different_from`, priors from accepted resolutions; `entity.search` v2 is its candidate generator.
- [ADR-031](adrs/adr-031.md): mentions and participants as entity references with display and context or role; `content_hash` as the information-bearer key; the article as an information content entity.
- [ADR-028] — business-graph predicates and registry-declared path families (the shapes this contract targets).
- [ADR-027: Adopt the OKF Agent Memory Convention](adrs/adr-027.md) — search-before-write and persistence honesty both couriers enforce.
- [ADR-026: Scribe v2](adrs/adr-026.md) — engine seam and schema-driven prompts; the `predicate:` hint is an input to both engines.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — `ffs_search` stays a thin pass-through; filtering happens daemon-side.
- task_43 — the intake spike that decides the fetch default and the email-only coverage number.
- task_45 — the resolver this task's dedup and `entity` back-fill run through.

## Deliverables
- `skills/courier/` bundle (mailbox, filters, ledger, ingest files, digest, dry run, opt-in cookie fetch with fallback) + `starter/config/courier.toml` **(REQUIRED)**.
- Article ingest contract (convention section) + the agent-hosted `ffs-courier` skill reframed with a worked example matching the bundle **(REQUIRED)**.
- Scribe support for `predicate:` hint, `## Mentions` / `## Events` / `## References`, body truncation, `courier-heuristic` hints **(REQUIRED)**.
- URL normalization (Python + Rust, shared fixtures) and article dedup via the resolver on `url` and `content_hash` **(REQUIRED)**.
- `entity.search` v2 (four-tier ordering, FTS via `search_fts`, `same_as` following, `different_from` exclusion) + `ffs_search` schema update with tests **(REQUIRED)**.
- `health.summary.courier`, auditor line and flag, `ffs courier run|status`, installer wiring.
- Onboarding + `docs/agent-memory/README.md` updates.
- Unit tests with 80%+ coverage on new Python and Rust modules **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `test_courier_parses_html_digest_into_items` / `test_courier_parses_text_digest_into_items` — synthetic `.eml` fixtures yield (headline, blurb, link) per item.
  - [ ] `test_courier_writes_one_ingest_file_per_item_and_one_digest` — N items → N files + 1 digest note in a temp ingest dir; every file validates against the contract (frontmatter keys, `predicate: source.article`, no body over the limit).
  - [ ] `test_courier_second_run_over_same_message_writes_nothing` — ledger and `entity.search` stub both consulted; zero new files.
  - [ ] `test_courier_dry_run_writes_to_scratch_only` — nothing in `ingest/`, N files in the dry-run dir, message not marked seen, ledger unchanged.
  - [ ] `test_courier_reads_secret_from_keychain_never_from_toml` — a password key in `courier.toml` is refused with a clear error.
  - [ ] `test_courier_sender_and_subject_filters` — messages outside the filters are ignored.
  - [ ] `test_fetch_mode_off_by_default` / `test_fetch_cookies_success_sets_content_hash_and_fetched` / `test_fetch_403_falls_back_to_email_only_with_reason` / `test_fetch_timeout_falls_back` / `test_fetch_oversize_falls_back` — stubbed `urllib`.
  - [ ] `test_normalize_url_strips_tracking_params_and_fragment` — `utm_*`, `fbclid`, `gclid`, `#frag` removed; host lowercased; trailing slash collapsed.
  - [ ] `test_normalize_url_is_idempotent` — normalizing twice equals once.
  - [ ] `test_article_basename_slug_is_deterministic` — same publication/date/title → same slug; differs on any change; the slug is not used as the entity id.
  - [ ] `test_predicate_hint_selects_source_article` — fixture with `predicate: source.article` yields a primary `source.article` proposal with `title`, `url`, `publication`, `published_at`.
  - [ ] `test_content_hash_and_fetch_frontmatter_round_trip_to_source_article_claim` — present values appear unchanged in the claim; absent yields no field.
  - [ ] `test_mentions_section_yields_person_and_org_proposals` — each bullet becomes a `person.generic` or `org.company` proposal carrying the article id in `mentions[]`.
  - [ ] `test_mentions_bullets_yield_entity_display_context_objects` — the article proposal's `mentions[]` items are `{display, context}` objects with `display` and `context` verbatim from the bullet and no `entity` before resolution.
  - [ ] `test_courier_heuristic_bullets_are_hints_not_facts` — a `courier-heuristic` tagged bullet is carried as a hint rationale, not as a bare proposal.
  - [ ] `test_events_section_yields_event_business_proposals` — kind parsed from the bullet prefix.
  - [ ] `test_events_bullets_yield_participants_with_role_in_event` — `| Name (role), Name (role)` parses into `participants[]` items `{display, role}` using the ADR-031 role vocabulary; an unknown role maps to `other`.
  - [ ] `test_body_over_limit_is_truncated_with_parse_warning`.
  - [ ] `test_digest_note_references_are_populated_from_bullets`.
  - [ ] Rust `url_normalization_matches_shared_fixtures` — the Rust helper agrees with every pair in the shared fixture file; `content_hash_accepts_blake3_and_blake2b_prefixes`.
  - [ ] `entity_search_matches_alias_field` / `entity_search_matches_tags_field` / `entity_search_matches_url_field` / `entity_search_filters_by_predicate` / `entity_search_orders_exact_name_first` — dispatcher unit tests.
  - [ ] `entity_search_fts_tier_uses_store_search_fts` — a query matching only a word in a claim body returns the entity with `matched_on: "fts"`.
  - [ ] `entity_search_orders_alias_tier_before_fts_tier` — an alias hit sorts above an FTS hit with a higher FTS rank.
  - [ ] `entity_search_orders_by_prior_count_within_alias_tier` — two entities sharing an alias sort by accepted-resolution count for that surface form.
  - [ ] `entity_search_follows_same_as_chain_to_winner` — a hit on a losing entity's alias returns the winner's id and path; a two-hop chain resolves to the final winner.
  - [ ] `entity_search_excludes_different_from_candidates_given_context_entity` — with `context_entity` set, entities it is `different_from` are absent from the results.
  - [ ] `resolver_fills_entity_on_mentions_and_participants` — after resolution, `mentions[]` and `participants[]` items carry the chosen entity id for `existing` outcomes and none for `new` or `ambiguous`.
  - [ ] `ffs_search_passes_predicate_family_and_context_entity_filters` / `ffs_search_hit_shape_includes_path_matched_on_and_score` — MCP translator tests.
  - [ ] `audit.py`: `courier_ran_line_present` / `courier_missed_schedule_is_a_flag`.
  - [ ] `ffs-cli`: `courier_run_dry_run_prints_would_write_list` / `courier_status_reads_health_summary`.
- Integration tests:
  - [ ] `courier_fixture_eml_files_through_pipeline_into_articles_and_digest` — `ingest_pipeline_e2e`: the bundle's output for the synthetic `.eml` lands as `articles/` files and one digest note with wikilinks after accept.
  - [ ] `same_url_submitted_twice_yields_one_article_entity_with_two_provenance_entries` — `ingest_pipeline_e2e`.
  - [ ] `same_content_hash_different_url_yields_one_article_entity` — `ingest_pipeline_e2e`; a syndicated copy with the same page bytes resolves to the existing article.
  - [ ] `article_with_mentions_files_person_org_and_event_entities` — after accept, `articles/`, `people/`, `orgs/` projections exist, the article's rendered markdown contains `[[target|display]]` wikilinks to each mention, and the stored `mentions[]` items carry entity ids.
  - [ ] `ffs_search_finds_org_by_alias_end_to_end` — `mcp_integration`.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- One real digest email, run in dry-run mode on the project lead's Mac, yields one ingest file per item and a digest note in the scratch directory; a second dry run of the same email yields zero new files; the tick result lists exactly the files and says "would submit", not "filed".
- A real run of the same email produces `articles/`, `people/`, and `orgs/` files in the vault (after review or auto-file), with wikilinks from each article to its mentions and from the digest to each article; the daily summary shows "courier ran" with the counts.
- Dropping the same article again (same URL, tweaked title) updates the existing article rather than creating a second one; both provenance entries are visible via `ffs_query`.
- `ffs_search "Acme"` finds an organization filed under the alias "Acme Corp" with `matched_on: "alias"` and a usable `path`; after that organization is merged into another with `entity.same_as`, the same search returns the winner.
- No courier ever writes an entity id; every `mentions[]` and `participants[]` item in the vault carries one after filing, or the proposal is sitting in review with a reconciliation picker.
- The bundle, the agent-hosted skill, the convention section, and the scribe agree byte for byte on the frontmatter keys and section names; the skill's worked example is the bundle's output for the same input.
- Default install privacy is unchanged except for what the owner configures: with `fetch.mode = "off"` the courier talks only to the owner's mailbox; with `cookies` it fetches only the URLs in the owner's own digest with the owner's own session; no publication text is stored and none is committed to the repo.
