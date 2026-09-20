"""Tests for the schema-driven prompt builder (task_36)."""

import json
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.insert(0, _HERE)



from engine import Submission  # noqa: E402
from registry import PredicateRegistry  # noqa: E402

import prompt  # noqa: E402

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
WIDGET = {
    "description": "A fixture predicate that did not exist when the prompt builder was written.",
    "claim_schema": {
        "type": "object",
        "required": ["widget_name"],
        "properties": {"widget_name": {"type": "string"}, "kind": {"type": "string", "enum": ["a", "b"]}},
    },
}


def _sub(text="hello", hint=None, filename="x.md"):
    return Submission(
        source_uri="file:///tmp/x.md",
        content_text=text,
        content_bytes=text.encode(),
        content_hash_hex="00",
        filename=filename,
        predicate_hint=hint,
    )


def test_two_spec_registry_renders_both_predicates():
    reg = PredicateRegistry.from_specs({"contact.person": CONTACT, "note": NOTE})
    system, user = prompt.build_prompt(reg, _sub())
    assert "### contact.person" in system
    assert "### note" in system
    assert "A person you have a relationship with." in system
    assert '"required":["display_name"]' in system
    assert prompt.ENVELOPE_CONTRACT in system


def test_adding_a_spec_appears_with_no_code_change():
    reg = PredicateRegistry.from_specs({"contact.person": CONTACT, "note": NOTE, "widget": WIDGET})
    system, _ = prompt.build_prompt(reg, _sub())
    assert "### widget" in system
    assert "widget_name" in system
    assert '"enum":["a","b"]' in system


def test_hint_line_present_only_when_hinted_and_registered():
    reg = PredicateRegistry.from_specs({"contact.person": CONTACT, "note": NOTE})
    system_plain, _ = prompt.build_prompt(reg, _sub())
    assert "Target predicate:" not in system_plain
    system_hinted, _ = prompt.build_prompt(reg, _sub(hint="contact.person"))
    assert "Target predicate: contact.person" in system_hinted
    system_unknown, _ = prompt.build_prompt(reg, _sub(hint="source.article"))
    assert "Target predicate:" not in system_unknown


def test_rendered_schema_json_is_valid_json():
    reg = PredicateRegistry.from_specs({"contact.person": CONTACT})
    block = prompt.render_predicate(reg, "contact.person")
    schema_line = [line for line in block.splitlines() if line.startswith("claim_schema: ")][0]
    parsed = json.loads(schema_line[len("claim_schema: ") :])
    assert parsed["required"] == ["display_name"]


def test_user_prompt_carries_source_filename_and_text():
    reg = PredicateRegistry.from_specs({"note": NOTE})
    _, user = prompt.build_prompt(reg, _sub(text="Body line.", filename="Jon Jones.md"))
    assert "file:///tmp/x.md" in user
    assert "Jon Jones.md" in user
    assert "<<<\nBody line.\n>>>" in user


def test_rules_are_rendered():
    reg = PredicateRegistry.from_specs({"note": NOTE})
    system, _ = prompt.build_prompt(reg, _sub())
    for rule in prompt.RULES:
        assert rule in system
