---
name: librarian
kind: librarian
entry_point: watcher.py
python: python3
timeout_ms: 30000
---

# Librarian

The working-set curator. Watches the materialized projection files on
disk and keeps them in sync with the substrate state:

- **Drift detection**: a projection's stored `last_render_hash` no
  longer matches a fresh render of the same path. Means an atom under
  it changed since the last materialization.
- **Refresh**: drifted projections get re-rendered and the working-set
  entry's hash is bumped.
- **Eviction**: when the working set exceeds the configured cap, the
  oldest non-pinned entries are dropped. User-pinned projections
  always survive eviction regardless of recency.

The librarian is a thin tick loop — the smart logic (capability
checks, render, hash comparison) lives in the daemon's
`working_set.*` JSON-RPC methods. The librarian's job is to schedule
those calls and decide when to evict.

## Wire shape

Input from the host (`invoke.input`):

```json
{
  "op": "tick",
  "cap": 1000
}
```

Supported ops:

- `tick`: run a full pass (refresh drifted, then evict to cap).
  Returns `{"refreshed": [...], "evicted": [...]}`.
- `refresh`: refresh drifted projections only.
- `evict`: enforce the size cap only.

## ADRs

- ADR-005 — Editor-agnostic working set materialization.
- ADR-016 — Single SQLite database; the `working_set` table backs
  the in-memory store the daemon currently uses.
