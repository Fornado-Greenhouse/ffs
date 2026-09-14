---
status: pending
title: Scribe v3 — multi-entity proposals + entity resolver (ADR-030, ADR-031)
type: backend
complexity: high
dependencies:
  - task_36
  - task_38
---

# Task 45: Scribe v3 — multi-entity proposals + entity resolver (ADR-030, ADR-031)

## Overview
The newspaper goal turns one dropped article into many records: the article itself, the people it names, the organizations they belong to, the events it reports, and the roles that connect them. Each of those records may already exist in the substrate, so the scribe has to decide, per mention, whether it is looking at someone it knows, someone new, or someone it cannot tell apart from two candidates. This task is that clerk.

It is deliberately separate from task_36, and ordered after task_38, for two reasons. First, task_36 is the tracer bullet: it proves the extraction engines against the three starter predicates and gives the vault a readable daily digest weeks before any identity work exists. Second, the resolver needs things that only exist after task_38: opaque entity ids (`EntityId::mint`), the `affiliation`, `entity.same_as`, and `entity.different_from` predicates, the starter `config/resolution.toml`, and the path-to-entity index. It also needs real extraction output from task_36 and spike task_42 to set sensible weights and thresholds. Building the resolver before that evidence exists would mean tuning against guesses.

ADR-030 (`docs/research/2026-09-14-entity-resolution.md`) supplies the identity design: Fellegi-Sunter weights and two thresholds, three outcomes, aliases and priors that grow from accepted resolutions, merges as signed reversible atoms, and a NIL policy borrowed from Wikipedia's notability rule. ADR-031 (`docs/research/2026-09-14-top-level-ontologies.md`) supplies the shapes: a role is its own `affiliation` atom with a bearer, a context, and a temporal window; article mentions and event participants are entity references, not strings.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST support multi-entity extraction per submission (ADR-028): one article submission yields one `source.article` proposal plus N `person.generic`, M `org.company`, and K `event.business` proposals. Every proposal in the set MUST carry the article URL in provenance, and proposals MUST cross-reference each other by entity id (or display name when the id is not yet assigned) so the article's `mentions[]`, the event's `participants[]` and `source`, and the person's `organization` resolve to the same entities after acceptance. The proposals envelope on the skills-host wire MUST stay backward compatible (a single-proposal result is the degenerate case).
- MUST perform entity resolution on the daemon side before enqueueing (ADR-030): in `crates/ffs-daemon/src/scribe.rs`, a resolver that (a) blocks candidates by path family and a normalized name key; (b) generates candidates from `entity.search` v2 (`display_name`, `aliases[]`, prior count; task_40) and the store's `search_fts`, following `entity.same_as` chains so a mention matching a losing alias lands on the winner; (c) scores candidates with Fellegi-Sunter field weights on `organization`, `role`, and `location` agreement plus the prior, reading weights and the two thresholds from `$FFS_DATA_DIR/config/resolution.toml` (starter file shipped by task_38, hot-reloaded like predicate specs); (d) treats any `entity.different_from` assertion as a hard block; (e) emits per proposal `resolution: "existing" | "new" | "ambiguous"` and `candidates: [{entity, score, matched_on}]`, where `ambiguous` covers scores between the thresholds or two candidates within a margin; (f) receives the whole multi-entity set for a submission so an org proposal in the same article can raise a person candidate already affiliated with it (coherence scoring itself is Phase 2; the interface ships now); (g) applies the NIL policy: `source.article`, `event.business`, and `org.company` always mint, a `person.generic` mints on first mention only with a distinguishing attribute (organization or role) or on a second sighting keyed by normalized name plus article organization context, otherwise the mention stays in the article's `mentions[]` with `entity` unset. Entity ids for `new` are opaque (16 random bytes, base58btc multibase, `EntityId::mint` from task_38), minted in the daemon's signing path at accept or auto-accept, never derived from a name. `resolution` and `candidates` are carried through quarantine storage and `ingest.list_pending` so the review UI can render a reconciliation picker and so task_39's additive-vs-conflict rule can act on them (`ambiguous` is always conflicting).
- MUST emit `affiliation` proposals (ADR-031) alongside person and org proposals whenever an article states or implies that a person holds a role at an organization: predicate `affiliation` with `person` (entity id or display name pending resolution), `organization` (same), `title`, `kind` (the ADR-031 enum), and `source` (the article URL); `valid_from` set to the stated role start when the article gives one, otherwise the article's `published_at`. A role ending ("stepped down", "departs") MUST be emitted as a supersession proposal on the existing affiliation setting `valid_to`, never as a new affiliation, so ADR-029 routes it to review. `mentions[]` items on `source.article` and `participants[]` items on `event.business` MUST be objects `{entity, display, context}` and `{entity, display, role}` respectively, with `entity` set when resolution returns `existing` and absent otherwise; `display` is always the name as printed.
- MUST grow aliases from accepted resolutions (ADR-030): when a proposal resolved to an existing entity is accepted or auto-accepted and its mention text differs from the entity's `display_name`, the mention text is appended to the entity's `aliases[]` as an additive edit. Priors for a surface form are the counts of accepted resolutions to each entity, read from the store, never cached in process memory.
- MUST extend the golden corpus with at least five business-press fixtures, paraphrased or synthetic per task_36's licensing rule: an executive hire, a funding round, an acquisition, a real-estate opening, and a profile piece, each paired with expected multi-entity proposals (article + people + orgs + events + affiliations) and each scored by the existing per-field precision/recall scorer.
- MUST score identity: every corpus proposal carries an expected entity id (or `new`); the scorer computes pairwise precision/recall over same-entity pairs and B-cubed over resolved clusters; four identity fixtures are required: the same person across three articles with a nickname, two different people with the same name at different organizations, a rename announced in an article, and a wrong merge with its undo via `same_as` supersession.
- MUST keep the resolver deterministic and offline by default: the heuristic resolver stands alone; any LLM-assisted reranking of candidates is behind the same opt-in as the extraction engine (ADR-026) and is not part of this task.
- MUST NOT add any pip dependency to the skill bundle and MUST NOT change the skills-host wire protocol beyond the backward-compatible envelope above.
</requirements>

