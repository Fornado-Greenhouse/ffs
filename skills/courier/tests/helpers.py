"""Test helpers: config text, FakeImap loading, fake fetchers."""

from __future__ import annotations

import os
from typing import Any, Dict, List, Tuple

from config import parse_courier, parse_sources
from mailbox import FakeImap

FIXTURES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")

COURIER_TOML = """
[mailbox]
host = "imap.example.test"
port = 993
folder = "INBOX"
auth = "app_password"
user = "owner@example.test"
mark_seen = true

[[mailbox.source]]
sender = "reply@news.example-ledger.test"
publisher = "example_ledger"

[[mailbox.source]]
sender = "metromorning.test"
publisher = "metro_morning"

[[tracking.wrapper]]
host_pattern = "go.example-ledger.test"
kind = "base64_path_segment"
segment_index = 2

[edgar]
user_agent = "Test Owner test@example.test"
max_rps = 8
"""

SOURCES_TOML = """
[[publisher]]
key = "example_ledger"
name = "Example Ledger"
domains = ["example-ledger.test"]
intake = "pointer"
fetch = "session"

[[publisher]]
key = "metro_morning"
name = "Metro Morning"
domains = ["metromorning.test"]
intake = "clip"
fetch = "off"
"""


def courier_cfg(text: str = COURIER_TOML, env: Dict[str, str] | None = None):
    return parse_courier(text, env or {})


def sources_cfg(text: str = SOURCES_TOML):
    return parse_sources(text)


def load_eml(*names: str) -> List[bytes]:
    out = []
    for n in names:
        with open(os.path.join(FIXTURES, n), "rb") as f:
            out.append(f.read())
    return out


def imap_factory_for(*names: str):
    fake = FakeImap(load_eml(*names))

    def factory(_mb):
        return fake

    factory.fake = fake  # type: ignore[attr-defined]
    return factory


def fetcher_from(table: Dict[str, Tuple[int, bytes]]):
    """A fetcher that matches by url prefix and records requests."""
    calls: List[Tuple[str, Dict[str, str]]] = []

    def fetcher(url: str, headers: Dict[str, str]) -> Tuple[int, bytes]:
        calls.append((url, dict(headers)))
        for prefix, resp in table.items():
            if url.startswith(prefix):
                return resp
        return 404, b""

    fetcher.calls = calls  # type: ignore[attr-defined]
    return fetcher


def read_fixture(name: str) -> bytes:
    with open(os.path.join(FIXTURES, name), "rb") as f:
        return f.read()
