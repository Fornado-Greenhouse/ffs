---
status: completed
title: Ingest stability window — let users write a note over time before scribe consumes it
type: backend
complexity: low
dependencies:
  - task_26
---

# Task 31: Ingest stability window — let users write a note over time before scribe consumes it

## Overview
The ingest watcher (task_26) consumes a new `.md` file the moment it appears under `$FFS_DATA_DIR/ingest/`. That's correct for the "drop a finished note" flow but wrong for the "create a new note in Obsidian and write to it over the next few minutes" flow — Obsidian's "New note" action creates the file immediately, the watcher fires, the file gets moved to `.processed/` while the user is still editing, and the user's edits land in a stale buffer with no on-disk backing. This task adds a stability window: a file is only submitted when its content hasn't changed for a configurable delay (default ~60s), giving the user space to compose.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add a configurable stability delay (default 60s) before the ingest watcher submits a newly-discovered `.md` file. The file's content hash is sampled at discovery and again after the delay; if unchanged, submit; if changed, reset the timer.
- MUST honor a new `FFS_INGEST_STABILITY_MS` env var on the daemon binary; 0 disables the delay (current behavior, useful for tests).
- MUST handle the edge case where the user deletes the file before it stabilizes — the pending submission is cancelled silently.
- MUST handle the edge case where the user closes the file in their editor after editing — the watcher detects the content stabilized and submits within one stability window.
- MUST NOT regress the existing `ingest_pipeline_e2e` test (it relies on near-immediate processing). Update the test to either set `FFS_INGEST_STABILITY_MS=0` or wait for the stability window.
- SHOULD surface "pending stabilization" entries in `ingest.list_pending` (or a new RPC) so the Obsidian plugin's panel can show "you're editing N notes — they'll be processed when you finish" rather than going silent during the delay.
</requirements>

## Subtasks
- [x] 31.1 Add a content-hash + last-seen-time tracking map to the `IngestWatcher`; a file enters the map at discovery and is submitted only after the stability delay elapses with no content change. *(`PendingFile { hash, first_seen }` stored in a per-`event_loop` `HashMap<PathBuf, PendingFile>`; a periodic 1 s `tokio::time::interval` drives `check_stable`, which re-reads each candidate file and resets the timer if the on-disk hash has drifted since the last FS event.)*
- [x] 31.2 Honor `FFS_INGEST_STABILITY_MS` (default 60000, 0 disables). *(Wired in `main.rs`; documented in the daemon's env-var docblock.)*
- [x] 31.3 Handle delete-before-stable cleanly (drop the pending entry without erroring). *(Two paths: `EventKind::Remove` events drop the entry immediately; `check_stable` also drops entries whose file vanished between the last event and the next check tick.)*
- [x] 31.4 Update `tests/ingest_pipeline_e2e.rs` to set `FFS_INGEST_STABILITY_MS=0` so its assertions don't have to wait 60s. *(Both e2e tests in that file plus the `quarantine_submission_survives_daemon_restart` test in `tests/sqlite_persistence.rs` opted out.)*
- [x] 31.5 Add unit tests covering the new state machine (entered-but-not-yet-stable, stabilized-and-submitted, modified-before-stable-resets-timer, deleted-before-stable-cancels). *(7 new `#[test]` / `#[tokio::test]` covering insert / hash-changed reset / hash-same idempotent / within-window skip / past-window submit / changed-since-event reset / delete-clears.)*

## Follow-ups
- The SHOULD requirement to surface stabilizing-but-not-yet-submitted files in `ingest.list_pending` is intentionally deferred. It would require a new RPC field and Obsidian UI work — outside the test list and beyond the gating success criteria. Scope to a future plugin-UX task when the panel is ready to show it.

## Implementation Details
Add a `PendingFile { hash: Multihash, first_seen: Instant }` map to `IngestWatcher`'s event loop. On each `Create`/`Modify` event for an eligible path:
- If not in the map, insert and start the timer.
- If in the map and the content hash matches the recorded one *and* `first_seen.elapsed() >= stability_window`, submit and remove.
- If in the map and the hash differs, update the hash and reset `first_seen` to now.

A `tokio::time::interval` ticking at ~1s drives the periodic stability check on the map (rather than racing against FS events for the trigger). On each tick, the loop iterates the map, looks for stable entries, and submits them.

Delete handling: when the watcher receives an `EventKind::Remove` for a tracked path, drop the map entry silently.

### Relevant Files
- `crates/ffs-daemon/src/ingest_watcher.rs` — `IngestWatcher` event loop, `process_one` function.
- `crates/ffs-daemon/src/main.rs` — wire-up site for the new env var.

### Dependent Files
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — set `FFS_INGEST_STABILITY_MS=0` for determinism.
- `obsidian-plugin/src/summary.ts` — may want to surface stabilizing-but-not-yet-submitted state (Phase 2 if cross-task gap).
- `docs/onboarding/first-use-guide.md` — explain that new notes are auto-submitted after ~60s of inactivity, or that the user can "save and walk away" to commit.

### Related ADRs
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Same editor-agnostic principle applies to ingest: the user can use Obsidian's normal note-creation flow.

## Deliverables
- Updated `IngestWatcher` with stability-window logic.
- `FFS_INGEST_STABILITY_MS` env var honored in `main.rs`.
- Updated e2e test that opts out of the delay for determinism.
- Unit tests with 80%+ coverage **(REQUIRED)** for the new state machine.

## Tests
- Unit tests:
  - [ ] A new file appears, no further events, after stability window elapses → submitted exactly once.
  - [ ] A new file appears, modified within the window → timer resets; not submitted until stable again.
  - [ ] A new file appears, modified once after the window elapses but before the periodic check → submitted within one check tick.
  - [ ] A new file appears, deleted before stability → no submission, no panic.
  - [ ] `FFS_INGEST_STABILITY_MS=0` produces near-immediate submission (current task_26 behavior).
- Integration tests:
  - [ ] `ingest_pipeline_e2e` continues to pass with `FFS_INGEST_STABILITY_MS=0` set.
  - [ ] A new e2e test creates a `.md` file under `ingest/`, modifies it once after 100ms, waits 1.1s (stability_ms=1000), and confirms the final content (post-modification) is what landed in the quarantine.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A user can right-click in Obsidian's `ingest/` folder, choose "New note," type freely for a minute, and the file is only consumed once they walk away or save-and-pause.
- The first-use-guide reflects the new write-then-leave workflow as the default.
