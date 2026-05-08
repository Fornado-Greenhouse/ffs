---
status: pending
title: ffs CLI — argv parser, `ffs://` URL resolver, static binaries for Linux/macOS/Windows
type: backend
complexity: medium
dependencies:
  - task_07
---

# Task 08: ffs CLI — argv parser, `ffs://` URL resolver, static binaries for Linux/macOS/Windows

## Overview
Build the `ffs` command-line tool that resolves `ffs://` URLs by calling the local daemon over UDS or named pipe. The CLI is one of the two MVP user-facing surfaces (the other is the Obsidian plugin) and ships as a single static binary per platform per the PRD's distribution requirement.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST implement the documented subcommands: `ffs cat`, `ffs ls`, `ffs get`, plus `ffs federation peer add` / `list`, `ffs predicate inspect`, `ffs health`.
- MUST resolve `ffs://<graph>/<address>[?<query>]` URLs across the three addressing modes (path, atom, entity) and bitemporal query parameters (`as_of`, `valid_at`).
- MUST emit plain-text output by default and structured JSON when `--json` is supplied.
- MUST use shell-friendly conventions: pipes, exit codes (0 = success, 1 = error, 2 = capability denied, 3 = not found, 64 = usage), stderr for diagnostics.
- MUST connect to the local daemon's UDS or named pipe and translate user input into JSON-RPC requests.
- MUST distribute as a single static binary on each of Linux (musl), macOS (universal or per-arch), Windows (MSVC).
- SHOULD provide `--help` text describing every subcommand and option.
</requirements>

## Subtasks
- [ ] 8.1 Define the CLI argument structure (using `clap`).
- [ ] 8.2 Implement the `ffs://` URL parser supporting path, atom, entity addressing and bitemporal query parameters.
- [ ] 8.3 Implement the JSON-RPC client speaking to the daemon over UDS / named pipe.
- [ ] 8.4 Implement each subcommand by translating argv into JSON-RPC and rendering responses.
- [ ] 8.5 Implement `--json` output mode emitting canonical envelopes for atoms.
- [ ] 8.6 Wire exit codes and structured stderr diagnostics.
- [ ] 8.7 Verify static-binary build artifacts for the three platforms.

## Implementation Details
Create `crates/ffs-cli/src/main.rs` and submodules. The static binary requirement means using `x86_64-unknown-linux-musl` for Linux, ensuring all dependencies are statically linked (verify with `ldd`/`otool -L`/`dumpbin /dependents`).

See ADR-006 (root) for the `ffs://` URL contract and TechSpec § Implementation Design § API Endpoints for the daemon methods the CLI invokes.

### Relevant Files
- `crates/ffs-cli/src/main.rs` (new) — entrypoint.
- `crates/ffs-cli/src/url.rs` (new) — `ffs://` parser.
- `crates/ffs-cli/src/client.rs` (new) — JSON-RPC client over UDS / named pipe.
- `crates/ffs-cli/src/commands/` (new) — per-subcommand handlers.

### Dependent Files
- Cross-platform installer (task_22) — distributes the binary.

### Related ADRs
- [ADR-006: `ffs://` URL scheme as public stable contract](adrs/adr-006.md) — URL syntax.
- [ADR-019: Local IPC via UDS / named pipe with JSON-RPC 2.0](adrs/adr-019.md) — Transport.
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Static-binary requirement.

## Deliverables
- `ffs` static binary for Linux musl, macOS (x86_64 + aarch64), Windows MSVC.
- Subcommand surface covering cat, ls, get, federation operations, predicate inspection, health summary.
- `--json` mode emitting canonical envelopes.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests exercising URL resolution against a live daemon **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] URL parser: `ffs://my-graph/atom/<hash>` parses to atom-mode addressing.
  - [ ] URL parser: `ffs://my-graph/contacts/by-name/S/?as_of=2026-04-15` parses with bitemporal query.
  - [ ] URL parser: malformed URLs return descriptive errors.
  - [ ] `--json` mode produces canonical envelope bytes verifiable by an external JCS implementation.
  - [ ] Exit code 2 on capability denial; exit code 3 on not-found; exit code 64 on usage error.
- Integration tests:
  - [ ] `ffs ls ffs://local/contacts/recent/` returns a list whose first entry is the most-recently-touched contact.
  - [ ] `ffs cat ffs://local/contacts/by-name/S/Sarah_Chen.md` returns the rendered projection markdown.
  - [ ] `ffs cat ffs://local/contacts/recent/?as_of=2026-04-15` returns historical state.
  - [ ] On Linux, `ldd target/release/ffs` reports no dynamic library dependencies.
  - [ ] CLI completes a `cat` against a 10000-atom graph in under 1s (PRD performance budget).
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- The static binary on each platform is verified to have no dynamic library dependencies (other than libc on macOS/Windows where statically linking is restricted).
- All documented subcommands execute end-to-end against a live daemon with the canonical fixture data.
