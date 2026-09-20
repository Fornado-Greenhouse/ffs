"""Courier configuration (task_40).

Two owner-edited TOML files under ``$FFS_DATA_DIR/config/``:

- ``courier.toml``: the mailbox, the mailbox sources, the feeds, the
  EDGAR declaration, output options, and the tracking-wrapper table.
- ``sources.toml``: per-publisher policy, ``intake`` and ``fetch``
  (ADR-035 as amended 2026-09-15). The loader validates the enums and
  rejects unknown keys. It carries no list of publisher domains of its
  own: the owner's values are applied as written.

Secrets never live in TOML. Any key named ``password``, ``token``,
``secret``, or ``cookie`` (at any depth) is refused with a clear
error; the mailbox secret comes from the OS keychain (service
``ffs.courier.<host>``) or ``FFS_COURIER_MAIL_PASSWORD`` for tests.
"""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass, field
from typing import Any, Dict, List, Mapping, Optional


class ConfigError(ValueError):
    """A configuration file is missing, malformed, or unsafe."""


INTAKE_VALUES = ("pointer", "clip")
FETCH_VALUES = ("off", "session", "scheduled")
AUTH_VALUES = ("app_password", "oauth")
ADAPTERS = ("meck_permits", "charlotte_legistar", "sec_edgar", "rss")
_SECRET_KEY_WORDS = ("password", "token", "secret", "cookie")

EDGAR_MAX_RPS_CEILING = 10
EDGAR_DEFAULT_RPS = 8


def data_dir(env: Optional[Mapping[str, str]] = None) -> str:
    e = env if env is not None else os.environ
    return e.get("FFS_DATA_DIR") or os.path.expanduser("~/.ffs")


@dataclass
class Publisher:
    key: str
    name: str
    domains: List[str]
    intake: str = "pointer"
    fetch: str = "session"
    daily_cap: int = 20
    min_pause_seconds: int = 8


@dataclass
class Sources:
    publishers: Dict[str, Publisher] = field(default_factory=dict)

    def get(self, key: str) -> Optional[Publisher]:
        return self.publishers.get(key)

    def for_host(self, host: str) -> Optional[Publisher]:
        h = host.lower()
        for p in self.publishers.values():
            for d in p.domains:
                d = d.lower()
                if h == d or h.endswith("." + d):
                    return p
        return None


@dataclass
class MailboxSource:
    sender: str
    publisher: str
    subject_filter: Optional[str] = None
    # Regex an item's URL path must match to count as an article. When
    # unset, a generic shape test applies (a year segment, an .html
    # suffix, or a deep path), which drops account and index links.
    path_filter: Optional[str] = None


@dataclass
class Mailbox:
    host: str
    port: int = 993
    folder: str = "INBOX"
    auth: str = "app_password"
    user: str = ""
    mark_seen: bool = True
    since: Optional[str] = None
    sources: List[MailboxSource] = field(default_factory=list)


@dataclass
class Feed:
    name: str
    adapter: str
    publication: str
    endpoint: str = ""
    url: str = ""
    since: str = ""
    filters: Dict[str, Any] = field(default_factory=dict)


@dataclass
class CourierConfig:
    mailbox: Optional[Mailbox]
    feeds: List[Feed]
    edgar_user_agent: str
    edgar_max_rps: int
    digest_title_prefix: str
    wrappers: List[Dict[str, Any]]
    body_limit: int = 4000


def _refuse_secrets(obj: Any, path: str = "") -> None:
    if isinstance(obj, dict):
        for k, v in obj.items():
            lk = str(k).lower()
            if any(w in lk for w in _SECRET_KEY_WORDS):
                raise ConfigError(
                    f"refusing secret-looking key {path + str(k)!r} in courier.toml; "
                    "secrets live in the OS keychain (service ffs.courier.<host>) or "
                    "FFS_COURIER_MAIL_PASSWORD for tests, never in config"
                )
            _refuse_secrets(v, path + str(k) + ".")
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            _refuse_secrets(v, f"{path}[{i}].")


