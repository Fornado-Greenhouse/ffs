"""Tests for the llm engine: backends, envelope parsing, fallback (task_36).

No network: every test injects a ``FakeTransport``.
"""

import json
import os
import sys

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.insert(0, _HERE)



from engine import Submission  # noqa: E402
from registry import PredicateRegistry  # noqa: E402

import llm  # noqa: E402

CONTACT = {
    "description": "A person you have a relationship with.",
    "claim_schema": {
        "type": "object",
        "required": ["display_name"],
        "properties": {"display_name": {"type": "string"}, "phone": {"type": "string"}},
    },
}
NOTE = {
    "description": "Free-form narrative note.",
    "claim_schema": {
        "type": "object",
        "required": ["title"],
        "properties": {"title": {"type": "string"}, "body": {"type": "string"}},
    },
}


def _registry():
    return PredicateRegistry.from_specs({"contact.person": CONTACT, "note": NOTE})


def _sub(text="Jon Jones\nPhone: 919-428-4074", hint=None):
    return Submission(
        source_uri="file:///tmp/Jon Jones.md",
        content_text=text,
        content_bytes=text.encode(),
        content_hash_hex="ab",
        filename="Jon Jones.md",
        predicate_hint=hint,
    )


class FakeTransport:
    def __init__(self, response=None, error=None):
        self.requests = []
        self.response = response
        self.error = error

    def post_json(self, url, headers, body, timeout):
        self.requests.append({"url": url, "headers": dict(headers), "body": body, "timeout": timeout})
        if self.error is not None:
            raise self.error
        return self.response


def _ollama_reply(envelope):
    return {"message": {"role": "assistant", "content": json.dumps(envelope)}}


def _anthropic_reply(text):
    return {"content": [{"type": "text", "text": text}], "model": "claude-sonnet-5"}


GOOD_ENVELOPE = {
    "proposals": [
        {
            "predicate": "contact.person",
            "claim": {"display_name": "Jon Jones", "phone": "919-428-4074"},
            "rationale": "name from filename and first line; phone from the Phone field",
        }
    ],
    "summary": "A contact card for Jon Jones.",
}


# ---- backends ----


def test_ollama_request_shape():
    t = FakeTransport(response=_ollama_reply(GOOD_ENVELOPE))
    b = llm.OllamaBackend("http://localhost:11434/", "llama3.1:8b", t)
    out = b.complete("SYS", "USER")
    assert json.loads(out) == GOOD_ENVELOPE
    req = t.requests[0]
    assert req["url"] == "http://localhost:11434/api/chat"
    assert req["body"]["model"] == "llama3.1:8b"
    assert req["body"]["messages"] == [
        {"role": "system", "content": "SYS"},
        {"role": "user", "content": "USER"},
    ]
    assert req["body"]["stream"] is False
    assert req["body"]["format"] == "json"
    assert req["body"]["options"] == {"temperature": 0}


def test_ollama_default_model_when_blank():
    b = llm.OllamaBackend("http://localhost:11434", "", FakeTransport())
    assert b.model == llm.DEFAULT_OLLAMA_MODEL


def test_anthropic_request_shape_and_headers():
    t = FakeTransport(response=_anthropic_reply(json.dumps(GOOD_ENVELOPE)))
    b = llm.AnthropicBackend("https://api.anthropic.com", "claude-sonnet-5", "sk-test", t)
    out = b.complete("SYS", "USER")
    assert json.loads(out) == GOOD_ENVELOPE
    req = t.requests[0]
    assert req["url"] == "https://api.anthropic.com/v1/messages"
    assert req["headers"]["x-api-key"] == "sk-test"
    assert req["headers"]["anthropic-version"] == "2023-06-01"
    assert req["headers"]["content-type"] == "application/json"
    assert req["body"]["model"] == "claude-sonnet-5"
    assert req["body"]["max_tokens"] == 4096
    assert req["body"]["system"] == [{"type": "text", "text": "SYS"}]
    assert req["body"]["messages"] == [{"role": "user", "content": "USER"}]


