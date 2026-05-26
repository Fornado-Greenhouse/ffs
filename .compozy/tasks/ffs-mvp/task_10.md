---
status: completed
title: ffs-skills-host — subprocess host + stdio bridging for Python skills
type: backend
complexity: medium
dependencies:
  - task_07
---

# Task 10: ffs-skills-host — subprocess host + stdio bridging for Python skills

## Overview
Run Python skills (scribe, librarian, auditor) as long-lived subprocesses of the daemon, route invocations to them over stdio, and broker their substrate access through the daemon's JSON-RPC layer. The host supervises crashes and timeouts, restarts skills with backoff, and isolates skill failures from the rest of the daemon.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST discover skills under `~/.ffs/skills/<name>/` (each conforms to `SKILL.md` shape) and register them at startup.
- MUST spawn each skill as a Python subprocess and communicate via line-delimited JSON over stdin/stdout.
- MUST proxy substrate access from skills (read atoms, submit ingest, query predicates) through the daemon's JSON-RPC layer with the skill's identity.
- MUST supervise child processes: on crash, restart with exponential backoff (1s → 60s).
- MUST enforce per-call timeouts; kill and restart hung skills.
- MUST emit `skill_crashed` and `skill_restarted` log events via `tracing`.
- MUST support graceful shutdown: send SIGTERM to all skill children on daemon shutdown, wait up to 5s, then SIGKILL.
- SHOULD provide a small Python helper library (`ffs_skill`) that handles the stdio protocol so skill authors write business logic, not transport.
</requirements>

## Subtasks
- [x] 10.1 Define the skill registry and `Skill` trait abstraction.
- [x] 10.2 Implement subprocess spawning and stdio framing.
- [x] 10.3 Implement the substrate-access proxy: skills call back to the daemon's JSON-RPC.
- [x] 10.4 Implement supervision: crash detection, exponential backoff, restart policy.
- [x] 10.5 Implement per-call timeouts and timeout-triggered restart.
- [x] 10.6 Implement graceful shutdown.
- [x] 10.7 Provide the Python `ffs_skill` helper library packaged with skill bundles.

## Implementation Details
Create `crates/ffs-skills-host/src/lib.rs` and submodules. The Python helper library lives at `skills/_lib/ffs_skill.py` and is imported by each skill. The host treats skills as opaque processes; only the stdio JSON protocol is observable.

See ADR-009 (root) for the `SKILL.md` contract and ADR-015 for the in-process host scope decision.

### Relevant Files
- `crates/ffs-skills-host/src/lib.rs` (new) — primary module.
- `crates/ffs-skills-host/src/registry.rs` (new) — skill discovery.
- `crates/ffs-skills-host/src/subprocess.rs` (new) — process spawning and supervision.
- `crates/ffs-skills-host/src/protocol.rs` (new) — stdio framing.
- `skills/_lib/ffs_skill.py` (new) — Python helper for skill authors.

### Dependent Files
- Scribe skill (task_11), librarian skill (task_12), auditor skill (task_13) — consume this host.
- `crates/ffs-daemon` (task_07) — embeds the host.

### Related ADRs
- [ADR-009: Claw integration via OpenClaw or Hermes pattern](adrs/adr-009.md) — `SKILL.md` shape conformance.
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — In-process host scope.

## Deliverables
- Subprocess host supervising Python skills.
- Skill discovery from `~/.ffs/skills/`.
- Substrate-access proxy bridging skills to the daemon's JSON-RPC.
- Crash and timeout handling with exponential-backoff restart.
- Python `ffs_skill` helper library.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests with real Python subprocess **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Skill registry discovers a `SKILL.md`-shaped directory and registers the skill.
  - [ ] Stdio protocol: well-formed request/response round-trips correctly.
  - [ ] Crashed skill triggers exponential backoff (1s, 2s, 4s, ..., capped at 60s).
  - [ ] Hung skill (no response within timeout) is killed and restarted.
  - [ ] Substrate access from a skill is rejected when the skill's identity lacks the required capability.
- Integration tests:
  - [ ] Spawn a stub Python skill that echoes a JSON request; verify it round-trips through the host.
  - [ ] Spawn a stub Python skill that crashes on first invocation; verify it restarts and succeeds on second invocation.
  - [ ] Daemon shutdown sends SIGTERM to all skill children and waits gracefully.
  - [ ] Skill calls `ffs_query` via the helper and receives capability-filtered atoms.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A misbehaving skill (crashes every 10s) does not destabilize the daemon; auditor flags repeated crashes in the daily summary.
- The Python `ffs_skill` helper library exposes a clean `def on_request(req): ...` interface.
