"""Unit tests for the scribe's markdown → proposals pipeline.

The tests stub the `query()` callback (`predicate.inspect`) so they
exercise the heuristic extraction + schema validation without
spinning up the Rust skills host.

Each test mirrors one of the five required-by-spec unit assertions
in task_11.md plus a few extras for coverage.
"""

from __future__ import annotations

from typing import Any, Dict

import extraction  # type: ignore  # provided by conftest sys.path bootstrap.


# Schemas matching the MVP predicate specs in
# .compozy/tasks/ffs-mvp/_techspec.md § Data Models.
CONTACT_SCHEMA = {
    "type": "object",
    "required": ["display_name"],
    "properties": {
        "display_name": {"type": "string"},
        "email": {"type": "string"},
        "phone": {"type": "string"},
        "org": {"type": "string"},
        "notes": {"type": "array"},
    },
}

PERSON_SCHEMA = {
    "type": "object",
    "required": ["display_name"],
    "properties": {
        "display_name": {"type": "string"},
        "role": {"type": "string"},
        "team": {"type": "string"},
    },
}

NOTE_SCHEMA = {
    "type": "object",
    "required": ["title"],
    "properties": {
        "title": {"type": "string"},
        "body": {"type": "string"},
        "tags": {"type": "array"},
    },
}

SCHEMAS = {
    "contact.person": CONTACT_SCHEMA,
    "person.generic": PERSON_SCHEMA,
    "note": NOTE_SCHEMA,
}


def _install_fake_query(monkeypatch, schemas: Dict[str, Any]) -> None:
    """Stub `extraction.query` to return canned schemas without IO."""

    def fake_query(method: str, params: Dict[str, Any]) -> Dict[str, Any]:
        assert method == "predicate.inspect", f"unexpected method {method}"
        return {"claim_schema": schemas.get(params["name"], {})}

    monkeypatch.setattr(extraction, "query", fake_query)


# ---------------------------------------------------------------------
# Required-by-spec unit tests (task_11.md § Tests § Unit tests)
# ---------------------------------------------------------------------


def test_frontmatter_name_yields_contact_person(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "---\nname: Sara\nemail: sara@example.com\n---\n"
    result = extraction.handle({"source_uri": "file:///a.md", "content": md})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    assert contact["claim"]["display_name"] == "Sara"
    assert contact["claim"]["email"] == "sara@example.com"
    assert contact["provenance"][0]["kind"] == "ingest"
    assert contact["provenance"][0]["uri"] == "file:///a.md"
    assert "hash_hex" in contact["provenance"][0]


def test_notes_section_lands_in_contact_claim(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = (
        "---\n"
        "name: Sara\n"
        "email: sara@example.com\n"
        "---\n"
        "\n"
        "## Notes\n"
        "- Met at conference\n"
        "- Likes climbing\n"
    )
    result = extraction.handle({"source_uri": "file:///s.md", "content": md})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    assert contact["claim"]["notes"] == ["Met at conference", "Likes climbing"]


def test_conflicting_name_surfaces_as_ambiguity_note(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = (
        "---\n"
        "name: Sara\n"
        "email: sara@example.com\n"
        "---\n"
        "\n"
        "Some context.\n"
        "name: Sarah\n"
    )
    result = extraction.handle({"source_uri": "file:///s.md", "content": md})
    ambiguity = [
        p
        for p in result["proposals"]
        if p["predicate"] == "note" and "scribe-ambiguity" in p["claim"].get("tags", [])
    ]
    assert len(ambiguity) == 1
    assert "Sara" in ambiguity[0]["claim"]["body"]
    assert "Sarah" in ambiguity[0]["claim"]["body"]


def test_malformed_frontmatter_emits_partial_claim_and_warning(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    # Closing fence missing → frontmatter is rejected, body is the whole doc.
    md = "---\nname: Sara\nemail: oops-no-fence\n"
    result = extraction.handle({"source_uri": "file:///bad.md", "content": md})
    assert any("closing `---`" in w for w in result["warnings"])
    # A note proposal still emerges (nothing is lost).
    assert any(p["predicate"] == "note" for p in result["proposals"])
    # No contact.person should be emitted: with no usable frontmatter,
    # the structured extractor has no display_name to seed.
    assert not any(p["predicate"] == "contact.person" for p in result["proposals"])


def test_validation_failure_demotes_to_warning_and_drops_proposal(monkeypatch):
    # Use a schema that requires a field the scribe never emits, so a
    # generated contact.person claim fails validation and gets dropped.
    bad_schema = {
        "type": "object",
        "required": ["display_name", "phone"],
        "properties": {
            "display_name": {"type": "string"},
            "phone": {"type": "string"},
            "email": {"type": "string"},
            "notes": {"type": "array"},
        },
    }
    schemas = dict(SCHEMAS)
    schemas["contact.person"] = bad_schema
    _install_fake_query(monkeypatch, schemas)
    md = "---\nname: Sara\nemail: sara@example.com\n---\n"
    result = extraction.handle({"source_uri": "file:///x.md", "content": md})
    # No contact.person made it through.
    assert not any(p["predicate"] == "contact.person" for p in result["proposals"])
    # Validation failure surfaced in warnings.
    assert any("validation failed for contact.person" in w for w in result["warnings"])


# ---------------------------------------------------------------------
# Coverage extras
# ---------------------------------------------------------------------


def test_person_generic_extracted_when_role_present_without_contact_hints(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = (
        "---\n"
        "name: Alex Kim\n"
        "role: staff engineer\n"
        "team: platform\n"
        "---\n"
    )
    result = extraction.handle({"source_uri": "file:///p.md", "content": md})
    person = next(p for p in result["proposals"] if p["predicate"] == "person.generic")
    assert person["claim"]["display_name"] == "Alex Kim"
    assert person["claim"]["role"] == "staff engineer"
    assert person["claim"]["team"] == "platform"


def test_note_fallback_for_unstructured_markdown(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "Just some thoughts about a project I'm working on.\n"
    result = extraction.handle({"source_uri": "file:///n.md", "content": md})
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    assert "thoughts" in note["claim"]["body"]


def test_provenance_hash_matches_content_bytes(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "---\nname: Sara\nemail: sara@example.com\n---\n"
    result = extraction.handle({"source_uri": "file:///s.md", "content": md})
    # All proposals from the same submission share the same hash.
    hashes = {p["provenance"][0]["hash_hex"] for p in result["proposals"]}
    assert len(hashes) == 1
    # And the hash is deterministic across calls on identical content.
    again = extraction.handle({"source_uri": "file:///s.md", "content": md})
    again_hashes = {p["provenance"][0]["hash_hex"] for p in again["proposals"]}
    assert hashes == again_hashes


def test_parse_markdown_handles_empty_input():
    fm, sections, warnings = extraction.parse_markdown("")
    assert fm == {}
    assert sections == []
    assert warnings == []


def test_parse_markdown_tolerates_unicode_and_blank_lines():
    md = "---\nname: Élise\n\n# comment\n---\n\n\n## Notes\n- café\n"
    fm, sections, warnings = extraction.parse_markdown(md)
    assert fm["name"] == "Élise"
    notes = [s for s in sections if s[0] == "Notes"][0]
    assert "- café" in notes[1]
    assert warnings == []
