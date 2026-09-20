"""The article ingest contract (task_40 § 40.5): `## Mentions`,
`## Events`, `## References`, frontmatter round-trip, truncation.
Runs with the heuristic engine (no FFS_SCRIBE_ENGINE) against the
starter predicate specs loaded from TOML, so a courier-written file
works on a fresh install with no LLM."""

from __future__ import annotations

import json

import pytest

from corpus_scorer import starter_registry_env  # type: ignore
from extraction import handle  # type: ignore

ARTICLE = """---
predicate: source.article
title: Example maker opens second plant
url: https://example.com/news/2026/09/20/example-plant?utm_source=x
publication: Example Business Weekly
published_at: 2026-09-20
byline: Staff
tags: [manufacturing]
intake: clip
fetch: session
reported_by: Example Ledger
content_hash: zQmExampleHash
---
Example Maker Co. will open a second plant next spring, adding 120 jobs.

## Mentions
- Pat Example — chief executive at Example Maker Co.
- Example Maker Co. — family-owned widget maker in Gastonia
- Riverbend Capital: investor backing the expansion

## Events
- expansion: Example Maker Co. opens a second plant | Example Maker Co. (employer), Riverbend Capital (investor), Pat Example (spokes)
"""


def _handle(text: str, name: str = "article.md") -> dict:
    return handle({"source_uri": f"file:///ingest/{name}", "content": text})


@pytest.fixture
def starter(tmp_path, monkeypatch):
    starter_registry_env(tmp_path, monkeypatch)


def _by_pred(out: dict, predicate: str) -> list:
    return [p for p in out["proposals"] if p["predicate"] == predicate]


def test_mentions_bullets_yield_display_context_objects_and_secondary_proposals(starter):
    out = _handle(ARTICLE)
    article = _by_pred(out, "source.article")
    assert len(article) == 1
    claim = article[0]["claim"]
    assert claim["mentions"] == [
        {"display": "Pat Example", "context": "chief executive at Example Maker Co."},
        {"display": "Example Maker Co.", "context": "family-owned widget maker in Gastonia"},
        {"display": "Riverbend Capital", "context": "investor backing the expansion"},
    ]
    assert all("entity" not in m for m in claim["mentions"])
    people = _by_pred(out, "person.generic")
    orgs = _by_pred(out, "org.company")
    assert [p["claim"]["display_name"] for p in people] == ["Pat Example"]
    assert people[0]["claim"]["organization"] == "Example Maker Co."
    assert people[0]["claim"]["role"].lower().startswith("chief executive")
    assert sorted(o["claim"]["display_name"] for o in orgs) == ["Example Maker Co.", "Riverbend Capital"]
    # refs bind the article's mentions to the secondary proposals
    refs = {r["field"]: r["local_ref"] for r in article[0]["refs"]}
    assert refs["mentions[0].entity"] == people[0]["local_ref"]
    assert refs["mentions[1].entity"] in {o["local_ref"] for o in orgs}
    # every proposal carries the article provenance with the raw url
    for p in out["proposals"]:
        kinds = [e["kind"] for e in p["provenance"]]
        assert "source_article" in kinds, p["predicate"]


def test_courier_structured_bullets_are_high_confidence_hints(starter):
    text = ARTICLE.replace("tags: [manufacturing]", "tags: [manufacturing, courier-structured]\nrecord_id: PERMIT-123")
    out = _handle(text)
    person = _by_pred(out, "person.generic")[0]
    assert person["rationale"].startswith("courier-structured hint")
    assert "PERMIT-123" in person["rationale"]


def test_events_bullets_yield_event_business_with_participants_and_refs(starter):
    out = _handle(ARTICLE)
    events = _by_pred(out, "event.business")
    assert len(events) == 1
    claim = events[0]["claim"]
    assert claim["kind"] == "expansion"
    assert claim["summary"] == "Example Maker Co. opens a second plant"
    assert claim["title"] == "Example Maker Co. opens a second plant"
    assert claim["date"] == "2026-09-20"
    assert claim["source"].startswith("https://example.com/news/")
    assert claim["participants"] == [
        {"display": "Example Maker Co.", "role": "employer"},
        {"display": "Riverbend Capital", "role": "investor"},
        {"display": "Pat Example", "role": "other"},
    ]
    assert any("unknown participant role 'spokes'" in w for w in out["warnings"])
    refs = {r["field"]: r["local_ref"] for r in events[0].get("refs", [])}
    person_ref = _by_pred(out, "person.generic")[0]["local_ref"]
    assert refs["participants[2].entity"] == person_ref


def test_unknown_event_kind_maps_to_other_with_warning(starter):
    text = ARTICLE.replace("- expansion:", "- ribbon-cutting:")
    out = _handle(text)
    assert _by_pred(out, "event.business")[0]["claim"]["kind"] == "other"
    assert any("unknown event kind 'ribbon-cutting'" in w for w in out["warnings"])


