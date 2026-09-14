---
status: pending
title: Attestations, derived status, and staleness (the local half of ADR-034)
type: backend
complexity: medium
dependencies:
  - task_38
  - task_41
---

# Task 46: Attestations, derived status, and staleness (the local half of ADR-034)

## Overview
The owner's second statement about federation was about accuracy, not intake: "ensuring it's accurate every day, like Wikipedia would: in any collection of N hosts, some percentage of N has to agree it's still accurate, at least one, preferably two or more." ADR-034 answers it with one predicate, one spec table, and one derived status, and the research behind it (`docs/research/2026-09-14-shared-accuracy.md`) says the important half needs no peer at all. Wikipedia keeps facts accurate by dating claims, requiring a citation per claim, and marking review status without asserting truth. Wikidata ranks competing statements instead of deleting the losers. Both work with N = 1.

This task makes "is it still accurate" a question the substrate answers before any peer exists: every accepted atom gains an attestation from the owner; each predicate spec says how many independent attestations a fact needs and how long they last; projections and the briefing show which facts are current, unconfirmed, stale, disputed, or deprecated; and nothing automatic ever changes an existing fact. Task_47 later lets peers contribute attestations through the same predicate.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST ship `starter/predicates/attestation.toml` (ADR-034 § Decision (1)): entity = the attested atom's multihash; claim `as_of` (ISO 8601), `basis` (enum `re_read_same_source | independent_source | primary_source | owner_knowledge | contradicted_by`), `source`, `note`; no `[path]` table (attestations never render as their own files); an `[ontology]` annotation per ADR-031 declaring it an IAO information content entity that is about an atom (`bfo = "information content entity"`, `iao` set to the IAO class, `note` explaining that the entity field is the subject atom's hash). The spec loader MUST accept a spec without `[path]` for this predicate and the materializer MUST skip it.
- MUST add an `[attestation]` table to the predicate-spec loader (ADR-034 § Decision (2)): `k` (default 1), `window_days` (optional; absent means no window), `independent` (default true). `RawSpec` is `deny_unknown_fields`, so the table is declared explicitly. Starter defaults: `affiliation` and `person.generic` k = 1, window 90; `org.company` k = 1, window 180; `source.article`, `event.business`, `contact.person`, `note` no window. A per-substrate override in `$FFS_DATA_DIR/config/attestation.toml` MAY raise `k` per predicate; it MUST NOT lower it below the spec.
- MUST auto-emit an attestation in the signing path on `ingest.accept` and on ADR-029 auto-accept: `basis = re_read_same_source` with `source` = the proposal's provenance uri when one exists, else `owner_knowledge` with `source = person:<owner>`; `as_of` = the accepted atom's `valid_from` or today; signed by the owner key; provenance `kind: accept` with the submission id. Auto-accepted atoms (ADR-029) get an attestation whose `basis` is `re_read_same_source` and whose provenance says `kind: auto_accept` with the grant hash, so the derived status can distinguish "the owner read it" from "the clerk filed it" when rendering.
- MUST implement the derived status as a pure function in `ffs-core` (`status_of(head, attestations, spec, now) -> current | unconfirmed | stale | disputed | deprecated`) per ADR-034 § Decision (3), with independence counting distinct `(basis, source)` pairs when `independent = true` (normalized url; `content_hash` equality counts as the same source). Property tests MUST establish: never `current` with fewer than `k` independent attestations; `independent = true` collapses attestations sharing a source; a newest qualifying attestation older than `window_days` yields `stale`; an unresolved `contradicted_by` attestation yields `disputed`; a supersession with `correction.reason = never_true` yields `deprecated` for the superseded atom; a head with no window is `current` after one attestation forever.
- MUST render the status: frontmatter `status:` on person, org, and affiliation projections (and any predicate with a window); an "as of" line naming the attesters and their basis ("as of 2026-06-21, confirmed by you (re-read)"); an "[unconfirmed since <date>]" or "[stale since <date>]" marker; a "Confirmed" section listing attestations. Templates for `person.generic`, `org.company`, and `affiliation` sections (rendered by reverse lookup per ADR-031) change; `contact.person` and `note` render the line only when a window is configured. Rendering MUST NOT store the status anywhere.
- MUST add a `correction` provenance marker on supersession (ADR-034 § Decision (4)): `ingest.retract` (task_39) and the review surface (inbox file or panel per ADR-032's outcome) accept `reason: world_changed | never_true`; `never_true` marks the superseded atom `deprecated` and its attestations as confirming a claim later found false (a flag the auditor counts; reliability weighting is later).
- MUST extend the auditor (`skills/auditor/audit.py`, stdlib only per ADR-009) to count heads per status per predicate in the daily summary, and MUST add a "Past their window" section to the briefing (task_41's `auditor.briefing` claim and template): facts whose window has elapsed, grouped by predicate, with how many are confirmed by the owner alone, and two actions per item or per batch: **Re-read source** (records a fresh `re_read_same_source` attestation after opening the source; uses the courier's fetch when `fetch.mode = cookies`, otherwise opens the url) and **Unchanged** (records an `owner_knowledge` attestation). Neither action is ever automatic; a bulk "all unchanged" is a single explicit click over a listed batch.
- MUST add `ffs attest <ffs://url> --basis <basis> --source <source> [--as-of <date>] [--note <text>]` to the CLI and an `attestation.create` daemon method it calls, both requiring the owner key (peers arrive through federation in task_47, not through this RPC).
- MUST keep the "nothing automatic changes a fact" rule (ADR-034 § Decision (5)): staleness only schedules re-confirmation; no status transition supersedes, retracts, or hides an atom.
- MUST NOT change the atom envelope, the `ffs://` scheme, or the MCP tool signatures; `ffs_audit_query` and `ffs_render_projection` expose the derived status through the briefing and the rendered frontmatter.
</requirements>

## Subtasks
- [ ] 46.1 `attestation.toml` starter spec with `[ontology]`; loader accepts a spec without `[path]`; materializer skips path-less predicates; spec-load test.
- [ ] 46.2 `[attestation]` table in the loader with defaults and the `config/attestation.toml` override (raise only); starter specs annotated; loader tests.
- [ ] 46.3 Derived-status function in `ffs-core` with property tests (never current below k; independence collapse; window elapsed; disputed; deprecated; no-window predicates).
- [ ] 46.4 Auto-attestation in the `ingest.accept` and auto-accept signing paths with provenance `kind: accept | auto_accept`; e2e assertion that every accepted atom has exactly one owner attestation.
- [ ] 46.5 Rendering: `status:` frontmatter, "as of" line, markers, "Confirmed" section in the person, org, and affiliation templates; render tests.
- [ ] 46.6 `correction.reason` on `ingest.retract` and the review surface; `deprecated` status; tests.
- [ ] 46.7 Auditor status counts in the daily summary; briefing "Past their window" section with Re-read source and Unchanged actions (inbox file or panel per ADR-032); vitest for the plugin model.
- [ ] 46.8 `attestation.create` daemon method and `ffs attest` CLI; docs in `docs/onboarding/first-use-guide.md` ("How FFS knows a fact is still true") and `docs/agent-memory/CONVENTION.md` § 12 (attestations as the FFS form of OKF `verified[]`).

## Implementation Details
The status function is the only new logic with state-machine flavor, so it gets property tests. Everything else is plumbing on seams that already exist: the accept path (`crates/ffs-daemon/src/dispatch.rs::ingest_accept` and the shared `sign_and_insert` helper task_39 extracts), the spec loader (`crates/ffs-core/src/predicate/mod.rs`), the renderer (`crates/ffs-core/src/projection/render.rs` reverse lookup from ADR-031), the auditor tick, and the briefing template.

Attestations are found by `list_by_entity(<subject hash>, Some("attestation"))`; the subject hash is the attested atom's content hash, so a superseded atom keeps its attestations and a new head starts with none until the owner accepts it (which emits one).

### Relevant Files
- `starter/predicates/attestation.toml` (new), `starter/predicates/*.toml` (`[attestation]` tables), `starter/config/attestation.toml` (new, override example).
- `crates/ffs-core/src/predicate/mod.rs` (loader), `crates/ffs-core/src/attestation.rs` (new: status function).
- `crates/ffs-daemon/src/dispatch.rs`, `api.rs` (auto-attestation, `attestation.create`, `correction.reason` on retract).
- `crates/ffs-core/src/projection/render.rs`, `starter/templates/*.tera` (status rendering).
- `skills/auditor/audit.py`, `skills/auditor/SKILL.md` (status counts, "Past their window").
- `obsidian-plugin/src/summary.ts` or the inbox materializer (actions), per ADR-032's outcome.
- `crates/ffs-cli/src/commands.rs` (`ffs attest`).

### Dependent Files
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` (attestation on accept; stale after window; attest flips to current).
- `docs/onboarding/first-use-guide.md`, `docs/agent-memory/CONVENTION.md` § 12.

### Related ADRs
- [ADR-034](adrs/adr-034.md): the decision; this task is its local half.
- [ADR-029](adrs/adr-029.md): auto-accept path that also emits attestations; `ingest.retract`.
- [ADR-030](adrs/adr-030.md): opaque ids; the attestation subject is a content hash, not an entity.
- [ADR-031](adrs/adr-031.md): `[ontology]` annotation; reverse-lookup rendering the status line reuses.
- [ADR-027](adrs/adr-027.md): generated versus verified; attestations are the FFS form of OKF `verified[]`.
- [ADR-032](adrs/adr-032.md): where the re-confirm actions live.

## Deliverables
- `attestation` predicate, `[attestation]` spec table with starter defaults, override file **(REQUIRED)**.
- Pure derived-status function with property tests **(REQUIRED)**.
- Auto-attestation on accept and auto-accept **(REQUIRED)**.
- Status rendering in projections; auditor counts; briefing "Past their window" with Re-read source and Unchanged **(REQUIRED)**.
- `correction.reason` on supersession; `deprecated` status **(REQUIRED)**.
- `attestation.create` RPC and `ffs attest` CLI.
- Docs.
- Unit tests with 80%+ coverage on new modules **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Property: `status_of` is never `current` with fewer than `k` independent attestations, for generated k, windows, and attestation sets.
  - [ ] Property: with `independent = true`, N attestations sharing a `source` count as one; with `independent = false` they count as N.
  - [ ] Window elapsed yields `stale`; a fresh attestation flips it back to `current`.
  - [ ] An unresolved `contradicted_by` attestation yields `disputed`; a later owner attestation resolves it.
  - [ ] Supersession with `correction.reason = never_true` yields `deprecated` for the superseded atom; `world_changed` yields plain history.
  - [ ] A predicate with no `[attestation]` table is `current` after one attestation, forever.
  - [ ] Loader: `[attestation]` parsed with defaults; override file can raise `k` but not lower it; spec without `[path]` loads and the materializer skips it.
  - [ ] Auto-attestation: `ingest.accept` emits exactly one owner attestation with the proposal's source; auto-accept emits one with `kind: auto_accept` provenance.
  - [ ] Render: person and affiliation projections carry `status:` frontmatter and the "as of" line; nothing is written to the store during render.
  - [ ] Auditor: status counts per predicate; "Past their window" lists only elapsed windows and reports the owner-alone count.
- Integration tests:
  - [ ] e2e: accept an affiliation, advance the clock 91 days, render: `status: stale` and the briefing lists it; `ffs attest ... --basis owner_knowledge` flips it to `current` on the next render.
  - [ ] e2e: two attestations from the same source url count as one at `k = 2`; an `independent_source` attestation makes it `current`.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- An affiliation accepted 91 days ago with no re-confirmation shows `stale` in its file and appears in the briefing's "Past their window"; the owner's attest flips it to `current`.
- Every accepted atom carries exactly one owner attestation with no extra click.
- No status transition ever supersedes, retracts, or hides an atom.
- All tests passing; coverage >= 80% on new modules.
