"""Tests for the `predicate:` frontmatter hint (task_36 § 36.4).

A registered hint narrows extraction to that predicate. `source.article`
or any unregistered predicate falls back to a `note` that keeps the
article's title, url, and summary so a courier drop is readable in the
vault before task_38 exists. The hint never raises.
"""

from __future__ import annotations

from typing import Any, Dict

import extraction  # type: ignore
from engine import EngineResult, Submission, apply_hint, make_proposal  # type: ignore
from registry import PredicateRegistry  # type: ignore

SCHEMAS: Dict[str, Any] = {
    "contact.person": {
        "type": "object",
        "required": ["display_name"],
        "properties": {"display_name": {"type": "string"}, "email": {"type": "string"}, "notes": {"type": "array"}},
    },
    "person.generic": {"type": "object", "required": ["display_name"], "properties": {"display_name": {"type": "string"}}},
    "note": {
        "type": "object",
        "required": ["title"],
        "properties": {
            "title": {"type": "string"},
            "author": {"type": "string"},
            "body": {"type": "string"},
            "tags": {"type": "array", "items": {"type": "string"}},
            "references": {"type": "array", "items": {"type": "string"}},
        },
    },
}


def _registry():
    return PredicateRegistry.from_specs({k: {"claim_schema": v} for k, v in SCHEMAS.items()})


def _install_fake_query(monkeypatch):
    def fake_query(method, params):
        return {"claim_schema": SCHEMAS.get(params["name"], {})}

    monkeypatch.setattr(extraction, "query", fake_query)


# ---------------------------------------------------------------------
# apply_hint unit level
# ---------------------------------------------------------------------


def test_no_hint_returns_result_unchanged():
    sub = Submission.from_input({"source_uri": "file:///a.md", "content": "hi"})
    res = EngineResult(proposals=[{"predicate": "note"}], warnings=["w"])
    out = apply_hint(sub, res, _registry(), "heuristic", "")
    assert out is res


def test_registered_hint_keeps_only_that_predicate():
    sub = Submission.from_input(
        {"source_uri": "file:///a.md", "content": "---\npredicate: contact.person\nname: Sara\nemail: s@x.com\n---\nbody\n"}
    )
    res = EngineResult(
        proposals=[
            make_proposal("contact.person", {"display_name": "Sara"}, sub, "r", "heuristic", ""),
            make_proposal("note", {"title": "x"}, sub, "r", "heuristic", ""),
        ],
        warnings=[],
    )
    out = apply_hint(sub, res, _registry(), "heuristic", "")
    assert [p["predicate"] for p in out.proposals] == ["contact.person"]


def test_registered_hint_with_no_matching_proposal_keeps_all_and_warns():
    sub = Submission.from_input({"source_uri": "file:///a.md", "content": "---\npredicate: person.generic\n---\nbody\n"})
    res = EngineResult(proposals=[make_proposal("note", {"title": "x"}, sub, "r", "heuristic", "")], warnings=[])
    out = apply_hint(sub, res, _registry(), "heuristic", "")
    assert [p["predicate"] for p in out.proposals] == ["note"]
    assert any("person.generic" in w for w in out.warnings)


ARTICLE = (
    "---\n"
    "predicate: source.article\n"
    "title: Widget maker opens second plant\n"
    "url: https://example.com/news/2026/09/14/widget-plant.html\n"
    "publication: Example Business Daily\n"
    "byline: Pat Reporter\n"
    "published_at: 2026-09-14\n"
    "summary: Acme Widgets will open a second plant in Gastonia, adding 120 jobs.\n"
    "tags: manufacturing, jobs\n"
    "---\n"
    "\n"
    "## Mentions\n"
    "- Acme Widgets — the company expanding\n"
    "- Jane Doe — chief executive, quoted\n"
    "\n"
    "See also https://example.com/related and https://example.com/related again.\n"
)


