"""Mecklenburg County permits via the county's ArcGIS FeatureServer
(``AccelaAllPermits``): REST JSON, no key, public record. One query per
tick, filtered by issue date >= since and optional minimum declared
cost and permit type. Each record becomes a ``feed`` item whose
``## Mentions`` carries the owner (courier-structured) and whose
``## Events`` bullet names the owner as developer.
"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any, Dict, List
from urllib.parse import urlencode

from . import Fetcher, fetch_json
from writer import Item

DEFAULT_ENDPOINT = (
    "https://meckgis.mecklenburgcountync.gov/server/rest/services/AccelaAllPermits/FeatureServer/0/query"
)
PUBLICATION = "Mecklenburg County permits"


def _epoch_ms_to_date(v: Any) -> str:
    try:
        return datetime.fromtimestamp(int(v) / 1000, tz=timezone.utc).date().isoformat()
    except (TypeError, ValueError, OSError):
        return ""


def _cost_number(v: Any) -> float:
    """The layer stores the declared cost as a string ("1,250,000" or
    "$41,100,000"); parse it leniently, 0.0 when absent."""
    if v is None:
        return 0.0
    try:
        return float(str(v).replace("$", "").replace(",", "").strip() or 0)
    except ValueError:
        return 0.0


def query_url(feed_endpoint: str, since: str, filters: Dict[str, Any]) -> str:
    # Verified live 2026-09-20: field names are lowercase; `issue_date` is
    # an esriFieldTypeDate (a DATE literal works); the cost field is an
    # esriFieldTypeString, so a numeric `>=` in `where` answers HTTP 400.
    # Filter by date and type on the server, by cost on the client.
    where = [f"issue_date >= DATE '{since}'"] if since else ["1=1"]
    if filters.get("permit_type"):
        where.append("permit_type = '" + str(filters["permit_type"]).replace("'", "''") + "'")
    params = {
        "where": " AND ".join(where),
        "outFields": "*",
        "orderByFields": "issue_date DESC",
        "resultRecordCount": str(int(filters.get("max_records", 200))),
        "f": "json",
    }
    return (feed_endpoint or DEFAULT_ENDPOINT) + "?" + urlencode(params)


def tick(feed: Any, since: str, fetcher: Fetcher, ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    url = query_url(feed.endpoint, since, feed.filters)
    data = fetch_json(fetcher, url, {"Accept": "application/json"})
    if isinstance(data, dict) and data.get("error"):
        raise RuntimeError(f"FeatureServer error: {data['error']}")
    min_cost = _cost_number(feed.filters.get("min_cost"))
    records: List[Dict[str, Any]] = []
    for f in data.get("features", []) or []:
        a = f.get("attributes") or {}
        pid = str(a.get("permit_number") or "").strip()
        if not pid:
            continue
        cost = _cost_number(a.get("building_construction_cost_customer"))
        if min_cost and cost < min_cost:
            continue
        date = _epoch_ms_to_date(a.get("issue_date")) or since
        owner = str(a.get("owner_name") or "").strip()
        project = str(a.get("project_name") or "").strip()
        ptype = str(a.get("permit_type") or "permit").strip()
        work = str(a.get("description_of_work") or a.get("description") or "").strip()
        city = str(a.get("owner_city") or "").strip()
        title = project or (f"{ptype}: {work[:60]}" if work else ptype)
        summary_bits = [f"Permit {pid}", ptype]
        if work:
            summary_bits.append(work[:200])
        if cost:
            summary_bits.append(f"declared cost ${cost:,.0f}")
        if owner:
            summary_bits.append(f"owner {owner}" + (f" ({city})" if city else ""))
        low = ptype.lower()
        kind = "opening" if "new" in low else ("expansion" if ("addition" in low or "alteration" in low) else "other")
        records.append(
            {
                "id": pid,
                "date": date,
                "title": title,
                "url": (feed.endpoint or DEFAULT_ENDPOINT).rsplit("/query", 1)[0] + f"?permit={pid}",
                "summary": "; ".join(summary_bits) + ".",
                "owner": owner,
                "kind": kind,
                "amount": cost or None,
                "raw": a,
            }
        )
    return records


def to_item(rec: Dict[str, Any], feed: Any, fetch_policy: str) -> Item:
    mentions = [(rec["owner"], f"permit owner, {rec['title']}")] if rec.get("owner") else []
    desc = rec["summary"].rstrip(".")
    events = [(rec["kind"], desc, [(rec["owner"], "developer")] if rec.get("owner") else [])]
    return Item(
        title=rec["title"],
        url=rec["url"],
        publication=feed.publication or PUBLICATION,
        published_at=rec["date"],
        intake="feed",
        fetch=fetch_policy,
        body=rec["summary"],
        tags=["courier-structured", "permit"],
        mentions=mentions,
        events=events,
    )
