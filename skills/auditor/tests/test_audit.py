"""Unit tests for the auditor's aggregation, threshold, and panel logic.

Stubs the `query()` helper so tests exercise the local logic without
spinning up the daemon. Atom-authoring correctness is verified Rust-
side in `crates/ffs-daemon/tests/auditor_integration.rs`.
"""

from __future__ import annotations

from typing import Any, Dict, List

import audit  # type: ignore  # provided by conftest sys.path bootstrap.


class _Recorder:
    def __init__(self, responses: Dict[str, Any]) -> None:
        self.responses = responses
        self.calls: List[Dict[str, Any]] = []

    def __call__(self, method: str, params: Any) -> Any:
        self.calls.append({"method": method, "params": params})
        return self.responses.get(method, {})


def _install(monkeypatch, recorder: _Recorder) -> None:
    monkeypatch.setattr(audit, "query", recorder)


# ---------------------------------------------------------------------
# Required-by-spec unit tests
# ---------------------------------------------------------------------


def test_metric_aggregation_pulls_health_summary_counts(monkeypatch):
    rec = _Recorder(
        {
            "health.summary": {
                "proposals": 7,
                "questions": 0,
                "drift_flags": 3,
                "atom_count": 100,
            }
        }
    )
    _install(monkeypatch, rec)
    metrics = audit.aggregate_metrics(window_hours=24)
    # Per the auditor's MVP design, `atom_count` from health.summary
    # is surfaced as `atom_author_rate`. Phase 2 replaces this with a
    # real windowed count.
    assert metrics["atom_author_rate"] == 100
    assert metrics["proposals"] == 7
    assert metrics["drift_flags"] == 3
    assert metrics["ingest_queue_depth"] == 7
    assert metrics["window_hours"] == 24


def test_capability_denials_above_threshold_trigger_flag():
    metrics = {
        "capability_denials_per_agent": {"agent-x": 11, "agent-y": 5},
        "federation_pull_failure_rate_per_peer": {},
        "fast_path_apply_count": 0,
        "slow_path_route_count": 0,
        "drift_flags": 0,
        "ingest_queue_depth": 0,
    }
    flags = audit.evaluate_flags(metrics)
    denial_flags = [f for f in flags if f["kind"] == "capability_denials"]
    assert len(denial_flags) == 1
    assert denial_flags[0]["agent"] == "agent-x"
    assert denial_flags[0]["count"] == 11
    assert "out-of-scope" in denial_flags[0]["message"]


def test_fast_path_inversion_triggers_advisory_flag():
    metrics = {
        "capability_denials_per_agent": {},
        "federation_pull_failure_rate_per_peer": {},
        "fast_path_apply_count": 3,
        "slow_path_route_count": 7,
        "drift_flags": 0,
        "ingest_queue_depth": 0,
    }
    flags = audit.evaluate_flags(metrics)
    inv = [f for f in flags if f["kind"] == "fast_path_inversion"]
    assert len(inv) == 1
    assert "reverse-map" in inv[0]["message"]


def test_five_item_limit_keeps_highest_priority_first():
    # Construct 10 candidate flags, mixed priorities. We expect the
    # top 5 by priority (lower number = higher priority) in stable
    # order.
    flags = [
        {"priority": 3, "kind": "drift", "message": "drift-A"},
        {"priority": 1, "kind": "federation_unhealthy", "message": "fed-A"},
        {"priority": 2, "kind": "capability_denials", "message": "cap-A"},
        {"priority": 5, "kind": "ingest_backlog", "message": "backlog-A"},
        {"priority": 4, "kind": "ingest_backlog", "message": "backlog-B"},
        {"priority": 1, "kind": "federation_unhealthy", "message": "fed-B"},
        {"priority": 2, "kind": "capability_denials", "message": "cap-B"},
        {"priority": 5, "kind": "drift", "message": "drift-B"},
        {"priority": 3, "kind": "fast_path_inversion", "message": "fp-A"},
        {"priority": 4, "kind": "fast_path_inversion", "message": "fp-B"},
    ]
    top = audit.top_n(flags, n=5)
    assert len(top) == 5
    # Priorities are sorted ascending and stable: priority 1 first,
    # in input order; then priority 2 in input order, etc.
    messages = [f["message"] for f in top]
    assert messages == ["fed-A", "fed-B", "cap-A", "cap-B", "drift-A"]


