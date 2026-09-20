"""Golden-corpus regression tests for the scribe's heuristic engine
(task_36). Every fixture under ``tests/corpus/`` runs through
``extraction.handle`` and must reproduce its expected proposals.

Fixture ids tell you what a failure means:

- ``required: true`` fixtures gate the task (Jon Jones, the
  frontmatter contact, the predicate hint, the source.article
  fallback, the unknown hint).
- ``awaits`` names the task_36 subtask whose landing makes the
  fixture pass, so a red fixture is attributable.

Schemas come from the starter TOMLs two ways at once: the registry
env (``FFS_DATA_DIR/config/predicates``) for an engine that reads
specs from disk, and a ``predicate.inspect`` stub for the host-query
path. Unknown predicates raise the way the host would.

The heuristic engine is the engine under test; ``FFS_SCRIBE_ENGINE``
is cleared so a developer's shell setting cannot leak in.
"""

from __future__ import annotations

import json
import os
from typing import List

import pytest

import corpus_scorer as cs  # type: ignore  # conftest put tests/ on sys.path
import extraction  # type: ignore

FIXTURES: List[cs.Fixture] = cs.load_corpus(cs.IN_REPO_CORPUS)
REQUIRED_IDS = {f.id for f in FIXTURES if f.required}

_SCORES: List[cs.Score] = []


def _ids(fixtures: List[cs.Fixture]) -> List[str]:
    return [f.id + (f" [awaits {f.awaits}]" if f.awaits else "") for f in fixtures]


@pytest.fixture
def engine_env(tmp_path, monkeypatch):
    monkeypatch.delenv("FFS_SCRIBE_ENGINE", raising=False)
    monkeypatch.delenv("FFS_SCRIBE_CORPUS_DIR", raising=False)
    cs.starter_registry_env(tmp_path, monkeypatch)
    cs.install_query_stub(monkeypatch)


def test_corpus_has_twenty_fixtures():
    assert len(FIXTURES) >= 20
    assert {"01-jon-jones-card", "10-hint-source-article", "11-hint-unknown-predicate", "02-frontmatter-contact"} <= REQUIRED_IDS


@pytest.mark.parametrize("fixture", FIXTURES, ids=_ids(FIXTURES))
def test_fixture_expected_proposals_are_matched(fixture: cs.Fixture, engine_env):
    actual = cs.run_engine(extraction.handle, fixture)
    s = cs.score(fixture.expected, actual, fixture.forbid, fixture.id)
    _SCORES.append(s)
    detail = "\n".join(
        f"  {m.predicate}: matched {m.matched_fields}/{m.expected_fields}, misses={m.misses}, soft_hits={m.soft_hits}"
        for m in s.matches
    )
    assert not s.forbidden_hits, f"{fixture.id}: forbidden predicates produced: {s.forbidden_hits}\n{json.dumps(actual, indent=1)[:1500]}"
    assert s.all_matched, (
        f"{fixture.id}: expected proposals not fully matched"
        + (f" (awaits {fixture.awaits})" if fixture.awaits else "")
        + f"\n{detail}\nextras={s.extras}\nactual={json.dumps(actual, indent=1)[:2000]}"
    )


def test_required_fixtures_have_full_recall(engine_env):
    """The task's gate: every required fixture at recall 1.0."""
    failures = []
    for f in FIXTURES:
        if not f.required:
            continue
        s = cs.score(f.expected, cs.run_engine(extraction.handle, f), f.forbid, f.id)
        if not s.all_matched:
            failures.append(f"{f.id} (awaits {f.awaits})" if f.awaits else f.id)
    assert not failures, f"required fixtures below recall 1.0: {failures}"


def test_corpus_dir_env_override_is_honored(tmp_path, monkeypatch):
    ext = tmp_path / "external-corpus" / "01-only"
    ext.mkdir(parents=True)
    (ext / "input.md").write_text("Only fixture\n\nbody\n", encoding="utf-8")
    (ext / "expected.json").write_text(
        json.dumps({"filename": "only.md", "proposals": [{"predicate": "note", "claim": {"title": "Only fixture"}}]}),
        encoding="utf-8",
    )
    (tmp_path / "external-corpus" / "junk").mkdir()  # no expected.json: skipped
    monkeypatch.setenv("FFS_SCRIBE_CORPUS_DIR", str(tmp_path / "external-corpus"))
    assert cs.corpus_dir() == str(tmp_path / "external-corpus")
    loaded = cs.load_corpus()
    assert [f.id for f in loaded] == ["01-only"]
    assert loaded[0].source_uri == "file:///ingest/only.md"
    monkeypatch.delenv("FFS_SCRIBE_CORPUS_DIR")
    assert cs.corpus_dir() == cs.IN_REPO_CORPUS


def test_scorer_field_semantics():
    assert cs.field_matches("Jon  Jones", "jon jones")
    assert cs.field_matches(["a", "b"], ["B", "c", "A"])
    assert not cs.field_matches(["a", "b"], ["a"])
    assert cs.field_matches({"$contains": "mill"}, "the Riverside Mill reopens")
    assert cs.field_matches(3, 3) and not cs.field_matches(3, True)
    s = cs.score(
        [{"predicate": "note", "claim": {"title": "x"}, "soft": {"body": "y"}}],
        [{"predicate": "note", "claim": {"title": "X", "body": "y"}}, {"predicate": "note", "claim": {"title": "other"}}],
        forbid=["contact.person"],
    )
    assert s.all_matched and s.recall == 1.0 and s.extras == ["note"]
    assert s.matches[0].soft_hits == ["body"]
    table = cs.format_table(cs.summarize([s]))
    assert "| note |" in table


@pytest.fixture(scope="module", autouse=True)
def _print_summary_at_end():
    yield
    if _SCORES:
        print("\n\nscribe heuristic corpus scores\n" + cs.format_table(cs.summarize(_SCORES)))