def _check_keys(table: Mapping[str, Any], allowed: set, where: str) -> None:
    unknown = sorted(set(table.keys()) - allowed)
    if unknown:
        raise ConfigError(f"unknown key(s) {unknown} in {where}; allowed: {sorted(allowed)}")


def _enum(value: Any, allowed: tuple, where: str) -> str:
    if value not in allowed:
        raise ConfigError(f"{where} must be one of {list(allowed)}, got {value!r}")
    return str(value)


def parse_sources(text: str) -> Sources:
    try:
        raw = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        raise ConfigError(f"sources.toml: {e}") from e
    _check_keys(raw, {"publisher"}, "sources.toml")
    out = Sources()
    for i, p in enumerate(raw.get("publisher", []) or []):
        where = f"sources.toml [[publisher]] #{i}"
        _check_keys(p, {"key", "name", "domains", "intake", "fetch", "daily_cap", "min_pause_seconds"}, where)
        key = str(p.get("key") or "").strip()
        if not key:
            raise ConfigError(f"{where}: key is required")
        if key in out.publishers:
            raise ConfigError(f"{where}: duplicate key {key!r}")
        domains = p.get("domains") or []
        if not isinstance(domains, list) or not all(isinstance(d, str) for d in domains):
            raise ConfigError(f"{where}: domains must be a list of strings")
        pub = Publisher(
            key=key,
            name=str(p.get("name") or key),
            domains=[d.lower().strip() for d in domains],
            intake=_enum(p.get("intake", "pointer"), INTAKE_VALUES, f"{where} intake"),
            fetch=_enum(p.get("fetch", "session"), FETCH_VALUES, f"{where} fetch"),
            daily_cap=int(p.get("daily_cap", 20)),
            min_pause_seconds=int(p.get("min_pause_seconds", 8)),
        )
        if pub.daily_cap < 0 or pub.min_pause_seconds < 0:
            raise ConfigError(f"{where}: daily_cap and min_pause_seconds must be non-negative")
        out.publishers[key] = pub
    return out


def load_sources(dir_: Optional[str] = None, env: Optional[Mapping[str, str]] = None) -> Sources:
    base = dir_ or data_dir(env)
    path = os.path.join(base, "config", "sources.toml")
    if not os.path.exists(path):
        return Sources()
    with open(path, encoding="utf-8") as f:
        return parse_sources(f.read())


