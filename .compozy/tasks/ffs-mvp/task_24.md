---
status: pending
title: Wire SQLite atom store as the daemon binary's default store
type: backend
complexity: low
dependencies:
  - task_04
  - task_22
---

# Task 24: Wire SQLite atom store as the daemon binary's default store

## Overview
The daemon binary currently constructs `MemAtomStore::new()` in `main.rs`, so the substrate's atoms vanish on every restart — which makes the first-use-guide flow ("capture a contact, restart your laptop, see it again") impossible to rehearse. Switch the binary to the `SqliteAtomStore` (already implemented and tested in `ffs-core` per task_04), persisting to `$FFS_DATA_DIR/atoms.db`.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST replace the `MemAtomStore::new()` call site in `crates/ffs-daemon/src/main.rs` with a `SqliteAtomStore` opened against `$FFS_DATA_DIR/atoms.db`.
- MUST read the SQLCipher DEK from the `FFS_SQLCIPHER_KEY_HEX` env var (64 hex chars → 32 bytes); when unset, generate a fresh DEK and warn the same way `FFS_OWNER_KEY_HEX` is handled today. (Keychain-pull is task_27's scope, not this task.)
- MUST surface SQLite/SQLCipher open errors via the existing `StartupError` enum without changing the daemon's exit-code contract.
- MUST NOT regress any existing workspace test; the daemon binary tests under `tests/binary_end_to_end.rs` and `tests/installer_layout.rs` continue to pass.
- SHOULD log the resolved database path at INFO so users see where their data lives.
</requirements>

## Subtasks
- [ ] 24.1 Read the SQLCipher DEK from `FFS_SQLCIPHER_KEY_HEX` (with the fresh-key-and-warn fallback path mirroring the existing owner-key handling).
- [ ] 24.2 Open `SqliteAtomStore` against `$FFS_DATA_DIR/atoms.db` and wire it into the `Dispatcher` in place of `MemAtomStore`.
- [ ] 24.3 Extend `StartupError` with a `Store` variant carrying the underlying `StoreError`, keeping the existing `result_large_err` discipline (boxed payload).
- [ ] 24.4 Update `tests/binary_end_to_end.rs` to set `FFS_SQLCIPHER_KEY_HEX` so the test stays deterministic, and add a new test that writes an atom, restarts the binary, and asserts the atom is still queryable.

## Implementation Details
Modify `crates/ffs-daemon/src/main.rs` only — the SQLite store is already feature-complete in `ffs-core::store::sqlite`. Honor the workspace's `bundled-sqlcipher` cargo feature when present (TechSpec § Known Risks calls out SQLCipher cross-compilation friction; see [`troubleshooting.md`](../../docs/onboarding/troubleshooting.md#sqlcipher-cross-platform-issues)). The `SqliteAtomStore` API is `open_with_key(path: &Path, key: &[u8; 32])`; the existing `decode_hex` helper handles the env-var parsing.

The daemon's existing `EventPublisher` and capability-evaluator wiring don't change — only the concrete store type behind `Arc<dyn AtomStore>` does.

### Relevant Files
- `crates/ffs-daemon/src/main.rs` — current `MemAtomStore::new()` call at the dispatcher construction site.
- `crates/ffs-core/src/store/sqlite.rs` — `SqliteAtomStore::open_with_key` constructor.
- `crates/ffs-core/src/store/mod.rs` — `StoreError` enum re-exported here.
- `crates/ffs-daemon/tests/binary_end_to_end.rs` — current end-to-end smoke that needs the env var set.

### Dependent Files
- `crates/ffs-daemon/tests/installer_layout.rs` — runs the binary through the installer, currently doesn't depend on persistence; should keep passing.
- `docs/onboarding/troubleshooting.md` — already documents SQLCipher failure modes; no edit needed unless the failure mode signature changes.

### Related ADRs
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Daemon owns persistence.
- [ADR-016: Single SQLite database per substrate with normalized atom store](adrs/adr-016.md) — The storage decision being honored.

## Deliverables
- `MemAtomStore::new()` removed from the daemon binary; replaced with `SqliteAtomStore::open_with_key`.
- `FFS_SQLCIPHER_KEY_HEX` env-var handling with the same fresh-key-and-warn fallback as `FFS_OWNER_KEY_HEX`.
- Updated `binary_end_to_end` test pinning the DEK for determinism.
- New integration test (`atoms_persist_across_restart`) that writes an atom, drops + recreates the daemon process, and confirms the atom is still queryable.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to the new env-var/keying helper if extracted.
- Integration tests for cross-restart persistence **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `FFS_SQLCIPHER_KEY_HEX` parsing rejects non-hex / wrong-length inputs with a clear error.
  - [ ] Missing env var generates a 32-byte DEK and emits a warning (parallels the existing owner-key behavior).
- Integration tests:
  - [ ] Daemon starts with `FFS_SQLCIPHER_KEY_HEX` set, writes one atom via the dispatcher, exits, restarts with the same DEK, and `atom.get` returns the same envelope bytes.
  - [ ] Daemon refuses to open an existing `atoms.db` when the supplied DEK is wrong (surfaced as a startup error with non-zero exit).
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- `ffs cat` against a daemon restarted with the same DEK returns previously-written atoms.
- The first-use-guide rehearsal step "open Sara Chen's contact after a daemon restart" works for the first time.