def test_anthropic_missing_key_errors_at_construction():
    with pytest.raises(llm.LlmError):
        llm.AnthropicBackend("https://api.anthropic.com", "", None, FakeTransport())


def test_select_backend_by_host():
    t = FakeTransport()
    assert isinstance(llm.select_backend("http://localhost:11434", "", None, t), llm.OllamaBackend)
    assert isinstance(
        llm.select_backend("https://api.anthropic.com", "", "k", t), llm.AnthropicBackend
    )
    assert llm.select_backend("https://api.anthropic.com", "", "k", t).model == llm.DEFAULT_ANTHROPIC_MODEL


def test_from_env_defaults_to_ollama(monkeypatch):
    env = {}
    eng = llm.LlmEngine.from_env(env, transport=FakeTransport())
    assert isinstance(eng.backend, llm.OllamaBackend)
    assert eng.backend.url == llm.DEFAULT_LLM_URL
    assert eng.name == "llm"
    assert eng.model == llm.DEFAULT_OLLAMA_MODEL


def test_from_env_anthropic_uses_env_key():
    env = {
        "FFS_SCRIBE_LLM_URL": "https://api.anthropic.com",
        "FFS_SCRIBE_LLM_MODEL": "claude-sonnet-5",
        "FFS_SCRIBE_ANTHROPIC_KEY": "sk-env",
    }
    eng = llm.LlmEngine.from_env(env, transport=FakeTransport())
    assert isinstance(eng.backend, llm.AnthropicBackend)
    assert eng.backend.api_key == "sk-env"


# ---- envelope parsing ----


def test_parse_envelope_strips_fences_and_prose():
    text = "Sure, here it is:\n```json\n" + json.dumps(GOOD_ENVELOPE) + "\n```\nHope this helps."
    assert llm.parse_envelope(text) == GOOD_ENVELOPE


def test_parse_envelope_takes_first_balanced_object_with_trailing_prose():
    text = "Result: " + json.dumps(GOOD_ENVELOPE) + " and that's all {not json"
    assert llm.parse_envelope(text) == GOOD_ENVELOPE


def test_parse_envelope_handles_braces_inside_strings():
    env = {"proposals": [], "summary": "a {brace} in text"}
    assert llm.parse_envelope("x " + json.dumps(env) + " y") == env


def test_parse_envelope_rejects_no_object():
    with pytest.raises(llm.LlmError):
        llm.parse_envelope("no json here")


# ---- engine happy path ----


def test_engine_emits_validated_proposals_with_llm_provenance():
    t = FakeTransport(response=_ollama_reply(GOOD_ENVELOPE))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "llama3.1:8b", t))
    result = eng.extract(_sub(), _registry())
    assert len(result.proposals) == 1
    p = result.proposals[0]
    assert p["predicate"] == "contact.person"
    assert p["claim"]["display_name"] == "Jon Jones"
    assert p["engine"] == "llm"
    assert p["model"] == "llama3.1:8b"
    assert "(llm" in p["rationale"]
    assert "summary: A contact card for Jon Jones." in p["rationale"]
    assert result.warnings == []


def test_hint_adds_target_line_to_system_prompt():
    t = FakeTransport(response=_ollama_reply(GOOD_ENVELOPE))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    eng.extract(_sub(hint="contact.person"), _registry())
    system = t.requests[0]["body"]["messages"][0]["content"]
    assert "Target predicate: contact.person" in system


# ---- drops and fallback ----


def test_schema_invalid_claim_dropped_with_warning_and_valid_kept():
    env = {
        "proposals": [
            {"predicate": "contact.person", "claim": {"phone": "x"}, "rationale": "no name"},
            {"predicate": "note", "claim": {"title": "T", "body": "B"}, "rationale": "ok"},
        ]
    }
    t = FakeTransport(response=_ollama_reply(env))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    result = eng.extract(_sub(), _registry())
    assert [p["predicate"] for p in result.proposals] == ["note"]
    assert any("missing required field: display_name" in w for w in result.warnings)
    assert result.proposals[0]["engine"] == "llm"


