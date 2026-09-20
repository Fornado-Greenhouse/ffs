"""Tests for the scribe's on-disk predicate registry (task_36 § 36.2).

The registry reads `$FFS_DATA_DIR/config/predicates/*.toml` with
stdlib `tomllib`; when no specs are on disk it falls back to the
host's `predicate.inspect` query so the daemon-hosted path keeps
working.
"""

from __future__ import annotations

import os
import textwrap

import extraction  # type: ignore
from registry import PredicateRegistry  # type: ignore

CONTACT_TOML = textwrap.dedent(
    """
    # contact.person — the substrate's primary contact-graph predicate.
    #
    # Atoms with this predicate represent a person you have a
    # relationship with.

    name = "contact.person"
    version = 1

    [claim_schema]
    type = "object"
    required = ["display_name"]

    [claim_schema.properties]
    display_name = { type = "string" }
    phone = { type = "string" }
    """
)

NOTE_TOML = textwrap.dedent(
    """
    name = "note"
    version = 1

    [claim_schema]
    type = "object"
    required = ["title"]

    [claim_schema.properties]
    title = { type = "string" }
    body = { type = "string" }
    """
)

WIDGET_TOML = textwrap.dedent(
    """
    # widget.thing — a predicate that did not exist when the scribe was written.
    name = "widget.thing"
    version = 1

    [claim_schema]
    type = "object"
    required = ["label"]

    [claim_schema.properties]
    label = { type = "string" }
    """
)


def _write_specs(data_dir, **specs):
    d = os.path.join(data_dir, "config", "predicates")
    os.makedirs(d, exist_ok=True)
    for fname, body in specs.items():
        with open(os.path.join(d, fname), "w", encoding="utf-8") as f:
            f.write(body)
    return d


def test_load_reads_every_toml_and_keys_by_name(tmp_path):
    _write_specs(tmp_path, **{"contact.person.toml": CONTACT_TOML, "note.toml": NOTE_TOML})
    reg = PredicateRegistry.load(str(tmp_path))
    assert reg.names() == ["contact.person", "note"]
    assert reg.has("note")
    assert reg.schema("contact.person")["required"] == ["display_name"]
    assert reg.raw("note")["version"] == 1
    assert reg.warnings == []


def test_newly_added_spec_appears_with_no_code_change(tmp_path):
    d = _write_specs(tmp_path, **{"contact.person.toml": CONTACT_TOML})
    assert "widget.thing" not in PredicateRegistry.load(str(tmp_path)).names()
    with open(os.path.join(d, "widget.thing.toml"), "w", encoding="utf-8") as f:
        f.write(WIDGET_TOML)
    reg = PredicateRegistry.load(str(tmp_path))
    assert "widget.thing" in reg.names()
    assert reg.schema("widget.thing")["required"] == ["label"]


def test_description_comes_from_leading_comment_block(tmp_path):
    _write_specs(tmp_path, **{"contact.person.toml": CONTACT_TOML, "note.toml": NOTE_TOML})
    reg = PredicateRegistry.load(str(tmp_path))
    assert reg.description("contact.person").startswith("contact.person")
    assert "contact-graph" in reg.description("contact.person")
    # No comment block: falls back to the name.
    assert reg.description("note") == "note"


def test_malformed_toml_is_skipped_with_a_warning(tmp_path):
    _write_specs(tmp_path, **{"good.toml": NOTE_TOML, "bad.toml": "name = \"broken\"\n[claim_schema\n"})
    reg = PredicateRegistry.load(str(tmp_path))
    assert reg.names() == ["note"]
    assert any("bad.toml" in w for w in reg.warnings)


def test_spec_without_name_is_skipped_with_a_warning(tmp_path):
    _write_specs(tmp_path, **{"anon.toml": "[claim_schema]\ntype = \"object\"\n"})
    reg = PredicateRegistry.load(str(tmp_path))
    assert reg.names() == []
    assert any("anon.toml" in w for w in reg.warnings)


def test_load_uses_env_data_dir_when_not_given(tmp_path, monkeypatch):
    _write_specs(tmp_path, **{"note.toml": NOTE_TOML})
    monkeypatch.setenv("FFS_DATA_DIR", str(tmp_path))
    assert PredicateRegistry.load().names() == ["note"]


def test_empty_dir_falls_back_to_host_query(tmp_path, monkeypatch):
    calls = []

    def fake_query(method, params):
        calls.append((method, params))
        return {"claim_schema": {"type": "object", "required": ["title"]}}

    monkeypatch.setattr(extraction, "query", fake_query)
    reg = PredicateRegistry.load(str(tmp_path), fallback=extraction._fetch_schema)
    assert reg.names() == []
    assert reg.has("note") is True  # resolvable through the host
    schema = reg.schema("note")
    assert schema == {"type": "object", "required": ["title"]}
    # Cached: a second lookup does not re-query the host.
    reg.schema("note")
    assert calls == [("predicate.inspect", {"name": "note"})]


def test_fallback_host_error_yields_none(tmp_path, monkeypatch):
    def failing_query(method, params):
        raise extraction.FfsSkillError("capability denied")

    monkeypatch.setattr(extraction, "query", failing_query)
    reg = PredicateRegistry.load(str(tmp_path), fallback=extraction._fetch_schema)
    assert reg.schema("note") is None
    assert reg.has("note") is False


def test_no_fallback_and_unknown_name_yields_none(tmp_path):
    reg = PredicateRegistry.load(str(tmp_path))
    assert reg.schema("nope") is None


def test_from_specs_for_tests():
    reg = PredicateRegistry.from_specs({"x": {"claim_schema": {"type": "object"}}})
    assert reg.names() == ["x"]
    assert reg.schema("x") == {"type": "object"}
    assert reg.schema("y") is None