## Subtasks
- [ ] 45.1 Multi-entity proposal envelope + prompt rendering for every registered predicate, including ADR-028's `org.company`, `source.article`, `event.business`, and `person.generic` v2; cross-references between proposals in one set; wire shape stays backward compatible.
- [ ] 45.2 `resolution.toml` reader in the daemon (weights as m/u pairs, two thresholds, margin, blocking options; hot-reload alongside predicate specs).
- [ ] 45.3 Daemon-side resolver in `scribe.rs`: blocking, candidate generation via `entity.search` v2 + `search_fts`, `same_as` chain following, `different_from` hard block, scoring, three outcomes with `candidates[]`, whole-envelope input with the coherence hook; `resolution` and `candidates` carried through quarantine storage (task_29 tables) and `ingest.list_pending`.
- [ ] 45.4 NIL policy and opaque id minting in the signing path for `new`; second-sighting key; back-fill of the first mention when the second sighting mints.
- [ ] 45.5 `affiliation` proposals and object-shaped `mentions[]` / `participants[]` (ADR-031); role-ending as supersession proposals.
- [ ] 45.6 Alias growth on accept and auto-accept; store-backed priors; unit tests.
- [ ] 45.7 Business-press corpus fixtures (hire, funding, acquisition, opening, profile) with expected multi-entity proposals including affiliations; expected entity ids; pairwise P/R + B-cubed in the scorer; the four identity fixtures; three-outcome tests; `same_as` and `different_from` tests.

## Implementation Details
The engine seam, prompt builder, and schema validator from task_36 are reused unchanged; this task widens the envelope the engine returns and adds the daemon-side pass between the skill's output and the quarantine. The resolver is a pure function over (proposal set, candidate lookups, config) so it can be unit-tested with an in-memory store.

Resolution runs once per submission over the whole set, in dependency order: organizations first (they are the context for people), then people, then affiliations (which need both), then events and the article (which reference all of the above).

