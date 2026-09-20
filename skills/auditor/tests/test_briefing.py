"""Unit tests for the auditor's morning briefing (task_41).

`_Substrate` stands in for the daemon: it answers `atom.list`
(entity-less `{predicate, since, limit}` form, newest first,
truncated at `limit`), `atom.get`, `audit.query {kind: briefing}`,
`ingest.list_pending`, and `audit.publish_summary` from fixture
atoms, so the whole `briefing` op runs without a daemon. Fixture
names are synthetic.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional

import pytest

import audit  # type: ignore  # conftest sys.path bootstrap
import briefing  # type: ignore
from ffs_skill import FfsSkillError  # type: ignore

NOW = datetime(2026, 9, 21, 6, 0, tzinfo=timezone.utc)
PREV_FROM = "2026-09-07T06:00:00Z"
PREV_TO = "2026-09-14T06:00:00Z"
IN = "2026-09-18T09:00:00Z"  # inside the window
IN2 = "2026-09-19T09:00:00Z"
IN3 = "2026-09-20T09:00:00Z"
PRIOR = "2026-09-10T09:00:00Z"  # inside the prior window
OLD = "2026-08-01T09:00:00Z"  # before both windows


def _atom(
    h: str,
    entity: str,
    predicate: str,
    claim: Dict[str, Any],
    tx: str,
    supersedes: Optional[str] = None,
    valid_from: Optional[str] = None,
    valid_to: Optional[str] = None,
    provenance: Optional[List[Dict[str, Any]]] = None,
) -> Dict[str, Any]:
    return {
        "hash": h,
        "entity": entity,
        "predicate": predicate,
        "claim": claim,
        "author": "scribe",
        "valid_from": valid_from or tx,
        "valid_to": valid_to,
        "tx_time": tx,
        "classification": "existence",
        "supersedes": supersedes,
        "provenance": provenance if provenance is not None else [{"kind": "ingest_file", "uri": "file:///x", "hash": "z"}],
        "signature": "sig",
    }


def _prev() -> List[Dict[str, Any]]:
    return [
        {
            "hash": "zprev",
            "entity": "briefing-0001",
            "predicate": "auditor.briefing",
            "claim": {"window": {"from": PREV_FROM, "to": PREV_TO}},
            "tx_time": PREV_TO,
        }
    ]


class _Substrate:
    def __init__(
        self,
        atoms: Optional[List[Dict[str, Any]]] = None,
        pending: Optional[List[Dict[str, Any]]] = None,
        previous: Optional[List[Dict[str, Any]]] = None,
        hidden: Optional[List[Dict[str, Any]]] = None,
    ) -> None:
        self.atoms = list(atoms or [])
        self.hidden = list(hidden or [])  # served by atom.get only
        self.pending = list(pending or [])
        self.previous = _prev() if previous is None else previous
        self.calls: List[Dict[str, Any]] = []
        self.fail: set = set()
        self.publish_hash = "z-briefing-hash"

    def __call__(self, method: str, params: Any) -> Any:
        self.calls.append({"method": method, "params": params})
        if method in self.fail:
            raise FfsSkillError(f"capability denied: {method}")
        if method == "atom.list":
            assert "entity" not in params, "briefing must use the entity-less atom.list form"
            pred = params["predicate"]
            since = briefing.parse_iso(params.get("since")) if params.get("since") else None
            limit = int(params.get("limit", 1000))
            rows = [a for a in self.atoms if a["predicate"] == pred and (since is None or briefing.parse_iso(a["tx_time"]) > since)]
            rows.sort(key=lambda a: (a["tx_time"], a["hash"]), reverse=True)
            return rows[:limit]
        if method == "atom.get":
            for a in self.atoms + self.hidden:
                if a["hash"] == params["hash"]:
                    return {k: v for k, v in a.items() if k != "hash"}
            raise FfsSkillError("atom not found")
        if method == "audit.query":
            return self.previous if params.get("kind") == "briefing" else []
        if method == "ingest.list_pending":
            return self.pending
        if method == "audit.publish_summary":
            return {"atom_hash": self.publish_hash}
        raise FfsSkillError("unknown method " + method)

    def published_claim(self) -> Dict[str, Any]:
        call = next(c for c in self.calls if c["method"] == "audit.publish_summary")
        return call["params"]["claim"]


def _run(monkeypatch, sub: _Substrate, **inp: Any):
    monkeypatch.setattr(briefing, "query", sub)
    result = briefing.briefing({"op": "briefing", **inp}, now=NOW)
    return result, sub.published_claim()


# --- fixture builders -------------------------------------------------


def _org(eid: str, name: str, tx: str = OLD, **claim: Any) -> Dict[str, Any]:
    return _atom(f"h-{eid}", eid, "org.company", {"display_name": name, **claim}, tx)


def _person(eid: str, name: str, tx: str = OLD, h: Optional[str] = None, **kw: Any) -> Dict[str, Any]:
    claim = {"display_name": name}
    for key in ("organization", "role", "aliases", "location"):
        if key in kw:
            claim[key] = kw.pop(key)
    return _atom(h or f"h-{eid}", eid, "person.generic", claim, tx, **kw)


def _contact(eid: str, name: str, tx: str = OLD, **claim: Any) -> Dict[str, Any]:
    return _atom(f"h-{eid}", eid, "contact.person", {"display_name": name, **claim}, tx)


def _article(eid: str, title: str, mentions: List[str], tx: str = IN, published_at: Optional[str] = None) -> Dict[str, Any]:
    return _atom(
        f"h-{eid}",
        eid,
        "source.article",
        {
            "title": title,
            "url": f"https://example.test/{eid}",
            "published_at": published_at or tx[:10],
            "mentions": [{"entity": m, "display": m} for m in mentions],
        },
        tx,
    )


def _affil(h: str, eid: str, person: str, org: str, title: str, tx: str = IN, **kw: Any) -> Dict[str, Any]:
    return _atom(h, eid, "affiliation", {"person": person, "organization": org, "title": title, "kind": "employee", **kw.pop("extra", {})}, tx, **kw)


# ---------------------------------------------------------------------
# Required-by-spec tests
# ---------------------------------------------------------------------


def test_briefing_new_people_from_first_seen_entity_ids(monkeypatch):
    sub = _Substrate(
        atoms=[
            _org("org-acme", "Acme Widgets"),
            _person("p-new", "Pat Example", tx=IN, organization="org-acme"),
            _person("p-old", "Sam Example", tx=OLD),
            _person("p-old", "Samuel Example", tx=IN2, h="h-p-old-2", supersedes="h-p-old"),
            _article("a-2", "Later piece", ["p-new"], tx=IN3, published_at="2026-09-20"),
            _article("a-1", "First sighting", ["p-new"], tx=IN2, published_at="2026-09-18"),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    assert [p["entity"] for p in claim["new_people"]] == ["p-new"]
    person = claim["new_people"][0]
    assert person["display"] == "Pat Example"
    assert person["organization"] == {"entity": "org-acme", "display": "Acme Widgets"}
    assert person["first_seen_article"] == {"entity": "a-1", "display": "First sighting"}
    # The rename is not a change either: scalar supersession on a person.
    assert claim["changes"] == []


def test_briefing_detects_joined_from_new_affiliation_atom(monkeypatch):
    sub = _Substrate(
        atoms=[
            _org("org-acme", "Acme Widgets"),
            _person("p-1", "Pat Example"),
            _article("a-1", "Acme hires a director", ["p-1", "org-acme"], tx=IN),
            _affil("h-af-1", "af-1", "p-1", "org-acme", "Director", tx=IN2, valid_from="2026-09-17T00:00:00Z", extra={"source": "a-1"}),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    assert len(claim["changes"]) == 1
    change = claim["changes"][0]
    assert change["kind"] == "joined"
    assert change["entity"] == "p-1" and change["display"] == "Pat Example"
    assert change["organization"] == {"entity": "org-acme", "display": "Acme Widgets"}
    assert change["from"] is None and change["to"] == "Director"
    assert change["as_reported"] == "2026-09-17"
    assert change["source_article"] == {"entity": "a-1", "display": "Acme hires a director"}


def test_briefing_detects_left_from_affiliation_valid_to(monkeypatch):
    parent = _affil("h-af-1", "af-1", "p-1", "org-acme", "Director", tx=OLD, valid_from="2025-01-01T00:00:00Z")
    child = _affil(
        "h-af-2",
        "af-1",
        "p-1",
        "org-acme",
        "Director",
        tx=IN2,
        supersedes="h-af-1",
        valid_from="2025-01-01T00:00:00Z",
        valid_to="2026-09-18T00:00:00Z",
    )
    # The parent is served only by atom.get, to exercise the fetch path.
    sub = _Substrate(atoms=[_org("org-acme", "Acme Widgets"), _person("p-1", "Pat Example"), child], hidden=[parent])
    _, claim = _run(monkeypatch, sub)
    assert [c["kind"] for c in claim["changes"]] == ["left"]
    change = claim["changes"][0]
    assert change["from"] == "Director" and change["to"] is None
    assert change["as_reported"] == "2026-09-18"
    assert any(c["method"] == "atom.get" and c["params"] == {"hash": "h-af-1"} for c in sub.calls)


def test_briefing_detects_retitle_from_affiliation_title_supersession(monkeypatch):
    sub = _Substrate(
        atoms=[
            _org("org-acme", "Acme Widgets"),
            _person("p-1", "Pat Example"),
            _affil("h-af-1", "af-1", "p-1", "org-acme", "Manager", tx=OLD),
            _affil("h-af-2", "af-1", "p-1", "org-acme", "Director", tx=IN, supersedes="h-af-1", valid_from="2026-09-16T00:00:00Z"),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    assert [c["kind"] for c in claim["changes"]] == ["retitled"]
    change = claim["changes"][0]
    assert change["from"] == "Manager" and change["to"] == "Director"
    assert change["as_reported"] == "2026-09-16"
    # No atom.get needed: the parent came back in the all-time list.
    assert not any(c["method"] == "atom.get" for c in sub.calls)


def test_briefing_person_scalar_supersession_is_not_a_change(monkeypatch):
    sub = _Substrate(
        atoms=[
            _person("p-1", "Pat Example", tx=OLD, location="Old Town"),
            _person("p-1", "Pat Example", tx=IN, h="h-p-1-2", supersedes="h-p-1", location="New Town"),
            _person("p-1", "Pat Example", tx=IN2, h="h-p-1-3", supersedes="h-p-1-2", location="New Town", role="CFO"),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    assert claim["changes"] == []
    assert claim["new_people"] == []


def test_briefing_lists_ambiguous_proposals_as_needs_your_eye(monkeypatch):
    pending = [
        {
            "id": "sub-1",
            "status": "extracted",
            "proposals": [
                {"predicate": "person.generic", "claim": {"display_name": "Pat Example"}, "resolution": "existing", "entity": "p-1", "candidates": []},
                {
                    "predicate": "person.generic",
                    "claim": {"display_name": "P. Example"},
                    "local_ref": "person-2",
                    "resolution": "ambiguous",
                    "candidates": [
                        {"entity": "p-1", "score": 6.5, "matched_on": ["alias"], "display": "Pat Example"},
                        {"entity": "p-2", "score": 7.2, "matched_on": ["display_name"], "display": "Patricia Example"},
                    ],
                },
                {"predicate": "org.company", "claim": {"display_name": "Acme Widgets"}, "resolution": "new", "candidates": []},
                {"predicate": "event.business", "claim": {"title": "Acme opens", "kind": "opening"}, "resolution": "ambiguous", "candidates": []},
            ],
        }
    ]
    sub = _Substrate(pending=pending)
    _, claim = _run(monkeypatch, sub)
    items = claim["needs_your_eye"]
    assert len(items) == 2
    by_ref = {i["local_ref"]: i for i in items}
    assert by_ref["person-2"]["submission_id"] == "sub-1"
    assert by_ref["person-2"]["predicate"] == "person.generic"
    assert by_ref["person-2"]["display"] == "P. Example"
    assert by_ref["person-2"]["candidates"] == [
        {"entity": "p-2", "display": "Patricia Example", "score": 7.2},
        {"entity": "p-1", "display": "Pat Example", "score": 6.5},
    ]
    assert by_ref["#3"]["display"] == "Acme opens" and by_ref["#3"]["candidates"] == []


def test_briefing_lists_alias_overlap_pairs_as_possible_duplicates(monkeypatch):
    sub = _Substrate(
        atoms=[
            _person("p-1", "Pat Example", aliases=["P. Example"]),
            _person("p-2", "Patricia Example", aliases=[" pat example "]),
            _person("p-3", "Sam Example", aliases=["S. Example"]),
            _person("p-4", "Samuel Example", aliases=["s. example"]),
            _atom("h-df", "p-3", "entity.different_from", {"other": "p-4"}, OLD),
            _person("p-5", "Lee Example", aliases=["L. Example"]),
            _person("p-6", "Leigh Example", aliases=["l. example"]),
            _atom("h-sa", "p-6", "entity.same_as", {"target": "p-5"}, OLD),
            _org("org-1", "Acme Widgets", aliases=["Acme"]),
            _org("org-2", "Acme Widgets Inc", aliases=["acme", "acme widgets"]),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    dups = claim["possible_duplicates"]
    assert [(d["family"], d["entity_a"]["entity"], d["entity_b"]["entity"]) for d in dups] == [
        ("orgs", "org-1", "org-2"),
        ("people", "p-1", "p-2"),
    ]
    assert dups[0]["shared_aliases"] == ["acme", "acme widgets"]
    assert dups[1]["shared_aliases"] == ["pat example"]
    assert dups[1]["entity_a"] == {"entity": "p-1", "display": "Pat Example"}


def test_briefing_trending_requires_growth_over_prior_window(monkeypatch):
    sub = _Substrate(
        atoms=[
            _org("org-1", "Acme Widgets"),
            _org("org-2", "Bolt Bakery"),
            _org("org-3", "Cog Cafe"),
            _article("a-1", "One", ["org-1", "org-2"], tx=IN),
            _article("a-2", "Two", ["org-1"], tx=IN2),
            _article("a-3", "Three", ["org-1", "org-3"], tx=IN3),
            _article("a-p1", "Prior one", ["org-1", "org-2"], tx=PRIOR),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    # org-1: 3 vs 1 → listed. org-2: 1 vs 1 → not. org-3: 1 vs 0 → below 2.
    assert claim["trending_orgs"] == [
        {"entity": "org-1", "display": "Acme Widgets", "mentions_this_window": 3, "mentions_prior_window": 1}
    ]


def test_briefing_promotion_threshold_by_article_count(monkeypatch):
    sub = _Substrate(
        atoms=[
            _person("p-3", "Pat Example"),
            _person("p-2", "Sam Example"),
            _article("a-1", "One", ["p-3", "p-2"], tx=IN),
            _article("a-2", "Two", ["p-3", "p-2"], tx=IN2),
            _article("a-3", "Three", ["p-3"], tx=IN3),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    assert [c["entity"] for c in claim["promotion_candidates"]] == ["p-3"]
    cand = claim["promotion_candidates"][0]
    assert cand["article_count"] == 3 and cand["mention_count"] == 3
    assert "3 articles" in cand["reason"]
    assert claim["new_people"] == []  # both were minted before the window


def test_briefing_promotion_by_shared_org_with_existing_contact(monkeypatch):
    sub = _Substrate(
        atoms=[
            _org("org-acme", "Acme Widgets"),
            _org("org-bolt", "Bolt Bakery"),
            _contact("c-1", "Chris Example"),
            _affil("h-af-c", "af-c", "c-1", "org-acme", "CEO", tx=OLD),
            _person("p-1", "Pat Example"),
            _affil("h-af-p1", "af-p1", "p-1", "org-acme", "Analyst", tx=OLD),
            _person("p-2", "Sam Example"),
            _affil("h-af-p2", "af-p2", "p-2", "org-bolt", "Baker", tx=OLD),
            # Scalar fallback: no affiliation atom yet, organization is the org id.
            _person("p-3", "Lee Example", organization="org-acme"),
            # A closed affiliation does not count.
            _person("p-4", "Ash Example"),
            _affil("h-af-p4", "af-p4", "p-4", "org-acme", "Intern", tx=OLD, valid_to="2026-01-01T00:00:00Z"),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    cands = {c["entity"]: c for c in claim["promotion_candidates"]}
    assert set(cands) == {"p-1", "p-3"}
    assert "Acme Widgets" in cands["p-1"]["reason"] and "Chris Example" in cands["p-1"]["reason"]
    assert cands["p-1"]["organization"] == {"entity": "org-acme", "display": "Acme Widgets"}
    assert cands["p-3"]["organization"] == {"entity": "org-acme", "display": "Acme Widgets"}
    # An existing contact is never a promotion candidate.
    assert "c-1" not in cands


def test_briefing_follow_ups_for_contacts_whose_org_made_news(monkeypatch):
    sub = _Substrate(
        atoms=[
            _org("org-acme", "Acme Widgets"),
            _org("org-bolt", "Bolt Bakery"),
            _contact("c-1", "Chris Example"),
            _affil("h-af-c1", "af-c1", "c-1", "org-acme", "CEO", tx=OLD),
            _contact("c-2", "Dana Example"),
            _affil("h-af-c2", "af-c2", "c-2", "org-acme", "CFO", tx=OLD, valid_to="2026-01-01T00:00:00Z"),
            _contact("c-3", "Evan Example", organization="org-acme"),
            _contact("c-4", "Finn Example", organization="Acme Widgets"),  # a name, never matched
            _contact("c-5", "Gale Example"),
            _affil("h-af-c5", "af-c5", "c-5", "org-bolt", "Owner", tx=OLD),
            _article("a-2", "Acme again", ["org-acme"], tx=IN2, published_at="2026-09-19"),
            _article("a-1", "Acme expands", ["org-acme"], tx=IN, published_at="2026-09-18"),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    ups = claim["follow_ups"]
    assert [u["entity"] for u in ups] == ["c-1", "c-3"]
    for u in ups:
        assert u["organization"] == {"entity": "org-acme", "display": "Acme Widgets"}
        assert u["triggering_article"] == {"entity": "a-1", "display": "Acme expands"}


def test_briefing_window_starts_at_previous_briefing_end(monkeypatch):
    sub = _Substrate(atoms=[_person("p-before", "Pat Example", tx="2026-09-14T05:00:00Z"), _person("p-after", "Sam Example", tx=IN)])
    _, claim = _run(monkeypatch, sub)
    assert claim["window"] == {"from": PREV_TO, "to": "2026-09-21T06:00:00Z"}
    assert claim["date"] == "2026-09-21"
    assert [p["entity"] for p in claim["new_people"]] == ["p-after"]
    windowed = [c["params"] for c in sub.calls if c["method"] == "atom.list" and c["params"]["predicate"] == "person.generic"]
    assert {"predicate": "person.generic", "limit": 500, "since": PREV_TO} in windowed
    assert {"predicate": "person.generic", "limit": 500} in windowed
    assert any(c["method"] == "audit.query" and c["params"] == {"kind": "briefing"} for c in sub.calls)


def test_first_briefing_window_is_seven_days(monkeypatch):
    sub = _Substrate(previous=[])
    _, claim = _run(monkeypatch, sub)
    assert claim["window"]["from"] == briefing.iso(NOW - timedelta(days=7))
    assert claim["window"]["to"] == briefing.iso(NOW)
    assert claim["cadence"] == "7d"


def test_briefing_cadence_env_and_window_days_override(monkeypatch):
    monkeypatch.setenv("FFS_AUDITOR_BRIEFING_INTERVAL", "1d")
    sub = _Substrate(previous=[])
    _, claim = _run(monkeypatch, sub)
    assert claim["window"]["from"] == briefing.iso(NOW - timedelta(days=1))
    assert claim["cadence"] == "1d"
    sub2 = _Substrate()  # a previous briefing exists, but window_days wins
    _, claim2 = _run(monkeypatch, sub2, window_days=3)
    assert claim2["window"]["from"] == briefing.iso(NOW - timedelta(days=3))
    assert briefing.parse_interval("12h") == timedelta(hours=12)
    assert briefing.parse_interval("30m") == timedelta(minutes=30)
    assert briefing.parse_interval("45s") == timedelta(seconds=45)
    assert briefing.parse_interval("nope") == timedelta(days=7)


def test_briefing_truncates_over_ceiling_and_says_so_in_narrative(monkeypatch):
    monkeypatch.setenv("FFS_BRIEFING_ATOM_CEILING", "2")
    sub = _Substrate(atoms=[_person(f"p-{i}", f"Person {i} Example", tx=IN) for i in range(5)])
    _, claim = _run(monkeypatch, sub)
    assert claim["truncated"] is True
    assert claim["ceiling"] == 2
    assert "lists truncated" in claim["narrative"]
    assert all(c["params"]["limit"] == 2 for c in sub.calls if c["method"] == "atom.list")
    assert len(claim["new_people"]) == 2
    # Under the ceiling: not truncated, and the narrative does not say so.
    monkeypatch.setenv("FFS_BRIEFING_ATOM_CEILING", "50")
    _, claim2 = _run(monkeypatch, _Substrate(atoms=[_person("p-1", "Pat Example", tx=IN)]))
    assert claim2["truncated"] is False and "truncated" not in claim2["narrative"]


def test_briefing_list_max_caps_every_section(monkeypatch):
    monkeypatch.setenv("FFS_BRIEFING_LIST_MAX", "3")
    atoms = [_person(f"p-{i}", f"Person {i} Example", tx=IN) for i in range(6)]
    atoms += [_atom(f"h-ev-{i}", f"ev-{i}", "event.business", {"title": f"Event {i}", "kind": "hire"}, IN) for i in range(6)]
    _, claim = _run(monkeypatch, _Substrate(atoms=atoms))
    assert len(claim["new_people"]) == 3
    assert sum(len(g["items"]) for g in claim["events"]) == 3


_CONTRACT_TYPES = {
    "date": str,
    "window": dict,
    "cadence": str,
    "generated_at": str,
    "narrative": str,
    "truncated": bool,
    "ceiling": int,
    "new_people": list,
    "changes": list,
    "trending_orgs": list,
    "events": list,
    "promotion_candidates": list,
    "follow_ups": list,
    "needs_your_eye": list,
    "possible_duplicates": list,
    "recent_merges": list,
    "filing": dict,
}


def _full_substrate() -> _Substrate:
    return _Substrate(
        atoms=[
            _org("org-acme", "Acme Widgets", aliases=["Acme"]),
            _org("org-acme2", "Acme Widgets LLC", aliases=["acme"]),
            _atom("h-org-acme-2", "org-acme", "org.company", {"display_name": "Acme Widgets Co", "aliases": ["Acme"], "location": "Springfield"}, IN, supersedes="h-org-acme"),
            _contact("c-1", "Chris Example"),
            _affil("h-af-c1", "af-c1", "c-1", "org-acme", "CEO", tx=OLD),
            _person("p-1", "Pat Example", tx=IN, organization="org-acme"),
            _affil("h-af-p1", "af-p1", "p-1", "org-acme", "Director", tx=IN2, extra={"source": "https://example.test/a-1"}),
            _article("a-1", "Acme hires", ["p-1", "org-acme"], tx=IN),
            _article("a-2", "Acme grows", ["org-acme"], tx=IN2),
            _article("a-p", "Acme prior", ["org-acme"], tx=PRIOR),
            _atom(
                "h-ev-1",
                "ev-1",
                "event.business",
                {"title": "Acme hires Pat", "kind": "hire", "date": "2026-09-18", "participants": [{"entity": "p-1", "display": "Pat Example", "role": "hire"}, {"display": "Someone", "entity": None}]},
                IN2,
                provenance=[{"kind": "auto_accept", "uri": "", "hash": "zcap"}],
            ),
            _atom("h-sa", "p-9", "entity.same_as", {"target": "p-1"}, IN3),
        ],
        pending=[{"id": "sub-1", "status": "extracted", "proposals": [{"predicate": "person.generic", "claim": {"display_name": "P. Example"}, "resolution": "ambiguous", "candidates": [{"entity": "p-1", "score": 5.0, "matched_on": ["alias"], "display": "Pat Example"}]}]}],
    )


def test_briefing_publishes_with_auditor_briefing_predicate(monkeypatch):
    sub = _full_substrate()
    result, claim = _run(monkeypatch, sub)
    publish = [c for c in sub.calls if c["method"] == "audit.publish_summary"]
    assert len(publish) == 1
    assert publish[0]["params"]["predicate"] == "auditor.briefing"
    assert set(publish[0]["params"]) == {"claim", "predicate"}
    for key, typ in _CONTRACT_TYPES.items():
        assert key in claim, key
        assert isinstance(claim[key], typ), (key, type(claim[key]))
    assert set(claim["window"]) == {"from", "to"}
    assert set(claim["filing"]) == {"auto_filed_count", "reviewed_count"}
    # In-window business atoms: org-acme, p-1, af-p1, a-1, a-2 (reviewed) + ev-1 (auto_accept).
    assert claim["filing"] == {"auto_filed_count": 1, "reviewed_count": 5}
    # Every section actually populated from the fixture.
    assert claim["new_people"] and claim["changes"] and claim["trending_orgs"] and claim["events"]
    assert claim["promotion_candidates"] and claim["follow_ups"] and claim["needs_your_eye"]
    assert claim["possible_duplicates"] and claim["recent_merges"]
    kinds = sorted(c["kind"] for c in claim["changes"])
    assert kinds == ["joined", "org_changed"]
    joined = next(c for c in claim["changes"] if c["kind"] == "joined")
    assert joined["source_article"] == {"entity": "a-1", "display": "Acme hires"}  # resolved by url
    org_changed = next(c for c in claim["changes"] if c["kind"] == "org_changed")
    assert org_changed["from"] == "Acme Widgets" and org_changed["to"] == "Acme Widgets Co"
    assert claim["events"][0]["kind"] == "hire"
    assert claim["events"][0]["items"][0]["participants"][1] == {"entity": None, "display": "Someone", "role": None}
    assert claim["recent_merges"][0]["same_as_hash"] == "h-sa"
    assert claim["recent_merges"][0]["target"] == {"entity": "p-1", "display": "Pat Example"}
    # The auditor never writes anything but the briefing.
    assert {c["method"] for c in sub.calls} <= {"audit.query", "atom.list", "atom.get", "ingest.list_pending", "audit.publish_summary"}
    assert result["atom_hash"] == "z-briefing-hash" and result["reason"] is None
    assert result["counts"]["new_people"] == 1 and result["counts"]["events"] == 1


def test_briefing_claim_is_deterministic(monkeypatch):
    _, claim_a = _run(monkeypatch, _full_substrate())
    sub_b = _full_substrate()
    sub_b.atoms.reverse()  # feed order must not matter
    _, claim_b = _run(monkeypatch, sub_b)
    assert claim_a == claim_b


# ---------------------------------------------------------------------
# Coverage extras
# ---------------------------------------------------------------------


def test_briefing_degrades_failed_queries_to_empty_and_still_publishes(monkeypatch):
    sub = _full_substrate()
    sub.fail = {"ingest.list_pending", "audit.query"}
    result, claim = _run(monkeypatch, sub)
    assert claim["needs_your_eye"] == []
    assert claim["window"]["from"] == briefing.iso(NOW - timedelta(days=7))  # first-run fallback
    assert result["atom_hash"] == "z-briefing-hash"


def test_briefing_publish_failure_returns_reason(monkeypatch):
    sub = _Substrate()
    sub.fail = {"audit.publish_summary"}
    monkeypatch.setattr(briefing, "query", sub)
    result = briefing.briefing({"op": "briefing"}, now=NOW)
    assert result["atom_hash"] is None and "capability denied" in result["reason"]


def test_handle_dispatches_briefing_op(monkeypatch):
    seen: List[Dict[str, Any]] = []
    monkeypatch.setattr(audit, "run_briefing", lambda inp: seen.append(inp) or {"atom_hash": "z", "reason": None, "counts": {}})
    out = audit.handle({"op": "briefing", "window_days": 2})
    assert out["atom_hash"] == "z"
    assert seen == [{"op": "briefing", "window_days": 2}]


def test_compute_window_prefers_newest_previous_briefing():
    prev = [
        {"claim": {"window": {"from": PREV_FROM, "to": PREV_TO}}},
        {"claim": {"window": {"from": "2026-08-31T06:00:00Z", "to": PREV_FROM}}},
    ]
    w_from, w_to = briefing.compute_window(prev, NOW, "7d")
    assert briefing.iso(w_from) == PREV_TO and w_to == NOW
    # Old daemons answer audit.query with daily summaries (no window): first-run rule.
    w_from, _ = briefing.compute_window([{"claim": {"metrics": {}}}], NOW, "2d")
    assert w_from == NOW - timedelta(days=2)


def test_possible_duplicates_ignore_undone_merges(monkeypatch):
    sub = _Substrate(
        atoms=[
            _person("p-1", "Pat Example"),
            _person("p-2", "Pat Example"),
            _atom("h-sa", "p-2", "entity.same_as", {"target": "p-1"}, OLD),
            # Undo = supersession setting valid_to; the pair is a duplicate again.
            _atom("h-sa-undo", "p-2", "entity.same_as", {"target": "p-1"}, IN, supersedes="h-sa", valid_to=IN),
        ]
    )
    _, claim = _run(monkeypatch, sub)
    assert len(claim["possible_duplicates"]) == 1
    assert claim["recent_merges"][0]["same_as_hash"] == "h-sa-undo"


@pytest.mark.parametrize("value,expected", [("abc", 500), ("", 500), ("9999999", 5000), ("0", 1)])
def test_ceiling_env_is_clamped(monkeypatch, value, expected):
    monkeypatch.setenv("FFS_BRIEFING_ATOM_CEILING", value)
    assert briefing.config_from_env()["ceiling"] == expected
