---
status: pending
title: Scribe v2 — predicate-schema-driven extraction with pluggable engines
type: backend
complexity: high
dependencies:
  - task_11
  - task_26
  - task_32
  - task_38
---

# Task 36: Scribe v2 — predicate-schema-driven extraction with pluggable engines

## Overview
The scribe's regex heuristics hit their structural ceiling on 2026-06-21: `Jon Jones.md`, a key-value contact card, was extracted as a `contact.person` named **"Bones Occupation"** — two random title-case words stitched across lines by the name-bigram scan. The postmortem catalogued five failure modes (filename ignored; `Field: value` card shape unrecognized; field labels treated as name candidates; no filename cross-check; verbatim typo propagation) and a deeper diagnosis: scribe ignores the predicate registry entirely and hardcodes two predicate shapes, so extraction can never keep pace with the substrate's schema.

ADR-026 records the decision this task implements: an `ExtractionEngine` seam inside the skill bundle with a `heuristic` engine (today's code, offline default) and an `llm` engine (stdlib-HTTP to a configurable backend — localhost Ollama or the Anthropic Messages API, strictly opt-in); prompts **generated from the registered predicate specs' `claim_schema`s**; LLM output validated against those same schemas; engine+model recorded in provenance; a golden-corpus eval harness so engine quality is measured rather than vibed. ADR-009's stdlib-only constraint shapes everything: `urllib` for HTTP, `tomllib` for predicate specs, zero pip deps.

Amended 2026-09-14 for the newspaper / movers-and-shakers goal (ADR-028, ADR-029). The scribe is the clerk in that pipeline: one business-press article dropped into `ingest/` has to become one `source.article` proposal plus the people, organizations, and events it mentions, each filed against an entity that may already exist. That adds three things to this task — a multi-entity proposal envelope, daemon-side entity resolution so a second article about the same person updates her file instead of creating a twin, and business-press fixtures in the golden corpus. The schema-driven prompt builder already picks up ADR-028's predicates with no code change; task_38 therefore becomes a dependency.

Amended 2026-09-14 (ADR-030, ADR-031). The research pass on entity identity (`docs/research/2026-09-14-entity-resolution.md`) and on top-level ontologies (`docs/research/2026-09-14-top-level-ontologies.md`) changed two things here. First, resolution is a three-outcome decision (existing, new, ambiguous) driven by Fellegi-Sunter weights and thresholds read from a config file, honoring `entity.same_as` and `entity.different_from` atoms, applying a NIL policy, and minting opaque ids for new entities; the slug rule is gone. Second, the scribe emits `affiliation` proposals as their own atoms (a role with a bearer, an organizational context, and a temporal window) and fills `mentions[]` and `participants[]` items with entity ids rather than bare names.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST introduce an `ExtractionEngine` interface in `skills/scribe/` with two implementations: `heuristic` (current `extraction.py` logic refactored behind the seam, behaviorally unchanged except the hygiene fixes below) and `llm` (new). Engine selection via `FFS_SCRIBE_ENGINE` env var; default `heuristic` — **nothing leaves the machine and no new runtime dependency exists unless the user opts in.**
- MUST implement the `llm` engine with ONLY the Python standard library (`urllib.request`, `json`) per ADR-009. Two backend adapters: Ollama chat API (`FFS_SCRIBE_LLM_URL` defaulting to `http://localhost:11434`) and the Anthropic Messages API (selected when the URL host is `api.anthropic.com`; key from `FFS_SCRIBE_ANTHROPIC_KEY` or the OS keychain via the task_27/33 machinery). Model via `FFS_SCRIBE_LLM_MODEL`.
- MUST generate the extraction prompt from the registered predicate specs: read every TOML in `$FFS_DATA_DIR/config/predicates/` with stdlib `tomllib`, render each predicate's name + `claim_schema` (+ description if present) into the instruction, and request JSON output conforming to a proposals envelope. Adding a predicate spec MUST extend extraction with zero scribe code changes.
- MUST validate LLM output against the corresponding predicate's `claim_schema` before emitting a proposal (minimal stdlib validator: required fields, primitive types, string/array shapes — full JSON-Schema fidelity not required for MVP). Schema-invalid or unparseable output MUST fall back to the heuristic engine for that submission; ingest must never hard-fail because a model or network did.
- MUST record `engine` and `model` (model empty for heuristic) in each proposal's provenance, and surface them in the quarantine's `ingest.list_pending` payload so the review UI can show what produced each proposal.
- MUST add a golden-corpus eval harness: `skills/scribe/tests/corpus/` with ~20 fixture notes (including the verbatim Jon Jones card, the task_32 rehearsal fixtures, frontmatter contacts, plain notes, mixed/adversarial cases) each paired with expected proposals; a pytest scorer computing per-fixture field-level precision/recall. Heuristic-engine scores run in CI and gate regressions; `llm`-engine scoring is invocable out-of-band (skips cleanly when no backend is reachable).
- MUST apply the bounded heuristic hygiene fixes from the postmortem, since heuristic remains the default: (1) filename (sans extension) as a name-candidate source, (2) `^[A-Z][a-z]*(\s[a-z]+)*:\s` card-shape detection → key-value parsing into known contact fields instead of the bigram scan, (3) field-label words (Nickname, Occupation, Phone, Email, Address, First, Last, Name, Title…) excluded from name candidacy. These three close the "Bones Occupation" class; further heuristic work is explicitly out of scope per ADR-026.
- MUST NOT add any pip dependency to the skill bundle, and MUST NOT change the skills-host wire protocol (the skill reads predicate TOMLs from disk; config arrives as env vars through the existing daemon → skills-host → subprocess path).
- SHOULD document engine configuration and the privacy posture (exactly what leaves the machine, when, to whom) in `docs/onboarding/first-use-guide.md` and the technical-friend checklist.
- SHOULD verify Anthropic API request shape against current docs at implementation time rather than trusting this spec's memory of it.
- MUST support multi-entity extraction per submission (amended 2026-09-14, ADR-028): one article submission yields one `source.article` proposal plus N `person.generic`, M `org.company`, and K `event.business` proposals. Every proposal in the set MUST carry the article URL in provenance, and proposals MUST cross-reference each other by entity id (or display name when the id is not yet assigned) so the article's `mentions[]`, the event's `parties[]` and `source`, and the person's `organization` resolve to the same entities after acceptance. The proposals envelope on the skills-host wire MUST stay backward compatible (a single-proposal result is the degenerate case).
- MUST perform entity resolution on the daemon side before enqueueing (amended 2026-09-14, ADR-030): in `crates/ffs-daemon/src/scribe.rs`, a resolver that (a) blocks candidates by path family and a normalized name key; (b) generates candidates from `entity.search` v2 (`display_name`, `aliases[]`, prior count; task_40) and the store's `search_fts`, following `entity.same_as` chains so a mention matching a losing alias lands on the winner; (c) scores candidates with Fellegi-Sunter field weights on `organization`, `role`, and `location` agreement plus the prior, reading weights and the two thresholds from `$FFS_DATA_DIR/config/resolution.toml` (starter file shipped by task_38, hot-reloaded like predicate specs); (d) treats any `entity.different_from` assertion as a hard block; (e) emits per proposal `resolution: "existing" | "new" | "ambiguous"` and `candidates: [{entity, score, matched_on}]`, where `ambiguous` covers scores between the thresholds or two candidates within a margin; (f) receives the whole multi-entity set for a submission so an org proposal in the same article can raise a person candidate already affiliated with it (coherence scoring itself is Phase 2; the interface ships now); (g) applies the NIL policy: `source.article`, `event.business`, and `org.company` always mint, a `person.generic` mints on first mention only with a distinguishing attribute (organization or role) or on a second sighting keyed by normalized name plus article organization context, otherwise the mention stays in the article's `mentions[]` with `entity` unset. Entity ids for `new` are opaque (16 random bytes, base58btc multibase), minted in the daemon's signing path at accept or auto-accept, never derived from a name. `resolution` and `candidates` are carried through quarantine storage and `ingest.list_pending` so the review UI can render a reconciliation picker and so task_39's additive-vs-conflict rule can act on them (`ambiguous` is always conflicting).
- MUST emit `affiliation` proposals (amended 2026-09-14, ADR-031) alongside person and org proposals whenever an article states or implies that a person holds a role at an organization: predicate `affiliation` with `person` (entity id or display name pending resolution), `organization` (same), `title`, `kind` (the ADR-031 enum), and `source` (the article URL); `valid_from` set to the stated role start when the article gives one, otherwise the article's `published_at`. A role ending ("stepped down", "departs") MUST be emitted as a supersession proposal on the existing affiliation setting `valid_to`, never as a new affiliation, so ADR-029 routes it to review. `mentions[]` items on `source.article` and `participants[]` items on `event.business` MUST be objects `{entity, display, context}` and `{entity, display, role}` respectively, with `entity` set when resolution returns `existing` and absent otherwise; `display` is always the name as printed.
- MUST grow aliases from accepted resolutions (amended 2026-09-14, ADR-030): when a proposal resolved to an existing entity is accepted or auto-accepted and its mention text differs from the entity's `display_name`, the mention text is appended to the entity's `aliases[]` as an additive edit. Priors for a surface form are the counts of accepted resolutions to each entity, read from the store, never cached in process memory.
- MUST extend the golden corpus with at least five business-press fixtures (amended 2026-09-14): an executive hire, a funding round, an acquisition, a real-estate opening, and a profile piece, each paired with expected multi-entity proposals (article + people + orgs + events) and each scored by the existing per-field precision/recall scorer.
</requirements>

## Subtasks
- [ ] 36.1 Refactor `extraction.py` behind an `ExtractionEngine` seam; `heuristic` engine preserves current behavior (all existing tests green before any other change).
- [ ] 36.2 Predicate-spec loader: read `$FFS_DATA_DIR/config/predicates/*.toml` via `tomllib`; render name + `claim_schema` into a prompt-builder; unit-test that a newly added predicate spec appears in the generated prompt with no code change.
- [ ] 36.3 `llm` engine: stdlib HTTP client, Ollama adapter + Anthropic adapter, JSON extraction + minimal schema validation, heuristic fallback on any failure. Unit-test with a stubbed HTTP layer (no network in CI).
- [ ] 36.4 Config plumbing: `FFS_SCRIBE_ENGINE` / `FFS_SCRIBE_LLM_URL` / `FFS_SCRIBE_LLM_MODEL` / key handling threaded through daemon env docs and the skills-host subprocess env; document in main.rs env-var docblock.
- [ ] 36.5 Provenance: engine + model on every proposal; through quarantine storage (task_29 tables) and `ingest.list_pending`; visible in the Obsidian panel's expanded proposal view.
- [ ] 36.6 Golden corpus + pytest scorer; wire heuristic scoring into the normal test run; out-of-band `llm` scoring entrypoint that skips without a reachable backend.
- [ ] 36.7 Heuristic hygiene: filename source, card-shape key-value parsing, field-label stop-words. Corpus must show the Jon Jones fixture extracting as `contact.person` with `display_name: Jon Jones`, `phone: 919-428-4074`, occupation captured.
- [ ] 36.8 Docs: engine setup, privacy statement, model recommendations; update first-use-guide + technical-friend checklist.
- [ ] 36.9 Live validation: drop the original Jon Jones card into `ingest/` under (a) default heuristic and (b) an opted-in `llm` backend; both must produce a correctly named contact proposal in the quarantine.
- [ ] 36.10 Multi-entity proposal envelope + prompt rendering for every registered predicate, including ADR-028's `org.company`, `source.article`, `event.business`, and `person.generic` v2; cross-references between proposals in one set; wire shape stays backward compatible.
- [ ] 36.11 Daemon-side resolver in `scribe.rs`: blocking, candidate generation via `entity.search` v2 + `search_fts`, `same_as` chain following, `different_from` hard block, NIL policy, whole-envelope input; `resolution: existing | new | ambiguous` plus `candidates[]` carried through quarantine storage (task_29 tables) and `ingest.list_pending`; opaque id minting in the signing path for `new`; `affiliation` proposals and object-shaped `mentions[]` / `participants[]` (ADR-031).
- [ ] 36.12 Business-press corpus fixtures (hire, funding, acquisition, opening, profile) with expected multi-entity proposals including affiliations; scorer covers per-predicate precision/recall across the set.
- [ ] 36.13 `resolution.toml` reader (weights as m/u pairs, two thresholds, blocking options; hot-reload); three-outcome tests (above upper threshold → existing, below lower → new, between or two close candidates → ambiguous); `same_as` chain test (mention of a losing alias resolves to the winner); `different_from` block test (a blocked candidate is never returned as existing, even with a perfect name match); alias growth on accept.
- [ ] 36.14 Corpus expected entity ids (or `new`) per proposal; pairwise precision/recall over same-entity pairs and B-cubed over resolved clusters in the scorer; the four identity fixtures: the same person across three articles with a nickname, two different people with the same name at different organizations, a rename announced in an article, a wrong merge and its undo via `same_as` supersession.

## Implementation Details
Current structure: `skills/scribe/extraction.py` (pure functions: `extract_contact_person_unstructured`, `detect_phone_numbers`, `extract_note`, venue masking, stop lists) invoked by the skill entry script via the skills-host stdio protocol; daemon side in `crates/ffs-daemon/src/scribe.rs` translates results into `Proposal`s. The engine seam lives entirely on the Python side; the Rust side only gains the provenance fields and env passthrough.

Predicate specs already carry JSON Schemas — e.g. `starter/predicates/contact.person.toml`'s `[claim_schema]` with `required = ["display_name"]` and typed properties. The prompt builder renders these; the validator re-reads them. The schema is the contract at both ends of the LLM call.

Failure-fallback shape: `llm` engine errors are caught per-submission, logged into the proposal set's rationale (so the user can see extraction degraded and why), and the heuristic result is emitted instead.

### Relevant Files
- `skills/scribe/extraction.py` — refactor behind the engine seam.
- `skills/scribe/` — new modules: engine interface, prompt builder, llm client/adapters, schema validator.
- `skills/scribe/tests/` — existing tests must stay green; new corpus + scorer.
- `crates/ffs-daemon/src/scribe.rs` — provenance fields on translated proposals.
- `crates/ffs-daemon/src/main.rs` — env-var docblock; skills-host env passthrough.
- `crates/ffs-core/src/quarantine.rs` + `quarantine_sqlite.rs` — provenance columns if not already generic JSON.
- `obsidian-plugin/src/summary.ts` + `main.ts` — engine/model display in the expanded proposal card.
- `starter/predicates/*.toml` — `claim_schema` blocks consumed by the prompt builder (read-only).

### Dependent Files
- `docs/onboarding/first-use-guide.md`, `docs/onboarding/technical-friend-checklist.md` — engine setup + privacy posture.
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — e2e stays on heuristic default; add an env-gated llm variant that skips without a backend.

### Related ADRs
- [ADR-026: Scribe v2 — predicate-schema-driven extraction with pluggable engines](adrs/adr-026.md) — this task's foundation; records the full decision + rejected alternatives.
- [ADR-009] — stdlib-only skill bundles (the load-bearing constraint).
- [ADR-005] — friction floor (why no mandatory model install).
- [ADR-028] — business-graph predicates the prompt builder renders; path families.
- [ADR-030: Entity identity, resolution, and merge](adrs/adr-030.md) — opaque ids, the three-outcome resolver, `resolution.toml`, `same_as` / `different_from`, NIL policy, alias growth, identity scoring.
- [ADR-031] — affiliation as its own role atom; object-shaped `mentions[]` / `participants[]`.

## Deliverables
- `ExtractionEngine` seam with `heuristic` + `llm` engines, stdlib-only.
- Predicate-schema-driven prompt builder + output validator.
- Config surface + provenance (engine/model) end to end.
- Golden corpus (~20 fixtures) + pytest scorer, heuristic scores gating CI **(REQUIRED)**.
- Heuristic hygiene fixes closing the "Bones Occupation" class **(REQUIRED)**.
- Unit tests with 80%+ coverage on new Python modules **(REQUIRED)**.
- Updated docs (engine setup + privacy statement).
- Multi-entity proposal sets with cross-references and article provenance **(REQUIRED, amended 2026-09-14)**.
- Daemon-side three-outcome resolver (`resolution: existing | new | ambiguous` with `candidates[]`), driven by `resolution.toml`, honoring `same_as` and `different_from`, applying the NIL policy, minting opaque ids **(REQUIRED, amended 2026-09-14, ADR-030)**.
- `affiliation` proposals and object-shaped `mentions[]` / `participants[]` with entity ids **(REQUIRED, amended 2026-09-14, ADR-031)**.
- Alias growth from accepted resolutions; store-backed priors **(REQUIRED, amended 2026-09-14, ADR-030)**.
- Five business-press corpus fixtures scored per predicate, plus four identity fixtures scored with pairwise P/R and B-cubed **(REQUIRED, amended 2026-09-14)**.

## Tests
- Unit tests:
  - [ ] Heuristic engine behind the seam reproduces all pre-refactor outputs (existing test suite green, unmodified assertions).
  - [ ] Prompt builder includes every predicate in `config/predicates/`; adding a fixture TOML adds it to the prompt with no code change.
  - [ ] Anthropic adapter and Ollama adapter each produce a correct request shape against a stubbed HTTP layer; responses parse into proposals.
  - [ ] Schema validator rejects missing-required / wrong-typed output; rejection triggers heuristic fallback.
  - [ ] Jon Jones corpus fixture: heuristic engine yields `contact.person` with `display_name: "Jon Jones"`, `phone: "919-428-4074"` (filename + card-shape + stop-word fixes together).
  - [ ] Field-label words never appear as `display_name` candidates.
- Integration tests:
  - [ ] `ingest_pipeline_e2e` unchanged and green (heuristic default).
  - [ ] Env-gated llm e2e: with `FFS_SCRIBE_ENGINE=llm` and a reachable backend, a dropped card produces a schema-valid proposal with `engine: llm` provenance; skips cleanly otherwise.
  - [ ] Corpus scorer runs in CI for heuristic; documented invocation for out-of-band llm scoring.
  - [ ] Multi-entity: the executive-hire fixture yields one `source.article`, one `person.generic`, one `org.company`, and one `event.business` (`kind: hire`) proposal, all carrying the article URL in provenance and cross-referencing by entity id.
  - [ ] Entity resolution: a second fixture mentioning an already-stored person resolves to the existing entity id with `resolution: "existing"`; an unknown person with an organization yields `resolution: "new"` and, on accept, an opaque base58btc id that is not derived from the name; a bare name with no distinguishing attribute stays in `mentions[]` with `entity` unset and mints on the second sighting.
  - [ ] Three outcomes: a score between the two thresholds, or two candidates within the margin, yields `resolution: "ambiguous"` with a populated `candidates[]`; changing the thresholds in `resolution.toml` moves the outcome without a code change.
  - [ ] `same_as` and `different_from`: a mention matching an alias of a merged (losing) entity resolves to the winner; a candidate blocked by `different_from` is never returned as `existing`.
  - [ ] Affiliation: the executive-hire fixture yields an `affiliation` proposal with `person`, `organization`, `title`, `kind: executive`, `source`, and `valid_from`; a "stepped down" fixture yields a supersession proposal setting `valid_to` on the existing affiliation, not a new one.
  - [ ] Alias growth: accepting a proposal whose mention text was "S. Chen" for the entity displayed as "Sara Chen" appends "S. Chen" to her `aliases[]`.
  - [ ] Identity scorer: pairwise precision/recall and B-cubed computed over the four identity fixtures; the wrong-merge-undo fixture restores both entities' clusters after the `same_as` atom is superseded.
  - [ ] `ingest.list_pending` exposes `resolution` and `candidates` on every proposal in a multi-entity set.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing; coverage ≥80% on new modules.
- The Jon Jones card extracts correctly under BOTH engines — never again "Bones Occupation."
- Registering a new predicate spec extends llm extraction with zero scribe code changes (demonstrated by a corpus fixture using a non-starter predicate).
- Default install behavior is byte-identical in privacy terms to today: no network calls, no new dependencies, until the user sets `FFS_SCRIBE_ENGINE=llm`.
- Corpus scorer output gives per-engine accuracy numbers the project can track release over release.
