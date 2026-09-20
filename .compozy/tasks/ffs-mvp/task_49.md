---
status: completed
title: Wire the fast-path watcher and inbox decisions into the production daemon (dependency inversion)
type: backend
complexity: medium
dependencies:
  - task_09
  - task_38
  - task_39
---

# Task 49: Wire the fast-path watcher and inbox decisions into the production daemon (dependency inversion)

## Overview
Found during task_39 (2026-09-20): the fast-path watcher from task_09 has never run in the production daemon binary. `crates/ffs-daemon/Cargo.toml` records why: `ffs-fastpath` depends on `ffs-daemon` (for `EventPublisher` and, since task_39, the `Dispatcher` behind `DispatcherSink`), so the daemon binary cannot start the watcher without a crate dependency cycle. `main.rs` never references the watcher, and `dispatch.rs` still carries `"fastpath.submit" => stub_not_implemented("task_09")`.

Two promises are broken in production as a result. ARCHITECTURE.md promises sub-200 ms fast-path absorption of editor edits to projection files; today those edits are not absorbed at all. ADR-032's inbox file renders in production (the `InboxMaterializer` is spawned in `main.rs`), but ticks in it do not apply, because the strict checkbox parser and `DispatcherSink` live in `ffs-fastpath`. Task_39's e2e proves the tick-to-atom path in-process through `crates/ffs-fastpath/tests/inbox_task39.rs`; this task carries that path, and the original task_09 path, into the running daemon.

This is a crate-graph change to internal modules only. No stability-listed surface (atom envelope, `ffs://` scheme, JSON-RPC method set, MCP tool signatures, predicate-spec format) changes. An ADR is therefore optional; a short ADR-036 recording the dependency inversion is recommended so the crate list in ARCHITECTURE.md has a decision behind it.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST decide and record the dependency shape before wiring anything. Recommended: invert the dependency. Move the small shared types the fast path needs down a level so that `ffs-daemon` can depend on `ffs-fastpath`: `EventPublisher` and `Event` from `crates/ffs-daemon/src/notify.rs`, the `DecisionSink` trait (today in `crates/ffs-fastpath/src/inbox.rs`), and any dispatcher-facing seam the sink calls, into `ffs-core` or a new tiny crate `ffs-events`. `SuppressionRegistry` already lives in `ffs-core` and needs no move. `DispatcherSink` then becomes a daemon-side implementation of the trait, and `main.rs` starts `FastPathWatcher` with it. The alternative, a separate `ffs-fastpath` sidecar binary that talks to the daemon over the UDS, MUST be listed with its costs (a second launchd job, a second process to install and sign under ADR-025, decisions applied over the socket instead of in-process, a second suppression registry to keep coherent) and rejected unless the inversion proves too invasive. Record the decision in this task's Implementation Details and, if written, in ADR-036.
- MUST start the watcher in `main.rs` after the working-set and inbox materializers, watching every projection family from `SpecRegistry::families()` plus `inbox/`, with `decision_sink` set to a `DispatcherSink` over the same `Dispatcher` the transport serves, and with the one `SuppressionRegistry` shared by the working-set materializer, the inbox materializer, and the watcher, so daemon-induced writes never re-enter as edits (ARCHITECTURE.md concurrency rule 3).
- MUST implement `fastpath.submit` in `dispatch.rs` (a direct submission of a classified projection edit, for the plugin's edit routing and for tests) or delete the stub with a note in the dispatcher explaining that the watcher is the only fast-path entry point. Either way the `"task_09"` placeholder MUST be gone.
- MUST add an e2e test in the daemon crate that spawns the real `ffs-daemon` binary on a scratch data dir (the shape `crates/ffs-daemon/tests/binary_end_to_end.rs` already uses), accepts a contact so it materializes, appends a bullet under an additive section of the materialized file, and observes the supersession atom committed within the fast-path budget (200 ms after the debounce window; assert on the atom, not on wall-clock alone).
- MUST add a second binary e2e that submits a proposal, waits for the rendered `inbox/<date>.md`, ticks `accept` on that block in the file, and observes the atom committed and the file re-rendered with the block under `## Decided`.
- MUST keep every existing `ffs-fastpath` and `ffs-daemon` test green, including `crates/ffs-fastpath/tests/inbox_task39.rs` and `fastpath_integration.rs`, adapting only construction sites if types move.
- MUST update ARCHITECTURE.md: the crate list and dependency description if the crate graph changes (a new `ffs-events` crate or new `ffs-core` modules), and the fast-path prose so it describes what actually runs in the binary.
- MUST NOT change the atom envelope, the capability evaluator, the JSON-RPC method names already served, or the skills-host wire protocol.
- SHOULD write ADR-036 (short) recording the inversion and the rejected sidecar.
</requirements>

## Subtasks
- [x] 49.1 Decide the dependency shape: attempt the inversion (move `EventPublisher`/`Event` and `DecisionSink` down; make `ffs-daemon` depend on `ffs-fastpath`); if it proves too invasive, fall back to the sidecar and record why. Write ADR-036 if the inversion lands.
- [x] 49.2 Move the shared types; adapt `ffs-fastpath` and `ffs-daemon` construction sites; workspace compiles with no cycle; all existing tests green.
- [x] 49.3 `main.rs`: start `FastPathWatcher` after the materializers with the registry's families plus `inbox/`, the shared `SuppressionRegistry`, and a `DispatcherSink`; log the watched roots at startup.
- [x] 49.4 `fastpath.submit`: implement or delete the stub with a note; dispatcher test either way.
- [x] 49.5 Binary e2e tests: editor edit to a materialized additive section becomes a supersession atom within budget; inbox `accept` tick becomes an atom and the file re-renders under `## Decided`.
- [x] 49.6 ARCHITECTURE.md crate list and fast-path prose; live validation on the project lead's Mac (edit a contact in Obsidian, see the atom; tick accept in today's inbox, see the section move to Decided); record the outcome in this task's Result section.

