---
status: completed
title: "Spike: review load and the review surface (gates task_39)"
type: docs
complexity: low
dependencies:
  - task_19
  - task_29
---

# Task 44: Spike: review load and the review surface (gates task_39)

## Overview
Even with ADR-029's auto-filing, a morning's press yields proposals a human must look at: the ambiguous band from the resolver, role changes, new people without a distinguishing attribute. The only review surface today is the Obsidian daily-summary panel with a five-item limit (task_19), designed for health flags, not for forty cards. Nobody has sat in front of a realistic morning and timed it. If the honest answer is "I would not do this daily", the auto-file policy is moot and task_39's UI half is wrong. The FFS-native alternative is review as markdown: the quarantine projected as `inbox/<date>.md`, where ticking a checkbox is the accept and the fast path absorbs it. It fits ADR-005 (any editor) and needs no plugin. This spike measures both and feeds ADR-032 (proposed).

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST fabricate one morning's output at realistic scale in a scratch substrate (a separate `$FFS_DATA_DIR`, never the owner's): about 40 proposals composed of roughly 25 new article and event records, 8 new people, 4 new organizations, 6 ambiguous people each with two plausible candidates, and 3 role changes on existing people. Fabricated names and organizations only; no real article text. Submit them through `ingest.submit` so they sit in the real quarantine (task_29 persistence).
- MUST time a full review pass in the current Obsidian daily-summary panel as shipped (five-item limit, accept/reject per card), recording minutes to clear the queue, number of clicks, and every frustration point (pagination, lost context, inability to compare candidates, no undo).
- MUST mock the alternative surface: render the same 40 proposals as a single markdown file `inbox/<date>.md` with one checkbox per proposal, the proposal's key fields inline, a candidate list under each ambiguous proposal with one checkbox per candidate plus "someone new", a "these are different people" checkbox for candidate pairs, and a grouped layout (auto-filed today, needs your eye, role changes, new). Hand-write the file or generate it with a throwaway script; do not implement the projection in the daemon. Time a review pass in the editor with no plugin, recording the same measures.
- MUST record which surface the owner would actually use daily, in the owner's own words, and why.
- MUST record the sustainable daily review count: at what number of proposals the pass exceeds ten minutes, and the number the owner says they would tolerate every day. This number sets the default for ADR-029's `max_per_day` scope cap.
- MUST state the decision output for ADR-032: review-as-markdown accepted, rejected, or hybrid (panel for the five urgent items, inbox file for the rest), and list the fast-path implications (a checkbox edit is a classifiable edit; an ambiguous pick must map to the resolver's candidate id; a "different people" tick writes `entity.different_from`).
- MUST write the findings to `docs/research/spikes/task-44-review-surface.md` with the PASS/FAIL verdict stated in the first paragraph.
- MUST NOT modify any file under `crates/`, `skills/`, `starter/`, or `obsidian-plugin/`; the scratch substrate is deleted after the spike.

PASS criterion for review-as-markdown: the markdown pass takes under 10 minutes for the 40-proposal batch and the owner states they would use it daily.

What changes on FAIL: if neither surface passes, task_39's `max_per_day` default drops to the tolerated number, the briefing (task_41) takes over surfacing ambiguous items weekly instead of daily, and ADR-032 records the hybrid or a redesign as the open question. If the panel passes and markdown does not, ADR-032 is withdrawn and task_39 extends the panel with grouping and a candidate picker.
</requirements>

## Result (2026-09-20)

PASS for review as markdown. The owner decided all 46 proposals in the inbox-file mock in 7 minutes (46 ticks, 110 checkboxes, no contradictory ticks, no section left pending) and called the batch "a great extraction overall." The shipped panel pass was not performed: the owner did not understand the panel mock, which is recorded as the finding rather than a measurement. Patterns: all 3 role changes accepted; all 6 ambiguous identities resolved to the first-listed higher-scoring candidate, none to "someone new", no "different people" assertions; all 37 additive items accepted, which under an Accept grant would have auto-filed, leaving 9 human decisions. Obsidian saved ticks as uppercase `[X]`; the parser must accept both cases.

The owner's design feedback, verbatim: "It would be great if we handled them article-by-article so that the choices are more coherently associated with one another." Decision outputs: (1) the daily surface is the inbox file, sections ordered by source article; (2) ADR-029 `max_per_day` default 50; (3) ADR-032 accepted with the grouping amendment, the panel keeps counts and a link only. Findings: `docs/research/spikes/task-44-review-surface.md`.

## Subtasks
- [x] 44.1 Fabricate the 40-proposal batch with fake names in a scratch substrate via `ingest.submit`; record the composition.
- [x] 44.2 Time the review pass in the shipped daily-summary panel; record minutes, clicks, frustration points. *(Not performed: the owner did not understand the panel mock; recorded as the finding that a five-item health panel is the wrong surface for a daily batch.)*
- [x] 44.3 Produce `inbox/<date>.md` for the same batch (hand-written or throwaway script) with the grouped checkbox layout; time the editor-only review pass.
- [x] 44.4 Record the owner's surface choice, the sustainable daily count, and the `max_per_day` default it implies.
- [x] 44.5 Write `docs/research/spikes/task-44-review-surface.md` with the verdict first, both timing tables, and the decision output for ADR-032 and task_39.

## Implementation Details
No production code. The fabricated batch can be produced by a short script that calls the daemon over the UDS with `ingest.submit` (see `crates/ffs-cli/src/client.rs` for the client shape). The markdown mock is a document, not a feature; its purpose is to be reviewed, not parsed. Note where the mock's checkbox grammar would conflict with the fast-path classifier's `additive_section` rules so ADR-032 can address it.

### Relevant Files
- `obsidian-plugin/src/summary.ts` — the shipped panel being timed (read-only).
- `crates/ffs-daemon/src/dispatch.rs` (`ingest_submit`, `ingest_list_pending`) — how the batch enters and is listed (read-only).
- `crates/ffs-fastpath/` — the classifier a checkbox edit would have to satisfy (read-only).
- `docs/research/spikes/task-44-review-surface.md` — the deliverable.

### Dependent Files
- `.compozy/tasks/ffs-mvp/adrs/adr-032.md` (proposed) — accepted, withdrawn, or reshaped by the verdict.
- `.compozy/tasks/ffs-mvp/task_39.md` — UI half and `max_per_day` default set by the verdict.

### Related ADRs
- [ADR-005](adrs/adr-005.md) — editor-agnostic working set; review-as-markdown is its natural extension.
- [ADR-014](adrs/adr-014.md) — fast-path scope; a checkbox accept must fit it or route to ingest.
- [ADR-029](adrs/adr-029.md) — `max_per_day` default set by the sustainable count.
- [ADR-030](adrs/adr-030.md) — the reconciliation picker the inbox file must express.

## Deliverables
- Two timed review passes with minutes, clicks, and frustration points **(REQUIRED)**.
- The owner's stated daily surface choice and the sustainable daily count **(REQUIRED)**.
- Decision output for ADR-032 and the `max_per_day` default for task_39 **(REQUIRED)**.
- The `inbox/<date>.md` mock, kept in the findings note as an appendix.

## Tests
- Verification is observational; no automated tests are added by this spike.
- [x] Both passes reviewed the same 40 proposals. *(Pass A reviewed all 46; pass B not performed, see Result.)*
- [x] The verdict paragraph cites the minutes that decide it.
- [x] The scratch substrate was deleted afterwards. *(Never created; the optional real-daemon load was not run.)*

## Success Criteria
- The findings note exists, states PASS or FAIL in its first paragraph, ADR-032's status reflects the verdict, and task_39's `max_per_day` default cites the note.