### Relevant Files
- `skills/scribe/` — envelope widening; affiliation emission; object-shaped mentions and participants.
- `skills/scribe/tests/corpus/` — new fixtures and the identity scorer.
- `crates/ffs-daemon/src/scribe.rs` — the resolver and its config reader.
- `crates/ffs-daemon/src/dispatch.rs` — minting on accept and auto-accept; alias growth.
- `crates/ffs-core/src/quarantine.rs` + `quarantine_sqlite.rs` — `resolution` and `candidates` columns.
- `crates/ffs-core/src/store/` — `resolve_same_as` helper (task_39 introduces it; reuse if present).
- `starter/config/resolution.toml` — shipped by task_38; consumed here.
- `obsidian-plugin/src/summary.ts` — `resolution` and `candidates` in the expanded proposal card (the picker itself is task_39).

### Dependent Files
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — multi-entity e2e over a paraphrased article.
- `docs/onboarding/first-use-guide.md` — what "ambiguous" means and where it shows up.

### Related ADRs
- [ADR-030: Entity identity, resolution, and merge](adrs/adr-030.md) — opaque ids, the three-outcome resolver, `resolution.toml`, `same_as` / `different_from`, NIL policy, alias growth, identity scoring.
- [ADR-031](adrs/adr-031.md) — affiliation as its own role atom; object-shaped `mentions[]` / `participants[]`.
- [ADR-028](adrs/adr-028.md) — business-graph predicates the prompt builder renders; path families.
- [ADR-029](adrs/adr-029.md) — `ambiguous` is always conflicting; alias growth is additive.
- [ADR-026](adrs/adr-026.md) — engine seam and opt-in rule this task inherits.

## Deliverables
- Multi-entity proposal sets with cross-references and article provenance **(REQUIRED)**.
- Daemon-side three-outcome resolver (`resolution: existing | new | ambiguous` with `candidates[]`), driven by `resolution.toml`, honoring `same_as` and `different_from`, applying the NIL policy, minting opaque ids **(REQUIRED)**.
- `affiliation` proposals and object-shaped `mentions[]` / `participants[]` with entity ids **(REQUIRED)**.
- Alias growth from accepted resolutions; store-backed priors **(REQUIRED)**.
- Five business-press corpus fixtures scored per predicate, plus four identity fixtures scored with pairwise P/R and B-cubed **(REQUIRED)**.
- Unit tests with 80%+ coverage on new modules **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Multi-entity: the executive-hire fixture yields one `source.article`, one `person.generic`, one `org.company`, one `event.business` (`kind: hire`), and one `affiliation` proposal, all carrying the article URL in provenance and cross-referencing by entity id.
  - [ ] Three outcomes: a score above the upper threshold yields `existing`; below the lower yields `new`; between, or two candidates within the margin, yields `ambiguous` with a populated `candidates[]`; changing the thresholds in `resolution.toml` moves the outcome without a code change.
  - [ ] `same_as` and `different_from`: a mention matching an alias of a merged (losing) entity resolves to the winner; a candidate blocked by `different_from` is never returned as `existing`, even with a perfect name match.
  - [ ] NIL policy: a bare name with no distinguishing attribute stays in `mentions[]` with `entity` unset and mints on the second sighting with both mentions back-filled; an unknown person with an organization mints on first sighting.
  - [ ] Opaque ids: a `new` resolution yields, on accept, a base58btc multibase id that is not derived from the name.
  - [ ] Affiliation: a "stepped down" fixture yields a supersession proposal setting `valid_to` on the existing affiliation, not a new one.
  - [ ] Alias growth: accepting a proposal whose mention text was "S. Chen" for the entity displayed as "Sara Chen" appends "S. Chen" to her `aliases[]`; the prior for "S. Chen" then counts one.
  - [ ] Resolution order: organizations resolve before the people that reference them within one submission.
- Integration tests:
  - [ ] Multi-entity e2e over a paraphrased article: articles/, people/, orgs/ files land in the vault with wikilinks after accept.
  - [ ] `ingest.list_pending` exposes `resolution` and `candidates` on every proposal in a multi-entity set.
  - [ ] Identity scorer: pairwise precision/recall and B-cubed computed over the four identity fixtures; the wrong-merge-undo fixture restores both entities' clusters after the `same_as` atom is superseded.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- The same person across three paraphrased articles, one using a nickname, resolves to one entity.
- Two people with the same name at different organizations stay separate.
- An ambiguous case produces a candidate list and routes to review; no Accept grant can auto-file it.
- All tests passing; coverage ≥80% on new modules; no copyrighted press text in git.
