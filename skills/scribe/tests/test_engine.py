"""Seam contract tests for the scribe's `ExtractionEngine` (task_36).

Covers: the `Submission` builder, engine selection via
`FFS_SCRIBE_ENGINE`, the fallback to heuristic when the llm engine is
unavailable, `make_proposal` carrying engine + model, and the minimal
schema validator in `validate.py`.
"""

from __future__ import annotations

import sys
import types

import engine as engine_mod  # type: ignore
import validate  # type: ignore
from engine import EngineResult, Submission, make_proposal, select_engine  # type: ignore
from heuristic import HeuristicEngine  # type: ignore
from registry import PredicateRegistry  # type: ignore


# ---------------------------------------------------------------------
# Submission
# ---------------------------------------------------------------------


def test_submission_from_input_parses_frontmatter_and_filename():
    sub = Submission.from_input(
        {"source_uri": "file:///home/u/.ffs/ingest/Jon%20Jones.md", "content": "---\nname: Jon\n---\nhi\n"}
    )
    assert sub.filename == "Jon Jones"
    assert sub.frontmatter["name"] == "Jon"
    assert sub.content_bytes == sub.content_text.encode("utf-8")
    assert len(sub.content_hash_hex) == 64
    assert sub.predicate_hint is None


def test_submission_filename_none_for_non_file_uri():
    sub = Submission.from_input({"source_uri": "mcp:agent/claude", "content": "x"})
    assert sub.filename is None


def test_submission_predicate_hint_is_normalized():
    sub = Submission.from_input(
        {"source_uri": "file:///a.md", "content": "---\npredicate:  Source.Article \n---\n"}
    )
    assert sub.predicate_hint == "source.article"


def test_submission_accepts_bytes_content():
    sub = Submission.from_input({"source_uri": "file:///a.md", "content": b"caf\xc3\xa9"})
    assert sub.content_text == "café"


# ---------------------------------------------------------------------
# Engine selection
# ---------------------------------------------------------------------


def test_heuristic_engine_satisfies_the_protocol():
    eng = HeuristicEngine()
    assert eng.name == "heuristic"
    assert eng.model == ""
    sub = Submission.from_input({"source_uri": "file:///n.md", "content": "Just thoughts.\n"})
    result = eng.extract(sub, PredicateRegistry.from_specs({}))
    assert isinstance(result, EngineResult)
    assert result.proposals and result.proposals[0]["engine"] == "heuristic"
    assert result.proposals[0]["model"] == ""


def test_select_engine_defaults_to_heuristic():
    assert select_engine({}).name == "heuristic"


def test_select_engine_unknown_value_falls_back_to_heuristic():
    assert select_engine({"FFS_SCRIBE_ENGINE": "quantum"}).name == "heuristic"


def test_select_engine_llm_import_failure_falls_back(monkeypatch):
    """If `llm.py` cannot be imported (missing or broken), the scribe
    must keep working on the heuristic engine."""
    monkeypatch.setitem(sys.modules, "llm", None)  # forces ImportError
    assert select_engine({"FFS_SCRIBE_ENGINE": "llm"}).name == "heuristic"


def test_select_engine_llm_constructor_failure_falls_back(monkeypatch):
    fake = types.ModuleType("llm")

    class LlmEngine:  # noqa: D401 - stub
        @classmethod
        def from_env(cls, env):
            raise RuntimeError("no backend configured")

    fake.LlmEngine = LlmEngine
    monkeypatch.setitem(sys.modules, "llm", fake)
    assert select_engine({"FFS_SCRIBE_ENGINE": "llm"}).name == "heuristic"


def test_select_engine_llm_uses_the_llm_module_when_available(monkeypatch):
    fake = types.ModuleType("llm")

    class LlmEngine:
        name = "llm"
        model = "stub-model"

        @classmethod
        def from_env(cls, env):
            return cls()

        def extract(self, submission, registry):
            return EngineResult(proposals=[], warnings=[])

    fake.LlmEngine = LlmEngine
    monkeypatch.setitem(sys.modules, "llm", fake)
    eng = select_engine({"FFS_SCRIBE_ENGINE": "llm"})
    assert eng.name == "llm"
    assert eng.model == "stub-model"


# ---------------------------------------------------------------------
# make_proposal
# ---------------------------------------------------------------------


def test_make_proposal_carries_engine_model_and_provenance():
    sub = Submission.from_input({"source_uri": "file:///a.md", "content": "hi"})
    p = make_proposal("note", {"title": "t"}, sub, "why", "llm", "sonnet")
    assert p["predicate"] == "note"
    assert p["engine"] == "llm"
    assert p["model"] == "sonnet"
    assert p["provenance"][0] == {"kind": "ingest", "uri": "file:///a.md", "hash_hex": sub.content_hash_hex}
    assert p["rationale"] == "why"


# ---------------------------------------------------------------------
# validate.py
# ---------------------------------------------------------------------


def test_validate_required_and_primitive_types():
    schema = {"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}, "n": {"type": "integer"}}}
    assert validate.validate_claim({"title": "x"}, schema) is None
    assert "missing required" in validate.validate_claim({}, schema)
    assert "must be a string" in validate.validate_claim({"title": 3}, schema)
    assert "must be an integer" in validate.validate_claim({"title": "x", "n": "3"}, schema)


def test_validate_enum_on_strings():
    schema = {"type": "object", "properties": {"status": {"type": "string", "enum": ["draft", "published"]}}}
    assert validate.validate_claim({"status": "draft"}, schema) is None
    assert "not one of" in validate.validate_claim({"status": "nope"}, schema)


def test_validate_array_item_types():
    schema = {"type": "object", "properties": {"tags": {"type": "array", "items": {"type": "string"}}}}
    assert validate.validate_claim({"tags": ["a", "b"]}, schema) is None
    assert "tags[1]" in validate.validate_claim({"tags": ["a", 2]}, schema)


def test_validate_array_of_objects_with_required_one_level():
    schema = {
        "type": "object",
        "properties": {
            "mentions": {
                "type": "array",
                "items": {"type": "object", "required": ["display"], "properties": {"display": {"type": "string"}}},
            }
        },
    }
    assert validate.validate_claim({"mentions": [{"display": "A", "context": "x"}]}, schema) is None
    assert "missing required" in validate.validate_claim({"mentions": [{"context": "x"}]}, schema)
    assert "must be an object" in validate.validate_claim({"mentions": ["A"]}, schema)


def test_validate_nested_object_required():
    schema = {
        "type": "object",
        "properties": {"scope": {"type": "object", "required": ["tier"], "properties": {"tier": {"type": "string"}}}},
    }
    assert validate.validate_claim({"scope": {"tier": "x"}}, schema) is None
    assert "scope" in validate.validate_claim({"scope": {}}, schema)


def test_validate_non_dict_schema_passes():
    assert validate.validate_claim({"anything": 1}, None) is None


def test_extraction_keeps_the_old_validator_alias():
    import extraction  # type: ignore

    assert extraction._validate_claim_against_schema is validate.validate_claim
