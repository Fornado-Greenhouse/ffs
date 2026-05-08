---
status: pending
title: Cargo workspace + cross-platform CI scaffolding
type: chore
complexity: medium
dependencies: []
---

# Task 01: Cargo workspace + cross-platform CI scaffolding

## Overview
Establish the Rust workspace that will hold every Rust crate the FFS MVP needs (core, daemon, CLI, MCP server, federation, fast-path, skills host). Wire up GitHub Actions to build and test on Linux, macOS, and Windows so the static-binary distribution requirement is verified continuously from day one.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST establish a Cargo workspace at the repository root with member crates `ffs-core`, `ffs-daemon`, `ffs-cli`, `ffs-mcp`, `ffs-federation`, `ffs-fastpath`, `ffs-skills-host`.
- MUST pin a workspace-wide `rust-toolchain.toml` to a stable Rust version known to support all required crates (Ed25519, ChaCha20-Poly1305, BLAKE3, rusqlite-bundled-sqlcipher, notify, tokio, rustls, axum).
- MUST configure CI to build and run `cargo test --workspace` on Linux, macOS, and Windows.
- MUST configure CI to produce static-binary release artifacts for `ffs-cli` on the three target triples (`x86_64-unknown-linux-musl`, `x86_64-apple-darwin` + `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`).
- MUST add lint and format gates (`cargo fmt --check`, `cargo clippy --workspace -- -D warnings`).
- SHOULD include a placeholder smoke test in each crate so CI exercises every member.
</requirements>

## Subtasks
- [ ] 1.1 Create the root `Cargo.toml` with workspace members and shared dependencies pinned at workspace level.
- [ ] 1.2 Create skeleton `Cargo.toml` and a `lib.rs` or `main.rs` placeholder for each member crate.
- [ ] 1.3 Configure `rust-toolchain.toml` and `.cargo/config.toml` (cross-platform build settings, target triples).
- [ ] 1.4 Add `.github/workflows/ci.yml` running build, test, fmt, clippy on the three OSes.
- [ ] 1.5 Add a `release.yml` workflow that produces static `ffs-cli` artifacts for each target on tagged releases.
- [ ] 1.6 Add a placeholder smoke test in every crate to confirm the workspace compiles and tests run.

## Implementation Details
Create the workspace layout described in TechSpec § Implementation Notes of ADR-015 (`crates/ffs-core`, `crates/ffs-daemon`, etc.). The CI matrix must verify that `bundled-sqlcipher` cross-compiles on each OS, since this is called out as a known risk in the TechSpec § Known Risks.

### Relevant Files
- `Cargo.toml` (new) — workspace manifest with members and shared dependency versions.
- `crates/*/Cargo.toml` (new × 7) — per-crate manifests.
- `rust-toolchain.toml` (new) — pinned toolchain.
- `.github/workflows/ci.yml` (new) — build/test/lint pipeline for three OSes.
- `.github/workflows/release.yml` (new) — release artifact generation.

### Dependent Files
- Every subsequent task assumes this workspace exists. Tasks 02-21 add modules and crates inside this scaffold.

### Related ADRs
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Defines the seven-crate layout and language stack.

## Deliverables
- A working Rust workspace at the repository root with all seven member crates declared.
- CI green on Linux, macOS, Windows for build, test, fmt, clippy.
- Release workflow producing `ffs-cli` static binaries for the three target triples on a tagged release.
- Unit tests with 80%+ coverage **(REQUIRED)** — applies to placeholder smoke tests; coverage target is per-crate in subsequent tasks.
- Integration tests for cross-platform build verification **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Each crate's placeholder smoke test compiles and passes.
- Integration tests:
  - [ ] CI matrix runs `cargo test --workspace` on Linux and reports green.
  - [ ] CI matrix runs `cargo test --workspace` on macOS and reports green.
  - [ ] CI matrix runs `cargo test --workspace` on Windows and reports green.
  - [ ] Release workflow produces a `ffs-cli` artifact for each target triple.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- `cargo build --workspace` succeeds on Linux, macOS, Windows.
- `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings` all green in CI.
- Release workflow yields a static-linked `ffs-cli` binary on each target triple verified by `file ffs-cli` showing no dynamic library dependencies on glibc/libc.
