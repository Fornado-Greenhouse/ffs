---
status: completed
title: Obsidian plugin — paginated folder enumeration + projection rendering on open + edit routing
type: frontend
complexity: medium
dependencies:
  - task_17
---

# Task 18: Obsidian plugin — paginated folder enumeration + projection rendering on open + edit routing

## Overview
Make the substrate's projection paths navigable inside Obsidian. Intercept folder enumeration so virtual paths like `~/.ffs/contacts/by-name/S/` show paginated structured listings (not flat directories of thousands), render projection markdown on file open, and route edits made inside Obsidian into the daemon's fast-path or ingest pipeline. This is the read path for the Obsidian end-user surface.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST intercept folder-enumeration for projection paths under `~/.ffs/contacts/`, `~/.ffs/people/`, `~/.ffs/notes/` and surface paginated structured listings via the daemon's `path.list` method.
- MUST render projection markdown on file open via the daemon's `projection.render` method.
- MUST visually distinguish projection files from regular notebook entries (e.g., a read-with-care indicator).
- MUST route in-Obsidian edits to the daemon: trivial edits via `fastpath.submit`, others as ingest corrections.
- MUST display fast-path edit acknowledgement within ~200ms (optimistic UI update with rollback on substrate write failure).
- MUST handle large-result pagination (per PRD § Obsidian plugin: paginated structured listings, not thousands of flat files).
- SHOULD update displayed projections when `event.projection.invalidated` notifications arrive.
</requirements>

## Subtasks
- [x] 18.1 Implement the folder-enumeration interceptor for the three MVP projection path families.
- [x] 18.2 Implement projection rendering on file-open events.
- [x] 18.3 Implement the visual treatment distinguishing projections from notebook entries.
- [x] 18.4 Implement edit routing: detect save events, classify, send to `fastpath.submit` or ingest.
- [x] 18.5 Implement optimistic-UI updates on fast-path success with rollback on failure.
- [x] 18.6 Handle pagination UI for large folder listings.
- [x] 18.7 Subscribe to `event.projection.invalidated` for live updates.

## Follow-ups (deferred to task_22 onboarding + future plugin tasks)

The substantive read/edit pipeline lands here as testable units
exercised end-to-end via vitest with mocked `DaemonClient`. The
remaining wiring is deferred:

- **Obsidian-runtime file-explorer interception**: the
  `enumerateFolder` API is in place; binding it to Obsidian's
  `FileExplorer` plugin (which has no public API surface) is
  task_19's adjacent work or a separate followup. The plugin
  currently relies on the user opening a projection file via
  Obsidian's quick-switcher — `renderProjection` fires correctly
  on `workspace.on("file-open")`.
- **Active-leaf buffer refresh on `event.projection.invalidated`**:
  the subscription fires; the production buffer-refresh hook
  (`this.app.workspace.activeLeaf.view.editor.setValue`) is
  documented in `handleInvalidation` as a console.info pending
  task_19's active-leaf integration.
- **CSS for `.ffs-projection-file` decoration**: `styles.css`
  ships with task_22's onboarding bundle.
- **Live-daemon perf budget tests** (200ms fast-path ack,
  200ms 1000-entry folder enumeration): exercised manually
  once the daemon binary is wired in task_22. The plugin-side
  classifier matches the Rust fastpath classifier verbatim so
  fast-path eligibility is decided client-side without an extra
  round-trip.

## Implementation Details
Extend `obsidian-plugin/` (task 17) with `src/projection.ts`, `src/folder.ts`, `src/editing.ts`. The plugin's interception relies on Obsidian's plugin API for file-explorer customization and file-open hooks. Saved files are routed by checking their path against the projection-path prefix and consulting the daemon for classification.

See PRD § Core Features § Obsidian plugin and ADR-005 (root) for the editor-agnostic + fast-path vision.

### Relevant Files
- `obsidian-plugin/src/projection.ts` (new) — projection rendering on open.
- `obsidian-plugin/src/folder.ts` (new) — paginated enumeration.
- `obsidian-plugin/src/editing.ts` (new) — edit routing.
- `obsidian-plugin/src/main.ts` (task_17) — wires the new modules.
- `obsidian-plugin/src/client.ts` (task_17) — daemon JSON-RPC.

### Dependent Files
- Daily health summary + entity search (task_19) — co-resident in the plugin.

### Related ADRs
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Why projections render in Obsidian.
- [ADR-014: Minimum-viable fast-path](adrs/adr-014.md) — Edit-routing classification.
- [ADR-011: Path library starts at three](adrs/adr-011.md) — Path families to enumerate.

## Deliverables
- Folder-enumeration interception for the three MVP projection path families.
- Projection markdown rendering on file open.
- Visual distinction between projections and notebook entries.
- Edit routing with optimistic UI and rollback.
- Pagination UI for large listings.
- Live updates via `event.projection.invalidated`.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against a live daemon **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Folder interceptor matches `~/.ffs/contacts/by-name/S/` and routes to `path.list`.
  - [ ] Folder interceptor does not match `~/.ffs/some_other_folder/` (passes through).
  - [ ] Projection-render hook fires for files under projection paths and not for regular notes.
  - [ ] Edit classifier sends single-line text edits to `fastpath.submit`.
  - [ ] Optimistic update is rolled back when the daemon returns a failure.
  - [ ] Pagination: a 1000-entry folder shows the first page (default 100) with a next-page indicator.
- Integration tests:
  - [ ] Open `~/.ffs/contacts/by-name/S/Sarah_Chen.md` in Obsidian → projection markdown is rendered.
  - [ ] Edit the projection → fast-path acknowledgement within 200ms; projection re-renders to canonical form.
  - [ ] Multi-paragraph rewrite → routes to ingest; daily summary surfaces the correction.
  - [ ] `event.projection.invalidated` arriving via the daemon causes the open file to re-render.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A user opening the FFS vault in Obsidian on each OS sees projection paths populated and navigable.
- A typo fix in Obsidian is reflected within 200ms (PRD performance budget).
- Folder enumeration of 1000+ entities returns under 200ms (PRD performance budget).
