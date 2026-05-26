"""FFS skill author-side helper.

This module hides the line-delimited JSON stdio protocol the FFS
skills host speaks so skill authors can focus on business logic.

Typical use::

    from ffs_skill import run, query, log

    def handle(inp):
        # `inp` is the JSON `input` the host sent in an `invoke` frame.
        atom = query("atom.get", {"hash": inp["target"]})
        log("info", f"loaded atom for {inp['target']}")
        return {"summary": atom["claim"]}

    if __name__ == "__main__":
        run(handle)

The wire protocol is documented in
``crates/ffs-skills-host/src/protocol.rs``. In short:

- The host sends ``{"kind": "invoke", "id": "...", "input": ...}``
  frames; ``run()`` calls ``handle`` and writes back
  ``{"kind": "result", "id": "...", "output": ...}``.
- The skill may emit ``{"kind": "query", "id": "...",
  "method": "atom.get", "params": ...}`` frames during ``handle``;
  the host replies with ``{"kind": "query_response", "id": "...",
  "result": ...}`` or ``{"kind": "query_error", ...}``.
- The host may send ``{"kind": "shutdown"}`` at any time. ``run()``
  exits cleanly on receipt.

The helper is intentionally synchronous: skill business logic runs
serially per skill. The host enforces a per-call timeout (default 30s
per ``SKILL.md`` ``timeout_ms``) — long-running skills should structure
work as multiple smaller invocations.
"""

from __future__ import annotations

import json
import sys
import threading
import uuid
from typing import Any, Callable, Dict, Optional

# Outbound writes (stdout) are serialized across the helper to keep
# query responses, result frames, and log frames from interleaving.
_write_lock = threading.Lock()

# Pending substrate queries: id -> condition + slot.
_pending: Dict[str, "_QuerySlot"] = {}
_pending_lock = threading.Lock()


class _QuerySlot:
    __slots__ = ("event", "ok", "value", "error")

    def __init__(self) -> None:
        self.event = threading.Event()
        self.ok = False
        self.value: Any = None
        self.error: Optional[str] = None


class FfsSkillError(Exception):
    """Raised when the host rejects a substrate query."""


def _write_frame(obj: Dict[str, Any]) -> None:
    line = json.dumps(obj, separators=(",", ":"))
    with _write_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def log(level: str, message: str) -> None:
    """Forward a log line to the host's tracing pipeline.

    `level` is a free-form string (`"info"`, `"warn"`, etc.); the host
    treats it as a label. The line bypasses the result stream so debug
    output never collides with invocation responses.
    """
    _write_frame({"kind": "log", "level": level, "message": message})


def query(method: str, params: Any) -> Any:
    """Issue a substrate-access query to the host.

    Blocks until the host responds. Raises ``FfsSkillError`` if the
    host rejects the call (e.g., capability-denied or
    ``method`` is not recognized).
    """
    qid = "q-" + uuid.uuid4().hex[:12]
    slot = _QuerySlot()
    with _pending_lock:
        _pending[qid] = slot
    _write_frame({"kind": "query", "id": qid, "method": method, "params": params})
    slot.event.wait()
    with _pending_lock:
        _pending.pop(qid, None)
    if slot.ok:
        return slot.value
    raise FfsSkillError(slot.error or "<no error message>")


_shutdown_event = threading.Event()


def _reader_loop(handler: Callable[[Any], Any]) -> None:
    """Run on a dedicated thread: read frames from stdin and dispatch.

    Invocations run on per-invocation worker threads so that
    ``query()`` calls made from inside ``handler`` can be answered by
    the next ``query_response`` frame this reader receives.
    """
    while True:
        raw = sys.stdin.readline()
        if not raw:
            _shutdown_event.set()
            return
        line = raw.rstrip("\n")
        if not line:
            continue
        try:
            frame = json.loads(line)
        except json.JSONDecodeError as e:
            log("warn", f"bad frame from host: {e}")
            continue
        kind = frame.get("kind")
        if kind == "invoke":
            inv_id = frame.get("id")
            inp = frame.get("input")
            threading.Thread(
                target=_run_invocation,
                args=(handler, inv_id, inp),
                daemon=True,
            ).start()
        elif kind == "query_response":
            qid = frame.get("id")
            with _pending_lock:
                slot = _pending.get(qid)
            if slot is not None:
                slot.ok = True
                slot.value = frame.get("result")
                slot.event.set()
        elif kind == "query_error":
            qid = frame.get("id")
            with _pending_lock:
                slot = _pending.get(qid)
            if slot is not None:
                slot.ok = False
                slot.error = frame.get("error")
                slot.event.set()
        elif kind == "shutdown":
            _shutdown_event.set()
            return
        else:
            log("warn", f"unknown frame kind: {kind!r}")


def _run_invocation(handler: Callable[[Any], Any], inv_id: Any, inp: Any) -> None:
    try:
        output = handler(inp)
    except Exception as e:  # noqa: BLE001 — surface anything as a skill error.
        _write_frame(
            {"kind": "error", "id": inv_id, "error": f"{type(e).__name__}: {e}"}
        )
        return
    _write_frame({"kind": "result", "id": inv_id, "output": output})


def run(handler: Callable[[Any], Any]) -> None:
    """Read invocation frames from stdin and dispatch them to ``handler``.

    ``handler`` receives the JSON ``input`` payload and returns the
    JSON-serializable ``output``. Exceptions from ``handler`` are
    surfaced to the host as ``{"kind": "error", "error": "..."}``.

    Returns when the host sends a ``shutdown`` frame or closes stdin.

    The reader runs on a dedicated thread so ``query()`` calls from
    inside ``handler`` can wait for their response frame without
    deadlocking the stdin reader.
    """
    reader = threading.Thread(target=_reader_loop, args=(handler,), daemon=True)
    reader.start()
    _shutdown_event.wait()
