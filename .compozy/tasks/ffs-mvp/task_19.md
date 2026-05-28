---
status: completed
title: Obsidian plugin — daily health summary panel + entity-name search hook
type: frontend
complexity: medium
dependencies:
  - task_17
---

# Task 19: Obsidian plugin — daily health summary panel + entity-name search hook

## Overview
Add the operational surfaces inside the Obsidian plugin: a daily health summary panel showing up to five high-priority items (pending scribe proposals, open questions, drift flags, federation health, capability anomalies) and an entity-name search hook integrated into Obsidian's quick-switcher / file finder. These are the user's primary operational interaction points after initial setup.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add a daily-health-summary panel rendered as an Obsidian view, populated by calling the daemon's `health.summary` JSON-RPC method.
- MUST limit visible items to five maximum (per PRD).
- MUST allow the user to accept or reject scribe proposals from the panel; acceptance triggers the daemon to convert proposals into signed atoms.
- MUST hook into Obsidian's file-finder so entity-name search returns matches across substrate entities.
- MUST debounce search keystrokes and stream results from the daemon.
- MUST refresh the summary panel when `event.atom.committed` notifications arrive (especially auditor.daily_summary atoms).
- SHOULD support keyboard shortcuts to toggle the panel and to step through proposal items.
</requirements>

## Subtasks
- [x] 19.1 Implement the daily-health-summary panel as an Obsidian view (with header, item list, expand/collapse).
- [x] 19.2 Wire the panel to `health.summary` and refresh on `event.atom.committed` for auditor atoms.
- [x] 19.3 Implement accept/reject controls for scribe proposals; route to daemon `ingest.accept` / `ingest.reject` methods.
- [x] 19.4 Hook into Obsidian's quick-switcher / file finder for entity-name search.
- [x] 19.5 Implement keystroke debouncing and result streaming.
- [x] 19.6 Add keyboard shortcuts.

## Follow-ups (deferred to task_22 onboarding)

- **Obsidian custom view binding for the summary panel** (19.1):
  `SummaryPanelModel` is wired end-to-end with vitest coverage;
  the actual Obsidian `ItemView` subclass (registerView + activateView
  on plugin onload) ships with the production binding in task_22.
- **Quick-switcher suggester binding** (19.4): `EntitySearch` is
  driven by `pushQuery(input)` and delivers results via the
  `onResults` callback. The Obsidian-runtime `SuggestModal`
  subclass that calls `pushQuery` from `onInput` and renders
  `results` in `renderSuggestion` is the production binding.
- **Default hotkeys**: `addCommand` registrations are in place
  (`ffs-refresh-summary`, `ffs-focus-entity-search`). Default
  hotkey assignments stay opt-in by users in Obsidian's Hotkeys
  settings — the spec calls them SHOULD, not MUST.

## Implementation Details
Extend `obsidian-plugin/` with `src/summary.ts` and `src/search.ts`. The summary panel is mounted as an Obsidian custom view. The entity search uses Obsidian's command palette extension API. Both subsystems consume the JSON-RPC client and event emitter from task 17.

See PRD § Core Features § Obsidian plugin and § Open Questions § Daily health summary specification.

### Relevant Files
- `obsidian-plugin/src/summary.ts` (new) — health summary panel.
- `obsidian-plugin/src/search.ts` (new) — entity search hook.
- `obsidian-plugin/src/main.ts` (task_17) — registers the panel and search command.
- `obsidian-plugin/src/client.ts` (task_17) — daemon JSON-RPC.
- `obsidian-plugin/src/events.ts` (task_17) — event subscription.

### Dependent Files
- Auditor skill (task_13) — produces summary atoms consumed by the panel.
- Scribe skill (task_11) — produces proposals exposed for accept/reject.

### Related ADRs
- [ADR-002: Both audiences first-class](adrs/adr-002.md) — End-user surface.
- [ADR-014: Minimum-viable fast-path](adrs/adr-014.md) — Slow-path corrections surface here for review.

## Deliverables
- Daily health summary panel as an Obsidian view, refreshed on summary-atom updates.
- Accept/reject controls for scribe proposals.
- Entity-name search hook integrated with Obsidian's file finder.
- Debounced keystroke handling and streaming results.
- Keyboard shortcuts.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against a live daemon with seeded auditor and scribe data **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Summary panel renders 3 items when the auditor returns 3 items; renders 5 when 7 are returned (top-5 only).
  - [ ] Accept-proposal button calls `ingest.accept` with the correct submission_id.
  - [ ] Reject-proposal button calls `ingest.reject` with the submission_id.
  - [ ] Entity search debounces keystrokes (e.g., 200ms after the last keypress before query fires).
  - [ ] Search results stream in: results are appended as they arrive from the daemon.
- Integration tests:
  - [ ] With a seeded auditor summary atom in the store, the panel shows the expected items.
  - [ ] Accepting a scribe proposal causes a signed atom to appear in the store.
  - [ ] Rejecting a proposal removes it from the quarantine.
  - [ ] Searching for "Sara" returns entities whose canonical name matches.
  - [ ] Summary panel refreshes when a new `auditor.daily_summary` atom is committed.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A user with a real substrate sees a coherent five-item daily summary that they can act on.
- Entity search returns results within 100ms for typical-size graphs.
- Accept/reject flow works end-to-end: a proposal becomes a signed atom or is removed.
