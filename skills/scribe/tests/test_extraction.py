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


# ---------------------------------------------------------------------
# task_32 — unstructured contact heuristics + improved note title
# ---------------------------------------------------------------------


def test_unstructured_contact_fires_when_name_and_phone_co_occur(monkeypatch):
    """Name + phone (2 signals) → contact.person, no frontmatter required."""
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "Met Sara Chen at the conference. Phone 919-428-4074."
    result = extraction.handle({"source_uri": "file:///c.md", "content": md})
    contact = next(
        (p for p in result["proposals"] if p["predicate"] == "contact.person"),
        None,
    )
    assert contact is not None, f"expected a contact.person; got {result['proposals']}"
    assert contact["claim"]["display_name"] == "Sara Chen"
    assert contact["claim"]["phone"] == "919-428-4074"
    assert "phone number" in contact["rationale"]


def test_unstructured_contact_fires_when_name_and_email_co_occur(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "Quick note: Sara Chen is sara@example.com — really sharp engineer."
    result = extraction.handle({"source_uri": "file:///c.md", "content": md})
    contact = next(
        (p for p in result["proposals"] if p["predicate"] == "contact.person"),
        None,
    )
    assert contact is not None
    assert contact["claim"]["display_name"] == "Sara Chen"
    assert contact["claim"]["email"] == "sara@example.com"


def test_venue_span_is_masked_from_name_detection(monkeypatch):
    """The rehearsal fixture: "Met at Ballantyne Country Club" must NOT
    produce a contact.person with display_name="Ballantyne Country".
    Venue detection runs first and masks its span before the name
    detector scans.
    """
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "919-428-4074, January 18, Met at Ballantyne Country Club"
    result = extraction.handle({"source_uri": "file:///r.md", "content": md})
    # No contact.person should have been emitted — the only
    # capitalized phrase is the masked venue.
    contacts = [p for p in result["proposals"] if p["predicate"] == "contact.person"]
    assert not contacts, f"venue should have been masked, got contacts: {contacts}"
    # A note proposal lands with a body-derived title (NOT "untitled").
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    title = note["claim"]["title"]
    assert title != "untitled"
    assert "428-4074" in title or "Ballantyne" in title or "January" in title


def test_name_starting_with_month_word_is_accepted(monkeypatch):
    """Someone named April Johnson is a real person, not a stopword.
    A pre-task_32 implementation that stop-listed month names would
    silently misclassify them. Guard against that regression.
    """
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "April Johnson — 919-428-4074. Met at the conference."
    result = extraction.handle({"source_uri": "file:///c.md", "content": md})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    assert contact["claim"]["display_name"] == "April Johnson"


def test_unstructured_single_signal_falls_back_to_note(monkeypatch):
    """One signal (phone alone) doesn't meet the 2-signal threshold;
    a note proposal lands instead with a body-derived title.
    """
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "919-428-4074"
    result = extraction.handle({"source_uri": "file:///p.md", "content": md})
    contacts = [p for p in result["proposals"] if p["predicate"] == "contact.person"]
    assert not contacts
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    assert "428-4074" in note["claim"]["title"]


def test_note_title_derived_from_first_body_line(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "Random thought about the federation protocol\nMore context below.\n"
    result = extraction.handle({"source_uri": "file:///n.md", "content": md})
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    title = note["claim"]["title"]
    assert title.startswith("Random thought")
    # First-line slug is capped at 6 words.
    assert len(title.split()) <= 6


def test_note_title_strips_markdown_list_prefix(monkeypatch):
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "- A bullet-style first line that should become the title\n"
    result = extraction.handle({"source_uri": "file:///n.md", "content": md})
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    title = note["claim"]["title"]
    assert not title.startswith("-")
    assert title.startswith("A bullet")


def test_empty_body_falls_back_to_untitled(monkeypatch):
    """When the body really is empty/whitespace, the literal
    "untitled" remains — the Rust dispatcher then derives a slug
    from tx_time so the entity ID is still navigable.
    """
    _install_fake_query(monkeypatch, SCHEMAS)
    md = "\n\n   \n"
    result = extraction.handle({"source_uri": "file:///e.md", "content": md})
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    assert note["claim"]["title"] == "untitled"


# ---------------------------------------------------------------------
# Heuristic helpers — unit-level coverage of the underlying detectors
# ---------------------------------------------------------------------


def test_detect_phone_numbers_handles_three_common_shapes():
    assert "919-428-4074" in extraction.detect_phone_numbers("call 919-428-4074")
    assert any("428-4074" in p for p in extraction.detect_phone_numbers("call (919) 428-4074 then"))
    assert any("428-4074" in p for p in extraction.detect_phone_numbers("call +1-919-428-4074 anytime"))


def test_detect_emails_picks_up_address():
    assert extraction.detect_emails("write to sara@example.com today") == ["sara@example.com"]


def test_detect_venue_mentions_returns_spans_for_masking():
    venues = extraction.detect_venue_mentions("We met at Foley Greenhouse last week.")
    assert venues
    venue_text, start, end = venues[0]
    assert "Foley Greenhouse" in venue_text
    assert end > start


def test_extract_capitalized_name_rejects_function_word_pairs():
    # "The Project" shouldn't qualify; "Sara Chen" should.
    assert extraction.extract_capitalized_name("The Project") is None
    assert extraction.extract_capitalized_name("Sara Chen") == "Sara Chen"
