"""Morning read (ADR-035, task_48): the read's frontmatter becomes two
provenance entries and never claim data; a clipped body carries the
clip hint; pointers do not; the session log renders its sections."""

from __future__ import annotations

from contract import READ_SESSION_KEYS, read_session_provenance, render_session_log
from engine import Submission


def _sub(body: str, **extra) -> Submission:
    fm = {"predicate": "source.article", "title": "Widget maker breaks ground", "url": "https://news.example-ledger.test/2026/09/21/widget-plant", "intake": "morning_read", "owner_present": "true", "session": "2026-09-21-0814", "actor": "mcp:agent/claude-code"}
    fm.update(extra)
    text = "---\n" + "\n".join(f"{k}: {v}" for k, v in fm.items()) + "\n---\n" + body
    return Submission.from_input({"source_uri": "https://news.example-ledger.test/2026/09/21/widget-plant", "content": text})


def test_read_session_frontmatter_becomes_two_provenance_entries_and_a_clip_hint():
    sub = _sub("The owner clipped this body.\n")
    entries, hint = read_session_provenance(sub, sub.frontmatter["url"])
    kinds = [e["kind"] for e in entries]
    assert kinds == ["morning_read", "session"]
    assert entries[0]["uri"] == sub.frontmatter["url"]
    assert entries[0]["hash_hex"] == sub.content_hash_hex
    assert entries[1]["uri"] == "ffs-session://mcp:agent/claude-code/2026-09-21-0814?owner_present=true"
    assert len(entries[1]["hash_hex"]) == 64
    assert hint == "clip"


def test_pointer_without_body_gets_provenance_but_no_clip_hint():
    sub = _sub("")
    entries, hint = read_session_provenance(sub, sub.frontmatter["url"])
    assert [e["kind"] for e in entries] == ["morning_read", "session"]
    assert hint is None


def test_owner_absent_never_yields_a_clip_hint():
    sub = _sub("A body without the owner present.\n", owner_present="false")
    entries, hint = read_session_provenance(sub, sub.frontmatter["url"])
    assert entries[1]["uri"].endswith("owner_present=false")
    assert hint is None


def test_read_keys_never_enter_the_claim_through_the_hint_path(monkeypatch):
    from corpus_scorer import install_query_stub

    install_query_stub(monkeypatch)
    from extraction import handle

    sub_text = "---\npredicate: source.article\ntitle: T\nurl: https://example.test/a\npublication: Example Ledger\npublished_at: 2026-09-21\nintake: morning_read\nowner_present: true\nsession: s1\nactor: mcp:agent/x\n---\nClipped body.\n"
    out = handle({"source_uri": "https://example.test/a", "content": sub_text})
    article = next(p for p in out["proposals"] if p["predicate"] == "source.article")
    for k in READ_SESSION_KEYS:
        assert k not in article["claim"], k
    assert article.get("classification_hint") == "clip"
    kinds = [e["kind"] for e in article["provenance"]]
    assert "morning_read" in kinds and "session" in kinds, kinds
    assert kinds.index("morning_read") < kinds.index("session")


def test_session_log_renders_opens_outcomes_refusals_and_clips():
    text = render_session_log(
        "2026-09-21",
        opened=[("08:14", "https://news.example-ledger.test/2026/09/21/widget-plant")],
        proposals=[("person.generic", "Pat Example", "filed"), ("org.company", "Example Maker Co", "proposed")],
        refusals=[("08:40", "open all of tomorrow's links")],
        clips=[("example-ledger-2026-09-21-widget-plant", "Widget maker breaks ground")],
    )
    assert text.startswith("---\npredicate: note\ntitle: Morning read 2026-09-21\ntags: [morning-read, session-log]\n")
    assert "## Opened\n- 08:14 https://news.example-ledger.test/2026/09/21/widget-plant" in text
    assert "## Proposals\n- person.generic: Pat Example (filed)\n- org.company: Example Maker Co (proposed)" in text
    assert "## Refusals\n- 08:40 open all of tomorrow's links: refused as bulk behavior" in text
    assert "## Clips\n- [[example-ledger-2026-09-21-widget-plant|Widget maker breaks ground]]" in text
    empty = render_session_log("2026-09-22", [], [], [], [])
    assert "## Refusals" not in empty and "## Clips" not in empty
