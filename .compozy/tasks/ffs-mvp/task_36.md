---
status: pending
title: Scribe v2 — predicate-schema-driven extraction with pluggable engines
type: backend
complexity: high
dependencies:
  - task_11
  - task_26
  - task_32
  - task_42
---

# Task 36: Scribe v2 — predicate-schema-driven extraction with pluggable engines

## Overview
The scribe's regex heuristics hit their structural ceiling on 2026-06-21: `Jon Jones.md`, a key-value contact card, was extracted as a `contact.person` named **"Bones Occupation"** — two random title-case words stitched across lines by the name-bigram scan. The postmortem catalogued five failure modes (filename ignored; `Field: value` card shape unrecognized; field labels treated as name candidates; no filename cross-check; verbatim typo propagation) and a deeper diagnosis: scribe ignores the predicate registry entirely and hardcodes two predicate shapes, so extraction can never keep pace with the substrate's schema.

ADR-026 records the decision this task implements: an `ExtractionEngine` seam inside the skill bundle with a `heuristic` engine (today's code, offline default) and an `llm` engine (stdlib-HTTP to a configurable backend — localhost Ollama or the Anthropic Messages API, strictly opt-in); prompts **generated from the registered predicate specs' `claim_schema`s**; LLM output validated against those same schemas; engine+model recorded in provenance; a golden-corpus eval harness so engine quality is measured rather than vibed. ADR-009's stdlib-only constraint shapes everything: `urllib` for HTTP, `tomllib` for predicate specs, zero pip deps.

Re-scoped 2026-09-14: this task is the tracer bullet. It runs against the three starter predicates that exist today and must not depend on task_38; multi-entity proposals, affiliation proposals, and the entity resolver moved to task_45 (ADR-030, ADR-031). Gated by spike task_42: its PASS/FAIL sets the llm engine's scope.

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
- MUST honor an explicit `predicate:` frontmatter hint on a submission. When the hint names a registered predicate, extraction targets that predicate. When the hint is `source.article` or names a predicate that is not registered (task_38 has not landed yet), the scribe MUST fall back to `note`, preserving the article's title as the note title and its url and summary in the note body and `references[]`, so a courier drop is readable in the vault before task_38 exists. The hint MUST never cause a hard failure.
- MUST record `engine` and `model` (model empty for heuristic) in each proposal's provenance, and surface them in the quarantine's `ingest.list_pending` payload so the review UI can show what produced each proposal.
- MUST add a golden-corpus eval harness: `skills/scribe/tests/corpus/` with ~20 fixture notes (including the verbatim Jon Jones card, the task_32 rehearsal fixtures, frontmatter contacts, plain notes, mixed/adversarial cases) each paired with expected proposals; a pytest scorer computing per-fixture field-level precision/recall. Heuristic-engine scores run in CI and gate regressions; `llm`-engine scoring is invocable out-of-band (skips cleanly when no backend is reachable).
- MUST NOT commit fixtures containing real Charlotte Business Journal text (or any other copyrighted press text). Corpus fixtures are paraphrased or synthetic. Real articles used for scoring live under `$FFS_DATA_DIR` or a gitignored directory, and the scorer MUST accept an external corpus path via `FFS_SCRIBE_CORPUS_DIR` so real-article scores can be produced locally without entering git.
- MUST apply the bounded heuristic hygiene fixes from the postmortem, since heuristic remains the default: (1) filename (sans extension) as a name-candidate source, (2) `^[A-Z][a-z]*(\s[a-z]+)*:\s` card-shape detection → key-value parsing into known contact fields instead of the bigram scan, (3) field-label words (Nickname, Occupation, Phone, Email, Address, First, Last, Name, Title…) excluded from name candidacy. These three close the "Bones Occupation" class; further heuristic work is explicitly out of scope per ADR-026.
- MUST NOT add any pip dependency to the skill bundle, and MUST NOT change the skills-host wire protocol (the skill reads predicate TOMLs from disk; config arrives as env vars through the existing daemon → skills-host → subprocess path).
- MUST NOT depend on task_38: no `[path]` tables, no business-graph predicates, no opaque-id minting, no resolver. Those arrive in task_38 and task_45; this task's prompt builder picks them up with no code change when they land.
- SHOULD document engine configuration and the privacy posture (exactly what leaves the machine, when, to whom) in `docs/onboarding/first-use-guide.md` and the technical-friend checklist.
- SHOULD verify Anthropic API request shape against current docs at implementation time rather than trusting this spec's memory of it.
</requirements>

## Subtasks
- [ ] 36.1 Refactor `extraction.py` behind an `ExtractionEngine` seam; `heuristic` engine preserves current behavior (all existing tests green before any other change).
- [ ] 36.2 Predicate-spec loader: read `$FFS_DATA_DIR/config/predicates/*.toml` via `tomllib`; render name + `claim_schema` into a prompt-builder; unit-test that a newly added predicate spec appears in the generated prompt with no code change.
- [ ] 36.3 `llm` engine: stdlib HTTP client, Ollama adapter + Anthropic adapter, JSON extraction + minimal schema validation, heuristic fallback on any failure. Unit-test with a stubbed HTTP layer (no network in CI).
- [ ] 36.4 `predicate:` frontmatter hint: registered predicate targets extraction; `source.article` or an unregistered predicate falls back to `note` with title, url, and summary preserved; unit tests for both branches.
- [ ] 36.5 Config plumbing: `FFS_SCRIBE_ENGINE` / `FFS_SCRIBE_LLM_URL` / `FFS_SCRIBE_LLM_MODEL` / key handling / `FFS_SCRIBE_CORPUS_DIR` threaded through daemon env docs and the skills-host subprocess env; document in main.rs env-var docblock.
- [ ] 36.6 Provenance: engine + model on every proposal; through quarantine storage (task_29 tables) and `ingest.list_pending`; visible in the Obsidian panel's expanded proposal view.
- [ ] 36.7 Golden corpus (paraphrased or synthetic only) + pytest scorer; wire heuristic scoring into the normal test run; out-of-band `llm` scoring entrypoint that skips without a reachable backend; external corpus path via env var.
- [ ] 36.8 Heuristic hygiene: filename source, card-shape key-value parsing, field-label stop-words. Corpus must show the Jon Jones fixture extracting as `contact.person` with `display_name: Jon Jones`, `phone: 919-428-4074`, occupation captured.
- [ ] 36.9 Docs: engine setup, privacy statement, model recommendations, corpus licensing rule; update first-use-guide + technical-friend checklist.
- [ ] 36.10 Live validation: drop the original Jon Jones card into `ingest/` under (a) default heuristic and (b) an opted-in `llm` backend; both must produce a correctly named contact proposal in the quarantine. Drop one paraphrased article with `predicate: source.article` and confirm it lands as a readable `note` in the vault.

## Implementation Details
Current structure: `skills/scribe/extraction.py` (pure functions: `extract_contact_person_unstructured`, `detect_phone_numbers`, `extract_note`, venue masking, stop lists) invoked by the skill entry script via the skills-host stdio protocol; daemon side in `crates/ffs-daemon/src/scribe.rs` translates results into `Proposal`s. The engine seam lives entirely on the Python side; the Rust side only gains the provenance fields and env passthrough.

Predicate specs already carry JSON Schemas — e.g. `starter/predicates/contact.person.toml`'s `[claim_schema]` with `required = ["display_name"]` and typed properties. The prompt builder renders these; the validator re-reads them. The schema is the contract at both ends of the LLM call.

Failure-fallback shape: `llm` engine errors are caught per-submission, logged into the proposal set's rationale (so the user can see extraction degraded and why), and the heuristic result is emitted instead.

Why this task stays slim: task_38 (path families, business-graph predicates, opaque ids) is the largest refactor in the plan. If extraction quality against the three existing predicates is not good, the refactor's shapes should change; if it is good, a readable daily digest lands in the vault weeks earlier. Spike task_42 produces that evidence; this task acts on it.

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
- `.gitignore` — the local real-article corpus directory, if one is chosen inside the repo.

### Related ADRs
- [ADR-026: Scribe v2 — predicate-schema-driven extraction with pluggable engines](adrs/adr-026.md) — this task's foundation; records the full decision + rejected alternatives.
- [ADR-009] — stdlib-only skill bundles (the load-bearing constraint).
- [ADR-005] — friction floor (why no mandatory model install).
- [ADR-027](adrs/adr-027.md) — proposal semantics: a scribe output is a quarantined proposal, never a saved atom.

## Deliverables
- `ExtractionEngine` seam with `heuristic` + `llm` engines, stdlib-only.
- Predicate-schema-driven prompt builder + output validator.
- `predicate:` frontmatter hint with the `note` fallback for articles **(REQUIRED)**.
- Config surface + provenance (engine/model) end to end.
- Golden corpus (~20 paraphrased or synthetic fixtures) + pytest scorer, heuristic scores gating CI, external corpus path for real articles **(REQUIRED)**.
- Heuristic hygiene fixes closing the "Bones Occupation" class **(REQUIRED)**.
- Unit tests with 80%+ coverage on new Python modules **(REQUIRED)**.
- Updated docs (engine setup + privacy statement + corpus licensing rule).

## Tests
- Unit tests:
  - [ ] Heuristic engine behind the seam reproduces all pre-refactor outputs (existing test suite green, unmodified assertions).
  - [ ] Prompt builder includes every predicate in `config/predicates/`; adding a fixture TOML adds it to the prompt with no code change.
  - [ ] Anthropic adapter and Ollama adapter each produce a correct request shape against a stubbed HTTP layer; responses parse into proposals.
  - [ ] Schema validator rejects missing-required / wrong-typed output; rejection triggers heuristic fallback.
  - [ ] `predicate: contact.person` hint targets that predicate; `predicate: source.article` (unregistered) falls back to `note` with title, url, and summary preserved in body and `references[]`; an unknown hint never raises.
  - [ ] Jon Jones corpus fixture: heuristic engine yields `contact.person` with `display_name: "Jon Jones"`, `phone: "919-428-4074"` (filename + card-shape + stop-word fixes together).
  - [ ] Field-label words never appear as `display_name` candidates.
  - [ ] Scorer reads fixtures from `FFS_SCRIBE_CORPUS_DIR` when set and from the in-repo corpus otherwise.
- Integration tests:
  - [ ] `ingest_pipeline_e2e` unchanged and green (heuristic default).
  - [ ] Env-gated llm e2e: with `FFS_SCRIBE_ENGINE=llm` and a reachable backend, a dropped card produces a schema-valid proposal with `engine: llm` provenance; skips cleanly otherwise.
  - [ ] Corpus scorer runs in CI for heuristic; documented invocation for out-of-band llm scoring.
  - [ ] A paraphrased article dropped with `predicate: source.article` lands as a `note` proposal whose references include the url.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing; coverage ≥80% on new modules.
- The Jon Jones card extracts correctly under BOTH engines — never again "Bones Occupation."
- Registering a new predicate spec extends llm extraction with zero scribe code changes (demonstrated by a corpus fixture using a non-starter predicate).
- Default install behavior is byte-identical in privacy terms to today: no network calls, no new dependencies, until the user sets `FFS_SCRIBE_ENGINE=llm`.
- Corpus scorer output gives per-engine accuracy numbers the project can track release over release.
- A courier-shaped article drop is readable in the vault as a note before task_38 lands.
- No copyrighted press text enters git.
