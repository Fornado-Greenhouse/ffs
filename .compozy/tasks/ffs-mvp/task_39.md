---
status: pending
title: Auto-file policy — `Accept` capability action + additive/conflict routing in the quarantine (ADR-029)
type: backend
complexity: high
dependencies:
  - task_29
  - task_38
  - task_44
  - task_45
---

# Task 39: Auto-file policy — `Accept` capability action + additive/conflict routing in the quarantine (ADR-029)

## Overview
The morning-newspaper goal produces 30–60 quarantine proposals a day. Clicking accept on each one is not "keeping the cabinet up to date"; it is the clerk's job done by hand. But the quarantine is the AARM Approval Service and ARCHITECTURE.md names batch approval as an anti-pattern, so the fix cannot be a button. ADR-029 records the decision: a new capability action `Accept`, granted as an ordinary `capability.grant` atom to a specific agent identity and scoped to predicates, plus an **additive rule** enforced by the substrate — auto-accept fires only for proposals that create a new entity, append to a spec-declared additive section, or add an append-only record (`source.article`, `event.business`); any proposal that would supersede an existing scalar field, that the scribe tagged `conflicts`, or that targets a multi-leaf entity always routes to review. Auto-accepted atoms carry the authorizing capability hash in provenance; the daily summary lists what was filed with per-item undo via a new `ingest.retract`; a fresh substrate has no grant (default off).

Amended 2026-09-14 (ADR-030). The entity-identity research (`docs/research/2026-09-14-entity-resolution.md`) made the quarantine the Fellegi-Sunter review band: task_36's resolver now emits `resolution: existing | new | ambiguous` with a `candidates[]` list. This task treats `ambiguous` as always conflicting, renders the review card for it as a reconciliation picker, appends the mention text to the chosen entity's aliases on accept, writes `entity.different_from` only on an explicit owner click, and adds a "Merge into..." action that writes `entity.same_as` with undo by supersession.

