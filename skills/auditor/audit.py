"""Auditor: daily health summary.

Aggregates substrate metrics over a 24h window, applies threshold
rules to surface anomalies, and publishes an `auditor.daily_summary`
atom via the daemon's `audit.publish_summary` JSON-RPC method.

Pipeline per invocation:

1. Call `health.summary` for the baseline counts (proposals,
   drift_flags, atom_count).
2. Call `atom.list` for the auditor entity to read the chain head
   (the prior summary) — used for narrative deltas in Phase 2.
3. Compute threshold flags from the optional `metrics` input field
   the daemon may attach in production. MVP scribe/librarian do not
   yet feed these counters; the auditor surfaces the structural
   counts and leaves agent-specific tallies at zero.
4. Sort candidate panel items by priority and truncate to 5.
5. Build the structured claim + narrative.
6. Publish via `audit.publish_summary`.

The auditor tolerates partial substrate access: a host that refuses
one query (e.g., `atom.list` capability denial) yields a summary with
zeros for that field plus a warning, rather than failing the entire
tick.
"""

from __future__ import annotations

import os
import sys
from typing import Any, Dict, List, Optional, Tuple

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir, "_lib"))
if _LIB not in sys.path:
    sys.path.insert(0, _LIB)

from ffs_skill import FfsSkillError, log, query, run  # noqa: E402


# Threshold constants (per TechSpec § Monitoring and Observability §
# Alerting thresholds). The auditor does not adapt these — they are
# substrate-wide defaults the user can re-tune in a future config.
DENIAL_THRESHOLD = 10
FEDERATION_FAILURE_RATIO = 0.5
INGEST_BACKLOG_THRESHOLD = 100
PANEL_MAX_ITEMS = 5


def aggregate_metrics(window_hours: int = 24) -> Dict[str, Any]:
    """Pull baseline counters from the daemon.

    `window_hours` is informational at MVP — the daemon's
    `health.summary` reports current state, not windowed counts.
    Phase 2 wires `audit.query` for windowed aggregation.
    """
    metrics: Dict[str, Any] = {
        "atom_author_rate": 0,
        "proposals": 0,
        "drift_flags": 0,
        "working_set_size": 0,
        "ingest_queue_depth": 0,
        "fast_path_apply_count": 0,
        "slow_path_route_count": 0,
        "capability_denials_per_agent": {},
        "federation_pull_failure_rate_per_peer": {},
        "window_hours": int(window_hours),
    }
    try:
        summary = query("health.summary", {})
    except FfsSkillError as e:
        log("warn", f"health.summary failed: {e}")
        return metrics
    if isinstance(summary, dict):
        metrics["proposals"] = int(summary.get("proposals") or 0)
        metrics["drift_flags"] = int(summary.get("drift_flags") or 0)
        # `health.summary.atom_count` is approximate at MVP (the
        # store has no total-count method yet). The auditor surfaces
        # it as `atom_author_rate` for now — Phase 2 replaces it with
        # a windowed count.
        metrics["atom_author_rate"] = int(summary.get("atom_count") or 0)
        metrics["ingest_queue_depth"] = metrics["proposals"]
    return metrics