def parse_courier(text: str, env: Optional[Mapping[str, str]] = None) -> CourierConfig:
    e = env if env is not None else os.environ
    try:
        raw = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e2:
        raise ConfigError(f"courier.toml: {e2}") from e2
    _refuse_secrets(raw)
    _check_keys(raw, {"mailbox", "feed", "edgar", "output", "tracking", "scribe"}, "courier.toml")

    mailbox: Optional[Mailbox] = None
    mb = raw.get("mailbox")
    if mb:
        _check_keys(mb, {"host", "port", "folder", "auth", "user", "mark_seen", "since", "source"}, "[mailbox]")
        sources: List[MailboxSource] = []
        for i, s in enumerate(mb.get("source", []) or []):
            _check_keys(s, {"sender", "subject_filter", "publisher", "path_filter"}, f"[[mailbox.source]] #{i}")
            if not s.get("sender") or not s.get("publisher"):
                raise ConfigError(f"[[mailbox.source]] #{i}: sender and publisher are required")
            sources.append(
                MailboxSource(
                    sender=str(s["sender"]).lower(),
                    publisher=str(s["publisher"]),
                    subject_filter=s.get("subject_filter") or None,
                    path_filter=s.get("path_filter") or None,
                )
            )
        mailbox = Mailbox(
            host=e.get("FFS_COURIER_MAIL_HOST") or str(mb.get("host") or ""),
            port=int(e.get("FFS_COURIER_MAIL_PORT") or mb.get("port", 993)),
            folder=e.get("FFS_COURIER_MAIL_FOLDER") or str(mb.get("folder", "INBOX")),
            auth=_enum(mb.get("auth", "app_password"), AUTH_VALUES, "[mailbox] auth"),
            user=e.get("FFS_COURIER_MAIL_USER") or str(mb.get("user") or ""),
            mark_seen=bool(mb.get("mark_seen", True)),
            since=mb.get("since"),
            sources=sources,
        )
        if not mailbox.host:
            raise ConfigError("[mailbox] host is required")

    feeds: List[Feed] = []
    for i, f in enumerate(raw.get("feed", []) or []):
        _check_keys(f, {"name", "adapter", "publication", "endpoint", "url", "since", "filters"}, f"[[feed]] #{i}")
        adapter = _enum(f.get("adapter"), ADAPTERS, f"[[feed]] #{i} adapter")
        feeds.append(
            Feed(
                name=str(f.get("name") or f"{adapter}-{i}"),
                adapter=adapter,
                publication=str(f.get("publication") or adapter),
                endpoint=str(f.get("endpoint") or ""),
                url=str(f.get("url") or ""),
                since=str(f.get("since") or ""),
                filters=dict(f.get("filters") or {}),
            )
        )

    edgar = raw.get("edgar") or {}
    _check_keys(edgar, {"user_agent", "max_rps"}, "[edgar]")
    ua = e.get("FFS_COURIER_EDGAR_USER_AGENT") or str(edgar.get("user_agent") or "")
    if any(f.adapter == "sec_edgar" for f in feeds) and not ua:
        raise ConfigError("[edgar] user_agent is required when a sec_edgar feed is configured (SEC fair-access policy)")
    max_rps = int(edgar.get("max_rps", EDGAR_DEFAULT_RPS))
    if max_rps < 1 or max_rps > EDGAR_MAX_RPS_CEILING:
        raise ConfigError(f"[edgar] max_rps must be between 1 and {EDGAR_MAX_RPS_CEILING}, got {max_rps}")

    output = raw.get("output") or {}
    _check_keys(output, {"digest_title_prefix"}, "[output]")
    tracking = raw.get("tracking") or {}
    _check_keys(tracking, {"wrapper"}, "[tracking]")
    wrappers: List[Dict[str, Any]] = []
    for i, w in enumerate(tracking.get("wrapper", []) or []):
        _check_keys(w, {"host_pattern", "kind", "segment_index"}, f"[[tracking.wrapper]] #{i}")
        if w.get("kind") != "base64_path_segment":
            raise ConfigError(f"[[tracking.wrapper]] #{i}: kind must be 'base64_path_segment'")
        wrappers.append({"host_pattern": str(w["host_pattern"]).lower(), "kind": "base64_path_segment", "segment_index": int(w.get("segment_index", 0))})
    scribe = raw.get("scribe") or {}
    _check_keys(scribe, {"body_limit"}, "[scribe]")

    return CourierConfig(
        mailbox=mailbox,
        feeds=feeds,
        edgar_user_agent=ua,
        edgar_max_rps=max_rps,
        digest_title_prefix=str(output.get("digest_title_prefix") or ""),
        wrappers=wrappers,
        body_limit=int(scribe.get("body_limit", 4000)),
    )


def load_courier(dir_: Optional[str] = None, env: Optional[Mapping[str, str]] = None) -> CourierConfig:
    base = dir_ or data_dir(env)
    path = os.path.join(base, "config", "courier.toml")
    if not os.path.exists(path):
        raise ConfigError(f"courier.toml not found at {path}; the installer seeds it from starter/config/courier.toml")
    with open(path, encoding="utf-8") as f:
        return parse_courier(f.read(), env)


def mailbox_secret(host: str, env: Optional[Mapping[str, str]] = None) -> Optional[str]:
    """The mailbox password: env for tests, else the OS keychain."""
    e = env if env is not None else os.environ
    if e.get("FFS_COURIER_MAIL_PASSWORD"):
        return e["FFS_COURIER_MAIL_PASSWORD"]
    try:
        from ffs_skill import keychain_secret  # type: ignore
    except Exception:  # noqa: BLE001
        return None
    return keychain_secret(f"ffs.courier.{host}")
