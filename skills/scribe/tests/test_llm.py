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
