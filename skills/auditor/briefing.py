"""Auditor: morning briefing (task_41).

Computes the `auditor.briefing` claim — who is new, who moved, which
organizations are trending, what happened, who to promote, who to
call, what the resolver could not decide, and which files might be
the same entity — from the atoms committed since the previous
briefing, and publishes it through `audit.publish_summary` with
`predicate: auditor.briefing`.

Layout:

- `collect()` is the only function that talks to the daemon. It
  issues `audit.query`, `atom.list`, `atom.get`, and
  `ingest.list_pending` and returns a plain `data` dict.
- `compute_claim(data, config)` and every `derive_*` function are pure
  over plain dicts, so tests feed fixture atoms without a daemon.
- `briefing(inp)` glues the two together and publishes.

Every entity reference the claim carries is a "ref":
`{"entity": <opaque id> | None, "display": <text>}`. Ids are opaque and
permanent (ADR-030); nothing here is keyed by name except the alias
overlap pass, which is *about* names.

Derivations follow ADR-031: "who moved" is read from `affiliation`
atom activity (a root atom is a join, a supersession that sets
`valid_to` is a departure, a supersession that changes `title` is a
retitle), and a person scalar supersession (location, role) is not a
change. Dates on affiliations are "as reported": the press reports
announcements, not start dates.

The auditor never writes anything but the briefing atom.
"""

from __future__ import annotations

import os
import re
import sys
from datetime import datetime, timedelta, timezone
from typing import Any, Dict, Iterable, List, Optional, Set, Tuple

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir, "_lib"))
if _LIB not in sys.path:
    sys.path.insert(0, _LIB)

from ffs_skill import FfsSkillError, log, query  # noqa: E402


BRIEFING_PREDICATE = "auditor.briefing"

# The six business-graph predicates the briefing reads (ADR-028).
BUSINESS_PREDICATES: Tuple[str, ...] = (
    "person.generic",
    "org.company",
    "source.article",
    "event.business",
    "affiliation",
    "contact.person",
)

# Predicates listed with `since = window.from`.
WINDOW_PREDICATES: Tuple[str, ...] = BUSINESS_PREDICATES + ("entity.same_as",)

# Predicates listed for the prior window (trending needs a baseline).
PRIOR_PREDICATES: Tuple[str, ...] = ("source.article", "event.business")

# Predicates listed with no `since` (live heads, up to the ceiling).
ALL_TIME_PREDICATES: Tuple[str, ...] = (
    "person.generic",
    "org.company",
    "contact.person",
    "affiliation",
    "entity.same_as",
    "entity.different_from",
)

# Duplicate families: family label -> predicate.
FAMILIES: Tuple[Tuple[str, str], ...] = (
    ("people", "person.generic"),
    ("orgs", "org.company"),
    ("contacts", "contact.person"),
)

# `SourceKind::AutoAccept` serializes as snake_case (crates/ffs-core/src/atom.rs).
AUTO_ACCEPT_KIND = "auto_accept"

DEFAULT_INTERVAL = "7d"
DEFAULT_PROMOTE_MIN_ARTICLES = 3
DEFAULT_ATOM_CEILING = 500
DEFAULT_LIST_MAX = 20
# `atom.list` refuses limits above this.
ATOM_LIST_HARD_MAX = 5000

_EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)
_INTERVAL_RE = re.compile(r"^\s*(\d+)\s*([dhms])?\s*$", re.IGNORECASE)


# ---------------------------------------------------------------------
# Configuration and small helpers
# ---------------------------------------------------------------------


def _env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None or not str(raw).strip():
        return default
    try:
        return int(str(raw).strip())
    except ValueError:
        log("warn", f"{name}={raw!r} is not an integer; using {default}")
        return default


def config_from_env() -> Dict[str, Any]:
    """Read the briefing's tunables from the environment (evaluated per
    call, so a host can change them between invocations)."""
    interval = os.environ.get("FFS_AUDITOR_BRIEFING_INTERVAL") or DEFAULT_INTERVAL
    ceiling = _env_int("FFS_BRIEFING_ATOM_CEILING", DEFAULT_ATOM_CEILING)
    ceiling = max(1, min(ceiling, ATOM_LIST_HARD_MAX))
    return {
        "interval": interval.strip(),
        "promote_min_articles": max(1, _env_int("FFS_BRIEFING_PROMOTE_MIN_ARTICLES", DEFAULT_PROMOTE_MIN_ARTICLES)),
        "ceiling": ceiling,
        "list_max": max(1, _env_int("FFS_BRIEFING_LIST_MAX", DEFAULT_LIST_MAX)),
    }


