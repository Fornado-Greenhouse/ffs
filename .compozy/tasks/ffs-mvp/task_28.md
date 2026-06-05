---
status: pending
title: Obsidian plugin polish — unsubscribe handles and render-on-demand fallback
type: refactor
complexity: low
dependencies:
  - task_17
  - task_19
---

# Task 28: Obsidian plugin polish — unsubscribe handles and render-on-demand fallback

## Overview
The plugin fixes landed in the post-task_22 rehearsal (commit `da09415`) wired up real Obsidian-runtime surfaces but punted on two ergonomic issues that the rehearsal surfaced: (1) `DaemonClient.onStateChange` and `SummaryPanelModel.onChange` don't return unsubscribe handles, so views leak listeners when they close; (2) the entity-search modal's `onChooseSuggestion` falls back to a console log when the selected entity's projection isn't materialized on disk yet, instead of triggering a render-on-demand. This task closes both gaps.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST change `DaemonClient.onStateChange(fn)` to return a `() => void` unsubscribe handle and update all call sites (currently `main.ts::SummaryView`) to invoke it on `onClose`.
- MUST change `SummaryPanelModel.onChange(fn)` to the same shape, with matching call-site updates.
- MUST update the entity-search modal's `onChooseSuggestion` to call `plugin.renderProjection(path)` for the resolved candidate path; if the daemon returns a render, write the markdown to the vault via `app.vault.create` (or `modify` if the file exists) and open the file in the active leaf.
- MUST surface clear errors when the render fails (capability denial, predicate-not-found) using Obsidian's `Notice` API rather than `console.warn`.
- MUST keep the existing 61 vitest tests passing; the unsubscribe-handle change is API-level and may require small test updates.
- MUST fix the daily-summary panel's "Recent proposals" overflow: the `source_uri` displayed for each proposal currently exceeds the sidebar width and gets visually cut off. Display the URI's basename (last path segment) by default, with the full URI available via `title=` attribute on hover. Same treatment for any other long fields in the panel (entity URIs, federation peer endpoints).
- SHOULD add a brief in-panel "Last commit: HH:MM:SS" indicator alongside the existing refresh timestamp, sourced from the most-recent `event.atom.committed` notification — useful confirmation that the substrate is live without opening the dev console.
</requirements>

## Subtasks
- [ ] 28.1 Change `DaemonClient.onStateChange` to return an unsubscribe handle; update `SummaryView` in `main.ts` to call it from `onClose`.
- [ ] 28.2 Change `SummaryPanelModel.onChange` to return an unsubscribe handle; same call-site update.
- [ ] 28.3 Implement render-on-demand in `EntitySearchModal.onChooseSuggestion`: try `plugin.renderProjection(path)`, write the file, open it.
- [ ] 28.4 Replace `console.warn` user-facing failures in the view with `new Notice(...)` calls so users actually see them.
- [ ] 28.5 Add a "Last commit" line to the panel header sourced from the `event.atom.committed` stream.
- [ ] 28.6 Truncate long `source_uri` strings in the proposals list to their basename, with the full path on hover via `title=`. Same for entity URIs and peer endpoints. Add CSS `text-overflow: ellipsis` as a defense-in-depth fallback for any field that exceeds the sidebar width.

## Implementation Details
Edit only `obsidian-plugin/src/client.ts`, `obsidian-plugin/src/summary.ts`, and `obsidian-plugin/src/main.ts`. The existing tests under `obsidian-plugin/tests/` rely on the current `onChange`/`onStateChange` signatures; update them to assert the unsubscribe-handle behavior (callback is no longer invoked after unsubscribe).

The render-on-demand path leverages the existing `projection.render` daemon RPC and the existing `renderProjection` helper in `obsidian-plugin/src/projection.ts`. Adding a `Notice` import to `main.ts` is the only new runtime touchpoint.

### Relevant Files
- `obsidian-plugin/src/client.ts` — `onStateChange` definition.
- `obsidian-plugin/src/summary.ts` — `SummaryPanelModel.onChange` definition.
- `obsidian-plugin/src/main.ts` — `SummaryView`, `EntitySearchModal`, view lifecycle hooks.
- `obsidian-plugin/src/projection.ts` — `renderProjection` helper used by the on-demand fallback.

### Dependent Files
- `obsidian-plugin/tests/summary.test.ts` — verifies `onChange` semantics; update to cover unsubscribe.
- `obsidian-plugin/tests/client.test.ts` — verifies `onStateChange`; update similarly.

### Related ADRs
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Render-on-demand is the editor-agnostic fallback when the working set hasn't materialized a path yet.

## Deliverables
- Unsubscribe handles on `DaemonClient.onStateChange` and `SummaryPanelModel.onChange`.
- Render-on-demand fallback in `EntitySearchModal.onChooseSuggestion`.
- User-facing failures surfaced via `Notice` rather than the dev console.
- "Last commit: HH:MM:SS" indicator in the daily-summary panel.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to the new unsubscribe semantics.
- Integration test for render-on-demand triggered by entity-search selection **(REQUIRED, against the mocked daemon)**.

## Tests
- Unit tests:
  - [ ] `DaemonClient.onStateChange` returns a function; calling it stops subsequent state notifications from invoking the original callback.
  - [ ] `SummaryPanelModel.onChange` returns a function; calling it stops subsequent `setState` calls from invoking the callback.
  - [ ] `EntitySearchModal.onChooseSuggestion` with a path that hasn't materialized yet calls `projection.render`, writes the file, and opens it (mocked vault).
  - [ ] `EntitySearchModal.onChooseSuggestion` with a path that has materialized opens it directly without re-rendering.
  - [ ] Failure-path `Notice` is constructed with a human-readable message (no stack traces).
- Integration tests:
  - [ ] Mocked daemon returns a render for a path; modal's onChooseSuggestion writes the file and opens it; vault's `create` and `workspace.openFile` are both called.
  - [ ] Mocked daemon returns a `CapabilityDenied` error; modal surfaces a `Notice` (no console-only warn).
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A live Obsidian rehearsal of the entity-search flow opens a contact whose projection isn't on disk yet (rendering on demand).
- Closing and reopening the summary panel doesn't accumulate ghost listeners (verifiable via dev-tools heap-snapshot — leak count stays flat across N open/close cycles).