def test_unregistered_predicate_dropped():
    env = {
        "proposals": [
            {"predicate": "org.company", "claim": {"display_name": "Acme"}, "rationale": "x"},
            {"predicate": "note", "claim": {"title": "T"}, "rationale": "ok"},
        ]
    }
    t = FakeTransport(response=_ollama_reply(env))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    result = eng.extract(_sub(), _registry())
    assert [p["predicate"] for p in result.proposals] == ["note"]
    assert any("unregistered predicate 'org.company'" in w for w in result.warnings)


def test_transport_error_triggers_heuristic_fallback():
    t = FakeTransport(error=llm.LlmError("connection refused"))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    result = eng.extract(_sub(), _registry())
    assert result.proposals, "fallback must still produce proposals"
    assert all(p["engine"] == "heuristic" for p in result.proposals)
    assert result.warnings[0].startswith("llm engine failed (LlmError: connection refused); heuristic fallback used")


def test_unparseable_output_triggers_fallback():
    t = FakeTransport(response=_ollama_reply_text("I cannot help with that."))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    result = eng.extract(_sub(), _registry())
    assert all(p["engine"] == "heuristic" for p in result.proposals)
    assert "heuristic fallback used" in result.warnings[0]


def _ollama_reply_text(text):
    return {"message": {"role": "assistant", "content": text}}


def test_zero_valid_proposals_triggers_fallback_and_keeps_drop_warnings():
    env = {"proposals": [{"predicate": "nope", "claim": {}, "rationale": ""}]}
    t = FakeTransport(response=_ollama_reply(env))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    result = eng.extract(_sub(), _registry())
    assert all(p["engine"] == "heuristic" for p in result.proposals)
    assert any("zero schema-valid proposals" in w for w in result.warnings)
    assert any("unregistered predicate" in w for w in result.warnings)


def test_extract_never_raises_even_if_backend_raises_unexpected_type():
    class Boom:
        name = "boom"
        model = "m"

        def complete(self, system, user):
            raise RuntimeError("kaboom")

    eng = llm.LlmEngine(Boom())
    result = eng.extract(_sub(), _registry())
    assert result.proposals
    assert "RuntimeError: kaboom" in result.warnings[0]


# ---- urllib transport error mapping (no network: bad scheme) ----


def test_urllib_transport_maps_url_errors_to_llm_error():
    with pytest.raises(llm.LlmError):
        llm.UrllibTransport().post_json("http://127.0.0.1:1/api/chat", {}, {"a": 1}, timeout=0.5)


# ---- task_45: multi-entity conventions ----

ORG = {
    "description": "An organization.",
    "claim_schema": {
        "type": "object",
        "required": ["display_name"],
        "properties": {"display_name": {"type": "string"}, "aliases": {"type": "array", "items": {"type": "string"}}, "industry": {"type": "string"}},
    },
}
PERSON = {
    "description": "A person in narrative content.",
    "claim_schema": {
        "type": "object",
        "required": ["display_name"],
        "properties": {"display_name": {"type": "string"}, "organization": {"type": "string"}, "role": {"type": "string"}},
    },
}
ARTICLE = {
    "description": "A published article.",
    "claim_schema": {
        "type": "object",
        "required": ["title", "url"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "published_at": {"type": "string"},
            "mentions": {"type": "array", "items": {"type": "object", "required": ["display"], "properties": {"entity": {"type": "string"}, "display": {"type": "string"}, "context": {"type": "string"}}}},
        },
    },
}
EVENT = {
    "description": "A business event.",
    "claim_schema": {
        "type": "object",
        "required": ["title", "kind"],
        "properties": {
            "title": {"type": "string"},
            "kind": {"type": "string", "enum": ["funding", "acquisition", "hire", "departure", "other"]},
            "date": {"type": "string"},
            "source": {"type": "string"},
            "participants": {"type": "array", "items": {"type": "object", "required": ["display"], "properties": {"entity": {"type": "string"}, "display": {"type": "string"}, "role": {"type": "string", "enum": ["hire", "employer", "departing", "other"]}}}},
        },
    },
}
AFFILIATION = {
    "description": "A role held by a person at an organization.",
    "claim_schema": {
        "type": "object",
        "required": ["person", "organization"],
        "properties": {"person": {"type": "string"}, "organization": {"type": "string"}, "title": {"type": "string"}, "kind": {"type": "string", "enum": ["employee", "executive", "board", "other"]}, "source": {"type": "string"}},
    },
}


