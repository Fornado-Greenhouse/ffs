"""The courier's local memory (task_40).

- ``$FFS_DATA_DIR/ingest/.courier/seen.json``: normalized url -> first
  seen date. The substrate is the other half of dedup (an
  ``entity.search`` for the url); the ledger keeps re-runs cheap and
  works when the host is absent.
- ``$FFS_DATA_DIR/ingest/.courier/last_run.json``: what the daemon's
  ``health.summary.courier`` reads.
- per-publisher daily fetch counts for the scheduled-fetch cap.
- per-feed watermarks.

Dry runs never touch any of these.
"""

from __future__ import annotations

import json
import os
import tempfile
from datetime import date, datetime, timezone
from typing import Any, Dict, Optional


def _atomic_write(path: str, obj: Any) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".tmp-", suffix=".json")
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(obj, f, indent=1, sort_keys=True)
        f.write("\n")
    os.replace(tmp, path)


def _load(path: str) -> Dict[str, Any]:
    if not os.path.exists(path):
        return {}
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


class Ledger:
    def __init__(self, data_dir: str) -> None:
        self.dir = os.path.join(data_dir, "ingest", ".courier")
        self.seen_path = os.path.join(self.dir, "seen.json")
        self.last_run_path = os.path.join(self.dir, "last_run.json")
        self.fetch_path = os.path.join(self.dir, "fetch_counts.json")
        self.watermarks_path = os.path.join(self.dir, "watermarks.json")
        self._seen = _load(self.seen_path)

    # -- seen urls --
    def has(self, url: str) -> bool:
        return url in self._seen

    def add(self, url: str, when: Optional[str] = None) -> None:
        self._seen.setdefault(url, when or date.today().isoformat())

    def save(self) -> None:
        _atomic_write(self.seen_path, self._seen)

    def __len__(self) -> int:
        return len(self._seen)

    # -- last run --
    def write_last_run(self, result: Dict[str, Any]) -> None:
        _atomic_write(
            self.last_run_path,
            {
                "last_run": datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
                "tick": result.get("tick"),
                "items_seen": int(result.get("items_seen", 0)),
                "files_written": int(result.get("files_written", 0)),
                "fetch_failures": int(result.get("fetch_failures", 0)),
                "last_error": result.get("last_error"),
            },
        )

    def last_run(self) -> Dict[str, Any]:
        return _load(self.last_run_path)

    # -- scheduled fetch counts --
    def fetch_count(self, publisher: str, day: Optional[str] = None) -> int:
        counts = _load(self.fetch_path)
        return int(counts.get(day or date.today().isoformat(), {}).get(publisher, 0))

    def bump_fetch(self, publisher: str, day: Optional[str] = None) -> int:
        counts = _load(self.fetch_path)
        d = day or date.today().isoformat()
        counts.setdefault(d, {})
        counts[d][publisher] = int(counts[d].get(publisher, 0)) + 1
        _atomic_write(self.fetch_path, counts)
        return counts[d][publisher]

    # -- feed watermarks --
    def watermark(self, feed: str, default: str = "") -> str:
        return str(_load(self.watermarks_path).get(feed, default))

    def set_watermark(self, feed: str, value: str) -> None:
        wm = _load(self.watermarks_path)
        wm[feed] = value
        _atomic_write(self.watermarks_path, wm)
