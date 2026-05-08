---
status: pending
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
- [ ] 18.1 Implement the folder-enumeration interceptor for the three MVP projection path families.
- [ ] 18.2 Implement projection rendering on file-open events.
- [ ] 18.3 Implement the visual treatment distinguishing projections from notebook entries.
- [ ] 18.4 Implement edit routing: detect save events, classify, send to `fastpath.submit` or ingest.
- [ ] 18.5 Implement optimistic-UI updates on fast-path success with rollback on failure.
- [ ] 18.6 Handle pagination UI for large folder listings.
- [ ] 18.7 Subscribe to `event.projection.invalidated` for live updates.

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