Amended 2026-09-14 (ADR-032 proposed, spike task_44) and 2026-09-20 (ADR-032 accepted: inbox file ordered by source, panel counts and link only; `max_per_day` default 50). The review surface is no longer conditional. ADR-032 proposes that the quarantine is also a projection, `inbox/<YYYY-MM-DD>.md`, with the decisions this task introduces (accept, reject, the reconciliation picker, merge, keep separate, undo) as checkbox lines the fast path parses. Spike task_44 puts one realistic morning in front of the owner in both shapes and decides. The requirements below carry the accepted branch; the daemon-side work (classifier, `Accept` action, RPCs, CLI) is identical either way. The resolver this task consumes now lives in task_45 (split out of task_36), and the `max_per_day` default is 50 from the spike.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add `Action::Accept` (serialized `"accept"`) to `ffs_core::capability::Action` and make `CapabilityClaim` deserialization tolerate unknown action strings (skip, do not fail) so peers on older versions can pull grants they do not understand. Evaluator tests MUST cover: a grant with `actions: ["accept"]` and matching predicate scope → `Allow`; predicate scope mismatch → `Deny`; a superseded grant → `Deny`; `as_of` outside the bitemporal window → `Deny`; a grant carrying only `Write` → `Deny` for `Accept`.
- MUST add `max_per_day: Option<u32>` to `CapabilityScope` (`serde(default)`, omitted when `None`). `narrows_or_equals` MUST treat a smaller-or-equal cap as narrowing and `None` (unlimited) as broader than any `Some`. Usage MUST be counted from the store (atoms with an `auto_accept` provenance entry whose hash equals the grant's hash, `tx_time` within the current UTC day), not from process memory, so the cap survives daemon restarts.
- MUST implement the additive/conflict classifier in `ffs-core::quarantine` as a pure function `classify(proposal, heads, spec) -> Filing::{Additive, Conflicting(reason)}` per ADR-029 § Decision (2). It MUST consult `spec.rendering.additive_sections`, a new optional `[quarantine] append_only = true` spec field (parsed by the predicate loader; ADR-021 extension), the scribe's `resolution` tag from task_36, and multi-leaf head state. Property tests MUST establish: a proposal setting any field that already has a scalar value on a head atom is never `Additive`; a proposal that only appends to `additive_sections` is `Additive` for every generated head state; an append-only predicate's proposal is `Additive` regardless of existing records; a proposal whose resolver candidates include a different `organization` at the same name is `Conflicting`.
- MUST treat `resolution: "ambiguous"` as `Conflicting` unconditionally (amended 2026-09-14, ADR-030): no `Accept` grant, cap, or predicate scope can auto-file an ambiguous proposal. The classifier consumes `resolution` and `candidates[]` from task_36; a proposal with `resolution: "existing"` is classified against that entity's heads, a proposal with `resolution: "new"` is classified as a new entity (additive when the predicate allows).
- MUST render the review card for an ambiguous proposal as a reconciliation picker (amended 2026-09-14, ADR-030): the candidates with their score and `matched_on` evidence (`display_name`, `alias`, `context`) and a one-line summary of each candidate's head (organization, role), a "someone new" option that accepts the proposal as a new entity, and a "these are different people" button that writes an `entity.different_from` atom between the proposal's chosen entity and the rejected candidate. Picking one candidate over another MUST NOT write `different_from`; only the explicit button does.
- MUST append the mention text to the chosen entity's `aliases[]` (amended 2026-09-14, ADR-030) whenever an accepted or auto-accepted proposal resolved to an existing entity and its mention text differs from that entity's `display_name`. The append is an additive-section edit through the same signing path, carries the accepting submission in provenance, and is itself eligible for auto-filing.
- MUST add a "Merge into..." action on person and org cards (amended 2026-09-14, ADR-030) that writes an `entity.same_as` atom (entity = the losing id, claim `{target, reason, criterion}`, owner-signed) via a new `entity.merge { source, target, reason, criterion }` daemon method, and an undo that supersedes the `same_as` atom with `valid_to = now` via `entity.unmerge { same_as_hash }`. The losing entity's atoms MUST stay in place; the materializer renders its projection as a single `Merged into [[...]]` line and the winner's projection includes the losing entity's atoms. `ingest.list_auto_filed` MUST also list merges and unmerges so the daily summary can show them.
- MUST wire auto-accept into the ingest pipeline at the point `quarantine.complete` attaches proposals: resolve the grantee identity from the submission's `source_uri` / provenance (never from content), evaluate `Accept` per proposal target, classify, and sign additive proposals through **the same signing + insert + `AtomCommitted` path as `ingest.accept`** (single writer; materializer renders the file). Conflicting proposals MUST remain `Extracted` and appear in `ingest.list_pending` unchanged. A submission with a mix of additive and conflicting proposals MUST auto-file the additive ones and leave the rest pending, with the submission status reflecting the partial state.
- MUST add `SubmissionStatus::AutoAccepted` (and a partial state for mixed submissions) to the quarantine trait and both backends (in-memory + SQLite, task_29 tables), populating `accepted_atom_hashes`. Existing `ingest.list_pending` semantics MUST NOT change.
- MUST stamp every auto-accepted atom with a provenance entry `{ kind: auto_accept, uri: ffs://local/atom/<capability-hash>, hash: <capability-hash> }` in addition to the proposal's own provenance. `SourceKind` gains `AutoAccept` and `Retraction`.
- MUST add `ingest.list_auto_filed { since? }` returning auto-accepted atoms (hash, entity, predicate, source_uri, tx_time) newest first, and `ingest.retract { atom_hash }` which supersedes the target with a copy carrying `valid_to = now` and provenance `kind: retraction`. `ingest.retract` MUST require the owner's `Supersede` capability on the target and MUST refuse atoms that are not heads. Nothing is erased.
- MUST extend the auditor daily summary (`skills/auditor/audit.py` + `health.summary`) with an `auto_filed` claim section: count for the window, per-predicate counts, and the item list. The panel's five-item cap applies to flags only, not to this list.
- MUST implement the review surface per ADR-032 as accepted 2026-09-20: the inbox file `inbox/<YYYY-MM-DD>.md`, sections ordered by SOURCE (one per article or feed item, headed by the source title, url, and provenance) with every proposal derived from that source nested under it (the article record, people, organizations, affiliations, events, ambiguous resolutions, role changes), each with its own checkbox block; a per-source `accept all under this article` checkbox that expands to the individual accepts and is refused with a parse warning if an ambiguous section under it has no candidate pick; a final Housekeeping section for cross-source items (merge suggestions, past-window attestations, parse warnings); the "Auto-filed today" list with per-item undo, the reconciliation picker for `ambiguous` proposals, the "Merge into..." and "keep separate" actions, and the retract undo all as rendered sections and checkbox lines per ADR-032 § Decision (1) to (3) and the Amendment; the daemon materializes and re-materializes the file through the suppression registry; the fast path gains the `inbox_decision` reverse-map edit kind whose parser accepts `[x]`, `[X]`, and any non-space character inside the brackets, matches candidate lines on the comment-carried ids, and turns ticks into `ingest.accept` (with `resolved_entity` for candidate lines), `ingest.reject`, `entity.assert_different`, `entity.merge`, and `ingest.retract`, routing contradictory or malformed blocks to a parse warning rendered under the section and surfaced in the panel; the Obsidian summary panel shows only a count line ("N pending, M need your eye") and a link to today's inbox. vitest covers the plugin model and the Obsidian-runtime wrapper stays in `main.ts` per CLAUDE.md.
- MUST set the default `max_per_day` printed in `ffs capability grant --help` and used by the first-use guide's example to 50, spike task_44's finding (46 decisions in 7 minutes on 2026-09-20), so that auto-filed plus inbox items add up to a morning the owner will do.
- MUST add a `capability` CLI subcommand: `ffs capability grant --action <read|write|supersede|accept|…> --grantee <identity> --predicates a,b [--classifications …] [--max-per-day N | --unlimited] [--valid-to …]`, `ffs capability list` (active grants, cap, today's usage), `ffs capability revoke <grant-hash>` (authors a superseding capability with an empty action list). `grant --action accept` without `--max-per-day` or `--unlimited` MUST refuse with a usage error. Backed by new daemon methods `capability.grant`, `capability.list`, `capability.revoke` (owner-signed; the CLI never holds the key).
- MUST default off: no `Accept` grant on a fresh substrate; the owner self-grant bootstrapped on first boot (commit `d1b51e0`) MUST NOT include `accept`.
- MUST document: `docs/onboarding/first-use-guide.md` gains "Letting the clerk file for you" (what auto-files, what never does, how to undo, how to turn it off); `docs/onboarding/troubleshooting.md` gains "Why did this get filed automatically?" (read the `auto_accept` provenance, `ffs capability list`, `ingest.retract`); `docs/agent-memory/CONVENTION.md` § 16 (Human override; renumbered 2026-09-20) gains a sentence that auto-filing is the owner's grant, not the agent's decision.
- MUST NOT change the `ffs_author_atom` MCP signature or the `ingest.accept` / `ingest.reject` semantics.
</requirements>

## Subtasks
- [ ] 39.1 `Action::Accept` + unknown-action-tolerant deserialization; `CapabilityScope.max_per_day` with `narrows_or_equals` semantics; evaluator + scope unit tests (`crates/ffs-core/src/capability/`).
- [ ] 39.2 Predicate loader: optional `[quarantine] append_only` field (`crates/ffs-core/src/predicate/mod.rs`); starter specs for `source.article` and `event.business` (from task_38) set it.
- [ ] 39.3 Classifier `classify(proposal, heads, spec) -> Filing` in `crates/ffs-core/src/quarantine.rs` with proptest coverage; `SourceKind::{AutoAccept, Retraction}`; new submission statuses in both quarantine backends + migration in `quarantine_sqlite.rs`.
- [ ] 39.4 Dispatcher wiring: grantee resolution from `source_uri` / provenance; `Accept` evaluation + classification on `quarantine.complete`; shared `sign_and_insert` helper extracted from `ingest_accept`; `max_per_day` counting from the store; `ingest.list_auto_filed`; `ingest.retract` (`crates/ffs-daemon/src/dispatch.rs`, `api.rs`).
- [ ] 39.5 `capability.grant` / `capability.list` / `capability.revoke` daemon methods; `ffs capability` CLI subcommand (`crates/ffs-cli/src/lib.rs`, `commands.rs`); refuse `accept` without a cap.
- [ ] 39.6 Auditor `auto_filed` section in `health.summary` and `audit.py`; plugin "Auto-filed today" section with undo (`summary.ts`, `main.ts`) + vitest.
- [ ] 39.7 Docs: first-use guide, troubleshooting, CONVENTION.md § 15 sentence; `docs/agent-memory/skill/ffs-memory/SKILL.md` notes that a `submission_id` may resolve to auto-filed atoms and how to check (`ingest.list_auto_filed` is not an MCP tool; the agent still reports "proposed").
- [ ] 39.8 Live validation on the project lead's Mac: grant `accept` for `source.article,event.business,org.company,person.generic` with `--max-per-day 50`; drop a fresh-company article → org file appears in the vault with no click; drop a role change for an existing person → review card, head unchanged; undo one auto-filed atom → file reverts; revoke → next drop is pending only.
- [ ] 39.9 Reconciliation picker (amended 2026-09-14, ADR-030): classifier treats `ambiguous` as `Conflicting`; `ingest.accept` gains an optional `resolved_entity` argument (an existing id or `"new"`) for ambiguous proposals; plugin card renders candidates with score, `matched_on`, and head summary, plus "someone new" and "these are different people"; the latter calls a new `entity.assert_different { a, b, criterion }` daemon method that authors an owner-signed `entity.different_from` atom; alias growth on accept through the shared signing path (`summary.ts`, `main.ts`, `dispatch.rs`, `api.rs`) + vitest.
- [ ] 39.11 Inbox review surface (ADR-032, accepted 2026-09-20; REQUIRED): `inbox/<YYYY-MM-DD>.md` template and materialization in `crates/ffs-daemon/src/materializer.rs` (sections per SOURCE with the derived proposals nested, per-source "accept all", decision blocks, Housekeeping, "Decided", "Auto-filed today" with undo, merge candidates), suppression-registry coverage for inbox writes, the `inbox_decision` edit kind and its strict checkbox parser in `crates/ffs-fastpath/` with the contradictory-tick and malformed-block parse-warning route, the panel count line plus link; parser unit tests (`- [x]`, `- [X]`, `* [x]`, and any non-space tick accepted; per-source accept-all expansion and its refusal when an ambiguous child has no pick; candidate lines matched on the comment-carried entity id; two ticks in one block is a warning) and an e2e that ticks `accept` in the file and observes the atom committed and the file re-rendered with the section under "Decided".
- [ ] 39.10 Merge and undo (amended 2026-09-14, ADR-030): `entity.merge` / `entity.unmerge` daemon methods authoring and superseding `entity.same_as`; store helper `resolve_same_as` with cycle guard used by `list_by_entity`, `head_of_chain`, and the materializer; materializer renders the `Merged into [[...]]` stub and folds the losing entity's atoms into the winner; "Merge into..." action with an entity search box on person and org cards; merges and unmerges in `ingest.list_auto_filed` and the "Auto-filed today" section (`dispatch.rs`, `store/`, `materializer.rs`, `summary.ts`) + tests.

## Implementation Details
Current structure: `ingest_submit` in `crates/ffs-daemon/src/dispatch.rs` spawns extraction and calls `quarantine.complete(id, proposals)`; `ingest_accept` builds an `AtomTemplate` per proposal (entity from `slug_for_proposal`, `valid_from = tx_time = now`, `classification = existence`, `supersedes = None`), signs with the daemon key, inserts, and publishes `AtomCommitted`. The auto-accept path is that same loop, invoked from the completion callback for the additive subset, with an extra provenance entry and a different terminal status. The evaluator already returns the matching capability's hash in `Decision::Allow { … }` (check `decision.rs`); that hash is what the provenance entry records and what `max_per_day` counts against.

Grantee resolution: `ffs_author_atom` submissions arrive with `source_uri = mcp:agent/<identity>`; ingest-folder drops arrive with `file://…` and the courier's identity comes from the daemon's `FFS_INGEST_AGENT_IDENTITY` (task_40 introduces it; until then filesystem drops have no grantee and never auto-file).

The `[quarantine] append_only` flag is the predicate spec's own statement that its records are events, not state. `note` does not set it (a note's title is a scalar); `source.article` and `event.business` do.

### Relevant Files
- `crates/ffs-core/src/capability/mod.rs`, `scope.rs`, `decision.rs` — `Accept`, `max_per_day`, tolerant deserialization.
- `crates/ffs-core/src/quarantine.rs`, `quarantine_sqlite.rs` — classifier, statuses, migration.
- `crates/ffs-core/src/predicate/mod.rs` — `[quarantine] append_only`.
- `crates/ffs-core/src/atom.rs` — `SourceKind::{AutoAccept, Retraction}`.
- `crates/ffs-daemon/src/dispatch.rs`, `api.rs` — auto-accept wiring, `ingest.list_auto_filed`, `ingest.retract`, `capability.*` methods.
- `crates/ffs-cli/src/lib.rs`, `commands.rs` — `capability` subcommand.
- `skills/auditor/audit.py` — `auto_filed` section.
- `obsidian-plugin/src/summary.ts`, `main.ts` — panel section + undo.

### Dependent Files
- `starter/predicates/source.article.toml`, `event.business.toml` (task_38) — set `append_only`.
- `docs/onboarding/first-use-guide.md`, `troubleshooting.md`, `docs/agent-memory/CONVENTION.md`, `docs/agent-memory/skill/ffs-memory/SKILL.md`.
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — auto-file variants.
- `.compozy/tasks/ffs-mvp/adrs/README.md` — ADR-029 rows (added with the ADR).

### Related ADRs
- [ADR-029: Capability-gated auto-filing of additive proposals](adrs/adr-029.md) — this task's foundation.
- [ADR-007] — capabilities as atoms; revocation is supersession.
- [ADR-013] — the quarantine as the agent-write boundary.
- [ADR-014] — `additive_sections` as the safe-edit line (reused).
- [ADR-021] — predicate spec format (extended with `[quarantine]`).
- [ADR-026] / task_36 — the engine seam behind the proposals.
- task_45 — the resolver; `resolution` and `candidates[]` the classifier consumes.
- [ADR-032](adrs/adr-032.md) (Accepted 2026-09-20) — review as markdown; the inbox file ordered by source is the review surface.
- task_44 — the review-load spike that accepted ADR-032 and set `max_per_day` to 50.
- [ADR-027] — never silently overwrite; owner resolves conflicts; report "proposed".
- [ADR-028] — the append-only business-graph predicates.
- [ADR-030: Entity identity, resolution, and merge](adrs/adr-030.md) — `ambiguous` as the review band; the reconciliation picker; `entity.same_as` / `entity.different_from`; alias growth; `different_from` only on an explicit click.

## Deliverables
- `Action::Accept`, `CapabilityScope.max_per_day`, tolerant action deserialization, with evaluator tests **(REQUIRED)**.
- Additive/conflict classifier with property tests **(REQUIRED)**.
- Auto-accept wiring through the shared signing path, `auto_accept` provenance, new submission statuses in both backends **(REQUIRED)**.
- `ingest.list_auto_filed`, `ingest.retract`, `capability.grant/list/revoke` RPCs + `ffs capability` CLI **(REQUIRED)**.
- Auditor `auto_filed` section + plugin section with undo.
- Reconciliation picker for `ambiguous` proposals, `entity.assert_different`, alias growth on accept **(REQUIRED, amended 2026-09-14, ADR-030)**.
- `entity.merge` / `entity.unmerge`, `resolve_same_as` store helper, merged-into stub rendering, "Merge into..." action **(REQUIRED, amended 2026-09-14, ADR-030)**.
- Docs (first-use, troubleshooting, convention).
- Unit tests with 80%+ coverage on new Rust modules and Python/TS changes **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `accept_grant_with_matching_predicate_allows`, `accept_grant_predicate_mismatch_denies`, `superseded_accept_grant_denies`, `accept_outside_bitemporal_window_denies`, `write_only_grant_denies_accept` (`capability`).
  - [ ] `unknown_action_string_is_skipped_not_fatal`; `max_per_day_smaller_narrows`, `max_per_day_none_broadens` (`capability::scope`).
  - [ ] Proptests: `proposal_touching_existing_scalar_is_never_additive`, `additive_section_append_is_always_additive`, `append_only_predicate_is_always_additive`, `same_name_different_org_candidate_conflicts`, `multi_leaf_head_is_conflicting`, `ambiguous_resolution_is_always_conflicting` (`quarantine`).
  - [ ] `accept_with_resolved_entity_targets_that_entity`, `accept_with_resolved_new_mints_opaque_id`, `assert_different_authors_owner_signed_different_from`, `picking_a_candidate_does_not_write_different_from`, `accept_appends_mention_text_to_aliases_when_it_differs` (`dispatch`).
  - [ ] `merge_authors_same_as_and_leaves_losing_atoms_in_place`, `unmerge_supersedes_same_as_with_valid_to`, `resolve_same_as_follows_chain_and_stops_on_cycle`, `materializer_renders_merged_into_stub_and_folds_atoms_into_winner` (`dispatch`, `store`, `materializer`).
  - [ ] `summary.ts`: `ambiguous_card_renders_candidates_with_matched_on_and_someone_new`, `different_people_button_calls_assert_different`, `merge_into_action_calls_entity_merge_and_undo_calls_unmerge`.
  - [ ] `max_per_day_counts_from_store_and_survives_reopen` (`quarantine_sqlite` + store).
  - [ ] `retract_refuses_non_head`, `retract_requires_supersede_capability`, `retract_sets_valid_to_and_retraction_provenance` (`dispatch`).
  - [ ] `grant_accept_without_cap_is_usage_error`, `revoke_authors_empty_action_supersession` (`ffs-cli`).
  - [ ] `audit.py`: `auto_filed_section_present_and_counted`; `summary.ts`: `auto_filed_items_render_below_pending_and_undo_calls_retract`.
- Integration tests:
  - [ ] `ingest_pipeline_e2e`: with an `accept` grant, an additive proposal is signed, inserted, materialized, and carries `auto_accept` provenance with the grant hash; `ingest.list_pending` does not list it.
  - [ ] `ingest_pipeline_e2e`: a conflicting proposal (existing `role`) stays `Extracted`; the head atom is unchanged.
  - [ ] `ingest_pipeline_e2e`: mixed submission files the additive subset and leaves the rest pending.
  - [ ] `ingest_pipeline_e2e`: `max_per_day = 1` — second additive proposal in the same UTC day routes to review.
  - [ ] `ingest_pipeline_e2e`: no grant on a fresh substrate → every proposal pending (default off).
  - [ ] `ingest_pipeline_e2e`: `ingest.retract` on an auto-filed atom moves the head back and re-renders the file.
  - [ ] `ingest_pipeline_e2e`: an `ambiguous` proposal stays pending under an unlimited `accept` grant; accepting it with `resolved_entity` set to a candidate files against that entity and appends the mention text to its aliases.
  - [ ] `ingest_pipeline_e2e`: `entity.merge` of two person entities renders the losing file as a `Merged into` stub and the winner's file with both entities' atoms; `entity.unmerge` restores both files; a later mention matching the losing entity's alias resolves to the winner while the merge is active and to the losing entity after unmerge.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing; coverage ≥80% on new modules.
- With a grant in place, dropping a fresh-company article yields an `orgs/…` file in the vault with no click; the atom's provenance names the article, the engine, and the grant.
- Dropping a role change for an existing person yields a review card and never an overwrite; the head atom is byte-identical before and after.
- A fresh substrate auto-files nothing until the owner runs `ffs capability grant --action accept …`; `ffs capability revoke` stops it with the next submission.
- The daily summary shows what was filed; one click undoes an item without erasing history.
- An ambiguous mention never files itself; the picker resolves it in one click, and "these are different people" stops the pair from being suggested again.
- Merging two duplicate files is one action, loses nothing, and is undone by one action.
- `cargo nextest run --workspace --all-features`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `.venv/bin/python -m pytest skills/`, and `npm test` (from `obsidian-plugin/`) all green.
- A tick on `accept` in `inbox/<date>.md` files the proposal and the section moves under "Decided" with no plugin involved; a per-source "accept all" files every undecided item under that article; the panel shows only counts and a link.