## Implementation Details

**Decision (2026-09-20): the dependency was inverted; the sidecar is rejected.** Recorded in ADR-036. `Event` and `EventPublisher` moved to `ffs_core::events` (re-exported from `ffs_daemon::notify`); `ParseWarning` and `INBOX_DIR` moved into `ffs_fastpath::inbox`; `DecisionSink` stays in `ffs-fastpath`; `DispatcherSink` is now `ffs_daemon::inbox_sink::DispatcherSink`. `ffs-fastpath` depends only on `ffs-core`; `ffs-daemon` depends on `ffs-fastpath`. `crates/ffs-fastpath/tests/inbox_task39.rs` moved to `crates/ffs-daemon/tests/inbox_ticks_task39.rs` because it constructs the daemon-side sink. `main.rs` starts `FastPathWatcher` after both materializers with the registry's families plus `inbox/` (decided per event by `is_watched_root`), the shared `SuppressionRegistry`, and a `DispatcherSink`; it logs `fast-path watcher started` with the roots. `fastpath.submit` is implemented over the new `process_edit` entry point and returns `EditOutcome` (`{"outcome": "ignored" | "applied" | "routed_to_ingest" | "inbox_decisions", ...}`); the `task_09` stub is gone.

Current structure: `crates/ffs-fastpath/src/watcher.rs` owns `FastPathWatcher` and `FastPathContext` (registry, store, path index, suppression registry, event publisher, `decision_sink: Option<Arc<dyn DecisionSink>>`); `crates/ffs-fastpath/src/inbox.rs` owns the strict checkbox parser, `InboxDecision`, `DecisionSink`, and `DispatcherSink` (which holds an `Arc<Dispatcher>` and calls `ingest.accept`, `ingest.reject`, `entity.assert_different`, `entity.merge`, `entity.unmerge`, `ingest.retract`, then publishes `QuarantineChanged`). `crates/ffs-daemon/src/main.rs` builds the store, registry, renderer, working-set materializer, inbox materializer, skills host, and transport, and never mentions the fast path. `crates/ffs-daemon/src/notify.rs` owns `Event` and `EventPublisher`.

Why the cycle exists: `ffs-fastpath` needs to publish `Event::ProjectionInvalidated` and, since task_39, to call the dispatcher. Both are daemon types. The clean cut is that the fast path should depend only on the seams it uses, not on the daemon crate. Moving `Event`/`EventPublisher` into `ffs-core` (they are plain broadcast types with no daemon-specific dependencies) and defining `DecisionSink` beside them lets `ffs-daemon` depend on `ffs-fastpath` for the watcher and implement the sink itself.

Sidecar alternative, for the record: a second binary `ffs-fastpath` that watches the vault and applies decisions over the UDS as a client. It avoids the crate move but doubles the process count, the launchd wiring, the ADR-025 signing surface, and the suppression bookkeeping (two registries that must agree on expected content hashes). Rejected unless 49.1 shows the inversion cannot be done in a bounded change.

