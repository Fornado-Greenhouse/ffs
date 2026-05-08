---
status: pending
title: Auditor skill (Python) — daily health summary atom authoring
type: backend
complexity: medium
dependencies:
  - task_04
  - task_05
  - task_07
  - task_10
---

# Task 13: Auditor skill (Python) — daily health summary atom authoring

## Overview
The auditor produces a daily atom summarizing substrate health: pending scribe proposals, open questions (structural ambiguity flagged by scribe), drift flags from the librarian, recent capability denials, federation pull failures, and fast-path/slow-path ratios. The summary is itself an atom (signed, capability-classified) that surfaces in the Obsidian plugin's daily-health-summary panel and via `ffs health`.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST be a `SKILL.md`-shaped Python skill hosted by ffs-skills-host (task 10).
- MUST run on a daily cadence (configurable; default 24h) plus on-demand via `health.summary` RPC.
- MUST aggregate the metrics listed in TechSpec § Monitoring and Observability § Key metrics.
- MUST surface threshold-based flags for capability denials, federation failures, ingest queue depth, and fast-path inversion.
- MUST author the summary as an atom with `predicate = "auditor.daily_summary"` and a structured claim payload.
- MUST limit the user-visible panel content to five items maximum (per PRD § Core Features § Obsidian plugin).
- MUST query the substrate via the daemon's JSON-RPC (`atom.list`, `audit_query`-style methods).
- SHOULD include a textual narrative summary in addition to structured metrics.
</requirements>

## Subtasks
- [ ] 13.1 Author the `SKILL.md` metadata.
- [ ] 13.2 Implement the metric aggregation logic.
- [ ] 13.3 Implement threshold evaluation for the four flag types.
- [ ] 13.4 Author the summary atom with structured claim payload + textual narrative.
- [ ] 13.5 Limit user-visible panel content to top-5 items.
- [ ] 13.6 Wire on-demand invocation via `health.summary` JSON-RPC method.

## Implementation Details
Create `skills/auditor/SKILL.md`, `skills/auditor/audit.py`, and `skills/auditor/tests/`. The auditor uses the daemon's `atom.list` and a `audit_query` method (added to the dispatcher in task 07) for metric aggregation.

See TechSpec § Monitoring and Observability for metric definitions and PRD § Core Features § Daily health summary specification (Open Question — auditor implementation chooses the priority order).

### Relevant Files
- `skills/auditor/SKILL.md` (new).
- `skills/auditor/audit.py` (new) — primary logic.
- `skills/auditor/tests/` (new) — pytest tests.
- `skills/auditor/definition.atom.json` (new) — FFS agent definition atom.

### Dependent Files
- Obsidian plugin daily-health-summary panel (task_19) — renders summary atoms.
- `ffs health` CLI subcommand (task_08) — reads summary atoms.

### Related ADRs
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — `ffs_audit_query` MCP tool depends on auditor metrics.

## Deliverables
- Daily summary atoms authored on schedule and on demand.
- Metric aggregation across atom-author rate, fast-path ratio, federation health, capability denials.
- Threshold-based flagging.
- Pytest test suite with golden expected summaries.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against a populated test substrate **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Metric aggregation: 100 atoms authored in 24h yields `atom_author_rate: 100`.
  - [ ] Threshold flag: 11 capability denials trigger the "X attempted out-of-scope writes" flag.
  - [ ] Fast-path inversion (slow-path > fast-path) triggers the predicate-spec advisory flag.
  - [ ] Five-item limit: with 10 candidate items, the panel shows the 5 highest-priority items.
- Integration tests:
  - [ ] On-demand invocation via `health.summary` returns a fresh summary atom.
  - [ ] Daily-cadence run authors a new atom (verifiable via `atom.list`).
  - [ ] Summary atoms have proper signature and provenance (auditor identity).
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A fresh substrate produces a "no issues to report" summary; a substrate with known seeded problems (capability denials, federation failures) produces a summary surfacing each.
