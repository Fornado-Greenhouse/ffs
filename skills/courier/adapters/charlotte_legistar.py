"""Charlotte City Council via the Legistar Web API (Granicus): JSON,
no token for Charlotte, replies capped at 1,000 rows. One events
query and one matters query per tick since the watermark. Petitioners
named in a matter's title or body become courier-structured mentions;
the meeting becomes an ``other`` event dated to the meeting.
"""

from __future__ import annotations

import re
from typing import Any, Dict, List
from urllib.parse import quote

from . import Fetcher, fetch_json
from writer import Item

DEFAULT_ENDPOINT = "https://webapi.legistar.com/v1/charlottenc"
PUBLICATION = "Charlotte City Council"

_PETITIONER_RE = re.compile(r"(?:petition(?:er)?|applicant|by)\s*[:\-]?\s*([A-Z][A-Za-z0-9&.,' -]{2,80}?)(?=[.;]|\s+for\b|\s+to\b|$)")


_ABBREV = ("co", "inc", "corp", "ltd", "llc", "assoc", "bros")


def petitioners(text: str) -> List[str]:
    out: List[str] = []
    for m in _PETITIONER_RE.finditer(text or ""):
        name = m.group(1).strip().rstrip(",")
        # Keep the period on a trailing abbreviation ("Co." not "Co").
        end = m.end(1)
        if name.split(" ")[-1].lower() in _ABBREV and text[end : end + 1] == ".":
            name += "."
        if name and name not in out:
            out.append(name)
    return out


def tick(feed: Any, since: str, fetcher: Fetcher, ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    base = (feed.endpoint or DEFAULT_ENDPOINT).rstrip("/")
    since_f = f"EventDate ge datetime'{since}T00:00:00'" if since else ""
    ev_url = f"{base}/events?$top=100" + (f"&$filter={quote(since_f)}" if since_f else "")
    events = fetch_json(fetcher, ev_url, {"Accept": "application/json"})
    mt_filter = f"MatterIntroDate ge datetime'{since}T00:00:00'" if since else ""
    mt_url = f"{base}/matters?$top=200" + (f"&$filter={quote(mt_filter)}" if mt_filter else "")
    matters = fetch_json(fetcher, mt_url, {"Accept": "application/json"})
    records: List[Dict[str, Any]] = []
    for ev in events or []:
        eid = str(ev.get("EventId") or "")
        if not eid:
            continue
        date = str(ev.get("EventDate") or "")[:10]
        body = str(ev.get("EventBodyName") or "Council")
        records.append(
            {
                "id": f"event-{eid}",
                "date": date,
                "title": f"{body} meeting {date}",
                "url": f"{base}/events/{eid}",
                "summary": f"{body} meeting on {date}; agenda: {ev.get('EventAgendaFile') or 'n/a'}.",
                "petitioners": [],
                "kind": "other",
            }
        )
    for m in matters or []:
        mid = str(m.get("MatterId") or "")
        if not mid:
            continue
        title = str(m.get("MatterTitle") or m.get("MatterName") or f"Matter {mid}").strip()
        date = str(m.get("MatterIntroDate") or "")[:10] or since
        names = petitioners(title)
        records.append(
            {
                "id": f"matter-{mid}",
                "date": date,
                "title": title[:160],
                "url": f"{base}/matters/{mid}",
                "summary": f"{m.get('MatterTypeName') or 'Matter'} {m.get('MatterFile') or mid}: {title}",
                "petitioners": names,
                "kind": "other",
            }
        )
    return records


def to_item(rec: Dict[str, Any], feed: Any, fetch_policy: str) -> Item:
    mentions = [(n, "petitioner before Charlotte City Council") for n in rec.get("petitioners", [])]
    events = [(rec["kind"], rec["summary"].rstrip("."), [(n, "other") for n in rec.get("petitioners", [])])]
    return Item(
        title=rec["title"],
        url=rec["url"],
        publication=feed.publication or PUBLICATION,
        published_at=rec["date"],
        intake="feed",
        fetch=fetch_policy,
        body=rec["summary"],
        tags=["courier-structured", "council"],
        mentions=mentions,
        events=events,
    )
