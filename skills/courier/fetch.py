"""Scheduled fetch (task_40 subtask 40.11), for publishers the owner
set to ``fetch = "scheduled"`` in ``sources.toml``.

Rules, enforced in code: only URLs that came from this tick's own
digest items are ever requested (never a listing, index, search, or
section page; never a link found inside a fetched page); one request
per url; a randomized pause of at least ``min_pause_seconds`` between
requests; at most ``daily_cap`` per publisher per day; every request
is logged in the tick result; on 200 the item's file is rewritten as
a clip with a ``content_hash``; on any other status the pointer stays
and ``fetch_failures`` counts it. Cookies come from the OS keychain
(service ``ffs.courier.cookies.<domain>``, Netscape cookie-jar text)
or ``FFS_COURIER_COOKIES_<DOMAIN>`` for tests. There is no code path
that bypasses a login, a paywall, or a bot wall.

The starter ``sources.toml`` leaves the terms-restricted publishers at
``fetch = "session"``, so this path never runs for them unless the
owner changes the setting.
"""

from __future__ import annotations

import os
import random
import re
import time
import urllib.error
import urllib.request
from html.parser import HTMLParser
from typing import Any, Callable, Dict, List, Optional, Tuple
from urllib.parse import urlsplit

import urlnorm  # skills/_lib

from config import Sources
from ledger import Ledger
from writer import Item, Output

Opener = Callable[[urllib.request.Request, float], Tuple[int, bytes]]

USER_AGENT = "Mozilla/5.0 (Macintosh) FFS-courier/0.1 (owner-present policy; one request per digest item)"


def default_opener(req: urllib.request.Request, timeout: float) -> Tuple[int, bytes]:
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 - url comes from the owner's digest
            return int(resp.status), resp.read()
    except urllib.error.HTTPError as e:
        return int(e.code), b""
    except (urllib.error.URLError, OSError, ValueError):
        return 0, b""


def cookies_for(domain: str) -> Optional[str]:
    env_key = "FFS_COURIER_COOKIES_" + re.sub(r"[^A-Za-z0-9]+", "_", domain).upper()
    val = os.environ.get(env_key)
    if val:
        return val
    try:
        from ffs_skill import keychain_secret  # type: ignore
    except Exception:  # noqa: BLE001
        return None
    return keychain_secret(f"ffs.courier.cookies.{domain}")


def cookie_header(jar_text: str, host: str) -> str:
    """Netscape cookie-jar lines to a Cookie header for `host`."""
    pairs = []
    for line in jar_text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) < 7:
            continue
        cdomain = cols[0].lstrip(".").lower()
        if host == cdomain or host.endswith("." + cdomain):
            pairs.append(f"{cols[5]}={cols[6]}")
    return "; ".join(pairs)


class _ArticleText(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.title = ""
        self._in_title = False
        self._in_article = 0
        self._skip = 0
        self.article_paras: List[str] = []
        self.all_paras: List[str] = []
        self._buf: List[str] = []
        self._in_p = False

    def handle_starttag(self, tag: str, attrs: Any) -> None:
        if tag in ("script", "style", "nav", "footer", "header"):
            self._skip += 1
        elif tag == "title":
            self._in_title = True
        elif tag == "article":
            self._in_article += 1
        elif tag == "p":
            self._in_p = True
            self._buf = []

    def handle_endtag(self, tag: str) -> None:
        if tag in ("script", "style", "nav", "footer", "header"):
            self._skip = max(0, self._skip - 1)
        elif tag == "title":
            self._in_title = False
        elif tag == "article":
            self._in_article = max(0, self._in_article - 1)
        elif tag == "p" and self._in_p:
            text = " ".join("".join(self._buf).split())
            if text:
                (self.article_paras if self._in_article else self.all_paras).append(text)
            self._in_p = False

    def handle_data(self, data: str) -> None:
        if self._skip:
            return
        if self._in_title:
            self.title += data
        if self._in_p:
            self._buf.append(data)


def extract_article_text(html: str) -> Tuple[str, str]:
    p = _ArticleText()
    p.feed(html)
    p.close()
    paras = p.article_paras or p.all_paras
    return " ".join(p.title.split()), "\n\n".join(paras)


def scheduled_fetch(
    items: List[Item],
    sources: Sources,
    ledger: Ledger,
    output: Output,
    opener: Opener = default_opener,
    sleep: Callable[[float], None] = time.sleep,
    rand: Callable[[], float] = random.random,
    today: Optional[str] = None,
) -> Dict[str, Any]:
    """Fetch the scheduled publishers' items from this tick. Returns
    {fetch_requests, fetch_failures, clipped, warnings}."""
    res: Dict[str, Any] = {"fetch_requests": [], "fetch_failures": 0, "clipped": [], "warnings": []}
    allowed_urls = {i.url for i in items}
    first = True
    for item in items:
        pub = sources.for_host(urlsplit(item.url).hostname or "")
        if pub is None or pub.fetch != "scheduled" or item.intake == "clip":
            continue
        assert item.url in allowed_urls, "scheduled fetch may only request a url from this tick's digest"
        if ledger.fetch_count(pub.key, today) >= pub.daily_cap:
            res["warnings"].append(f"daily_cap {pub.daily_cap} reached for {pub.key}; {item.url} stays a pointer")
            continue
        if not first:
            sleep(pub.min_pause_seconds + rand() * pub.min_pause_seconds)
        first = False
        host = urlsplit(item.url).hostname or ""
        headers = {"User-Agent": USER_AGENT, "Accept": "text/html"}
        jar = cookies_for(host) or next((cookies_for(d) for d in pub.domains if cookies_for(d)), None)
        if jar:
            ck = cookie_header(jar, host)
            if ck:
                headers["Cookie"] = ck
        req = urllib.request.Request(item.url, headers=headers)
        status, body = opener(req, 20.0)
        if not output.dry_run:
            ledger.bump_fetch(pub.key, today)
        res["fetch_requests"].append({"url": item.url, "status": status})
        if status != 200 or not body:
            res["fetch_failures"] += 1
            continue
        title, text = extract_article_text(body.decode("utf-8", errors="replace"))
        if not text:
            res["fetch_failures"] += 1
            res["warnings"].append(f"no article text extracted from {item.url}; pointer kept")
            continue
        item.intake = "clip"
        item.body = text
        item.content_hash = urlnorm.content_hash_multibase(body)
        output.write_item(item)
        res["clipped"].append(item.url)
    return res