# ---------------------------------------------------------------------
# Coverage extras
# ---------------------------------------------------------------------


def test_no_threshold_breaches_yields_empty_flag_list():
    metrics = {
        "capability_denials_per_agent": {"agent-z": 3},
        "federation_pull_failure_rate_per_peer": {"peer-a": 0.1},
        "fast_path_apply_count": 50,
        "slow_path_route_count": 5,
        "drift_flags": 0,
        "ingest_queue_depth": 4,
    }
    assert audit.evaluate_flags(metrics) == []


def test_narrative_no_flags_announces_all_quiet():
    metrics = {"atom_author_rate": 5, "proposals": 0, "drift_flags": 0, "window_hours": 24}
    text = audit.narrative(metrics, [])
    assert text.startswith("All quiet")
    assert "5 atom" in text


def test_narrative_with_flags_bullets_each_message():
    metrics = {"window_hours": 24}
    flags = [
        {"message": "fed-A"},
        {"message": "cap-A"},
    ]
    text = audit.narrative(metrics, flags)
    assert "- fed-A" in text
    assert "- cap-A" in text
    assert "2 flag" in text


def test_drift_flag_emitted_when_drift_count_positive():
    metrics = {
        "capability_denials_per_agent": {},
        "federation_pull_failure_rate_per_peer": {},
        "fast_path_apply_count": 0,
        "slow_path_route_count": 0,
        "drift_flags": 4,
        "ingest_queue_depth": 0,
    }
    flags = audit.evaluate_flags(metrics)
    drift = [f for f in flags if f["kind"] == "drift"]
    assert len(drift) == 1
    assert drift[0]["count"] == 4


def test_federation_failure_above_50pct_triggers_flag():
    metrics = {
        "capability_denials_per_agent": {},
        "federation_pull_failure_rate_per_peer": {"peer-a": 0.7, "peer-b": 0.3},
        "fast_path_apply_count": 1,
        "slow_path_route_count": 0,
        "drift_flags": 0,
        "ingest_queue_depth": 0,
    }
    flags = audit.evaluate_flags(metrics)
    fed = [f for f in flags if f["kind"] == "federation_unhealthy"]
    assert len(fed) == 1
    assert fed[0]["peer"] == "peer-a"


def test_tick_calls_publish_with_built_claim(monkeypatch):
    rec = _Recorder(
        {
            "health.summary": {"proposals": 0, "drift_flags": 0, "atom_count": 12},
            "audit.publish_summary": {"atom_hash": "z-stub-hash"},
        }
    )
    _install(monkeypatch, rec)
    result = audit.tick(window_hours=24)
    assert result["atom_hash"] == "z-stub-hash"
    # Verify the publish call carried a structured claim including
    # the narrative.
    publish_call = next(c for c in rec.calls if c["method"] == "audit.publish_summary")
    claim = publish_call["params"]["claim"]
    assert "metrics" in claim
    assert "panel" in claim
    assert "narrative" in claim
    assert claim["metrics"]["atom_author_rate"] == 12


def test_tick_returns_panel_truncated_to_five(monkeypatch):
    """End-to-end: many flags upstream → panel has at most 5."""
    fake_metrics = {
        "atom_author_rate": 0,
        "proposals": 0,
        "drift_flags": 11,
        "working_set_size": 0,
        "ingest_queue_depth": 200,
        "fast_path_apply_count": 1,
        "slow_path_route_count": 50,
        "capability_denials_per_agent": {f"agent-{i}": 100 for i in range(6)},
        "federation_pull_failure_rate_per_peer": {"peer-a": 0.9, "peer-b": 0.8},
        "window_hours": 24,
    }
    monkeypatch.setattr(audit, "aggregate_metrics", lambda *_args, **_kwargs: fake_metrics)
    rec = _Recorder({"audit.publish_summary": {"atom_hash": "h"}})
    _install(monkeypatch, rec)
    result = audit.tick()
    assert len(result["panel"]) == 5


def test_publish_failure_returns_reason(monkeypatch):
    from ffs_skill import FfsSkillError  # type: ignore

    def boom(method: str, params: Any) -> Any:
        raise FfsSkillError("capability denied: write on auditor.daily_summary")

    monkeypatch.setattr(audit, "query", boom)
    result = audit.publish({"hello": "world"})
    assert result["atom_hash"] is None
    assert "capability denied" in result["reason"]
