"""SEC EDGAR full-text search with a declared User-Agent and a
token-bucket limiter at ``max_rps`` (ceiling 10, per the SEC
fair-access policy). One search query per tick; no crawling beyond
the query results. An 8-K with item 5.02 (officer changes) yields a
courier-structured mention for the officer with the company as
context.
"""

from __future__ import annotations

import re
import time
from typing import Any, Callable, Dict, List
from urllib.parse import quote

from . import Fetcher, fetch_json
from writer import Item

DEFAULT_ENDPOINT = "https://efts.sec.gov/LATEST/search-index"
PUBLICATION = "SEC EDGAR"


class RateLimiter:
    """At most ``max_rps`` acquisitions per rolling second."""

    def __init__(self, max_rps: int, clock: Callable[[], float] = time.monotonic, sleep: Callable[[float], None] = time.sleep) -> None:
        self.max_rps = max(1, int(max_rps))
        self.clock = clock
        self.sleep = sleep
        self.stamps: List[float] = []

    def acquire(self) -> None:
        now = self.clock()
        self.stamps = [t for t in self.stamps if now - t < 1.0]
        if len(self.stamps) >= self.max_rps:
            wait = 1.0 - (now - self.stamps[0])
            if wait > 0:
                self.sleep(wait)
            now = self.clock()
            self.stamps = [t for t in self.stamps if now - t < 1.0]
        self.stamps.append(now)


_OFFICER_RE = re.compile(r"(?:appoint(?:ed|ment of)|named|elected)\s+([A-Z][a-z]+(?:\s+[A-Z]\.?)?(?:\s+[A-Z][a-z]+){1,2})")


def tick(feed: Any, since: str, fetcher: Fetcher, ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    ua = ctx.get("edgar_user_agent") or ""
    if not ua:
        raise RuntimeError("EDGAR requires a declared User-Agent ([edgar] user_agent in courier.toml)")
    limiter: RateLimiter = ctx.get("limiter") or RateLimiter(int(ctx.get("edgar_max_rps", 8)))
    q = str(feed.filters.get("q") or "").strip()
    forms = feed.filters.get("forms") or ["8-K"]
    if not q:
        raise RuntimeError("EDGAR feed needs filters.q (company or phrase)")
    base = feed.endpoint or DEFAULT_ENDPOINT
    url = f"{base}?q={quote(q)}&forms={quote(','.join(forms))}"
    if since:
        # EDGAR's documented custom range takes both ends; omit enddt
        # and some queries answer 500 intermittently (seen live 2026-09-20).
        from datetime import date as _date

        end = str(ctx.get("today") or _date.today().isoformat())
        url += f"&dateRange=custom&startdt={since}&enddt={end}"
    limiter.acquire()
    data = fetch_json(fetcher, url, {"User-Agent": ua, "Accept": "application/json"})
    records: List[Dict[str, Any]] = []
    for hit in (data.get("hits", {}) or {}).get("hits", []) or []:
        src = hit.get("_source") or {}
        hid = str(hit.get("_id") or "")
        if not hid:
            continue
        company = ", ".join(src.get("display_names") or []) or str(src.get("entity_name") or "")
        form = str(src.get("form") or "")
        date = str(src.get("file_date") or since)[:10]
        items = str(src.get("items") or "")
        title = f"{company}: {form} filed {date}"
        officer = None
        desc = str(src.get("description") or "")
        if "5.02" in items:
            m = _OFFICER_RE.search(desc)
            if m:
                officer = m.group(1)
        adsh = hid.split(":")[0]
        records.append(
            {
                "id": hid,
                "date": date,
                "title": title,
                "url": f"https://www.sec.gov/Archives/edgar/data/{src.get('ciks', [''])[0] if src.get('ciks') else ''}/{adsh.replace('-', '')}/{adsh}-index.htm",
                "summary": f"{form} ({items or 'no items listed'}) by {company}, filed {date}. {desc}".strip(),
                "company": company,
                "officer": officer,
                "kind": "hire" if officer else "other",
            }
        )
    return records


def to_item(rec: Dict[str, Any], feed: Any, fetch_policy: str) -> Item:
    mentions = []
    events = []
    if rec.get("company"):
        mentions.append((rec["company"], f"filer of {rec['title']}"))
    if rec.get("officer"):
        mentions.append((rec["officer"], f"officer named in 8-K item 5.02, {rec['company']}"))
        events.append(("hire", f"officer change at {rec['company']} per 8-K item 5.02", [(rec["officer"], "hire"), (rec["company"], "employer")]))
    return Item(
        title=rec["title"],
        url=rec["url"],
        publication=feed.publication or PUBLICATION,
        published_at=rec["date"],
        intake="feed",
        fetch=fetch_policy,
        body=rec["summary"],
        tags=["courier-structured", "edgar"],
        mentions=mentions,
        events=events,
    )
