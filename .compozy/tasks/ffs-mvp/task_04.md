---
status: completed
title: SQLite atom store with SQLCipher and bitemporal indexes
type: backend
complexity: high
dependencies:
  - task_02
---

# Task 04: SQLite atom store with SQLCipher and bitemporal indexes

## Overview
Implement the per-substrate SQLite database that stores atoms, classifications, capabilities, provenance, federation peers, and the working-set materialization metadata. The store is encrypted at rest via SQLCipher with a DEK from the OS keychain. All other components read and write atoms through this module.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST create the schema described in TechSpec § Implementation Design § Data Models (`atoms`, `classifications`, `capabilities`, `provenance`, `entities`, `claims_fts`, `federation_peers`, `working_set`, `ingest_quarantine`).
- MUST integrate SQLCipher (via `rusqlite` `bundled-sqlcipher` feature) and source the DEK from the OS keychain (via the `keyring` crate).
- MUST persist the canonical-JSON envelope as the source-of-truth `atoms.envelope` BLOB; indexable columns are derived copies.
- MUST verify atom signature and content hash before insert; reject invalid atoms.
- MUST resolve supersession-chain heads efficiently via the `(supersedes)` index.
- MUST maintain FTS5 index over claim payloads via triggers or explicit reindex on insert.
- MUST persist a `schema_version` row and refuse to open databases with an unknown future version.
- MUST expose an `AtomStore` trait so other modules can be tested against in-memory implementations.
- MUST commit atom writes atomically (single SQL transaction per atom group).
</requirements>

## Subtasks
- [x] 4.1 Define the `AtomStore` trait with read/write/query methods.
- [x] 4.2 Author the SQLite schema migrations (initial v1 schema).
- [x] 4.3 Implement the `SqliteAtomStore` against `rusqlite` with the `bundled-sqlcipher` feature.
- [x] 4.4 Source DEK from the OS keychain via the `keyring` crate.
- [x] 4.5 Implement signature and hash validation on every insert.
- [x] 4.6 Build composite indexes for bitemporal queries (entity × predicate × tx_time).
- [x] 4.7 Wire FTS5 indexing for claim payload search.
- [x] 4.8 Implement an in-memory `MemAtomStore` for downstream tests.

## Implementation Details
Create `crates/ffs-core/src/store/` with submodules `mod.rs`, `sqlite.rs`, `mem.rs`, `schema.rs`, `migrations.rs`. The single SQLite file lives at `~/.ffs/store.db`. Backup is `cp store.db backup.db` plus the keychain DEK separately.

See ADR-016 for the schema layout decisions and TechSpec § Implementation Design § Data Models for the table-by-table description.

### Relevant Files
- `crates/ffs-core/src/store/mod.rs` (new) — `AtomStore` trait and re-exports.
- `crates/ffs-core/src/store/sqlite.rs` (new) — SQLCipher implementation.
- `crates/ffs-core/src/store/mem.rs` (new) — in-memory implementation for testing.
- `crates/ffs-core/src/store/schema.rs` (new) — schema declarations.
- `crates/ffs-core/src/store/migrations.rs` (new) — schema migration runner.
- `crates/ffs-core/src/atom.rs` (task_02) — envelope verification used at insert time.

### Dependent Files
- `crates/ffs-core/src/capability.rs` (task_05) — reads capabilities.
- `crates/ffs-core/src/projection.rs` (task_06) — reads atoms for rendering.
- `crates/ffs-daemon` (task_07) — owns the store handle.
- `crates/ffs-federation` (tasks 14, 15) — reads atoms for federation pulls.

### Related ADRs
- [ADR-016: Single SQLite database per substrate with normalized atom store](adrs/adr-016.md) — Schema and indexing.
- [ADR-018: Cryptographic primitives — Ed25519, ChaCha20-Poly1305, BLAKE3](adrs/adr-018.md) — DEK and SQLCipher integration.

## Deliverables
- `AtomStore` trait + `SqliteAtomStore` + `MemAtomStore` implementations.
- Schema v1 migration runner.
- SQLCipher DEK integration via OS keychain on macOS, Windows, Linux.
- FTS5 index maintained on claim payloads.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests for SQLCipher cross-platform open/close **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Insert + lookup round-trip preserves the canonical envelope byte-for-byte.
  - [ ] Insert with a tampered signature is rejected with `StoreError::InvalidSignature`.
  - [ ] Insert with a tampered content hash is rejected with `StoreError::HashMismatch`.
  - [ ] `head_of_chain(entity, predicate)` returns the latest non-superseded atom for two-deep and three-deep chains.
  - [ ] Bitemporal point query: `as_of(t)` returns the atom whose tx_time <= t.
  - [ ] FTS5 query returns atoms whose claim payload matches the search term.
  - [ ] Opening a database with an unknown future schema_version refuses to open.
  - [ ] `MemAtomStore` and `SqliteAtomStore` produce the same results for the canonical fixture set.
- Integration tests:
  - [ ] SQLCipher-encrypted store opens with the correct DEK and rejects an incorrect DEK.
  - [ ] Database file is binary-encrypted on disk (verifiable by inspecting raw bytes).
  - [ ] Cross-platform open: a database created on macOS opens correctly on Linux given the same DEK.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- 1000-atom insert benchmark completes in under 5 seconds on developer hardware.
- Bitemporal query for `(entity, predicate, as_of)` against a 10000-atom store returns under 50ms.
