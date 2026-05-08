---
status: pending
title: Librarian skill (Python) — working-set manager and drift watcher
type: backend
complexity: low
dependencies:
  - task_04
  - task_06
  - task_07
  - task_10
---

# Task 12: Librarian skill (Python) — working-set manager and drift watcher

## Overview
The librarian curates the materialized projection working set on disk and watches for drift — projection files whose backing atoms have changed since last render, or working-set entries the user has not interacted with in a while. It refreshes stale projections and surfaces drift flags for the daily health summary.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST be a `SKILL.md`-shaped Python skill hosted by ffs-skills-host (task 10).
- MUST apply the MVP working-set heuristic: most-recently-touched plus user-pinned.
- MUST detect drift: a projection file whose render-hash differs from the current source-atom render-hash.
- MUST refresh drifted projections by calling `projection.render` and writing the result to disk.
- MUST surface drift flags as items in the daily health summary (via the auditor's input queue).
- MUST evict working-set entries that exceed a configurable size cap, oldest-first.
- SHOULD run on a configurable cadence (default every 30s) plus on-demand triggers via the dispatcher.
</requirements>

## Subtasks
- [ ] 12.1 Author the `SKILL.md` metadata.
- [ ] 12.2 Implement the working-set heuristic (recency + user-pinned).
- [ ] 12.3 Implement drift detection by comparing stored `last_render_hash` against the current render.
- [ ] 12.4 Implement projection refresh on detected drift.
- [ ] 12.5 Surface drift flags via the daily-summary input queue.
- [ ] 12.6 Implement size-cap eviction.

## Implementation Details
Create `skills/librarian/SKILL.md`, `skills/librarian/watcher.py`, and `skills/librarian/tests/`. The librarian consumes `working_set` table state (per ADR-016) via the daemon's JSON-RPC and writes refreshed projections to disk via the daemon's `projection.render` plus a `working_set.materialize` method (the latter added to the dispatcher in task 07's method set).

See PRD § Core Features § Editor-agnostic working set and § Open Questions § Working set materialization heuristics.

### Relevant Files
- `skills/librarian/SKILL.md` (new).
- `skills/librarian/watcher.py` (new) — primary logic.
- `skills/librarian/tests/` (new) — pytest tests.
- `skills/librarian/definition.atom.json` (new) — FFS agent definition atom.

### Dependent Files
- Daily health summary (task 13, task 19) — consumes drift flags.

### Related ADRs
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Why working set exists.
- [ADR-016: Single SQLite database per substrate](adrs/adr-016.md) — `working_set` table.

## Deliverables
- A working librarian skill in `SKILL.md` form.
- Working-set heuristic implementation.
- Drift detection and refresh.
- Size-cap eviction.
- Pytest test suite.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against the daemon's working-set methods **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Recency-based working-set heuristic ranks recently-touched projections higher.
  - [ ] User-pinned projections never evict regardless of recency.
  - [ ] Drift detection: a render-hash mismatch triggers a refresh.
  - [ ] Eviction at size cap removes the oldest non-pinned projection.
- Integration tests:
  - [ ] An atom inserted into the store causes the librarian to detect drift and refresh the affected projection.
  - [ ] A pinned projection survives an eviction pass at the size cap.
  - [ ] A drift flag surfaces in the daily-summary input queue.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Working-set refresh cadence does not measurably impact daemon idle CPU usage.
- Drift detection latency: a write to the substrate produces a refreshed projection on disk within the librarian's cadence (default 30s).
