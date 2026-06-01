---
status: completed
title: Working-set materializer — render projection files to disk on atom commit
type: backend
complexity: medium
dependencies:
  - task_06
  - task_07
  - task_22
  - task_24
---

# Task 25: Working-set materializer — render projection files to disk on atom commit

## Overview
The projection renderer (task_06) produces markdown in memory; nothing currently writes those projections to disk. So even when atoms are committed, the path-library directories (`~/.ffs/contacts/by-name/S/Sara_Chen.md`, etc.) stay empty — the first-use-guide step "navigate to your new contact" finds nothing. This task adds a working-set materializer that subscribes to `event.atom.committed`, looks up the affected entity's projection path via the existing path-library mapping, renders, and writes the file under `$FFS_DATA_DIR/`.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add a working-set materializer that subscribes to the daemon's `EventPublisher::atom.committed` channel and writes the rendered projection for the affected entity to disk under `$FFS_DATA_DIR/<predicate-graph>/by-name/<letter>/<entity>.md` (using the existing `crates/ffs-core/src/projection/path.rs` resolver).
- MUST honor capability filtering: the materializer renders with the daemon's owner identity, so projections only contain atoms the owner can read; if there are zero readable atoms for an entity, no file is written.
- MUST coordinate with the fast-path watcher to avoid a write→event→re-write loop: writes from the materializer carry an in-process marker (e.g., a recent-paths set with TTL) the fast-path classifier consults before submitting a synthetic edit.
- MUST update `WorkingSetEntry` (or an equivalent record) so `librarian` and the Obsidian plugin can see which paths are materialized.
- MUST be idempotent: re-materializing an entity produces byte-identical output and doesn't churn the file's mtime when the render hash matches the existing render hash.
- SHOULD batch writes when the daemon publishes multiple commit events for the same entity within a short window (≤50 ms) to avoid thrashing.
</requirements>

## Subtasks
- [x] 25.1 Add a `WorkingSetMaterializer` struct that owns the projection renderer, an `AsyncWriteCoordinator` for FS writes, and the recent-paths anti-loop marker.
- [x] 25.2 Subscribe to `event.atom.committed`; on each event, resolve the entity's projection path via `projection::path::resolve_for_entity` (or add a new resolver if one doesn't exist yet) and render.
- [x] 25.3 Write the rendered markdown atomically (temp-file-and-rename) to avoid Obsidian seeing a half-written file.
- [x] 25.4 Update the fast-path classifier to skip events whose source path is in the materializer's recent-writes set. *(Honored via the existing `SuppressionRegistry` — moved from `ffs-fastpath` to `ffs-core` so the materializer and the fast-path watcher share one instance without a dependency cycle. Hash-keyed rather than TTL-keyed, which is stronger than the spec suggested.)*
- [x] 25.5 Wire the materializer into the daemon binary's `main.rs` alongside the dispatcher construction.

## Implementation Details
Add a new module `crates/ffs-core/src/working_set/materializer.rs` (or `crates/ffs-daemon/src/materializer.rs` if it ends up depending on the daemon's `EventPublisher`). The path-library mapping already exists in `crates/ffs-core/src/projection/path.rs` with the `<family>/by-name/<letter>/<entity>.md` pattern.

The anti-loop coordination is the subtle part: the fast-path watcher fires on every FS event, so without a guard the materializer's own writes would trigger fast-path edits which would write supersession atoms which would fire commit events which would re-materialize, etc. A short-TTL `HashSet<PathBuf>` (1s expiry) consulted at fast-path entry is sufficient; the test suite's existing fast-path integration tests are the regression bar.

### Relevant Files
- `crates/ffs-core/src/projection/render.rs` — `ProjectionRenderer::render` API.
- `crates/ffs-core/src/projection/path.rs` — path-library mapping.
- `crates/ffs-core/src/working_set.rs` — `WorkingSetEntry`, `WorkingSetStore` trait.
- `crates/ffs-daemon/src/notify.rs` — `EventPublisher` with `event.atom.committed` channel.
- `crates/ffs-fastpath/src/dispatch.rs` — needs a hook to consult the materializer's recent-writes set.
- `crates/ffs-daemon/src/main.rs` — wire the materializer into the daemon binary.

### Dependent Files
- `crates/ffs-fastpath/tests/fastpath_integration.rs` — verify the anti-loop coordination doesn't break existing fast-path tests.
- `obsidian-plugin/src/folder.ts` — folder enumeration now sees real files; production rehearsal should confirm.

### Related ADRs
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Real files on disk for any editor.
- [ADR-014: Minimum-viable fast-path](adrs/adr-014.md) — Coordination with the watcher.
- [ADR-021: Predicate spec format — TOML with embedded JSON Schema](adrs/adr-021.md) — Templates the materializer renders against.

## Deliverables
- `WorkingSetMaterializer` subscribing to `event.atom.committed` and writing projections to disk.
- Anti-loop coordination with `ffs-fastpath` (recent-writes guard).
- Idempotence guarantee: re-rendering doesn't change mtime when the render hash is unchanged.
- Daemon-binary wiring in `main.rs`.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to the materializer's resolve-and-write logic.
- Integration tests for end-to-end commit → file-on-disk **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `resolve_path_for_entity` returns `contacts/by-name/S/Sara_Chen.md` for a `contact.person` atom with `display_name: "Sara Chen"`.
  - [ ] Materializing an entity with zero readable atoms returns `Ok(None)` and writes nothing.
  - [ ] Re-materializing with the same atom set produces byte-identical output and skips the write when the render hash matches.
  - [ ] The recent-writes guard expires entries after the configured TTL.
- Integration tests:
  - [ ] Daemon-binary test: commit one `contact.person` atom; assert `~/.ffs/contacts/by-name/<letter>/<name>.md` appears within 500 ms with the expected frontmatter.
  - [ ] Fast-path anti-loop: commit an atom, wait for the materialized file to land, confirm the fast-path watcher does NOT submit a supersession event for the materializer's own write.
  - [ ] Capability-filtered render: commit an atom under classification the owner doesn't have a read capability for; assert no file is written.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- After `ffs author` (or `ingest.accept`) commits a `contact.person` atom, the projection file appears under `~/.ffs/contacts/by-name/<letter>/` and Obsidian's folder view picks it up within one watcher tick.
- The first-use-guide step "find your new contact under contacts/by-name/" works for the first time.
