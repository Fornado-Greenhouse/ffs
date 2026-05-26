---
status: completed
title: ffs-fastpath — filesystem watcher + diff classifier + supersession-or-route-to-ingest
type: backend
complexity: high
dependencies:
  - task_03
  - task_04
  - task_06
  - task_07
---

# Task 09: ffs-fastpath — filesystem watcher + diff classifier + supersession-or-route-to-ingest

## Overview
Detect projection-file edits via OS-level filesystem watchers, classify each diff against the active predicate's reverse-map rules, and either author a supersession atom (fast-path success) or route the edit to the ingest folder as a correction (slow-path fallback). This is the load-bearing path that makes editor-agnostic editing work — the user fixes a typo in Notepad and sees it reflected within ~200ms.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST integrate the `notify` crate for cross-platform filesystem watching (inotify, kqueue, ReadDirectoryChangesW).
- MUST debounce rapid successive saves (a typical editor-save sequence emits 2-5 events) into a single classification pass.
- MUST diff the changed file against its last rendered projection and classify the diff against the active predicate's reverse-map rules.
- MUST handle the three MVP edit categories: single-line text-field, frontmatter value, additive-section bullet.
- MUST author a supersession atom on fast-path success and re-render the projection.
- MUST route ambiguous diffs to ingest as correction notebook entries (slow-path).
- MUST refuse fast-path on federated projections (`from/<peer>/`) and projections without reverse-map annotations.
- MUST acknowledge fast-path edits within 200ms (PRD performance budget).
- MUST emit `event.fastpath.applied` or `event.fastpath.routed_to_ingest` notifications via the dispatcher.
- SHOULD reconcile on daemon restart: scan working-set files for changes that occurred while the daemon was down.
</requirements>

## Subtasks
- [x] 9.1 Define `ProjectionDiff`, `EditClassification`, `FastPathResult` types.
- [x] 9.2 Wire the `notify` crate against the projection working-set directories.
- [x] 9.3 Implement event debouncing.
- [x] 9.4 Implement the diff classifier consuming reverse-map rules.
- [x] 9.5 Implement fast-path supersession-atom authoring and re-render.
- [x] 9.6 Implement slow-path routing to ingest folder with provenance.
- [x] 9.7 Implement on-restart reconciliation against the working-set state.

## Implementation Details
Create `crates/ffs-fastpath/src/lib.rs` and submodules. The classifier consumes reverse-map annotations from task 06's projection renderer (each render emits annotations linking output elements to atom fields). On a diff, walk the reverse-map rules in order; the first rule that fully accounts for the diff wins. No match → slow-path.

See ADR-014 (root) for the fast-path scope decision and ADR-021 for reverse-map rule semantics.

### Relevant Files
- `crates/ffs-fastpath/src/lib.rs` (new) — primary module.
- `crates/ffs-fastpath/src/watcher.rs` (new) — filesystem watching.
- `crates/ffs-fastpath/src/classifier.rs` (new) — diff classification.
- `crates/ffs-fastpath/src/dispatch.rs` (new) — supersession authoring + ingest routing.
- `crates/ffs-core/src/predicate.rs` (task_03) — reverse-map rule source.
- `crates/ffs-core/src/projection.rs` (task_06) — last-rendered hash and reverse-map annotations.
- `crates/ffs-daemon` (task_07) — owns the fastpath instance.

### Dependent Files
- Obsidian plugin (task_18) — receives `event.fastpath.applied` for optimistic UI updates.

### Related ADRs
- [ADR-014: Minimum-viable fast-path](adrs/adr-014.md) — Three edit categories, three predicate types.
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Why fast-path exists.
- [ADR-021: Predicate spec format](adrs/adr-021.md) — Reverse-map rule shape.

## Deliverables
- Cross-platform filesystem watching for projection paths.
- Diff classifier supporting the three MVP edit categories.
- Fast-path supersession authoring and slow-path ingest routing.
- Restart reconciliation.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests covering each edit category end-to-end **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Single-line text edit (e.g., `display_name: "Sarah" → "Sara"`) classifies as `single_line_text` and produces a supersession atom on the matching field.
  - [ ] Frontmatter value edit (e.g., `tier: introducible → discreet`) classifies as `frontmatter_value` and produces a supersession.
  - [ ] Additive-section edit (e.g., new bullet in Notes section) classifies as `additive_section` and produces a new atom.
  - [ ] Multi-paragraph rewrite classifies as ambiguous and routes to ingest.
  - [ ] Edit to a federated projection (`from/<peer>/...`) routes to ingest without authoring.
  - [ ] Edit to a projection without reverse-map annotations routes to ingest.
  - [ ] Debouncer collapses 5 rapid save events into one classification pass.
- Integration tests:
  - [ ] End-to-end: editor writes a file → watcher fires → classifier authors atom → projection re-renders within 200ms.
  - [ ] On Linux (inotify), macOS (kqueue), and Windows (ReadDirectoryChangesW), file edits are detected and classified.
  - [ ] Daemon restart picks up an edit made while the daemon was down.
  - [ ] `event.fastpath.applied` notification is published with the new atom hash.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- p95 latency from filesystem-event to atom-commit under 200ms.
- Each of the three MVP predicate types (`contact.person`, `person.generic`, `note`) supports fast-path edits for the three edit categories.
