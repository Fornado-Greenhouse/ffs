"""Courier: the deterministic, scheduled half of the newspaper goal
(task_40, ADR-035).

One tick reads the owner's own mailbox and the configured public
record or permitted feeds, and writes one ingest file per new item
per the article contract plus the day's digest note. Extraction is
the scribe's job, identity is the resolver's; the courier moves text
into a folder and says exactly what it did: "submitted" (a real run)
or "would submit" (a dry run). Never "filed".

What the courier may do with each publisher is the owner's policy in
``sources.toml``. There is no domain list in this bundle.

Invoke input: ``{"op": "tick", "dry_run": false, "ticks": ["mailbox",
"feeds"]}`` (all keys optional; default runs both) or ``{"op":
"status"}``.
"""

from __future__ import annotations

import importlib
import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir, "_lib"))
for p in (_HERE, _LIB):
    if p not in sys.path:
        sys.path.insert(0, p)

from ffs_skill import log, query, run  # noqa: E402

from config import ConfigError, CourierConfig, Sources, data_dir, load_courier, load_sources  # noqa: E402
from fetch import scheduled_fetch  # noqa: E402
from ledger import Ledger  # noqa: E402
from mailbox import default_imap_factory, mailbox_tick  # noqa: E402
from writer import Output  # noqa: E402

RESULT_KEYS = (
    "tick",
    "dry_run",
    "items_seen",
    "files_written",
    "files",
    "would_submit",
    "fetch_requests",
    "fetch_failures",
    "skipped_seen",
    "warnings",
    "last_error",
)


# Descriptive default User-Agent for feed requests. Public-record servers
# (the county FeatureServer answers 403 to Python's default agent) and
# the SEC fair-access policy both expect a client that names itself.
# An adapter that sets its own User-Agent (EDGAR's declared contact)
# wins; this is only the fallback.
COURIER_USER_AGENT = "FFS-courier/0.1 (personal use)"


def make_fetcher(user_agent: str):
    """A fetcher that names the courier as the owner phrased it in
    `[courier] user_agent`; per-request headers (EDGAR's declared
    contact) still win."""

    def fetcher(url: str, headers: Dict[str, str]):
        merged = {"User-Agent": user_agent or COURIER_USER_AGENT}
        merged.update(headers or {})
        req = urllib.request.Request(url, headers=merged)
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 - feed endpoints from the owner's config
                return resp.status, resp.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read() if hasattr(e, "read") else b""

    return fetcher


def default_fetcher(url: str, headers: Dict[str, str]):
    merged = {"User-Agent": COURIER_USER_AGENT}
    merged.update(headers or {})
    req = urllib.request.Request(url, headers=merged)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 - feed endpoints from the owner's config
            return int(resp.status), resp.read()
    except urllib.error.HTTPError as e:
        return int(e.code), b""
    except (urllib.error.URLError, OSError, ValueError):
        return 0, b""


def _public(result: Dict[str, Any]) -> Dict[str, Any]:
    return {k: result.get(k) for k in RESULT_KEYS}


def _dry_run_dir(base: str) -> str:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return os.path.join(base, "ingest", ".courier", "dry-run", stamp)