def parse_interval(text: str) -> timedelta:
    """`<n>d|h|m|s` (unit defaults to days). Unparseable → 7 days."""
    m = _INTERVAL_RE.match(str(text or ""))
    if not m:
        log("warn", f"bad briefing interval {text!r}; using {DEFAULT_INTERVAL}")
        return timedelta(days=7)
    n = int(m.group(1))
    unit = (m.group(2) or "d").lower()
    if unit == "d":
        return timedelta(days=n)
    if unit == "h":
        return timedelta(hours=n)
    if unit == "m":
        return timedelta(minutes=n)
    return timedelta(seconds=n)


def parse_iso(ts: Any) -> Optional[datetime]:
    """Parse an ISO 8601 stamp into an aware UTC datetime, or None."""
    if ts is None:
        return None
    try:
        dt = datetime.fromisoformat(str(ts).strip().replace("Z", "+00:00"))
    except (TypeError, ValueError):
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def iso(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def date_of(ts: Any) -> Optional[str]:
    """`YYYY-MM-DD` of an ISO stamp (or a bare date), else None."""
    dt = parse_iso(ts)
    if dt is None:
        return None
    return dt.date().isoformat()


def _claim(atom: Dict[str, Any]) -> Dict[str, Any]:
    c = atom.get("claim")
    return c if isinstance(c, dict) else {}


def _s(value: Any) -> str:
    return "" if value is None else str(value)


def _lower(value: Any) -> str:
    return _s(value).strip().lower()


def _tx(atom: Dict[str, Any]) -> datetime:
    return parse_iso(atom.get("tx_time")) or _EPOCH


def _newest_key(atom: Dict[str, Any]) -> Tuple[datetime, str]:
    return (_tx(atom), _s(atom.get("hash")))


def heads_by_entity(atoms: Iterable[Dict[str, Any]]) -> Dict[str, Dict[str, Any]]:
    """Newest atom per entity id (by `tx_time`, then hash)."""
    heads: Dict[str, Dict[str, Any]] = {}
    for atom in atoms:
        if not isinstance(atom, dict):
            continue
        eid = _s(atom.get("entity"))
        if not eid:
            continue
        cur = heads.get(eid)
        if cur is None or _newest_key(atom) > _newest_key(cur):
            heads[eid] = atom
    return heads


def _is_live(atom: Dict[str, Any]) -> bool:
    return atom.get("valid_to") in (None, "")


def in_range(atoms: Iterable[Dict[str, Any]], start: datetime, end: datetime) -> List[Dict[str, Any]]:
    """Atoms with `start < tx_time <= end`."""
    out: List[Dict[str, Any]] = []
    for atom in atoms:
        if not isinstance(atom, dict):
            continue
        tx = parse_iso(atom.get("tx_time"))
        if tx is None:
            continue
        if start < tx <= end:
            out.append(atom)
    return out


def cap(items: List[Any], n: int) -> List[Any]:
    return list(items[: max(0, int(n))])


# ---------------------------------------------------------------------
# Window
# ---------------------------------------------------------------------


def compute_window(
    previous: Any,
    now: datetime,
    interval: str,
    window_days: Optional[int] = None,
) -> Tuple[datetime, datetime]:
    """`(from, to)`: `from` is the previous briefing's `window.to`
    (newest first), else `now - interval`; `window_days` overrides
    `from` for manual runs. `to` is `now`."""
    to = now.astimezone(timezone.utc)
    if window_days is not None:
        try:
            days = max(0, int(window_days))
        except (TypeError, ValueError):
            days = 0
        return (to - timedelta(days=days), to)
    if isinstance(previous, list):
        for item in previous:
            if not isinstance(item, dict):
                continue
            window = _claim(item).get("window")
            if not isinstance(window, dict):
                continue
            prev_to = parse_iso(window.get("to"))
            if prev_to is not None:
                return (min(prev_to, to), to)
    return (to - parse_interval(interval), to)


# ---------------------------------------------------------------------
# Collect (the only layer that talks to the daemon)
# ---------------------------------------------------------------------


def _safe_query(method: str, params: Any, default: Any) -> Any:
    try:
        return query(method, params)
    except FfsSkillError as e:
        log("warn", f"{method} failed: {e}; section degraded")
        return default


def _list_rows(rows: Any) -> List[Dict[str, Any]]:
    if not isinstance(rows, list):
        return []
    return [r for r in rows if isinstance(r, dict)]


def collect(config: Dict[str, Any], now: datetime, window_days: Optional[int] = None) -> Dict[str, Any]:
    """Fetch everything the derivations need into a plain dict."""
    ceiling = int(config["ceiling"])
    previous = _safe_query("audit.query", {"kind": "briefing"}, default=[])
    w_from, w_to = compute_window(previous, now, config["interval"], window_days)
    p_from = w_from - (w_to - w_from)
    truncated = False

    def list_pred(pred: str, since: Optional[datetime]) -> List[Dict[str, Any]]:
        nonlocal truncated
        params: Dict[str, Any] = {"predicate": pred, "limit": ceiling}
        if since is not None:
            params["since"] = iso(since)
        rows = _list_rows(_safe_query("atom.list", params, default=[]))
        if len(rows) >= ceiling:
            truncated = True
        return rows

    health = _safe_query("health.summary", {}, default={})
    window_atoms = {pred: in_range(list_pred(pred, w_from), w_from, w_to) for pred in WINDOW_PREDICATES}
    prior_atoms = {pred: in_range(list_pred(pred, p_from), p_from, w_from) for pred in PRIOR_PREDICATES}
    all_atoms = {pred: list_pred(pred, None) for pred in ALL_TIME_PREDICATES}

    # Parents of in-window supersessions we have not already fetched.
    known: Set[str] = set()
    for group in (window_atoms, prior_atoms, all_atoms):
        for atoms in group.values():
            for atom in atoms:
                if atom.get("hash"):
                    known.add(_s(atom["hash"]))
    parents: Dict[str, Dict[str, Any]] = {}
    for pred in ("affiliation", "org.company"):
        for atom in window_atoms.get(pred) or []:
            sup = _s(atom.get("supersedes"))
            if not sup or sup in known or sup in parents:
                continue
            parent = _safe_query("atom.get", {"hash": sup}, default=None)
            if isinstance(parent, dict):
                parent = dict(parent)
                parent.setdefault("hash", sup)
                parents[sup] = parent

    pending = _list_rows(_safe_query("ingest.list_pending", {}, default=[]))

    return {
        "now": now.astimezone(timezone.utc),
        "window": (w_from, w_to),
        "prior": (p_from, w_from),
        "cadence": config["interval"],
        "window_atoms": window_atoms,
        "health": health if isinstance(health, dict) else {},
        "prior_atoms": prior_atoms,
        "all_atoms": all_atoms,
        "parents": parents,
        "pending": pending,
        "truncated": truncated,
    }


# ---------------------------------------------------------------------
# Context: id → head maps built once
# ---------------------------------------------------------------------


def _mention_counts(articles: Iterable[Dict[str, Any]], events: Iterable[Dict[str, Any]]) -> Dict[str, int]:
    counts: Dict[str, int] = {}
    for atom in articles:
        for m in _claim(atom).get("mentions") or []:
            if isinstance(m, dict) and m.get("entity"):
                eid = _s(m["entity"])
                counts[eid] = counts.get(eid, 0) + 1
    for atom in events:
        for p in _claim(atom).get("participants") or []:
            if isinstance(p, dict) and p.get("entity"):
                eid = _s(p["entity"])
                counts[eid] = counts.get(eid, 0) + 1
    return counts


def _article_key(atom: Dict[str, Any]) -> Tuple[str, datetime, str]:
    """Earliest-first ordering: `published_at`, then `tx_time`."""
    return (_s(_claim(atom).get("published_at")), _tx(atom), _s(atom.get("entity")))


def build_context(data: Dict[str, Any]) -> Dict[str, Any]:
    window_atoms: Dict[str, List[Dict[str, Any]]] = data.get("window_atoms") or {}
    prior_atoms: Dict[str, List[Dict[str, Any]]] = data.get("prior_atoms") or {}
    all_atoms: Dict[str, List[Dict[str, Any]]] = data.get("all_atoms") or {}

    def both(pred: str) -> List[Dict[str, Any]]:
        return list(all_atoms.get(pred) or []) + list(window_atoms.get(pred) or [])

    person_heads = heads_by_entity(both("person.generic"))
    contact_heads = heads_by_entity(both("contact.person"))
    org_heads = heads_by_entity(both("org.company"))
    affil_heads = heads_by_entity(both("affiliation"))
    article_heads = heads_by_entity(window_atoms.get("source.article") or [])
    event_heads = heads_by_entity(window_atoms.get("event.business") or [])
    prior_article_heads = heads_by_entity(prior_atoms.get("source.article") or [])
    prior_event_heads = heads_by_entity(prior_atoms.get("event.business") or [])

    names: Dict[str, str] = {}
    for heads in (org_heads, person_heads, contact_heads):
        for eid, atom in heads.items():
            dn = _s(_claim(atom).get("display_name")).strip()
            if dn:
                names[eid] = dn
    for eid, atom in article_heads.items():
        names.setdefault(eid, _s(_claim(atom).get("title")).strip() or eid)
    for eid, atom in event_heads.items():
        names.setdefault(eid, _s(_claim(atom).get("title")).strip() or eid)

    by_hash: Dict[str, Dict[str, Any]] = {}
    for group in (all_atoms, window_atoms, prior_atoms):
        for atoms in group.values():
            for atom in atoms:
                if isinstance(atom, dict) and atom.get("hash"):
                    by_hash[_s(atom["hash"])] = atom
    for h, atom in (data.get("parents") or {}).items():
        if isinstance(atom, dict):
            by_hash[_s(h)] = atom

    article_by_url: Dict[str, str] = {}
    for eid, atom in article_heads.items():
        url = _s(_claim(atom).get("url")).strip()
        if url and url not in article_by_url:
            article_by_url[url] = eid

    # In-window articles mentioning an id, earliest first.
    articles_by_mention: Dict[str, List[Dict[str, Any]]] = {}
    for atom in sorted(article_heads.values(), key=_article_key):
        seen: Set[str] = set()
        for m in _claim(atom).get("mentions") or []:
            if isinstance(m, dict) and m.get("entity"):
                eid = _s(m["entity"])
                if eid in seen:
                    continue
                seen.add(eid)
                articles_by_mention.setdefault(eid, []).append(atom)

    # Current (open) affiliations per person id, from affiliation heads.
    affil_orgs_by_person: Dict[str, Set[str]] = {}
    for atom in affil_heads.values():
        if not _is_live(atom):
            continue
        c = _claim(atom)
        person = _s(c.get("person"))
        org = _s(c.get("organization"))
        if person and org:
            affil_orgs_by_person.setdefault(person, set()).add(org)

    return {
        "window": data.get("window"),
        "prior": data.get("prior"),
        "window_atoms": window_atoms,
        "person_heads": person_heads,
        "contact_heads": contact_heads,
        "org_heads": org_heads,
        "affil_heads": affil_heads,
        "article_heads": article_heads,
        "event_heads": event_heads,
        "names": names,
        "by_hash": by_hash,
        "article_by_url": article_by_url,
        "articles_by_mention": articles_by_mention,
        "affil_orgs_by_person": affil_orgs_by_person,
        "mentions_window": _mention_counts(article_heads.values(), event_heads.values()),
        "mentions_prior": _mention_counts(prior_article_heads.values(), prior_event_heads.values()),
        "same_as_all": list(all_atoms.get("entity.same_as") or []) + list(window_atoms.get("entity.same_as") or []),
        "different_from_all": list(all_atoms.get("entity.different_from") or []),
    }


def ref(ctx: Dict[str, Any], eid: Any) -> Dict[str, Any]:
    eid = _s(eid)
    return {"entity": eid, "display": ctx["names"].get(eid, eid)}


def org_ref(ctx: Dict[str, Any], value: Any) -> Optional[Dict[str, Any]]:
    """An org id → ref with its display; a free name → entity null."""
    value = _s(value).strip()
    if not value:
        return None
    if value in ctx["org_heads"]:
        return ref(ctx, value)
    return {"entity": None, "display": value}


def article_ref(ctx: Dict[str, Any], atom: Optional[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
    if not isinstance(atom, dict):
        return None
    eid = _s(atom.get("entity"))
    return {"entity": eid, "display": _s(_claim(atom).get("title")).strip() or eid}


def source_ref(ctx: Dict[str, Any], source: Any) -> Optional[Dict[str, Any]]:
    """An affiliation's `source` resolved against in-window article
    heads by entity id or url; else entity null with the raw string."""
    source = _s(source).strip()
    if not source:
        return None
    if source in ctx["article_heads"]:
        return article_ref(ctx, ctx["article_heads"][source])
    eid = ctx["article_by_url"].get(source)
    if eid:
        return article_ref(ctx, ctx["article_heads"][eid])
    return {"entity": None, "display": source}


def person_ref(ctx: Dict[str, Any], eid: Any) -> Dict[str, Any]:
    return ref(ctx, eid)


# ---------------------------------------------------------------------
# Derivations (pure)
# ---------------------------------------------------------------------


def derive_new_people(ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Entity ids whose root `person.generic` atom landed in the window."""
    roots: Set[str] = set()
    for atom in ctx["window_atoms"].get("person.generic") or []:
        if not atom.get("supersedes") and atom.get("entity"):
            roots.add(_s(atom["entity"]))
    out: List[Dict[str, Any]] = []
    for eid in roots:
        head = ctx["person_heads"].get(eid)
        claim = _claim(head) if head else {}
        first = (ctx["articles_by_mention"].get(eid) or [None])[0]
        out.append(
            {
                "entity": eid,
                "display": ctx["names"].get(eid, eid),
                "organization": org_ref(ctx, claim.get("organization")),
                "first_seen_article": article_ref(ctx, first),
            }
        )
    out.sort(key=lambda p: (p["display"].lower(), p["entity"]))
    return out


def _change(
    ctx: Dict[str, Any],
    entity: str,
    kind: str,
    organization: Optional[Dict[str, Any]],
    frm: Optional[str],
    to: Optional[str],
    as_reported: Optional[str],
    source_article: Optional[Dict[str, Any]],
) -> Dict[str, Any]:
    return {
        "entity": entity,
        "display": ctx["names"].get(entity, entity),
        "kind": kind,
        "organization": organization,
        "from": frm,
        "to": to,
        "as_reported": as_reported,
        "source_article": source_article,
    }


def derive_changes(ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Joins / departures / retitles from `affiliation` activity
    (ADR-031) plus `org_changed` from `org.company` scalar supersession."""
    out: List[Dict[str, Any]] = []
    by_hash = ctx["by_hash"]

    for atom in ctx["window_atoms"].get("affiliation") or []:
        claim = _claim(atom)
        person = _s(claim.get("person"))
        if not person:
            continue
        organization = org_ref(ctx, claim.get("organization"))
        title = _s(claim.get("title")).strip() or None
        source = source_ref(ctx, claim.get("source"))
        sup = _s(atom.get("supersedes"))
        if not sup:
            out.append(_change(ctx, person, "joined", organization, None, title, date_of(atom.get("valid_from")), source))
            continue
        parent = by_hash.get(sup)
        if not isinstance(parent, dict):
            log("warn", f"affiliation {atom.get('hash')} supersedes unknown atom {sup}; skipped")
            continue
        parent_title = _s(_claim(parent).get("title")).strip() or None
        if _is_live(parent) and not _is_live(atom):
            # A departure is reported on the date the role ended.
            reported = date_of(atom.get("valid_to")) or date_of(atom.get("valid_from"))
            out.append(_change(ctx, person, "left", organization, parent_title, None, reported, source))
        elif parent_title != title:
            out.append(_change(ctx, person, "retitled", organization, parent_title, title, date_of(atom.get("valid_from")), source))

    for atom in ctx["window_atoms"].get("org.company") or []:
        sup = _s(atom.get("supersedes"))
        if not sup:
            continue
        parent = by_hash.get(sup)
        if not isinstance(parent, dict):
            continue
        new_claim, old_claim = _claim(atom), _claim(parent)
        eid = _s(atom.get("entity"))
        for field in ("display_name", "location"):
            old, new = _s(old_claim.get(field)).strip(), _s(new_claim.get(field)).strip()
            if old != new:
                out.append(_change(ctx, eid, "org_changed", None, old or None, new or None, date_of(atom.get("valid_from")), None))
                break

    out.sort(key=lambda c: (c["as_reported"] or "", c["display"].lower(), c["kind"], c["entity"]))
    out.reverse()
    return out


def derive_trending_orgs(ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    this, prior = ctx["mentions_window"], ctx["mentions_prior"]
    out: List[Dict[str, Any]] = []
    for eid, count in this.items():
        if eid not in ctx["org_heads"]:
            continue
        before = int(prior.get(eid, 0))
        if count >= 2 and count > before:
            out.append(
                {
                    "entity": eid,
                    "display": ctx["names"].get(eid, eid),
                    "mentions_this_window": int(count),
                    "mentions_prior_window": before,
                }
            )
    out.sort(key=lambda o: (-o["mentions_this_window"], o["display"].lower(), o["entity"]))
    return out


def derive_events(ctx: Dict[str, Any], list_max: int) -> List[Dict[str, Any]]:
    """Newest head per in-window event, grouped by `kind` in first-seen
    order (walking newest first); at most `list_max` items in total."""
    heads = sorted(ctx["event_heads"].values(), key=_newest_key, reverse=True)
    groups: Dict[str, List[Dict[str, Any]]] = {}
    total = 0
    for atom in heads:
        if total >= list_max:
            break
        claim = _claim(atom)
        kind = _s(claim.get("kind")).strip() or "other"
        participants = []
        for p in claim.get("participants") or []:
            if not isinstance(p, dict):
                continue
            pid = _s(p.get("entity")).strip() or None
            participants.append(
                {
                    "entity": pid,
                    "display": _s(p.get("display")).strip() or (ctx["names"].get(pid, pid) if pid else ""),
                    "role": _s(p.get("role")).strip() or None,
                }
            )
        eid = _s(atom.get("entity"))
        groups.setdefault(kind, []).append(
            {
                "entity": eid,
                "display": _s(claim.get("title")).strip() or eid,
                "date": _s(claim.get("date")).strip() or None,
                "participants": participants,
            }
        )
        total += 1
    return [{"kind": kind, "items": items} for kind, items in groups.items()]


def _contact_orgs(ctx: Dict[str, Any]) -> Dict[str, Set[str]]:
    """org id → contact ids affiliated with it (open affiliation atom,
    or the contact head's `organization` scalar equal to an org id)."""
    out: Dict[str, Set[str]] = {}
    for cid, head in ctx["contact_heads"].items():
        if not _is_live(head):
            continue
        orgs = set(ctx["affil_orgs_by_person"].get(cid) or ())
        scalar = _s(_claim(head).get("organization")).strip()
        if scalar in ctx["org_heads"]:
            orgs.add(scalar)
        for org in orgs:
            out.setdefault(org, set()).add(cid)
    return out


def derive_promotion_candidates(ctx: Dict[str, Any], min_articles: int) -> List[Dict[str, Any]]:
    contact_orgs = _contact_orgs(ctx)
    contact_ids = set(ctx["contact_heads"])
    out: List[Dict[str, Any]] = []
    for eid, head in ctx["person_heads"].items():
        if eid in contact_ids or not _is_live(head):
            continue
        claim = _claim(head)
        article_count = len(ctx["articles_by_mention"].get(eid) or ())
        mention_count = int(ctx["mentions_window"].get(eid, 0))
        person_orgs = set(ctx["affil_orgs_by_person"].get(eid) or ())
        scalar = _s(claim.get("organization")).strip()
        if scalar in ctx["org_heads"]:
            person_orgs.add(scalar)
        shared = sorted(org for org in person_orgs if org in contact_orgs)
        reasons: List[str] = []
        if article_count >= min_articles:
            reasons.append(f"mentioned in {article_count} articles this window (threshold {min_articles})")
        if shared:
            org = shared[0]
            contact = sorted(contact_orgs[org])[0]
            reasons.append(
                f"affiliated with {ctx['names'].get(org, org)}, where existing contact "
                f"{ctx['names'].get(contact, contact)} is affiliated"
            )
        if not reasons:
            continue
        organization = org_ref(ctx, scalar) or (ref(ctx, sorted(person_orgs)[0]) if person_orgs else None)
        out.append(
            {
                "entity": eid,
                "display": ctx["names"].get(eid, eid),
                "organization": organization,
                "mention_count": mention_count,
                "article_count": article_count,
                "reason": "; ".join(reasons) + ".",
            }
        )
    out.sort(key=lambda p: (-p["article_count"], -p["mention_count"], p["display"].lower(), p["entity"]))
    return out


def derive_follow_ups(ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    mentioned = {eid for eid in ctx["mentions_window"] if eid in ctx["org_heads"]}
    out: List[Dict[str, Any]] = []
    for cid, head in ctx["contact_heads"].items():
        if not _is_live(head):
            continue
        orgs = set(ctx["affil_orgs_by_person"].get(cid) or ())
        if not orgs:
            scalar = _s(_claim(head).get("organization")).strip()
            if scalar in ctx["org_heads"]:
                orgs = {scalar}
        for org in sorted(orgs & mentioned):
            first = (ctx["articles_by_mention"].get(org) or [None])[0]
            out.append(
                {
                    "entity": cid,
                    "display": ctx["names"].get(cid, cid),
                    "organization": ref(ctx, org),
                    "triggering_article": article_ref(ctx, first),
                }
            )
    out.sort(key=lambda f: (f["display"].lower(), f["organization"]["display"].lower(), f["entity"]))
    return out


def derive_needs_your_eye(pending: Any) -> List[Dict[str, Any]]:
    """Quarantined proposals the resolver marked `ambiguous`."""
    out: List[Dict[str, Any]] = []
    for sub in pending if isinstance(pending, list) else []:
        if not isinstance(sub, dict):
            continue
        sid = _s(sub.get("id"))
        for idx, prop in enumerate(sub.get("proposals") or []):
            if not isinstance(prop, dict) or _lower(prop.get("resolution")) != "ambiguous":
                continue
            claim = _claim(prop)
            candidates = []
            for cand in prop.get("candidates") or []:
                if not isinstance(cand, dict):
                    continue
                try:
                    score = float(cand.get("score") or 0.0)
                except (TypeError, ValueError):
                    score = 0.0
                cid = _s(cand.get("entity"))
                candidates.append({"entity": cid, "display": _s(cand.get("display")).strip() or cid, "score": score})
            candidates.sort(key=lambda c: (-c["score"], c["entity"]))
            out.append(
                {
                    "submission_id": sid,
                    "local_ref": _s(prop.get("local_ref")).strip() or f"#{idx}",
                    "predicate": _s(prop.get("predicate")),
                    "display": _s(claim.get("display_name") or claim.get("title")).strip(),
                    "candidates": candidates,
                }
            )
    out.sort(key=lambda n: (n["submission_id"], n["local_ref"], n["predicate"]))
    return out


def _live_relations(atoms: Iterable[Dict[str, Any]], key: str) -> Set[frozenset]:
    """Unordered pairs asserted by relation atoms that are neither
    superseded nor closed (`valid_to`), so an undone merge no longer
    relates its pair."""
    atoms = [a for a in atoms if isinstance(a, dict)]
    superseded = {_s(a.get("supersedes")) for a in atoms if a.get("supersedes")}
    pairs: Set[frozenset] = set()
    for atom in atoms:
        if _s(atom.get("hash")) in superseded or not _is_live(atom):
            continue
        a, b = _s(atom.get("entity")), _s(_claim(atom).get(key))
        if a and b and a != b:
            pairs.add(frozenset((a, b)))
    return pairs


def derive_possible_duplicates(ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Pairs of live entities in one family sharing >= 1 lowercase-
    trimmed name (display_name or alias), not already related by
    `entity.same_as` or `entity.different_from`."""
    related = _live_relations(ctx["same_as_all"], "target") | _live_relations(ctx["different_from_all"], "other")
    heads_for = {"people": ctx["person_heads"], "orgs": ctx["org_heads"], "contacts": ctx["contact_heads"]}
    out: List[Dict[str, Any]] = []
    for family, _pred in FAMILIES:
        name_index: Dict[str, Set[str]] = {}
        for eid, head in heads_for[family].items():
            if not _is_live(head):
                continue
            claim = _claim(head)
            names = {_lower(claim.get("display_name"))}
            for alias in claim.get("aliases") or []:
                names.add(_lower(alias))
            names.discard("")
            for name in names:
                name_index.setdefault(name, set()).add(eid)
        shared: Dict[Tuple[str, str], Set[str]] = {}
        for name, ids in name_index.items():
            if len(ids) < 2:
                continue
            ordered = sorted(ids)
            for i, a in enumerate(ordered):
                for b in ordered[i + 1 :]:
                    if frozenset((a, b)) in related:
                        continue
                    shared.setdefault((a, b), set()).add(name)
        for (a, b), names in shared.items():
            out.append(
                {
                    "family": family,
                    "entity_a": ref(ctx, a),
                    "entity_b": ref(ctx, b),
                    "shared_aliases": sorted(names),
                }
            )
    out.sort(key=lambda d: (-len(d["shared_aliases"]), d["family"], d["entity_a"]["entity"], d["entity_b"]["entity"]))
    return out


def derive_recent_merges(ctx: Dict[str, Any]) -> List[Dict[str, Any]]:
    out: List[Dict[str, Any]] = []
    for atom in ctx["window_atoms"].get("entity.same_as") or []:
        target = _s(_claim(atom).get("target"))
        if not atom.get("entity") or not target:
            continue
        out.append(
            {
                "same_as_hash": _s(atom.get("hash")),
                "source": ref(ctx, atom.get("entity")),
                "target": ref(ctx, target),
                "tx_time": _s(atom.get("tx_time")),
            }
        )
    out.sort(key=lambda m: (m["tx_time"], m["same_as_hash"]), reverse=True)
    return out


def derive_past_window(data: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Group the daemon's past-window list by predicate for the briefing.
    Each item keeps the atom hash so the section can carry the same
    `attest:` lines the inbox does."""
    health = data.get("health") or {}
    att = health.get("attestation_status") if isinstance(health, dict) else None
    items = (att or {}).get("past_window") or []
    out: List[Dict[str, Any]] = []
    for it in items:
        if not isinstance(it, dict):
            continue
        out.append(
            {
                "entity": _s(it.get("entity")),
                "predicate": _s(it.get("predicate")),
                "display": _s(it.get("display")) or _s(it.get("entity")),
                "atom_hash": _s(it.get("atom_hash")),
                "last_confirmed": _s(it.get("last_confirmed")) or None,
                "owner_alone": bool(it.get("owner_alone")),
            }
        )
    out.sort(key=lambda x: (x["predicate"], x["display"]))
    return out


def derive_filing(ctx: Dict[str, Any]) -> Dict[str, int]:
    auto = reviewed = 0
    for pred in BUSINESS_PREDICATES:
        for atom in ctx["window_atoms"].get(pred) or []:
            prov = atom.get("provenance") or []
            if any(isinstance(p, dict) and _lower(p.get("kind")) == AUTO_ACCEPT_KIND for p in prov):
                auto += 1
            else:
                reviewed += 1
    return {"auto_filed_count": auto, "reviewed_count": reviewed}


# ---------------------------------------------------------------------
# Claim assembly
# ---------------------------------------------------------------------


LIST_SECTIONS: Tuple[str, ...] = (
    "new_people",
    "changes",
    "trending_orgs",
    "promotion_candidates",
    "follow_ups",
    "needs_your_eye",
    "possible_duplicates",
    "recent_merges",
)


def _event_count(events: List[Dict[str, Any]]) -> int:
    return sum(len(g.get("items") or []) for g in events)


def build_narrative(claim: Dict[str, Any]) -> str:
    filing = claim["filing"]
    text = (
        f"Briefing for {date_of(claim['window']['from'])} to {claim['date']} ({claim['cadence']} cadence): "
        f"{len(claim['new_people'])} new people, {len(claim['changes'])} changes, "
        f"{len(claim['trending_orgs'])} trending organizations, {_event_count(claim['events'])} events, "
        f"{len(claim['promotion_candidates'])} promotion candidates, {len(claim['follow_ups'])} follow-ups, "
        f"{len(claim['needs_your_eye'])} items that need your eye, "
        f"{len(claim['possible_duplicates'])} possible duplicates, and {len(claim['recent_merges'])} recent merges. "
        f"{filing['auto_filed_count']} atoms were auto-filed and {filing['reviewed_count']} were reviewed. "
        "Affiliation dates are as reported by the press, not start dates."
    )
    if claim["truncated"]:
        text += f" Some lists truncated: the {claim['ceiling']}-atom ceiling was reached."
    return text


def compute_claim(data: Dict[str, Any], config: Dict[str, Any]) -> Dict[str, Any]:
    """The full `auditor.briefing` claim from collected (or fixture)
    data. Pure: no daemon access."""
    ctx = build_context(data)
    list_max = int(config["list_max"])
    w_from, w_to = data["window"]
    now = data.get("now") or w_to
    claim: Dict[str, Any] = {
        "date": w_to.astimezone(timezone.utc).date().isoformat(),
        "window": {"from": iso(w_from), "to": iso(w_to)},
        "cadence": _s(config.get("interval") or DEFAULT_INTERVAL),
        "generated_at": iso(now),
        "narrative": "",
        "truncated": bool(data.get("truncated")),
        "ceiling": int(config["ceiling"]),
        "new_people": cap(derive_new_people(ctx), list_max),
        "changes": cap(derive_changes(ctx), list_max),
        "trending_orgs": cap(derive_trending_orgs(ctx), list_max),
        "events": derive_events(ctx, list_max),
        "promotion_candidates": cap(derive_promotion_candidates(ctx, int(config["promote_min_articles"])), list_max),
        "follow_ups": cap(derive_follow_ups(ctx), list_max),
        "needs_your_eye": cap(derive_needs_your_eye(data.get("pending")), list_max),
        "possible_duplicates": cap(derive_possible_duplicates(ctx), list_max),
        "recent_merges": cap(derive_recent_merges(ctx), list_max),
        "filing": derive_filing(ctx),
        # ADR-034 (task_46): facts past their confirmation window, from
        # the daemon's derived status. The inbox carries the ticks; the
        # briefing repeats them so the morning read sees the batch.
        "past_window": cap(derive_past_window(data), list_max),
        "attestation_status": (data.get("health") or {}).get("attestation_status", {}).get("by_predicate", {}) if isinstance((data.get("health") or {}).get("attestation_status"), dict) else {},
    }
    claim["narrative"] = build_narrative(claim)
    return claim


def section_counts(claim: Dict[str, Any]) -> Dict[str, int]:
    counts = {key: len(claim.get(key) or []) for key in LIST_SECTIONS}
    counts["events"] = _event_count(claim.get("events") or [])
    return counts


def publish_briefing(claim: Dict[str, Any]) -> Dict[str, Any]:
    try:
        result = query("audit.publish_summary", {"claim": claim, "predicate": BRIEFING_PREDICATE})
    except FfsSkillError as e:
        log("warn", f"audit.publish_summary failed: {e}")
        return {"atom_hash": None, "reason": str(e)}
    if not isinstance(result, dict):
        return {"atom_hash": None, "reason": "unexpected response from audit.publish_summary"}
    return {"atom_hash": result.get("atom_hash"), "reason": None}


def briefing(inp: Optional[Dict[str, Any]] = None, now: Optional[datetime] = None) -> Dict[str, Any]:
    """One briefing pass: collect, compute, publish."""
    inp = inp if isinstance(inp, dict) else {}
    config = config_from_env()
    window_days = inp.get("window_days")
    if window_days is not None:
        try:
            window_days = int(window_days)
        except (TypeError, ValueError):
            log("warn", f"ignoring bad window_days {window_days!r}")
            window_days = None
    current = now or datetime.now(timezone.utc)
    data = collect(config, current, window_days)
    claim = compute_claim(data, config)
    pub = publish_briefing(claim)
    return {
        "atom_hash": pub.get("atom_hash"),
        "reason": pub.get("reason"),
        "counts": section_counts(claim),
    }