### Relevant Files
- `crates/ffs-daemon/src/main.rs`, `dispatch.rs`, `notify.rs`, `Cargo.toml`
- `crates/ffs-fastpath/src/watcher.rs`, `inbox.rs`, `lib.rs`, `Cargo.toml`
- `crates/ffs-core/src/` (new `events.rs` or a new `crates/ffs-events` crate)
- `crates/ffs-daemon/tests/binary_end_to_end.rs` (shape to copy for the new e2e tests)
- `ARCHITECTURE.md` (crate list, fast-path prose)
- `.compozy/tasks/ffs-mvp/adrs/adr-036.md` (new, optional but recommended)

### Dependent Files
- `crates/ffs-fastpath/tests/inbox_task39.rs`, `fastpath_integration.rs` (construction sites)
- `crates/ffs-daemon/tests/inbox_task39.rs`, `autofile_task39.rs` (unchanged expectations, must stay green)
- `installer/launchd/com.ffs.daemon.plist` only if the sidecar alternative is chosen

### Related ADRs
- ADR-014 (minimum-viable fast path; the watcher this task finally starts)
- ADR-015 (minimal Rust daemon; the binary the watcher joins)
- ADR-025 (why a second signed process is expensive on macOS; the cost of the sidecar alternative)
- ADR-032 (review as markdown; the inbox ticks this task makes real in production)
- ADR-029 (auto-file; the retract and merge decisions the sink applies)
- ARCHITECTURE.md concurrency rule 3 (daemon-induced writes must not re-enter as edits)

## Deliverables
- Dependency shape decided and recorded; workspace compiles with `ffs-daemon` starting the watcher **(REQUIRED)**.
- `FastPathWatcher` running in the production binary with a `DispatcherSink` and the shared suppression registry **(REQUIRED)**.
- `fastpath.submit` implemented or the stub removed with a note.
- Two binary e2e tests (editor edit absorbed; inbox tick applied) **(REQUIRED)**.
- ARCHITECTURE.md updated; ADR-036 if the inversion lands.

## Tests
- Unit tests:
  - [x] Existing `ffs-fastpath` and `ffs-daemon` suites green after the type move, with assertions unmodified.
  - [x] `main.rs` startup wiring covered by a construction test that builds the watcher context from a registry with the three starter families plus `inbox/` and asserts the watched roots.
  - [x] `fastpath.submit` dispatcher test (or a test that the method name is absent and documented).
- Integration tests:
  - [x] Binary e2e: a bullet appended to a materialized contact's additive section becomes a supersession atom within the fast-path budget; the daemon's own re-render of that file does not produce a second atom (suppression registry).
  - [x] Binary e2e: ticking `accept` in the rendered inbox file commits the atom and re-renders the block under `## Decided`.
- Test coverage target: >=80% on new code
- All tests must pass

## Success Criteria
- Editing a contact file in Obsidian on the project lead's Mac produces a supersession atom without any click, and the file does not bounce.
- Ticking `accept` in today's inbox file in Obsidian commits the atom and the section moves to Decided on the next render.
- `cargo tree` shows no cycle; `ffs-daemon` depends on `ffs-fastpath`, not the reverse.
- All tests passing; `cargo fmt`, `cargo clippy -D warnings` clean.

## Result (2026-09-20)

- Binary e2e `crates/ffs-daemon/tests/fastpath_binary_task49.rs`: `fastpath_absorbs_additive_edit_in_binary` accepts a scribe-extracted contact, appends a Notes bullet to the materialized file, and observes the supersession atom on the entity's chain 89 ms after the save (50 ms debounce included); the daemon's re-render keeps the bullet and authors no third atom. `inbox_tick_applies_in_binary` ticks `accept` in `inbox/<date>.md` and observes the submission leave the pending list, the block under `## Decided`, and the contact materialized.
- Dispatcher tests `crates/ffs-daemon/tests/fastpath_submit_task49.rs` (applied, routed_to_ingest, ignored, `..` rejected) and watched-root test `crates/ffs-fastpath/tests/watched_roots_task49.rs` (six starter families plus `inbox/` watched; ingest/run/log/skills/config/dotfiles not).
- ARCHITECTURE.md gained the one-way crate graph and the in-binary fast-path paragraph; ADR-036 written and indexed.
- 49.6 live validation: the binary e2e is the scratch-daemon validation (temp data dir, throwaway keys). The Obsidian hand-check on the production daemon needs the new binary installed (`installer/`) and the daemon restarted; it is the owner's step and is not claimed here.

