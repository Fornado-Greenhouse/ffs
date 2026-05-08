---
status: pending
title: JSON-RPC 2.0 dispatcher in ffs-daemon over UDS / Windows named pipe
type: backend
complexity: high
dependencies:
  - task_04
  - task_05
  - task_06
---

# Task 07: JSON-RPC 2.0 dispatcher in ffs-daemon over UDS / Windows named pipe

## Overview
Stand up the long-running `ffs-daemon` process that owns the substrate state and exposes it to local clients (CLI, Obsidian plugin, MCP server, skills) over a Unix domain socket on Linux/macOS or a Windows named pipe. The dispatcher routes JSON-RPC 2.0 method calls, publishes server-to-client notifications for filesystem and substrate change events, and is the single entry point all reads and writes pass through.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST listen on a UDS at `~/.ffs/run/ffs.sock` on Linux/macOS with `0600` permissions, or a named pipe at `\\.\pipe\ffs-<user_sid>` on Windows.
- MUST speak JSON-RPC 2.0 with newline-delimited frames.
- MUST implement the method set listed in TechSpec § Implementation Design § API Endpoints (`atom.get`, `atom.list`, `projection.render`, `path.list`, `ingest.submit`, `fastpath.submit`, `capability.evaluate`, `federation.peer.add`, `federation.peer.list`, `federation.pull`, `predicate.inspect`, `health.summary`).
- MUST publish notifications: `event.atom.committed`, `event.projection.invalidated`, `event.fastpath.applied`, `event.federation.peer.changed`.
- MUST run dispatch concurrently per connection with shared store access; serialize writes through a single writer.
- MUST evaluate capabilities (task 05) for every method that reads or writes substrate state.
- MUST refuse connections from processes not running as the substrate's user (filesystem permissions on UDS, pipe ACL on Windows).
- MUST gracefully shut down on SIGTERM and clean up the socket / pipe.
- SHOULD support backpressure: pause notifications when a client's queue exceeds 1000 messages and offer `event.resync`.
</requirements>

## Subtasks
- [ ] 7.1 Define the `Request` enum with serde method-tag dispatch (per TechSpec Core Interfaces).
- [ ] 7.2 Implement the `tokio` UDS listener and Windows named-pipe listener behind a unified abstraction.
- [ ] 7.3 Implement the dispatch table mapping methods to handler functions.
- [ ] 7.4 Wire each method to the corresponding ffs-core module (atom, predicate, store, capability, projection).
- [ ] 7.5 Implement notification publishing with per-connection event queues.
- [ ] 7.6 Implement graceful shutdown and socket cleanup.
- [ ] 7.7 Add structured logging via `tracing` for every method call and capability decision.

## Implementation Details
Create `crates/ffs-daemon/src/main.rs` (binary entrypoint) and `crates/ffs-daemon/src/dispatch/` (dispatch logic). The daemon owns a single `Arc<dyn AtomStore>` and serializes writes with a `Mutex` or single-writer task. Reads can be concurrent.

See ADR-019 for transport decisions and TechSpec § System Architecture for daemon responsibilities.

### Relevant Files
- `crates/ffs-daemon/src/main.rs` (new) — daemon entrypoint.
- `crates/ffs-daemon/src/dispatch/mod.rs` (new) — method routing.
- `crates/ffs-daemon/src/dispatch/transport.rs` (new) — UDS / named pipe abstraction.
- `crates/ffs-daemon/src/dispatch/notification.rs` (new) — event publisher.
- `crates/ffs-core/src/api.rs` (new in this task) — shared `Request` and `Response` types.

### Dependent Files
- `crates/ffs-cli` (task_08) — JSON-RPC client.
- `crates/ffs-mcp` (task_16) — JSON-RPC client.
- `crates/ffs-fastpath` (task_09) — calls ingest/fastpath.submit; receives notifications.
- `crates/ffs-skills-host` (task_10) — proxies skill access through dispatcher.
- Obsidian plugin (task_17) — TS JSON-RPC client.

### Related ADRs
- [ADR-019: Local IPC via Unix domain socket / Windows named pipe with JSON-RPC 2.0](adrs/adr-019.md) — Transport and method shape.
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Daemon scope.

## Deliverables
- A running `ffs-daemon` binary that listens on UDS/named pipe and dispatches the documented method set.
- Notification publishing with per-connection queues and backpressure.
- Capability checks fire on every read/write method.
- Graceful shutdown removes the socket file / closes the named pipe.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests for end-to-end JSON-RPC over UDS and named pipe **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Method dispatch: each documented method routes to its handler with correct argument deserialization.
  - [ ] Capability denial on `atom.get` returns a structured error code.
  - [ ] Notification publisher delivers `event.atom.committed` after a successful `ingest.submit`.
  - [ ] Backpressure: when client queue exceeds 1000 events, server stops publishing and waits.
- Integration tests:
  - [ ] On Linux, two clients connect over UDS and both receive notifications for the same write.
  - [ ] On macOS, the daemon refuses to start if `~/.ffs/run/` permissions are world-writable.
  - [ ] On Windows, named pipe ACL grants only the substrate's user account access.
  - [ ] SIGTERM triggers graceful shutdown; the socket file is removed.
  - [ ] `health.summary` returns proposal/question/drift counts consistent with the underlying store.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Daemon successfully serves a 1000-request burst from a single client without dropping connections.
- p95 latency for `atom.get` and `path.list` under 50ms against a 10000-atom store.
- The CLI (task 08) successfully resolves an `ffs://` URL by calling the daemon.