def _biz_registry():
    return PredicateRegistry.from_specs(
        {"org.company": ORG, "person.generic": PERSON, "source.article": ARTICLE, "event.business": EVENT, "affiliation": AFFILIATION, "note": NOTE}
    )


MULTI_ENVELOPE = {
    "proposals": [
        {"predicate": "source.article", "local_ref": "article-1", "rationale": "the document",
         "claim": {"title": "Harbor Logistics names new chief operating officer", "url": "https://example.com/news/harbor-coo",
                   "published_at": "2026-09-18",
                   "mentions": [{"display": "Dana Whitfield", "context": "named COO"}, {"display": "Harbor Logistics", "context": "the company"}, {"display": "Lee Marsh", "context": "outgoing COO"}]}},
        {"predicate": "org.company", "local_ref": "org-1", "rationale": "the employer", "claim": {"display_name": "Harbor Logistics", "industry": "freight"}},
        {"predicate": "person.generic", "local_ref": "person-1", "rationale": "the hire", "claim": {"display_name": "Dana Whitfield", "organization": "harbor logistics", "role": "Chief Operating Officer"}},
        {"predicate": "person.generic", "local_ref": "person-2", "rationale": "the departing exec", "claim": {"display_name": "Lee Marsh", "organization": "Harbor Logistics"}},
        {"predicate": "event.business", "local_ref": "event-1", "rationale": "the hire event",
         "claim": {"title": "Harbor Logistics hires Dana Whitfield as COO", "kind": "hire", "date": "2026-09-18", "source": "https://example.com/news/harbor-coo",
                   "participants": [{"display": "Dana Whitfield", "role": "hire"}, {"display": "Harbor Logistics", "role": "employer"}]}},
        {"predicate": "affiliation", "rationale": "stated role", "valid_from": "2026-10-01",
         "claim": {"person": "Dana Whitfield", "organization": "Harbor Logistics", "title": "Chief Operating Officer", "kind": "executive", "source": "https://example.com/news/harbor-coo"}},
        {"predicate": "affiliation", "rationale": "stepped down", "ends_role": True, "valid_to": "2026-09-30",
         "claim": {"person": "Lee Marsh", "organization": "Harbor Logistics", "title": "Chief Operating Officer", "kind": "executive", "source": "https://example.com/news/harbor-coo"}},
    ],
    "summary": "Harbor Logistics named Dana Whitfield COO, succeeding Lee Marsh.",
}


def _article_sub():
    text = "Harbor Logistics names new chief operating officer\n\nDana Whitfield will become COO on Oct. 1, succeeding Lee Marsh, who stepped down Sept. 30."
    return Submission(source_uri="file:///ingest/harbor-coo.md", content_text=text, content_bytes=text.encode(), content_hash_hex="c0ffee", filename="harbor-coo")


def _refs(p):
    return {(r["field"], r["local_ref"]) for r in p.get("refs", [])}


