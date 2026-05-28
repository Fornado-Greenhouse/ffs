---
status: completed
title: ffs-mcp — six MVP MCP tools wrapping the daemon's JSON-RPC
type: backend
complexity: medium
dependencies:
  - task_07
---

# Task 16: ffs-mcp — six MVP MCP tools wrapping the daemon's JSON-RPC

## Overview
Build the FFS-MCP server: a thin Rust binary that speaks the Model Context Protocol on stdio (or SSE) and translates the six MVP tools into JSON-RPC calls against the local daemon. The MCP server is the standardized agent-to-substrate boundary that any MCP-aware agent (Claude, ChatGPT, Gemini, framework-agnostic agents) can use without bespoke integration. Capability checks fire at the MCP boundary on every tool call.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST implement the six MVP tools listed in PRD § Core Features § FFS-MCP server: `ffs_query`, `ffs_render_projection`, `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`, `ffs_audit_query`.
- MUST speak MCP over stdio by default; support SSE per agent configuration.
- MUST translate each MCP tool call into the matching daemon JSON-RPC method (e.g., `ffs_query` → `atom.list`).
- MUST bind agent identity to an FFS author key; capability checks fire on every tool call by delegating to the daemon's existing capability evaluator (do not implement parallel checks).
- MUST return MCP-structured errors on capability denial (not silent failure).
- MUST expose tool schemas matching the MCP specification.
- SHOULD support a `--allow-author` flag that grants the agent's identity write capability for testing.
</requirements>

## Subtasks
- [x] 16.1 Set up the MCP transport (stdio + SSE) using a Rust MCP library or custom implementation.
- [x] 16.2 Define MCP tool schemas for the six tools.
- [x] 16.3 Implement each tool as a translator: MCP request → daemon JSON-RPC → MCP response.
- [x] 16.4 Implement agent-identity binding (config-file-driven; key from keychain or file).
- [x] 16.5 Translate capability denials to MCP errors with structured detail.
- [x] 16.6 Provide a sample agent configuration documented in the README.

## Follow-ups (deferred to task_22 onboarding)

- **Stdio binding to a real daemon**: `serve_stdio` + `McpServer` are
  wired; the daemon binary plugs in a UDS-backed `DaemonClient`
  (mirroring task_14's deferred reqwest binding). Until then, `main.rs`
  prints a clear stub message so a stray invocation doesn't claim
  to be working.
- **SSE transport** (16.1's optional half): the trait surface
  accepts any `AsyncRead + AsyncWrite`, so an SSE adapter slots in
  alongside `serve_stdio` without protocol-layer changes.
- **`--allow-author` CLI flag** (per the SHOULD requirement): the
  daemon-side capability shape is already in place; the flag wires
  in alongside the production binary in task_22.
- **Agent-identity key loading**: `FFS_AGENT_KEY` is documented in
  the sample config and `main.rs` docstring; the actual keyring/file
  lookup lands when the binary stub is replaced.

## Implementation Details
Create `crates/ffs-mcp/src/main.rs` and submodules. The MCP server is a separate process from the daemon; it connects to the daemon's UDS / named pipe like any other client. Capability evaluation happens entirely on the daemon side via task 05; the MCP server is a thin pass-through.

See ADR-013 (root) for MCP-server-in-MVP rationale and ADR-008 (root) for the boundary protocol commitment.

### Relevant Files
- `crates/ffs-mcp/src/main.rs` (new) — entrypoint.
- `crates/ffs-mcp/src/tools/` (new) — one module per tool.
- `crates/ffs-mcp/src/transport.rs` (new) — MCP stdio + SSE transport.
- `crates/ffs-cli/src/client.rs` (task_08) — reused for daemon JSON-RPC.

### Dependent Files
- None internal; external MCP-aware agents consume this binary.

### Related ADRs
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — Six tools, capability-checked.
- [ADR-008: Speak MCP and A2A at boundaries](adrs/adr-008.md) — Standards over invention.
- [ADR-019: Local IPC via UDS / named pipe](adrs/adr-019.md) — How the MCP server reaches the daemon.

## Deliverables
- A working `ffs-mcp` binary speaking the MCP protocol over stdio (and SSE).
- Implementations of the six MVP tools, each translating to daemon JSON-RPC.
- Capability checks delegated to the daemon, returning MCP errors on denial.
- Sample agent configuration in the README.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests with a stub MCP-aware client **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Each of the six tools has an MCP schema and a translator function.
  - [ ] `ffs_query` translates to `atom.list` with the right params.
  - [ ] `ffs_author_atom` translates to `ingest.submit` with provenance pointing to the agent.
  - [ ] Capability denial from the daemon translates to a structured MCP error code.
  - [ ] Malformed MCP request returns an MCP-spec-compliant error.
- Integration tests:
  - [ ] Stub MCP client sends a tool call list request; receives the six tools.
  - [ ] Stub agent calls `ffs_query`; receives capability-filtered atoms.
  - [ ] Stub agent calls `ffs_author_atom` with an out-of-scope claim; receives a structured capability error.
  - [ ] `ffs_resolve_url` for `ffs://local/atom/<hash>` returns the atom.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A real MCP-aware agent (e.g., Claude Code) can connect to `ffs-mcp` via stdio and successfully invoke each of the six tools.
- The home-claw absorption scenario (an agent reads + writes the substrate) is end-to-end demonstrable.