def test_frontmatter_round_trip_content_hash_reported_by_intake_fetch(starter):
    out = _handle(ARTICLE)
    article = _by_pred(out, "source.article")[0]
    claim = article["claim"]
    assert claim["content_hash"] == "zQmExampleHash"
    assert "intake" not in claim and "fetch" not in claim and "reported_by" not in claim
    assert "intake-clip" in claim["tags"]
    assert "intake: clip, fetch: session" in article["rationale"]
    rb = [e for e in article["provenance"] if e["kind"] == "reported_by"]
    assert rb and rb[0]["uri"] == "outlet:Example Ledger"
    # the prose body lands in summary, without the Mentions bullets
    assert claim["summary"].startswith("Example Maker Co. will open")
    assert "Pat Example" not in claim["summary"]


def test_absent_optional_frontmatter_yields_nothing(starter):
    text = "---\npredicate: source.article\ntitle: T\nurl: https://example.com/t\n---\n"
    out = _handle(text)
    article = _by_pred(out, "source.article")[0]
    assert "content_hash" not in article["claim"]
    assert "tags" not in article["claim"]
    assert all(e["kind"] != "reported_by" for e in article["provenance"])


def test_pointer_file_yields_exactly_one_article_proposal(starter):
    text = "---\npredicate: source.article\ntitle: Pointer only\nurl: https://example.com/p\npublication: Example Weekly\npublished_at: 2026-09-20\nintake: pointer\n---\n"
    out = _handle(text)
    assert [p["predicate"] for p in out["proposals"]] == ["source.article"]
    assert "mentions" not in out["proposals"][0]["claim"]
    assert "summary" not in out["proposals"][0]["claim"]


def test_body_over_limit_is_truncated_with_parse_warning(starter, monkeypatch):
    monkeypatch.setenv("FFS_SCRIBE_BODY_LIMIT", "300")
    long_body = "word " * 200
    text = f"---\npredicate: source.article\ntitle: Long\nurl: https://example.com/long\n---\n{long_body}\n"
    out = _handle(text)
    article = _by_pred(out, "source.article")[0]
    assert article["claim"]["summary"].endswith("(truncated)")
    assert len(article["claim"]["summary"]) <= 300 + len(" (truncated)")
    assert "parse-warning: body truncated from" in article["rationale"]
    assert any(w.startswith("parse-warning: body truncated") for w in out["warnings"])


def test_heuristic_note_body_over_limit_is_truncated_too(starter, monkeypatch):
    monkeypatch.setenv("FFS_SCRIBE_BODY_LIMIT", "300")
    out = _handle("# Just prose\n\n" + "text " * 200)
    note = _by_pred(out, "note")[0]
    assert note["claim"]["body"].endswith("(truncated)")
    assert "parse-warning" in note["rationale"]


def test_digest_note_references_are_populated_from_bullets(starter):
    text = """---
predicate: note
title: Example Weekly digest 2026-09-20
---
Filed today.

## References
- [[example-weekly-2026-09-20-example-plant|Example maker opens second plant]]
- https://example.com/news/other-story?utm_medium=email
- [[example-weekly-2026-09-20-example-plant|dup]]
"""
    out = _handle(text)
    note = _by_pred(out, "note")[0]
    assert note["claim"]["references"] == [
        "example-weekly-2026-09-20-example-plant",
        "https://example.com/news/other-story?utm_medium=email",
    ]
    assert note["claim"]["title"] == "Example Weekly digest 2026-09-20"


def test_heuristic_note_without_hint_also_gets_references(starter):
    out = _handle("Read these.\n\n## References\n- https://example.com/a\n- [[some-basename|Some]]\n")
    note = _by_pred(out, "note")[0]
    assert note["claim"]["references"] == ["https://example.com/a", "some-basename"]


def test_mentions_without_person_predicate_stay_on_article_with_warning(tmp_path, monkeypatch):
    # A registry with only source.article: mentions stay on the article.
    from corpus_scorer import install_query_stub, load_starter_schemas  # type: ignore

    schemas = load_starter_schemas()
    install_query_stub(monkeypatch, {"source.article": schemas["source.article"]})
    monkeypatch.setenv("FFS_DATA_DIR", str(tmp_path))
    out = _handle(ARTICLE)
    assert [p["predicate"] for p in out["proposals"]] == ["source.article"]
    assert len(out["proposals"][0]["claim"]["mentions"]) == 3
    assert any("no person or organization predicate" in w for w in out["warnings"])


def test_classification_helpers():
    from contract import classify_mention, looks_like_org, looks_like_person  # type: ignore

    assert looks_like_org("Acme Widgets Inc.")
    assert looks_like_person("Pat Example")
    assert not looks_like_person("Mecklenburg County")
    assert classify_mention("Sam Rivera", "board member") == "person"
    assert classify_mention("Riverbend", "") == "org"
    assert classify_mention("Sara Chen", "") == "person"


def test_serialisable(starter):
    json.dumps(_handle(ARTICLE))
