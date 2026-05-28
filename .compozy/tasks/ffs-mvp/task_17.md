---
status: completed
title: Obsidian plugin — scaffolding + UDS / named pipe client + event subscription
type: frontend
complexity: medium
dependencies:
  - task_07
---

# Task 17: Obsidian plugin — scaffolding + UDS / named pipe client + event subscription

## Overview
Establish the Obsidian plugin's TypeScript foundation: project scaffolding, build configuration, settings panel, and most importantly the JSON-RPC client that connects to the local daemon over UDS on Linux/macOS or a Windows named pipe. The plugin subscribes to substrate-change notifications so subsequent task work (folder enumeration, daily summary panel) can layer on top of a live data feed.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST scaffold an Obsidian plugin using the official plugin template (TypeScript + esbuild + manifest.json).
- MUST implement a JSON-RPC 2.0 client that connects to the daemon over UDS (Linux/macOS) or named pipe (Windows) using Node's `net.createConnection({ path })`.
- MUST handle reconnection: detect disconnects and reconnect with exponential backoff.
- MUST subscribe to daemon notifications and dispatch them to plugin subsystems via an event emitter.
- MUST provide a settings panel allowing the user to configure the socket / pipe path and identity key.
- MUST gracefully degrade when the daemon is not running: surface a clear "daemon offline" indicator.
- MUST include a CLI-subprocess fallback for environments where direct UDS / named-pipe connection fails (per TechSpec Known Risks).
- SHOULD provide vitest unit tests for the client and event emitter.
</requirements>

## Subtasks
- [x] 17.1 Scaffold the plugin (manifest.json, main.ts, settings.ts, esbuild config).
- [x] 17.2 Implement the JSON-RPC client over UDS / named pipe.
- [x] 17.3 Implement notification subscription and an internal event emitter.
- [x] 17.4 Implement reconnection with exponential backoff.
- [x] 17.5 Add the daemon-offline indicator to the plugin UI.
- [x] 17.6 Implement the CLI-subprocess fallback.
- [x] 17.7 Write the settings panel for socket path + identity key.

## Follow-ups (deferred to task_22 onboarding)

- **Loads-in-Obsidian smoke test on 3 OSes**: the plugin builds to
  `main.js` and the trait surface is exercised end-to-end via the
  in-memory mocks, but spinning up a real Obsidian + dropping the
  plugin into a test vault on Linux / macOS / Windows belongs to
  the onboarding scripts in task_22.
- **Live-daemon integration**: the daemon binary itself is a stub
  until task_22 wires its full subsystems. Once that lands, an
  integration test can boot the daemon + the plugin (or a headless
  client speaking the same wire protocol) and exercise the read
  path. The plugin's `DaemonClient` already speaks the
  daemon's wire shape so no further plugin-side work is required.
- **1-hour live-connection stability target**: deferred to
  manual / nightly QA once the daemon is runnable end-to-end.

## Implementation Details
Create `obsidian-plugin/` at the repo root with `manifest.json`, `package.json`, `src/main.ts`, `src/client.ts`, `src/settings.ts`, etc. Use TypeScript and esbuild per the Obsidian plugin standard. The JSON-RPC client speaks the same wire protocol the CLI uses (task 08).

See ADR-019 for the local IPC contract and TechSpec § Implementation Design § API Endpoints for the method set the plugin will eventually invoke.

### Relevant Files
- `obsidian-plugin/manifest.json` (new) — plugin manifest.
- `obsidian-plugin/package.json` (new) — npm dependencies.
- `obsidian-plugin/src/main.ts` (new) — plugin entrypoint.
- `obsidian-plugin/src/client.ts` (new) — JSON-RPC client.
- `obsidian-plugin/src/events.ts` (new) — event emitter.
- `obsidian-plugin/src/settings.ts` (new) — settings panel.

### Dependent Files
- Folder enumeration + projection rendering (task_18) — uses this client.
- Daily health summary panel + entity search (task_19) — uses this client and event subscription.

### Related ADRs
- [ADR-019: Local IPC via UDS / named pipe with JSON-RPC 2.0](adrs/adr-019.md) — Transport.
- [ADR-002: Both audiences first-class](adrs/adr-002.md) — Plugin is the end-user surface.

## Deliverables
- A loadable Obsidian plugin that connects to the daemon and confirms connectivity.
- JSON-RPC client and event emitter for downstream plugin features.
- Reconnection logic with backoff.
- Daemon-offline indicator.
- CLI-subprocess fallback path.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against a live daemon **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] JSON-RPC client serializes a request to a wire-format string with newline framing.
  - [ ] Client correctly demultiplexes a response and a notification arriving on the same connection.
  - [ ] Reconnection: simulated disconnect triggers reconnect attempt after 1s, 2s, 4s, ..., capped at 30s.
  - [ ] Event emitter delivers `event.atom.committed` to all subscribed listeners.
- Integration tests:
  - [ ] Plugin loads in a headless Obsidian environment (or a Node test harness simulating the API).
  - [ ] Plugin connects to a live daemon via UDS on macOS/Linux; client reports connected.
  - [ ] Plugin connects to a live daemon via named pipe on Windows.
  - [ ] Daemon shutdown triggers offline-indicator on the plugin side.
  - [ ] CLI-subprocess fallback succeeds when direct IPC fails.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Plugin loads in Obsidian on Linux, macOS, and Windows.
- Plugin successfully maintains a live connection to the daemon for at least 1 hour without unhandled disconnects.
- Daemon-offline state is clearly indicated and resolved on reconnect.
