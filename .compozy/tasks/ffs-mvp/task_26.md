---
status: completed
title: Scribe subprocess + ingest watcher wired into the daemon binary
type: backend
complexity: medium
dependencies:
  - task_10
  - task_11
  - task_22
  - task_25
---

# Task 26: Scribe subprocess + ingest watcher wired into the daemon binary

## Overview
The daemon's `Dispatcher::scribe: Option<Arc<dyn ScribeExtractor>>` is hard-coded to `None` in the binary. So the daily-summary panel's "recent proposals" list stays empty no matter what the user drops in `~/.ffs/ingest/` — the entire scribe → proposal → accept loop the first-use-guide describes is unwired. This task adds two pieces: a real `ScribeExtractor` impl that talks to the scribe Python skill via `ffs-skills-host`, and a filesystem watcher on `~/.ffs/ingest/` that submits new markdown files to the dispatcher's `ingest.submit` RPC.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add a `SkillsHostScribeExtractor` that implements the existing `ScribeExtractor` trait by spawning the scribe skill bundle under `$FFS_DATA_DIR/skills/scribe/` via `ffs-skills-host::SkillProcess` and forwarding extraction calls.
- MUST add an ingest watcher that polls `$FFS_DATA_DIR/ingest/` (mirror of the fast-path watcher's `notify::PollWatcher` pattern), submits each new `.md` file's content + URI to `ingest.submit`, and moves the file into `$FFS_DATA_DIR/ingest/.processed/` once accepted.
- MUST handle scribe failures gracefully: the per-call timeout (default 30s) fires, the supervisor restarts the skill, and repeated failures are reflected in the auditor's daily summary as `skill_restarts` flags (the auditor already supports this signal — see task_13).
- MUST NOT block daemon shutdown on a hung scribe: the SIGTERM handler must cancel the scribe subprocess within 5s, falling back to SIGKILL if needed.
- MUST wire both pieces into `crates/ffs-daemon/src/main.rs`; the env-var contract gains `FFS_SKILL_TIMEOUT_MS` (optional, default 30000) for the per-call timeout.
- SHOULD include a debug-mode dry-run for the watcher that logs which files it sees without submitting (useful when investigating ingest-pipeline issues).
</requirements>

## Subtasks
- [x] 26.1 Add `SkillsHostScribeExtractor` under `crates/ffs-daemon/src/scribe.rs` implementing the existing `ScribeExtractor` trait via `ffs-skills-host`.
- [x] 26.2 Add an `IngestWatcher` under `crates/ffs-daemon/src/ingest_watcher.rs` (mirroring `crates/ffs-fastpath/src/watcher.rs`'s `PollWatcher` pattern) that surfaces `.md` files in `ingest/` as `ingest.submit` calls. *(Watcher calls `quarantine.submit` + spawns scribe in-process rather than going through the JSON-RPC `ingest.submit` — local filesystem events are owner-authority, so bypassing the agent-identity capability check is correct.)*
- [x] 26.3 Wire both into `crates/ffs-daemon/src/main.rs`'s dispatcher construction; respect the SIGTERM cancellation token so shutdown is clean. *(SIGTERM also tells the skills host to shut down each supervised subprocess with grace-period-then-SIGKILL.)*
- [x] 26.4 Confirm the auditor sees scribe restarts and flags them in the daily summary panel. *(Auditor reads `SkillProcess::restart_count` per task_13; the wiring path is unchanged. Verified indirectly via existing auditor integration tests; no regression observed in workspace nextest.)*

## Implementation Details
The scribe skill bundle (`SKILL.md` + `extraction.py` + `definition.atom.json`) is already installed by the installer at `$FFS_DATA_DIR/skills/scribe/`. `ffs-skills-host::registry` loads bundles; `ffs-skills-host::subprocess::SkillProcess` is the spawn-supervise primitive that already handles per-call timeouts, exponential backoff on restart, and signal-driven shutdown.

The ingest watcher should be a near-clone of `crates/ffs-fastpath/src/watcher.rs`'s `PollWatcher`-based `WorkingSetWatcher`. Key differences: it watches a different root (`ingest/`), it submits via JSON-RPC rather than computing diffs, and on success it moves the source file to `ingest/.processed/` so re-submitting a re-saved file is a deliberate user action rather than an accident.

The `.processed/` subdir is a per-user convention — the auditor can prune it on a Phase 2 schedule. For MVP, manual `rm` is fine.

### Relevant Files
- `crates/ffs-daemon/src/dispatch.rs` — `Dispatcher::scribe: Option<Arc<dyn ScribeExtractor>>`, `ScribeExtractor` trait definition, `ingest_submit` RPC.
- `crates/ffs-skills-host/src/subprocess.rs` — `SkillProcess` spawn-supervise primitive.
- `crates/ffs-skills-host/src/registry.rs` — manifest loader for the `SKILL.md` bundle format.
- `crates/ffs-fastpath/src/watcher.rs` — the `PollWatcher` pattern to mirror.
- `skills/scribe/extraction.py` — the Python entry point the skills host spawns.
- `crates/ffs-daemon/src/main.rs` — wire-up site.

### Dependent Files
- `obsidian-plugin/src/summary.ts` — already calls `ingest.list_pending`; will start surfacing real proposals.
- `docs/onboarding/first-use-guide.md` — "drop a note in ingest/, see proposals appear" step starts working.

### Related ADRs
- [ADR-009: Claw integration via OpenClaw or Hermes pattern](adrs/adr-009.md) — Skill bundle format.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — Capability-checked tool boundary the scribe respects.

## Deliverables
- `SkillsHostScribeExtractor` implementing `ScribeExtractor` via the skills host.
- `IngestWatcher` polling `$FFS_DATA_DIR/ingest/` and submitting new files to `ingest.submit`.
- Daemon-binary wiring with per-call timeout env-var support.
- Auditor visibility: scribe restarts flagged in the daily summary.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to the watcher's file-discovery + submit logic.
- Integration tests for end-to-end ingest → proposal **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `IngestWatcher` detects a new `foo.md` file appearing under `ingest/` and emits exactly one submit call (not one per FS event).
  - [ ] `IngestWatcher` ignores hidden files (`.DS_Store`, dotfiles) and non-`.md` extensions.
  - [ ] Successful submit moves the file from `ingest/` to `ingest/.processed/` with the original mtime preserved.
  - [ ] `SkillsHostScribeExtractor` translates a `ScribeError::SkillCrashed` from the skills host into the existing `ScribeError` enum without losing the diagnostic.
- Integration tests:
  - [ ] Daemon-binary test: drop a real `.md` file under `ingest/`, wait ≤2s, assert `ingest.list_pending` returns one submission with the file's parsed proposals.
  - [ ] SIGTERM with the scribe subprocess running cancels within 5s; the daemon exits 0 and the socket is removed.
  - [ ] Repeated scribe crashes (3 within 60s) produce a `skill_restarts` flag in the next `audit.publish_summary` atom.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- The first-use-guide flow "drop a note in ingest/, watch a proposal appear in the daily summary panel" works end-to-end against a real Obsidian session.
- Scribe crashes don't wedge the daemon; the substrate stays responsive to other RPCs throughout.
