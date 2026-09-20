"""Newsroom RSS 2.0 and Atom feeds (a publisher's own press releases).
Guid dedup; each entry becomes a ``clip`` item whose ``reported_by`` is
the publisher itself (the source is primary).
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from datetime import datetime
from email.utils import parsedate_to_datetime
from html.parser import HTMLParser
from typing import Any, Dict, List

import urlnorm  # skills/_lib

from . import Fetcher
from writer import Item

PUBLICATION = "newsroom"
_ATOM = "{http://www.w3.org/2005/Atom}"


class _Strip(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: List[str] = []

    def handle_data(self, d: str) -> None:
        self.parts.append(d)


def strip_html(s: str) -> str:
    p = _Strip()
    p.feed(s or "")
    p.close()
    return " ".join("".join(p.parts).split())


def _date(s: str) -> str:
    s = (s or "").strip()
    if not s:
        return ""
    try:
        return parsedate_to_datetime(s).date().isoformat()
    except (TypeError, ValueError):
        pass
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00")).date().isoformat()
    except ValueError:
        return s[:10]


def parse_feed(xml_bytes: bytes) -> List[Dict[str, Any]]:
    head = xml_bytes.lstrip()[:512].lower()
    if head.startswith(b"<!doctype html") or head.startswith(b"<html") or b"<html" in head[:64]:
        raise ValueError(
            "not an XML feed: the URL returned an HTML page (check the feed url; "
            "publishers often serve the feed at /rss.xml or /feed)"
        )
    try:
        root = ET.fromstring(xml_bytes)
    except ET.ParseError as e:
        raise ValueError(f"not well-formed XML feed: {e}") from e
    out: List[Dict[str, Any]] = []
    if root.tag == f"{_ATOM}feed":
        for e in root.findall(f"{_ATOM}entry"):
            link = ""
            for l in e.findall(f"{_ATOM}link"):
                if l.get("rel") in (None, "alternate"):
                    link = l.get("href") or ""
                    break
            out.append(
                {
                    "id": (e.findtext(f"{_ATOM}id") or link).strip(),
                    "date": _date(e.findtext(f"{_ATOM}published") or e.findtext(f"{_ATOM}updated") or ""),
                    "title": (e.findtext(f"{_ATOM}title") or "").strip(),
                    "url": link,
                    "summary": strip_html(e.findtext(f"{_ATOM}summary") or e.findtext(f"{_ATOM}content") or ""),
                }
            )
    else:
        for it in root.iter("item"):
            link = (it.findtext("link") or "").strip()
            out.append(
                {
                    "id": (it.findtext("guid") or link).strip(),
                    "date": _date(it.findtext("pubDate") or ""),
                    "title": (it.findtext("title") or "").strip(),
                    "url": link,
                    "summary": strip_html(it.findtext("description") or ""),
                }
            )
    return [r for r in out if r["url"] and r["title"]]


def tick(feed: Any, since: str, fetcher: Fetcher, ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    status, body = fetcher(feed.url, {"Accept": "application/rss+xml, application/atom+xml, application/xml"})
    if status != 200:
        raise RuntimeError(f"HTTP {status} from {feed.url}")
    records = parse_feed(body)
    seen_guids = set(ctx.get("seen_guids") or [])
    fresh = []
    for r in records:
        if r["id"] in seen_guids:
            continue
        if since and r["date"] and r["date"] < since:
            continue
        r["url"] = urlnorm.normalize_url(r["url"])
        fresh.append(r)
    return fresh


def to_item(rec: Dict[str, Any], feed: Any, fetch_policy: str) -> Item:
    return Item(
        title=rec["title"],
        url=rec["url"],
        publication=feed.publication or PUBLICATION,
        published_at=rec["date"] or "",
        intake="clip",
        fetch=fetch_policy,
        body=rec["summary"],
        tags=["press-release"],
        reported_by=feed.publication or PUBLICATION,
    )