def evaluate_flags(metrics: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Apply threshold rules. Returns a prioritized list of flag
    dicts, each with `priority`, `kind`, and `message`.

    Priority numbering: lower is higher-priority (1 sorts before 5).
    """
    flags: List[Dict[str, Any]] = []

    # Federation health (priority 1): rolled up across peers.
    federation = metrics.get("federation_pull_failure_rate_per_peer") or {}
    if isinstance(federation, dict):
        for peer, ratio in federation.items():
            try:
                if float(ratio) > FEDERATION_FAILURE_RATIO:
                    flags.append(
                        {
                            "priority": 1,
                            "kind": "federation_unhealthy",
                            "peer": peer,
                            "message": f"bridge with peer {peer} is unhealthy ({float(ratio) * 100:.0f}% failures)",
                        }
                    )
            except (TypeError, ValueError):
                continue

    # Capability denials (priority 2).
    denials = metrics.get("capability_denials_per_agent") or {}
    if isinstance(denials, dict):
        for agent, count in denials.items():
            try:
                if int(count) > DENIAL_THRESHOLD:
                    flags.append(
                        {
                            "priority": 2,
                            "kind": "capability_denials",
                            "agent": agent,
                            "count": int(count),
                            "message": f"agent {agent} attempted {int(count)} out-of-scope writes",
                        }
                    )
            except (TypeError, ValueError):
                continue

    # Fast-path inversion (priority 3).
    fast = int(metrics.get("fast_path_apply_count") or 0)
    slow = int(metrics.get("slow_path_route_count") or 0)
    if slow > fast and (fast + slow) > 0:
        flags.append(
            {
                "priority": 3,
                "kind": "fast_path_inversion",
                "fast_path_apply_count": fast,
                "slow_path_route_count": slow,
                "message": "consider whether predicate specs need additional reverse-map coverage",
            }
        )

    # Drift flags (priority 4) — single rollup, not per-projection.
    drift = int(metrics.get("drift_flags") or 0)
    if drift > 0:
        flags.append(
            {
                "priority": 4,
                "kind": "drift",
                "count": drift,
                "message": f"{drift} projection{'s' if drift != 1 else ''} drifted; the librarian will refresh on next tick",
            }
        )

    # Ingest backlog (priority 5).
    backlog = int(metrics.get("ingest_queue_depth") or 0)
    if backlog > INGEST_BACKLOG_THRESHOLD:
        flags.append(
            {
                "priority": 5,
                "kind": "ingest_backlog",
                "count": backlog,
                "message": f"you have a backlog of {backlog} scribe proposals; consider reviewing",
            }
        )

    return flags


def top_n(flags: List[Dict[str, Any]], n: int = PANEL_MAX_ITEMS) -> List[Dict[str, Any]]:
    """Sort flags by priority and return the top `n`. Stable sort:
    items at the same priority keep their input order, so the
    aggregator's domain ordering (which uses dict-iteration order
    over agents / peers in Python 3.7+) is preserved.
    """
    ordered = sorted(flags, key=lambda f: int(f.get("priority", 99)))
    return ordered[:n]


def narrative(metrics: Dict[str, Any], flags: List[Dict[str, Any]]) -> str:
    """Build a short human-readable narrative summarizing the day."""
    if not flags:
        return (
            f"All quiet. {metrics.get('atom_author_rate', 0)} atom(s) over the last "
            f"{metrics.get('window_hours', 24)}h; "
            f"{metrics.get('proposals', 0)} pending proposals; "
            f"{metrics.get('drift_flags', 0)} drifted projections. No threshold flags."
        )
    bullets = "\n".join(f"- {f['message']}" for f in flags)
    return (
        f"{len(flags)} flag(s) over the last {metrics.get('window_hours', 24)}h:\n{bullets}"
    )


def build_claim(metrics: Dict[str, Any], flags: List[Dict[str, Any]]) -> Tuple[Dict[str, Any], List[Dict[str, Any]]]:
    """Compose the structured `auditor.daily_summary` claim plus the
    top-N panel list. Returns `(claim, panel)`.
    """
    panel = top_n(flags)
    claim = {
        "metrics": metrics,
        "flags": flags,
        "panel": panel,
        "narrative": narrative(metrics, flags),
    }
    return claim, panel


def publish(claim: Dict[str, Any]) -> Dict[str, Any]:
    """Send the claim to the daemon. Returns `{"atom_hash": "..."}`
    on success, `{"atom_hash": None, "reason": "..."}` on failure.
    """
    try:
        result = query("audit.publish_summary", {"claim": claim})
    except FfsSkillError as e:
        log("warn", f"audit.publish_summary failed: {e}")
        return {"atom_hash": None, "reason": str(e)}
    if not isinstance(result, dict):
        return {"atom_hash": None, "reason": "unexpected response from audit.publish_summary"}
    return {"atom_hash": result.get("atom_hash")}


def tick(window_hours: int = 24) -> Dict[str, Any]:
    """One auditor pass: aggregate, evaluate, publish."""
    metrics = aggregate_metrics(window_hours)
    flags = evaluate_flags(metrics)
    claim, panel = build_claim(metrics, flags)
    pub = publish(claim)
    return {
        "atom_hash": pub.get("atom_hash"),
        "reason": pub.get("reason"),
        "panel": panel,
    }


def handle(inp: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    """Dispatch `invoke` frames from the host.

    `inp` shape::

        {"op": "tick", "window_hours": 24}

    Defaults to a 24h tick when fields are missing.
    """
    if not isinstance(inp, dict):
        inp = {}
    op = str(inp.get("op") or "tick").lower()
    window = int(inp.get("window_hours") or 24)
    if op == "tick":
        return tick(window)
    log("warn", f"unknown op: {op!r}; defaulting to tick")
    return tick(window)


if __name__ == "__main__":
    run(handle)
