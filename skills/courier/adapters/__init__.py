"""Feed adapters (task_40). Each exposes ``tick(feed, since, fetcher,
ctx) -> list[Record]`` and ``to_item(record, feed) -> Item``. Records
are plain dicts with at least ``id``, ``date`` (YYYY-MM-DD), ``title``,
``url``, ``summary``; adapters may pre-fill ``mentions`` and ``events``
from structured fields, tagged ``courier-structured``.

``fetcher(url, headers) -> (status, bytes)`` is injected so tests use
recorded public-record fixtures and never touch the network.
"""

from __future__ import annotations

from typing import Any, Callable, Dict, Tuple

Fetcher = Callable[[str, Dict[str, str]], Tuple[int, bytes]]


def fetch_json(fetcher: Fetcher, url: str, headers: Dict[str, str]) -> Any:
    import json

    status, body = fetcher(url, headers)
    if status != 200:
        raise RuntimeError(f"HTTP {status} from {url}")
    return json.loads(body.decode("utf-8"))
