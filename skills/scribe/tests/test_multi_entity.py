"""task_45: the multi-entity conventions, exercised deterministically.

Every llm-only corpus fixture carries the model envelope a capable
model produced (``model_output.json``, one per article). These tests
feed that envelope through ``LlmEngine`` with a fake transport, so the
binding of cross-references, the affiliation conventions, the article
provenance, and the per-fixture expectations are all checked in CI
with no network and no model.

Schemas are the starter TOMLs on disk, so registering a predicate spec
changes what the engine accepts with no code change here.
"""

from __future__ import annotations

import json
import os
from typing import Any, Dict, List

import pytest

import corpus_scorer as cs  # type: ignore  # conftest put tests/ on sys.path
import llm  # type: ignore
from engine import Submission  # type: ignore
from registry import PredicateRegistry  # type: ignore

LLM_FIXTURES: List[cs.Fixture] = [f for f in cs.load_corpus(cs.IN_REPO_CORPUS) if f.engine == "llm"]
PRESS = [f for f in LLM_FIXTURES if not f.identity]
IDENTITY = [f for f in LLM_FIXTURES if f.identity]


class CannedTransport:
    """Returns the fixture's canned envelope as an Ollama reply."""

    def __init__(self, envelope: Dict[str, Any]):
        self.envelope = envelope
        self.requests: List[Dict[str, Any]] = []

    def post_json(self, url, headers, body, timeout):
        self.requests.append({"url": url, "body": body})
        return {"message": {"role": "assistant", "content": json.dumps(self.envelope)}}


def _registry() -> PredicateRegistry:
    schemas = cs.load_starter_schemas()
    assert schemas, "starter predicate TOMLs not found"
    return PredicateRegistry.from_specs({k: {"claim_schema": v} for k, v in schemas.items()})


def _run(fixture: cs.Fixture, article: str = "input.md"):
    envelope = fixture.model_output(article)
    assert envelope is not None, f"{fixture.id}: no model output for {article}"
    transport = CannedTransport(envelope)
    engine = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "canned", transport))
    sub = Submission.from_input({"source_uri": f"file:///ingest/{fixture.filename}", "content": fixture.article_input(article)})
    return engine.extract(sub, _registry()), transport


def _by_ref(proposals):
    return {p["local_ref"]: p for p in proposals}


@pytest.mark.parametrize("fixture", PRESS, ids=[f.id for f in PRESS])
def test_press_fixture_matches_expected_multi_entity_proposals(fixture: cs.Fixture):
    result, transport = _run(fixture)
    assert not result.warnings, f"{fixture.id}: {result.warnings}"
    # the prompt was rendered from the registry, with the role guidance present
    system = transport.requests[0]["body"]["messages"][0]["content"]
    assert "affiliation" in system and "local_ref" in system
    s = cs.score(fixture.expected, result.proposals, fixture.forbid, fixture.id)
    detail = "\n".join(f"  {m.predicate}: {m.matched_fields}/{m.expected_fields} misses={m.misses}" for m in s.matches)
    assert not s.forbidden_hits, f"{fixture.id}: forbidden {s.forbidden_hits}"
    assert s.all_matched, f"{fixture.id}\n{detail}\n{json.dumps(result.proposals, indent=1)[:3000]}"


@pytest.mark.parametrize("fixture", PRESS, ids=[f.id for f in PRESS])
def test_press_fixture_conventions_hold(fixture: cs.Fixture):
    result, _ = _run(fixture)
    props = result.proposals
    refs = [p["local_ref"] for p in props]
    assert len(refs) == len(set(refs)), "local_refs unique within the set"
    url = next(p["claim"]["url"] for p in props if p["predicate"] == "source.article")
    for p in props:
        prov = [(e["kind"], e["uri"]) for e in p["provenance"]]
        assert prov[0][0] == "ingest"
        assert ("source_article", url) in prov, f"{p['local_ref']} lacks the article provenance"
        for r in p.get("refs", []):
            assert r["local_ref"] in refs and r["local_ref"] != p["local_ref"]
        # the scribe never invents ids in object lists
        for f in ("mentions", "participants"):
            for item in p["claim"].get(f) or []:
                assert "entity" not in item
        if p["predicate"] == "affiliation":
            assert p["claim"]["kind"] in {"employee", "executive", "board", "founder", "investor", "advisor", "member", "other"}
            if p.get("ends_role"):
                assert "valid_to" in p


def test_exec_hire_fixture_is_the_task_45_unit_case():
    """task_45 unit test: article + person + org + event(hire) + affiliation, all cross-referenced."""
    fx = next(f for f in PRESS if f.id == "21-exec-hire")
    result, _ = _run(fx)
    by = _by_ref(result.proposals)
    preds = sorted(p["predicate"] for p in result.proposals)
    assert preds.count("source.article") == 1 and preds.count("event.business") == 1
    assert preds.count("person.generic") == 2 and preds.count("org.company") == 2 and preds.count("affiliation") == 3
    hire = by["event-1"]
    assert hire["claim"]["kind"] == "hire"
    assert {(r["field"], r["local_ref"]) for r in hire["refs"]} == {("participants[0].entity", "person-1"), ("participants[1].entity", "org-1")}
    assert {(r["field"], r["local_ref"]) for r in by["person-1"]["refs"]} == {("organization", "org-1")}
    assert by["aff-2"]["ends_role"] is True and by["aff-2"]["valid_to"] == "2026-09-30"
    assert by["aff-1"]["valid_from"] == "2026-10-01" and "ends_role" not in by["aff-1"]


@pytest.mark.parametrize("fixture", IDENTITY, ids=[f.id for f in IDENTITY])
def test_identity_fixture_articles_extract_and_bind(fixture: cs.Fixture):
    """Python checks extraction and refs per article; the clustering in
    expected_identity.json is consumed by the daemon resolver tests."""
    identity = fixture.expected_identity()
    assert identity and "clusters" in identity
    seen_displays = set()
    for article in fixture.articles:
        result, _ = _run(fixture, article)
        assert not result.warnings, f"{fixture.id}/{article}: {result.warnings}"
        people = [p for p in result.proposals if p["predicate"] == "person.generic"]
        assert people, f"{fixture.id}/{article}: no person proposal"
        for p in people:
            assert any(r["field"] == "organization" for r in p.get("refs", [])), f"{fixture.id}/{article}: person not bound to its org"
            seen_displays.add(p["claim"]["display_name"])
    expected_displays = {m["display"] for members in identity["clusters"].values() for m in members if m["predicate"] == "person.generic"}
    assert expected_displays <= seen_displays, f"{fixture.id}: expected {expected_displays}, extracted {seen_displays}"
    s = cs.score(fixture.expected, _run(fixture, fixture.articles[0])[0].proposals, fixture.forbid, fixture.id)
    assert s.all_matched, f"{fixture.id}: first-article expectation not matched: {[m.misses for m in s.matches]}"


def test_scorer_refs_count_and_top_semantics():
    actual = [{"predicate": "affiliation", "claim": {"person": "A"}, "refs": [{"field": "person", "local_ref": "p-1"}], "ends_role": True}]
    ok = cs.score([{"predicate": "affiliation", "claim": {"person": "a"}, "refs_count": 1, "top": {"ends_role": True}}], actual)
    assert ok.all_matched and ok.matches[0].expected_fields == 3
    bad = cs.score([{"predicate": "affiliation", "claim": {"person": "a"}, "refs_count": 2, "top": {"valid_to": "2026-01-01"}}], actual)
    assert not bad.all_matched and set(bad.matches[0].misses) == {"refs_count(1!=2)", "top.valid_to"}