def test_source_article_hint_falls_back_to_note_with_title_url_summary():
    sub = Submission.from_input({"source_uri": "file:///ingest/widget-plant.md", "content": ARTICLE})
    res = EngineResult(proposals=[make_proposal("note", {"title": "wrong"}, sub, "r", "heuristic", "")], warnings=[])
    out = apply_hint(sub, res, _registry(), "llm", "m")
    assert len(out.proposals) == 1
    note = out.proposals[0]
    assert note["predicate"] == "note"
    assert note["engine"] == "llm" and note["model"] == "m"
    claim = note["claim"]
    assert claim["title"] == "Widget maker opens second plant"
    assert claim["body"].startswith("Acme Widgets will open a second plant")
    assert "Jane Doe" in claim["body"]
    assert claim["references"][0] == "https://example.com/news/2026/09/14/widget-plant.html"
    assert claim["references"].count("https://example.com/related") == 1
    assert "source-article" in claim["tags"] and "manufacturing" in claim["tags"]
    assert claim["author"] == "Pat Reporter"
    assert "source.article" in note["rationale"]


def test_unregistered_hint_falls_back_to_note_with_filename_title():
    sub = Submission.from_input(
        {"source_uri": "file:///ingest/Board%20Minutes.md", "content": "---\npredicate: org.company\n---\nSome body text.\n"}
    )
    out = apply_hint(sub, EngineResult(proposals=[], warnings=[]), _registry(), "heuristic", "")
    assert out.proposals[0]["claim"]["title"] == "Board Minutes"
    assert "org.company" in out.proposals[0]["rationale"]
    assert "source-article" not in out.proposals[0]["claim"].get("tags", [])


def test_unregistered_hint_uses_first_heading_when_no_title():
    sub = Submission.from_input({"source_uri": "mcp:agent/x", "content": "---\npredicate: event.business\n---\n# Big Deal\ntext\n"})
    out = apply_hint(sub, EngineResult(proposals=[], warnings=[]), _registry(), "heuristic", "")
    assert out.proposals[0]["claim"]["title"] == "Big Deal"


def test_hint_never_raises_on_garbage(monkeypatch):
    sub = Submission.from_input({"source_uri": "file:///a.md", "content": "---\npredicate: ???\n---\n"})
    # Even if the registry explodes, apply_hint returns a result.
    class Boom:
        def has(self, name):
            raise RuntimeError("registry down")

    out = apply_hint(sub, EngineResult(proposals=[], warnings=[]), Boom(), "heuristic", "")
    assert isinstance(out, EngineResult)
    assert out.proposals and out.proposals[0]["predicate"] == "note"


# ---------------------------------------------------------------------
# End to end through handle()
# ---------------------------------------------------------------------


def test_handle_with_contact_hint_targets_contact_person(monkeypatch):
    _install_fake_query(monkeypatch)
    md = "---\npredicate: contact.person\nname: Sara\nemail: sara@example.com\n---\n\nLong body text that would also make a note.\n"
    result = extraction.handle({"source_uri": "file:///s.md", "content": md})
    assert [p["predicate"] for p in result["proposals"]] == ["contact.person"]
    assert result["proposals"][0]["engine"] == "heuristic"


def test_handle_with_article_hint_lands_as_note_with_url_reference(monkeypatch):
    _install_fake_query(monkeypatch)
    result = extraction.handle({"source_uri": "file:///ingest/widget-plant.md", "content": ARTICLE})
    assert len(result["proposals"]) == 1
    note = result["proposals"][0]
    assert note["predicate"] == "note"
    assert "https://example.com/news/2026/09/14/widget-plant.html" in note["claim"]["references"]
    assert note["claim"]["title"] == "Widget maker opens second plant"


def test_flow_list_tags_string_is_split_without_brackets(monkeypatch):
    _install_fake_query(monkeypatch)
    from extraction import handle

    content = (
        "---\npredicate: source.article\ntitle: T\nurl: https://example.com/a\n"
        "tags: [manufacturing, jobs]\n---\nbody\n"
    )
    out = handle({"source_uri": "file:///ingest/a.md", "content": content})
    note = next(p for p in out["proposals"] if p["predicate"] == "note")
    assert note["claim"]["tags"] == ["manufacturing", "jobs", "source-article"]
