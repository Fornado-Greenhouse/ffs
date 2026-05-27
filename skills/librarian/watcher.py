"""Librarian: drift watcher + working-set curator.

This is a thin scheduler. The substantive work (render, hash
comparison, capability-checked re-materialization) lives in the
daemon's ``working_set.*`` JSON-RPC methods. The librarian's job is:

1. Trigger ``working_set.refresh_drifted`` on a cadence (default 30s
   per ``SKILL.md``).
2. Call ``working_set.evict_to_cap`` to enforce the size budget.
3. Honor explicit ``op`` requests from the host so the daemon can
   trigger a tick on demand (e.g., after a federation pull).

The librarian intentionally has no atom-authorship capability — it
only reshapes the materialized layer. See ``definition.atom.json``.
"""

from __future__ import annotations

import os
import sys
from typing import Any, Dict, List, Optional

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir, "_lib"))
if _LIB not in sys.path:
    sys.path.insert(0, _LIB)

from ffs_skill import FfsSkillError, log, query, run  # noqa: E402


DEFAULT_CAP = 1000


def refresh_drifted() -> List[Dict[str, Any]]:
    """Ask the daemon to refresh every drifted projection.

    Returns the list of `{path, render_hash, markdown}` objects the
    daemon re-materialized. An empty list means nothing was stale.
    """
    try:
        result = query("working_set.refresh_drifted", {})
    except FfsSkillError as e:
        log("warn", f"working_set.refresh_drifted failed: {e}")
        return []
    if not isinstance(result, dict):
        return []
    refreshed = result.get("refreshed") or []
    if not isinstance(refreshed, list):
        return []
    return refreshed


def evict_to_cap(cap: int) -> List[str]:
    """Enforce the size cap. Returns the list of evicted paths."""
    try:
        result = query("working_set.evict_to_cap", {"cap": int(cap)})
    except FfsSkillError as e:
        log("warn", f"working_set.evict_to_cap failed: {e}")
        return []
    if not isinstance(result, dict):
        return []
    evicted = result.get("evicted") or []
    if not isinstance(evicted, list):
        return []
    return [str(p) for p in evicted]


def tick(cap: int = DEFAULT_CAP) -> Dict[str, Any]:
    """One full librarian pass: refresh, then evict.

    Refresh happens BEFORE eviction so a drifted projection isn't
    evicted just because its hash is stale; the refresh updates
    `last_touched_at` too, which is the eviction key.
    """
    refreshed = refresh_drifted()
    if refreshed:
        log("info", f"refreshed {len(refreshed)} drifted projections")
    evicted = evict_to_cap(cap)
    if evicted:
        log("info", f"evicted {len(evicted)} projections to fit cap {cap}")
    return {
        "refreshed": [r.get("path") for r in refreshed if isinstance(r, dict)],
        "evicted": evicted,
    }


def handle(inp: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    """Dispatch ``invoke`` frames from the host.

    `inp` shape::

        {"op": "tick" | "refresh" | "evict", "cap": <int>}

    Defaults to ``tick`` with the default cap when fields are missing.
    """
    if not isinstance(inp, dict):
        inp = {}
    op = str(inp.get("op") or "tick").lower()
    cap = int(inp.get("cap") or DEFAULT_CAP)
    if op == "refresh":
        refreshed = refresh_drifted()
        return {"refreshed": [r.get("path") for r in refreshed if isinstance(r, dict)]}
    if op == "evict":
        return {"evicted": evict_to_cap(cap)}
    if op == "tick":
        return tick(cap)
    log("warn", f"unknown op: {op!r}; defaulting to tick")
    return tick(cap)


if __name__ == "__main__":
    run(handle)
