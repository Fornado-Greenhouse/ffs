---
name: auditor
kind: auditor
entry_point: audit.py
python: python3
timeout_ms: 30000
---

# Auditor

The substrate's daily-health reporter. Aggregates metrics over a
24-hour window, applies threshold rules to surface anomalies, and
authors an `auditor.daily_summary` atom into the substrate. The
Obsidian plugin's daily-health-summary panel (task 19) renders the
latest one; `ffs health` reads it from the CLI.

## Metrics aggregated

Per TechSpec § Monitoring and Observability:

- `atom_author_rate` — atoms authored in the last 24h.
- `proposals` — pending scribe submissions awaiting user acceptance.
- `drift_flags` — working-set entries whose render hash diverged.
- `capability_denials_per_agent` — counts of denials per agent key.
- `federation_pull_failure_rate_per_peer` — failed-pull ratio per peer.
- `fast_path_vs_slow_path_ratio` — fast-path applies vs slow-path routes.
- `working_set_size` — current materialized projection count.
- `ingest_queue_depth` — backlog of pending scribe submissions.

## Threshold flags

- >10 capability denials per agent per day → "agent X attempted
  out-of-scope writes".
- federation pull failure rate >50% over 24h → "bridge with peer X
  is unhealthy".
- ingest queue depth >100 → "you have a backlog of scribe proposals".
- fast-path / slow-path ratio inversion (slow > fast) → "consider
  predicate-spec coverage".

## Panel limit

The user-visible panel shows the top 5 items by priority. Priorities
(highest first): federation health, capability denials, fast-path
inversion, drift flags, ingest backlog.

## Wire shape

Input from the host (`invoke.input`):

```json
{ "op": "tick" }
```

Returns `{"atom_hash": "..."}` for the published summary, or
`{"atom_hash": null, "reason": "..."}` if publishing is unavailable.

## ADRs

- ADR-013 — MCP server in MVP. `ffs_audit_query` MCP tool reads
  auditor summary atoms.