def feed_tick(
    feed: Any,
    cfg: CourierConfig,
    sources: Sources,
    ledger: Ledger,
    output: Output,
    fetcher=default_fetcher,
    ctx_extra: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    result: Dict[str, Any] = {
        "tick": f"feed:{feed.name}",
        "dry_run": output.dry_run,
        "items_seen": 0,
        "files_written": 0,
        "files": [],
        "would_submit": [],
        "fetch_requests": [],
        "fetch_failures": 0,
        "skipped_seen": 0,
        "warnings": [],
        "last_error": None,
    }
    try:
        mod = importlib.import_module(f"adapters.{feed.adapter}")
    except ImportError as e:
        result["last_error"] = f"adapter {feed.adapter}: {e}"
        return result
    since = ledger.watermark(feed.name, feed.since)
    ctx: Dict[str, Any] = {"edgar_user_agent": cfg.edgar_user_agent, "edgar_max_rps": cfg.edgar_max_rps}
    ctx.update(ctx_extra or {})
    pub = sources.get(feed.name) or sources.for_host(_host_of(feed.url or feed.endpoint))
    fetch_policy = pub.fetch if pub else "off"
    try:
        records = mod.tick(feed, since, fetcher, ctx)
    except Exception as e:  # noqa: BLE001
        result["last_error"] = f"{feed.adapter} tick: {e}"
        return result
    result["items_seen"] = len(records)
    newest = since
    refs = {}
    all_written = True
    for rec in records:
        try:
            item = mod.to_item(rec, feed, fetch_policy)
        except Exception as e:  # noqa: BLE001
            result["warnings"].append(f"{feed.adapter}: record {rec.get('id')!r} skipped: {e}")
            continue
        if not item.published_at:
            item.published_at = datetime.now(timezone.utc).date().isoformat()
        if ledger.has(item.url):
            result["skipped_seen"] += 1
            continue
        try:
            path = output.write_item(item)
        except OSError as e:
            all_written = False
            result["warnings"].append(f"write failed for {item.url}: {e}")
            continue
        result["files"].append(path)
        refs.setdefault((item.publication, item.published_at), []).append((item.basename, item.title))
        if not output.dry_run:
            ledger.add(item.url, item.published_at)
        if rec.get("date") and rec["date"] > newest:
            newest = rec["date"]
    for (pub_name, day), r in refs.items():
        path = output.write_or_append_digest(pub_name, day, r, cfg.digest_title_prefix)
        if path and path not in result["files"]:
            result["files"].append(path)
    if not output.dry_run:
        ledger.save()
        if all_written and newest:
            ledger.set_watermark(feed.name, newest)
    result["files_written"] = 0 if output.dry_run else len(result["files"])
    if output.dry_run:
        result["would_submit"] = list(result["files"])
    return result


def _host_of(url: str) -> str:
    from urllib.parse import urlsplit

    try:
        return urlsplit(url).hostname or ""
    except ValueError:
        return ""


def run_tick(
    inp: Dict[str, Any],
    *,
    base_dir: Optional[str] = None,
    cfg: Optional[CourierConfig] = None,
    sources: Optional[Sources] = None,
    imap_factory=default_imap_factory,
    fetcher=default_fetcher,
    substrate_query=None,
    fetch_opener=None,
) -> Dict[str, Any]:
    base = base_dir or data_dir()
    dry = bool(inp.get("dry_run")) or os.environ.get("FFS_COURIER_DRY_RUN") == "1"
    ticks = inp.get("ticks") or ["mailbox", "feeds"]
    try:
        cfg = cfg or load_courier(base)
        if fetcher is default_fetcher and getattr(cfg, "user_agent", ""):
            fetcher = make_fetcher(cfg.user_agent)
        sources = sources or load_sources(base)
    except ConfigError as e:
        return {"results": [], "last_error": str(e), "dry_run": dry}
    ledger = Ledger(base)
    output = Output(os.path.join(base, "ingest"), _dry_run_dir(base) if dry else None)
    results: List[Dict[str, Any]] = []
    if "mailbox" in ticks and cfg.mailbox is not None:
        r = mailbox_tick(cfg, sources, ledger, output, imap_factory=imap_factory, query=substrate_query)
        new_items = r.pop("new_items", [])
        if new_items:
            kwargs = {"opener": fetch_opener} if fetch_opener else {}
            fr = scheduled_fetch(new_items, sources, ledger, output, **kwargs)
            r["fetch_requests"] = fr["fetch_requests"]
            r["fetch_failures"] = fr["fetch_failures"]
            r["warnings"].extend(fr["warnings"])
        results.append(_public(r))
        if not dry:
            ledger.write_last_run(r)
    if "feeds" in ticks:
        for feed in cfg.feeds:
            r = feed_tick(feed, cfg, sources, ledger, output, fetcher=fetcher)
            results.append(_public(r))
            if not dry:
                ledger.write_last_run(r)
    verb = "would submit" if dry else "submitted"
    total = sum(len(r["files"]) for r in results)
    log("info", f"courier: {verb} {total} file(s) across {len(results)} tick(s)")
    return {"results": results, "dry_run": dry, "last_error": next((r["last_error"] for r in results if r["last_error"]), None)}


def status(base_dir: Optional[str] = None) -> Dict[str, Any]:
    return Ledger(base_dir or data_dir()).last_run()


def handle(inp: Any) -> Dict[str, Any]:
    if not isinstance(inp, dict):
        inp = {}
    op = inp.get("op", "tick")
    if op == "status":
        return status()
    return run_tick(inp, substrate_query=_host_query)


def _host_query(method: str, params: Dict[str, Any]) -> Any:
    return query(method, params)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] in ("tick", "run", "status"):
        # Standalone invocation (no host): `python courier.py run [--dry-run]`
        if sys.argv[1] == "status":
            print(json.dumps(status(), indent=1))
        else:
            dry = "--dry-run" in sys.argv[2:]
            print(json.dumps(run_tick({"dry_run": dry}), indent=1))
    else:
        run(handle)