def test_multi_entity_envelope_yields_refs_local_refs_and_article_provenance():
    t = FakeTransport(response=_ollama_reply(MULTI_ENVELOPE))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    out = eng.extract(_article_sub(), _biz_registry())
    assert not out.warnings, out.warnings
    by_ref = {p["local_ref"]: p for p in out.proposals}
    assert set(by_ref) == {"article-1", "org-1", "person-1", "person-2", "event-1", "affiliation-1", "affiliation-2"}
    # display references bound within the set, case-insensitively
    assert _refs(by_ref["person-1"]) == {("organization", "org-1")}
    assert _refs(by_ref["person-2"]) == {("organization", "org-1")}
    assert _refs(by_ref["article-1"]) == {("mentions[0].entity", "person-1"), ("mentions[1].entity", "org-1"), ("mentions[2].entity", "person-2")}
    assert _refs(by_ref["event-1"]) == {("participants[0].entity", "person-1"), ("participants[1].entity", "org-1")}
    assert _refs(by_ref["affiliation-1"]) == {("person", "person-1"), ("organization", "org-1")}
    # the scribe never invents ids
    for m in by_ref["article-1"]["claim"]["mentions"]:
        assert "entity" not in m
    # affiliation extras
    assert by_ref["affiliation-1"]["valid_from"] == "2026-10-01"
    assert "ends_role" not in by_ref["affiliation-1"]
    assert by_ref["affiliation-2"]["ends_role"] is True
    assert by_ref["affiliation-2"]["valid_to"] == "2026-09-30"
    # every proposal carries the article url in provenance, once
    for p in out.proposals:
        kinds = [(e["kind"], e["uri"]) for e in p["provenance"]]
        assert kinds.count(("source_article", "https://example.com/news/harbor-coo")) == 1, kinds
        assert kinds[0][0] == "ingest"
        assert p["engine"] == "llm"


def test_single_proposal_envelope_has_no_new_keys_beyond_local_ref():
    t = FakeTransport(response=_ollama_reply(GOOD_ENVELOPE))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    out = eng.extract(_sub(), _registry())
    assert len(out.proposals) == 1
    p = out.proposals[0]
    assert p["local_ref"] == "person-1"
    for key in ("refs", "valid_from", "valid_to", "ends_role"):
        assert key not in p
    assert [e["kind"] for e in p["provenance"]] == ["ingest"], "no article url known, no source_article entry"


def test_malformed_extras_are_dropped_with_warnings_and_proposal_kept():
    env = {
        "proposals": [
            {"predicate": "affiliation", "rationale": "r", "ends_role": "yes", "valid_from": "next spring", "valid_to": 2026, "local_ref": "",
             "claim": {"person": "A B", "organization": "C Co", "kind": "executive"}}
        ]
    }
    t = FakeTransport(response=_ollama_reply(env))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    out = eng.extract(_article_sub(), _biz_registry())
    assert len(out.proposals) == 1
    p = out.proposals[0]
    assert "ends_role" not in p and "valid_from" not in p and "valid_to" not in p
    assert p["local_ref"] == "affiliation-1"
    joined = " ".join(out.warnings)
    assert "ends_role" in joined and "valid_from" in joined and "valid_to" in joined and "local_ref" in joined


def test_article_url_from_submission_frontmatter_is_stamped_when_no_article_proposal():
    env = {"proposals": [{"predicate": "org.company", "rationale": "r", "claim": {"display_name": "Harbor Logistics"}}]}
    t = FakeTransport(response=_ollama_reply(env))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    sub = _article_sub()
    sub.frontmatter = {"url": "https://example.com/fm-url"}
    out = eng.extract(sub, _biz_registry())
    kinds = [(e["kind"], e["uri"]) for e in out.proposals[0]["provenance"]]
    assert ("source_article", "https://example.com/fm-url") in kinds


def test_duplicate_model_local_refs_are_reassigned_uniquely():
    env = {"proposals": [
        {"predicate": "org.company", "local_ref": "x", "rationale": "r", "claim": {"display_name": "A Co"}},
        {"predicate": "org.company", "local_ref": "x", "rationale": "r", "claim": {"display_name": "B Co"}},
    ]}
    t = FakeTransport(response=_ollama_reply(env))
    eng = llm.LlmEngine(llm.OllamaBackend("http://localhost:11434", "m", t))
    out = eng.extract(_article_sub(), _biz_registry())
    refs = [p["local_ref"] for p in out.proposals]
    assert refs[0] == "x" and refs[1] == "company-1" and len(set(refs)) == 2
