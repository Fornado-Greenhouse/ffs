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


# ---------------------------------------------------------------------
# task_40: courier line and missed-schedule flag
# ---------------------------------------------------------------------


def test_courier_ran_line_present(monkeypatch):
    rec = _Recorder(
        {
            "health.summary": {
                "proposals": 0,
                "questions": 0,
                "drift_flags": 0,
                "atom_count": 3,
                "courier": {
                    "last_run": "2026-09-21T12:00:00Z",
                    "items_seen": 14,
                    "files_written": 9,
                    "fetch_failures": 0,
                    "last_error": None,
                },
            }
        }
    )
    _install(monkeypatch, rec)
    metrics = audit.aggregate_metrics(window_hours=24)
    assert metrics["courier"]["items_seen"] == 14
    text = audit.narrative(metrics, [])
    assert "courier ran 2026-09-21T12:00:00Z: 14 items, 9 files" in text
    # A fresh run is not a missed schedule.
    from datetime import datetime, timezone

    assert audit.courier_missed(metrics, now=datetime(2026, 9, 22, 0, 0, tzinfo=timezone.utc)) is None


def test_courier_missed_schedule_is_a_flag():
    from datetime import datetime, timezone

    stale = {"courier": {"last_run": "2026-09-19T06:00:00Z", "items_seen": 1, "files_written": 1}}
    flag = audit.courier_missed(stale, now=datetime(2026, 9, 21, 6, 0, tzinfo=timezone.utc))
    assert flag is not None
    assert flag["kind"] == "courier_missed"
    assert flag["priority"] == 3
    assert "has not run since 2026-09-19T06:00:00Z" in flag["message"]
    never = {"courier": None}
    flag2 = audit.courier_missed(never)
    assert flag2 is not None and "since never" in flag2["message"]
    assert audit.courier_line(never) == "courier has not run"
    # No courier key at all: nothing to say, nothing to flag.
    assert audit.courier_missed({}) is None
    assert audit.courier_line({}) is None
    long_ago = {"courier": {"last_run": "2020-01-01T00:00:00Z", "items_seen": 1, "files_written": 1}}
    flags = audit.evaluate_flags(long_ago)
    assert any(f["kind"] == "courier_missed" for f in flags)


# task_39: auto-filed section (ADR-029)
def test_auto_filed_section_present_and_counted(monkeypatch):
    import audit

    def fake_query(method, params):
        if method == "health.summary":
            return {
                "proposals": 2,
                "drift_flags": 0,
                "atom_count": 10,
                "auto_filed": {
                    "count": 3,
                    "by_predicate": {"source.article": 2, "org.company": 1},
                    "items": [
                        {"hash": "z1", "entity": "zA", "predicate": "source.article", "source_uri": "file:///a", "tx_time": "2026-09-21T08:00:00Z", "kind": "auto_accept"},
                        {"hash": "z2", "entity": "zB", "predicate": "source.article", "source_uri": "file:///b", "tx_time": "2026-09-21T08:01:00Z", "kind": "auto_accept"},
                        {"hash": "z3", "entity": "zC", "predicate": "org.company", "source_uri": "file:///c", "tx_time": "2026-09-21T08:02:00Z", "kind": "auto_accept"},
                    ],
                },
            }
        raise audit.FfsSkillError("unexpected " + method)

    monkeypatch.setattr(audit, "query", fake_query)
    metrics = audit.aggregate_metrics()
    flags = audit.evaluate_flags(metrics)
    claim, panel = audit.build_claim(metrics, flags)
    assert claim["auto_filed"]["count"] == 3
    assert claim["auto_filed"]["by_predicate"] == {"source.article": 2, "org.company": 1}
    assert len(claim["auto_filed"]["items"]) == 3
    assert "auto-filed 3 item(s)" in claim["narrative"]
    # the five-item panel cap does not apply to the auto-filed list
    assert len(panel) <= 5


def test_auto_filed_section_absent_on_old_daemon(monkeypatch):
    import audit

    monkeypatch.setattr(audit, "query", lambda m, p: {"proposals": 0, "drift_flags": 0, "atom_count": 0})
    metrics = audit.aggregate_metrics()
    claim, _ = audit.build_claim(metrics, audit.evaluate_flags(metrics))
    assert claim["auto_filed"] == {"count": 0, "by_predicate": {}, "items": []}
    assert "auto-filed" not in claim["narrative"]
