---
status: completed
title: SQLite-backed quarantine — persist pending submissions across daemon restarts
type: backend
complexity: medium
dependencies:
  - task_24
  - task_26
---

# Task 29: SQLite-backed quarantine — persist pending submissions across daemon restarts

## Overview
The dispatcher's quarantine is `InMemoryQuarantine` — pending submissions vanish when the daemon restarts, even though the upstream files have already been moved to `ingest/.processed/`. So if a user drops a note in `ingest/` and doesn't accept the proposals before the daemon restarts (laptop sleep, install upgrade, crash), the proposals are lost AND the source file is gone from `ingest/`. This task adds a SQLite-backed `IngestQuarantine` implementation that survives restarts.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add a `SqliteQuarantine` implementation of the existing `IngestQuarantine` trait, backed by a SQLite table inside the same SQLCipher-encrypted database the atom store uses (`$FFS_DATA_DIR/atoms.db`) so the DEK already protects submissions and proposals.
- MUST replace `InMemoryQuarantine` in the daemon binary's `main.rs` with the SQLite-backed variant; the existing `Dispatcher::quarantine: Arc<dyn IngestQuarantine>` slot accepts the new type without further changes.
- MUST preserve the existing trait semantics: idempotent `complete()`, status state-machine (Pending → Extracted → Accepted/Rejected/Failed), accepted_atom_hashes recorded on Accept.
- MUST handle the schema migration cleanly: adding the quarantine table doesn't break stores from task_24. Bump `SCHEMA_VERSION` and add a new migration step.
- MUST NOT regress the existing `tests/dispatch_integration.rs` and `tests/scribe_integration.rs` — they use `InMemoryQuarantine` directly and that backend stays in place (tests don't need SQLCipher).
- SHOULD include a "list-all" debug RPC (or just expand `ingest.list_pending`) that surfaces Failed and Rejected submissions for the daily summary so they don't silently disappear from the user's view.
</requirements>

## Subtasks
- [x] 29.1 Add the `submissions` + `proposals` SQLite table schema as a new migration step. *(Tables named `quarantine_submissions` + `quarantine_proposals` to avoid colliding with the v1 placeholder `ingest_quarantine` table that was never used; the v1 placeholder stays untouched for backwards compatibility.)*
- [x] 29.2 Implement `SqliteQuarantine` against the existing `IngestQuarantine` trait, mirroring the `InMemoryQuarantine` semantics. *(New module `crates/ffs-core/src/quarantine_sqlite.rs` with 9 unit tests + carrying the SQLite blessing comment block per CLAUDE.md convention.)*
- [x] 29.3 Wire `SqliteQuarantine` into the daemon binary's `main.rs` alongside the atom store. *(Holds its own `Mutex<Connection>` to the same atoms.db file; WAL mode allows the two connections to read concurrently without contention. Sharing the atom store's Mutex would have required a deeper refactor of `SqliteAtomStore`'s API and wasn't worth it for MVP.)*
- [x] 29.4 Bump `SCHEMA_VERSION` and verify the migration applies cleanly to an existing task_24 atoms.db. *(Migration runner rewritten to step forward one version at a time. 5 new tests + the live-deploy onto the user's existing atoms.db confirmed the v1→v2 step succeeded silently and the existing atoms + capabilities + materializer state were preserved.)*
- [x] 29.5 Add an integration test that proves a submission with extracted proposals survives a daemon restart. *(`quarantine_submission_survives_daemon_restart` in `tests/sqlite_persistence.rs`. Also verified live: dropped a note, restarted launchd, watched `ingest.list_pending` still return the submission with its proposals.)*

## Implementation Details
The existing `IngestQuarantine` trait lives at `crates/ffs-core/src/quarantine.rs`. The SQLite atom store machinery (migrations, connection management, SQLCipher key) is at `crates/ffs-core/src/store/sqlite.rs` + `crates/ffs-core/src/store/migrations.rs`. The new quarantine should reuse that infrastructure rather than opening its own SQLite handle.

Schema sketch:

```sql
CREATE TABLE submissions (
    id TEXT PRIMARY KEY,
    source_uri TEXT NOT NULL,
    content_hash BLOB NOT NULL,
    content BLOB NOT NULL,
    tx_time TEXT NOT NULL,
    status TEXT NOT NULL,  -- pending / extracted / accepted / rejected / failed
    failure_reason TEXT,
    accepted_atom_hashes TEXT,  -- JSON array of multibase strings
    created_at TEXT NOT NULL
);

CREATE TABLE proposals (
    submission_id TEXT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    predicate TEXT NOT NULL,
    claim TEXT NOT NULL,       -- JSON
    provenance TEXT NOT NULL,  -- JSON
    rationale TEXT NOT NULL,
    PRIMARY KEY (submission_id, seq)
);

CREATE INDEX submissions_status_idx ON submissions(status, tx_time DESC);
```

### Relevant Files
- `crates/ffs-core/src/quarantine.rs` — `IngestQuarantine` trait + `InMemoryQuarantine` (stays in place for tests).
- `crates/ffs-core/src/store/sqlite.rs` — SqliteAtomStore + connection management.
- `crates/ffs-core/src/store/migrations.rs` — schema migration applier.
- `crates/ffs-core/src/store/mod.rs` — `SCHEMA_VERSION` constant.
- `crates/ffs-daemon/src/main.rs` — wire-up site.

### Dependent Files
- `crates/ffs-daemon/tests/sqlite_persistence.rs` — extend to cover quarantine persistence.
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — assert the post-restart Accept path still works.

### Related ADRs
- [ADR-016: Single SQLite database per substrate with normalized atom store](adrs/adr-016.md) — Same DB, new tables.
- [ADR-018: Cryptographic primitives — Ed25519, ChaCha20-Poly1305, BLAKE3](adrs/adr-018.md) — SQLCipher already protects the DB at rest.

## Deliverables
- `SqliteQuarantine` implementation backed by the same SQLCipher DB as the atom store.
- Daemon binary wired to use it instead of `InMemoryQuarantine`.
- Schema migration that applies cleanly to existing task_24 substrates.
- Unit tests with 80%+ coverage **(REQUIRED)** for the SqliteQuarantine implementation.
- Integration test for cross-restart submission persistence **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `SqliteQuarantine` submit-then-get returns the same Submission.
  - [ ] `complete()` is idempotent — a second call with the same proposals is a no-op rather than an error.
  - [ ] `accept()` transitions Extracted → Accepted only; trying to accept a Pending submission errors.
  - [ ] `list()` with `Some(SubmissionStatus::Extracted)` filter returns only Extracted entries.
  - [ ] `list()` with `None` filter returns every submission regardless of status.
  - [ ] Schema migration applies cleanly to a fresh DB AND to a task_24 DB with the atom store but no quarantine tables yet.
- Integration tests:
  - [ ] Daemon starts, ingest pipeline lands a submission with extracted proposals, daemon restarts (same DEK), `ingest.list_pending` still surfaces the submission and `ingest.accept` succeeds.
  - [ ] Failed and Rejected submissions still appear when the new debug-list RPC (or extended list_pending) is queried, so they aren't lost to the user.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A user who drops a note in `ingest/`, lets the laptop sleep + wake (daemon restarts), and returns to Obsidian sees the proposal in the daily-summary panel exactly as before.
- The first-use-guide flow tolerates daemon restarts at any point in the proposal-review cycle without losing user-visible state.
