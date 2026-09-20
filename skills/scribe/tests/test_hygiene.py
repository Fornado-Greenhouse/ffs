"""Heuristic hygiene tests (task_36 § 36.8): the "Bones Occupation" class.

Three bounded fixes: the filename as a name candidate, key-value card
detection instead of the bigram scan, and field-label words excluded
from name candidacy.
"""

from __future__ import annotations

from typing import Any, Dict

import extraction  # type: ignore

SCHEMAS: Dict[str, Any] = {
    "contact.person": {
        "type": "object",
        "required": ["display_name"],
        "properties": {
            "display_name": {"type": "string"},
            "phone": {"type": "string"},
            "role": {"type": "string"},
            "organization": {"type": "string"},
            "work_email": {"type": "string"},
            "personal_email": {"type": "string"},
            "notes": {"type": "array"},
        },
    },
    "person.generic": {"type": "object", "required": ["display_name"], "properties": {"display_name": {"type": "string"}}},
    "note": {"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}, "body": {"type": "string"}}},
}


def _install_fake_query(monkeypatch):
    def fake_query(method, params):
        return {"claim_schema": SCHEMAS.get(params["name"], {})}

    monkeypatch.setattr(extraction, "query", fake_query)


# The verbatim card from ADR-026 / task_36's postmortem.
JON_JONES = "Nickname: Bones\nOccupation: UFC fighter\nPhone numbe r- 9194284074\n"


def test_jon_jones_card_extracts_by_filename_card_and_stopwords(monkeypatch):
    _install_fake_query(monkeypatch)
    result = extraction.handle({"source_uri": "file:///home/u/.ffs/ingest/Jon%20Jones.md", "content": JON_JONES})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    claim = contact["claim"]
    assert claim["display_name"] == "Jon Jones"
    assert claim["phone"] == "919-428-4074"
    assert claim.get("role") == "UFC fighter"
    assert any("Nickname: Bones" == n for n in claim.get("notes", []))
    assert "card" in contact["rationale"]


def test_field_label_words_never_become_display_name():
    for text in ("Nickname Occupation", "Phone Email", "First Last", "Bones Occupation"):
        assert extraction.extract_capitalized_name(text) is None, text
    assert extraction.extract_capitalized_name("Jon Jones") == "Jon Jones"


def test_card_shape_is_detected_and_bigram_scan_skipped():
    card = extraction.parse_card_lines("Name: Sara Chen\nPhone: (919) 428-4074\nCompany: Acme Corp\nEmail: sara@acme.com\n")
    assert card is not None
    assert card["display_name"] == "Sara Chen"
    assert card["phone"] == "919-428-4074"
    assert card["organization"] == "Acme Corp"
    assert card["work_email"] == "sara@acme.com"


def test_card_labels_match_loosely_on_prefix_and_personal_email_domain():
    card = extraction.parse_card_lines("Nam: Ada Lovelace\nMobil: 919.428.4074\nEmail: ada@gmail.com\nAddress: 1 Main St\n")
    assert card is not None
    assert card["display_name"] == "Ada Lovelace"
    assert card["phone"] == "919-428-4074"
    assert card["personal_email"] == "ada@gmail.com"
    assert "Address: 1 Main St" in card["notes"]


def test_prose_is_not_mistaken_for_a_card():
    assert extraction.parse_card_lines("Note: call him tomorrow about the deal.\n") is None
    assert extraction.parse_card_lines("Met Sara Chen at the conference. Phone 919-428-4074.") is None


def test_filename_beats_a_body_name_that_fails_the_cross_check(monkeypatch):
    """No card here (prose), so the bigram scan runs and finds a
    capitalized pair that is not the person; the filename wins because
    it looks like a person name and shares no word with the body name."""
    _install_fake_query(monkeypatch)
    md = "Some Random words about a fighter. Phone 919-428-4074.\n"
    result = extraction.handle({"source_uri": "file:///ingest/Jon_Jones.md", "content": md})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    assert contact["claim"]["display_name"] == "Jon Jones"


def test_filename_is_ignored_when_it_does_not_look_like_a_person(monkeypatch):
    _install_fake_query(monkeypatch)
    md = "Met Sara Chen at the conference. Phone 919-428-4074."
    result = extraction.handle({"source_uri": "file:///ingest/2026-05-26-meeting-notes.md", "content": md})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    assert contact["claim"]["display_name"] == "Sara Chen"


def test_filename_is_ignored_when_body_name_agrees(monkeypatch):
    _install_fake_query(monkeypatch)
    md = "Met Sara Chen at the conference. Phone 919-428-4074."
    result = extraction.handle({"source_uri": "file:///ingest/Sara_Chen.md", "content": md})
    contact = next(p for p in result["proposals"] if p["predicate"] == "contact.person")
    assert contact["claim"]["display_name"] == "Sara Chen"


def test_filename_looks_like_person_name():
    assert extraction.filename_as_person_name("Jon Jones") == "Jon Jones"
    assert extraction.filename_as_person_name("Jon_Jones") == "Jon Jones"
    assert extraction.filename_as_person_name("mary-ann-smith") is None  # lowercase
    assert extraction.filename_as_person_name("2026-05-26-meeting-notes") is None
    assert extraction.filename_as_person_name("Phone List") is None  # field label
    assert extraction.filename_as_person_name(None) is None


def test_card_with_phone_but_no_name_and_no_person_filename_yields_no_contact(monkeypatch):
    _install_fake_query(monkeypatch)
    md = "Occupation: welder\nPhone: 919-428-4074\n"
    result = extraction.handle({"source_uri": "file:///ingest/scrap.md", "content": md})
    assert not any(p["predicate"] == "contact.person" for p in result["proposals"])
    assert any(p["predicate"] == "note" for p in result["proposals"])


def test_frontmatter_title_alone_never_yields_a_person(monkeypatch):
    """A `title` is a note title unless `name`/`display_name` is present.
    Pre-task_36, "title: Grocery list" minted a person.generic with
    display_name and role both "Grocery list"."""
    _install_fake_query(monkeypatch)
    md = "---\ntitle: Grocery list\nrole: weekly\n---\n- eggs\n- milk\n"
    result = extraction.handle({"source_uri": "file:///ingest/groceries.md", "content": md})
    kinds = [p["predicate"] for p in result["proposals"]]
    assert "person.generic" not in kinds and "contact.person" not in kinds
    note = next(p for p in result["proposals"] if p["predicate"] == "note")
    assert note["claim"]["title"] == "Grocery list"


def test_frontmatter_title_is_the_role_when_a_name_is_present(monkeypatch):
    _install_fake_query(monkeypatch)
    md = "---\nname: Alex Kim\ntitle: Staff Engineer\n---\n"
    result = extraction.handle({"source_uri": "file:///ingest/alex.md", "content": md})
    person = next(p for p in result["proposals"] if p["predicate"] == "person.generic")
    assert person["claim"]["display_name"] == "Alex Kim"
    assert person["claim"]["role"] == "Staff Engineer"


def test_key_value_card_does_not_also_emit_a_duplicate_fallback_note(monkeypatch):
    from corpus_scorer import install_query_stub

    install_query_stub(monkeypatch)
    from extraction import handle

    out = handle(
        {
            "source_uri": "file:///ingest/Jon%20Jones.md",
            "content": "Nickname: Bones\nOccupation: UFC fighter\nPhone numbe r- 9194284074\n",
        }
    )
    predicates = [p["predicate"] for p in out["proposals"]]
    assert predicates == ["contact.person"], predicates
