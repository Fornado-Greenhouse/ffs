"""Unit tests for the librarian's scheduler logic.

The tests stub the `query()` callback so they exercise the librarian's
dispatch (refresh / evict / tick) without spinning up the daemon.
Drift detection and eviction *correctness* are tested Rust-side in
`crates/ffs-core/src/working_set.rs` and
`crates/ffs-daemon/tests/librarian_integration.rs`; these tests focus
on the Python tick loop's behavior.
"""

from __future__ import annotations

from typing import Any, Dict, List

import watcher  # type: ignore  # provided by conftest sys.path bootstrap.


class _Recorder:
    """Records every `query()` call so tests can assert ordering."""

    def __init__(self, responses: Dict[str, Any]) -> None:
        self.responses = responses
        self.calls: List[Dict[str, Any]] = []

    def __call__(self, method: str, params: Any) -> Any:
        self.calls.append({"method": method, "params": params})
        return self.responses.get(method, {})


def _install(monkeypatch, recorder: _Recorder) -> None:
    monkeypatch.setattr(watcher, "query", recorder)


def test_tick_refreshes_then_evicts_in_order(monkeypatch):
    rec = _Recorder(
        {
            "working_set.refresh_drifted": {
                "refreshed": [
                    {"path": "contacts/by-name/S/Sara.md", "render_hash": "h1", "markdown": "..."},
                ]
            },
            "working_set.evict_to_cap": {"evicted": ["contacts/by-name/B/Bob.md"]},
        }
    )
    _install(monkeypatch, rec)
    result = watcher.tick(cap=100)
    assert result == {
        "refreshed": ["contacts/by-name/S/Sara.md"],
        "evicted": ["contacts/by-name/B/Bob.md"],
    }
    # Refresh before evict.
    assert [c["method"] for c in rec.calls] == [
        "working_set.refresh_drifted",
        "working_set.evict_to_cap",
    ]
    assert rec.calls[1]["params"] == {"cap": 100}


def test_handle_dispatches_op_tick_with_default_cap(monkeypatch):
    rec = _Recorder({
        "working_set.refresh_drifted": {"refreshed": []},
        "working_set.evict_to_cap": {"evicted": []},
    })
    _install(monkeypatch, rec)
    result = watcher.handle({"op": "tick"})
    assert result == {"refreshed": [], "evicted": []}
    # Default cap is wired through to the daemon call.
    assert rec.calls[-1]["params"] == {"cap": watcher.DEFAULT_CAP}


def test_handle_dispatches_op_refresh_only(monkeypatch):
    rec = _Recorder({
        "working_set.refresh_drifted": {
            "refreshed": [{"path": "x.md", "render_hash": "h", "markdown": ""}]
        },
    })
    _install(monkeypatch, rec)
    result = watcher.handle({"op": "refresh"})
    assert result == {"refreshed": ["x.md"]}
    # No evict call.
    assert all(c["method"] != "working_set.evict_to_cap" for c in rec.calls)


def test_handle_dispatches_op_evict_only(monkeypatch):
    rec = _Recorder({"working_set.evict_to_cap": {"evicted": ["oldest.md"]}})
    _install(monkeypatch, rec)
    result = watcher.handle({"op": "evict", "cap": 5})
    assert result == {"evicted": ["oldest.md"]}
    assert all(c["method"] != "working_set.refresh_drifted" for c in rec.calls)
    assert rec.calls[-1]["params"] == {"cap": 5}


def test_unknown_op_defaults_to_tick(monkeypatch):
    rec = _Recorder({
        "working_set.refresh_drifted": {"refreshed": []},
        "working_set.evict_to_cap": {"evicted": []},
    })
    _install(monkeypatch, rec)
    result = watcher.handle({"op": "no-such-thing"})
    # Behaves like tick.
    assert result == {"refreshed": [], "evicted": []}
    assert [c["method"] for c in rec.calls] == [
        "working_set.refresh_drifted",
        "working_set.evict_to_cap",
    ]


def test_missing_input_treated_as_default_tick(monkeypatch):
    rec = _Recorder({
        "working_set.refresh_drifted": {"refreshed": []},
        "working_set.evict_to_cap": {"evicted": []},
    })
    _install(monkeypatch, rec)
    # handle(None) is what the helper passes when the host's
    # invoke frame omits `input`.
    result = watcher.handle(None)
    assert result == {"refreshed": [], "evicted": []}


def test_refresh_swallows_host_error_and_returns_empty(monkeypatch):
    """A failing host call should not propagate as an exception —
    the librarian tolerates a transient daemon issue and retries on
    the next tick.
    """
    from ffs_skill import FfsSkillError  # type: ignore

    def boom(method: str, params: Any) -> Any:
        raise FfsSkillError("transient: capability denied")

    monkeypatch.setattr(watcher, "query", boom)
    assert watcher.refresh_drifted() == []
    assert watcher.evict_to_cap(10) == []
