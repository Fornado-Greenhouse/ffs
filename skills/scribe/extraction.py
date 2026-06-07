"""Scribe: heuristic markdown → proposed atoms.

The MVP scribe is regex- and frontmatter-driven; no LLM is required.
Phase 2 will wire an LLM client for richer extraction, but the
heuristics here are deterministic, testable, and fast.

Pipeline per invocation:

1. Parse YAML-style ``---`` frontmatter (tolerant: malformed YAML is
   reported as a warning, not an exception).
2. Segment the body into ``## Heading`` sections.
3. Classify the document:
   - frontmatter `name` + (`email`/`phone`/`org` or a `Notes` section)
     → `contact.person`.
   - frontmatter `name` + (`role`/`team`) without contact hints
     → `person.generic`.
   - everything else → `note`.
4. Validate each proposed claim against the predicate's schema via
   the host's `predicate.inspect` proxy. Rejected claims are demoted
   to `note` proposals with a `validation-failure` rationale so
   nothing is lost.
5. Attach provenance to every proposal pointing back to the
   submission's source URI + content hash.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
from typing import Any, Dict, List, Optional, Tuple

# Path bootstrap: the host launches us with cwd == skills/scribe/, so
# the parent's _lib helper is at ../_lib.
_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir, "_lib"))
if _LIB not in sys.path:
    sys.path.insert(0, _LIB)

from ffs_skill import FfsSkillError, log, query, run  # noqa: E402


# --------------------------------------------------------------------
# Frontmatter + body parsing
# --------------------------------------------------------------------

_FM_FENCE = re.compile(r"^---\s*$")


def parse_markdown(content: str) -> Tuple[Dict[str, Any], List[Tuple[str, List[str]]], List[str]]:
    """Return ``(frontmatter, sections, warnings)``.

    Sections is a list of ``(name, lines)`` where ``name`` is the
    ``## Heading`` text (or ``""`` for the implicit preamble before
    the first header). Lines preserve their original strings minus
    trailing newlines.

    Malformed frontmatter falls back to an empty dict plus a warning.
    """
    lines = content.splitlines()
    fm: Dict[str, Any] = {}
    warnings: List[str] = []
    body_start = 0

    if lines and _FM_FENCE.match(lines[0]):
        # Look for closing fence.
        end = None
        for i in range(1, len(lines)):
            if _FM_FENCE.match(lines[i]):
                end = i
                break
        if end is None:
            warnings.append("frontmatter has no closing `---`; ignored")
        else:
            for line_no, raw in enumerate(lines[1:end], start=2):
                stripped = raw.strip()
                if not stripped or stripped.startswith("#"):
                    continue
                if ":" not in stripped:
                    warnings.append(f"malformed frontmatter at line {line_no}: {raw!r}")
                    continue
                k, v = stripped.split(":", 1)
                k = k.strip()
                v = v.strip().strip("\"'")
                if not k:
                    warnings.append(f"empty frontmatter key at line {line_no}")
                    continue
                fm[k] = v
            body_start = end + 1

    # Sectionize body.
    sections: List[Tuple[str, List[str]]] = []
    current_name = ""
    current_lines: List[str] = []
    for raw in lines[body_start:]:
        if raw.startswith("## "):
            if current_name or any(l.strip() for l in current_lines):
                sections.append((current_name, current_lines))
            current_name = raw[3:].strip().rstrip(":")
            current_lines = []
        else:
            current_lines.append(raw)
    if current_name or any(l.strip() for l in current_lines):
        sections.append((current_name, current_lines))

    return fm, sections, warnings


def find_section(sections: List[Tuple[str, List[str]]], name: str) -> Optional[List[str]]:
    """Case-insensitive lookup for a `## name` section's lines."""
    lower = name.lower()
    for n, lines in sections:
        if n.lower() == lower:
            return lines
    return None


def collect_bullets(lines: List[str]) -> List[str]:
    bullets: List[str] = []
    for raw in lines:
        s = raw.strip()
        if s.startswith("- "):
            bullets.append(s[2:].strip())
        elif s.startswith("* "):
            bullets.append(s[2:].strip())
    return bullets


# --------------------------------------------------------------------
# Predicate-specific extractors
# --------------------------------------------------------------------

_EMAIL_RE = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
_PHONE_RE = re.compile(r"(?:\+?\d{1,3}[-.\s]?)?(?:\(?\d{3}\)?[-.\s]?){2}\d{4}")


def _name_field(fm: Dict[str, Any]) -> Optional[str]:
    """Pull a display name from frontmatter, in preference order."""
    for key in ("display_name", "name", "title"):
        if key in fm and fm[key]:
            return str(fm[key])
    return None


def _conflicting_name(fm: Dict[str, Any], body: str) -> Optional[str]:
    """Detect a name in the body that disagrees with frontmatter's name.

    Returns the *body* name when a conflict is found. Otherwise None.

    Walks every `name:`/`Name:` line in `body`; the first one whose
    value differs from frontmatter's name is the conflict. This is
    tolerant of being passed the full document (the frontmatter line
    matches its own value and is skipped) or just the body.
    """
    fm_name = _name_field(fm)
    if not fm_name:
        return None
    for m in re.finditer(r"^\s*[Nn]ame\s*:\s*(.+?)\s*$", body, flags=re.MULTILINE):
        candidate = m.group(1).strip("\"' ")
        if candidate and candidate != fm_name:
            return candidate
    return None


def extract_contact_person(
    fm: Dict[str, Any],
    sections: List[Tuple[str, List[str]]],
) -> Optional[Dict[str, Any]]:
    name = _name_field(fm)
    if not name:
        return None
    # A contact-person has at least a name AND one of: email/phone/org/Notes section.
    email = fm.get("email")
    phone = fm.get("phone")
    org = fm.get("org") or fm.get("organization") or fm.get("company")
    notes_lines = find_section(sections, "Notes")
    notes_bullets = collect_bullets(notes_lines) if notes_lines else []
    if not (email or phone or org or notes_bullets):
        return None
    claim: Dict[str, Any] = {"display_name": name}
    if email:
        claim["email"] = email
    if phone:
        claim["phone"] = phone
    if org:
        claim["org"] = org
    if notes_bullets:
        claim["notes"] = notes_bullets
    return claim


# Stop-list for the capitalized-name detector. Two narrow classes:
#
#  1. English grammar function words ("The", "A", "His") — these
#     never appear inside personal names regardless of culture.
#  2. A small set of past-tense interaction verbs ("Met", "Saw",
#     "Called", "Spoke") that get capitalized only because they're
#     at sentence start. Catching these is necessary because the
#     regex naively matches the first capitalized bigram, so "Met
#     Sara Chen at …" would otherwise produce display_name "Met
#     Sara". The verbs are also handled structurally where possible
#     (the venue-masking pass eats their "Met at X" surroundings)
#     but standalone "Met <Name>" without a venue still needs to
#     be skipped.
#
# Explicitly NOT included: months ("April", "May" are real names),
# days, location words ("Country", "Club"), or any open-class
# noun. Those false-positive classes are handled structurally:
# venues by masking, months/days by accepting them.
_NAME_STOPWORDS = frozenset(
    [
        # Function words.
        "the", "a", "an",
        "this", "that", "these", "those",
        "his", "her", "their", "our", "your", "my",
        # Past-tense interaction verbs commonly seen at sentence
        # start in a contact note.
        "met", "saw", "called", "spoke", "phoned",
        "emailed", "texted", "talked", "visited", "heard", "bumped",
    ]
)


_PHONE_PATTERNS = (
    # 919-428-4074
    re.compile(r"\b\d{3}-\d{3}-\d{4}\b"),
    # (919) 428-4074, (919)428-4074
    re.compile(r"\(\s*\d{3}\s*\)\s*\d{3}[\s-]?\d{4}"),
    # +1 919 428 4074, +1-919-428-4074
    re.compile(r"\+?1[-\s]?\d{3}[-\s]?\d{3}[-\s]?\d{4}"),
)

_EMAIL_PATTERN = re.compile(r"\b[\w.+-]+@[\w-]+\.[\w.-]+\b")

# "Met at <Capitalized…>" / "saw … at <Capitalized…>" venue
# patterns. Capture the venue span (the `<Capitalized…>` group) so
# the caller can both report it AND mask it from the name detector.
# The pattern intentionally requires an initial capital so "at the
# office" (lowercase venue) doesn't fire — those don't claim to be
# proper nouns and don't risk swallowing a name.
_VENUE_PATTERNS = (
    re.compile(r"\b[Mm]et\s+(?:at|with)\s+((?:[A-Z][\w\-]*(?:\s+[A-Z][\w\-]*)*))", re.MULTILINE),
    re.compile(r"\b[Ss]aw\b[^.]*?\bat\s+((?:[A-Z][\w\-]*(?:\s+[A-Z][\w\-]*)*))", re.MULTILINE),
)


def detect_phone_numbers(text: str) -> List[str]:
    """Return every phone-number-looking substring in `text`."""
    out: List[str] = []
    for pat in _PHONE_PATTERNS:
        out.extend(m.group(0) for m in pat.finditer(text))
    return out


def detect_emails(text: str) -> List[str]:
    """Return every email-looking substring in `text`."""
    return [m.group(0) for m in _EMAIL_PATTERN.finditer(text)]


def detect_venue_mentions(text: str) -> List[Tuple[str, int, int]]:
    """Detect "Met at X" / "saw … at X" mentions. Returns a list of
    `(venue_text, start_offset, end_offset)` tuples where the
    offsets are into the venue match (group 1), not the whole
    pattern. Used both for the venue signal AND for masking the
    venue from the name detector so something like "Met at
    Ballantyne Country Club" doesn't get classified as a person
    named "Ballantyne Country".
    """
    out: List[Tuple[str, int, int]] = []
    for pat in _VENUE_PATTERNS:
        for m in pat.finditer(text):
            out.append((m.group(1).strip(), m.start(1), m.end(1)))
    return out


def _mask_spans(text: str, spans: List[Tuple[int, int]]) -> str:
    """Replace each `(start, end)` span in `text` with same-length
    placeholder characters that won't match any of the heuristic
    patterns (specifically: avoid uppercase letters so the
    capitalized-name detector skips them). Preserves text length so
    the offsets of other matches stay correct.
    """
    if not spans:
        return text
    chars = list(text)
    for start, end in spans:
        for i in range(start, min(end, len(chars))):
            chars[i] = "_"
    return "".join(chars)


def extract_capitalized_name(text: str) -> Optional[str]:
    """Find a two-word capitalized name in `text`. Skips a small
    set of English grammar function words and common past-tense
    interaction verbs (the only false positives that are
    universally safe to reject). The caller is expected to
    pre-mask venue spans, so we don't need a venue stop-list here.

    Uses a lookahead so finditer finds overlapping bigrams — this
    is necessary because input like "Met Sara Chen" has its first
    bigram "Met Sara" rejected by the stop list, but we still need
    to see "Sara Chen" as a candidate. Without the lookahead, the
    "Sara" characters would already be consumed by the rejected
    "Met Sara" match.
    """
    pattern = re.compile(r"\b(?=([A-Z][a-z]+)\s+([A-Z][a-z]+)\b)")
    for m in pattern.finditer(text):
        first, last = m.group(1), m.group(2)
        if first.lower() in _NAME_STOPWORDS or last.lower() in _NAME_STOPWORDS:
            continue
        return f"{first} {last}"
    return None


def extract_contact_person_unstructured(content_text: str) -> Optional[Tuple[Dict[str, Any], List[str]]]:
    """Walk the body for unstructured contact signals: phone, email,
    venue mention, capitalized name. Emit a `contact.person` claim
    when ≥2 distinct signals fire AND a name is extractable from
    text where venue spans have been masked out.

    Returns `(claim, signals)` — `signals` is the list of signal
    names that fired (used for the proposal's `rationale` string).
    """
    # Detect venues FIRST. Venues are masked from the text before
    # we run the name detector so e.g. "Met at Ballantyne Country
    # Club" doesn't get misclassified as a person named "Ballantyne
    # Country".
    venues = detect_venue_mentions(content_text)
    masked = _mask_spans(content_text, [(s, e) for _, s, e in venues])

    name = extract_capitalized_name(masked)
    if not name:
        return None
    signals: List[str] = ["capitalized name"]
    claim: Dict[str, Any] = {"display_name": name}

    phones = detect_phone_numbers(content_text)
    if phones:
        signals.append("phone number")
        claim["phone"] = phones[0]
    emails = detect_emails(content_text)
    if emails:
        signals.append("email address")
        claim["email"] = emails[0]
    if venues:
        venue_text = venues[0][0]
        signals.append(f"venue mention ({venue_text})")
        claim.setdefault("notes", []).append(f"Met at {venue_text}")

    if len(signals) < 2:
        return None
    return claim, signals


def extract_person_generic(
    fm: Dict[str, Any],
    sections: List[Tuple[str, List[str]]],
) -> Optional[Dict[str, Any]]:
    _ = sections  # not used at MVP; reserved for future role-from-body extraction.
    name = _name_field(fm)
    if not name:
        return None
    role = fm.get("role") or fm.get("title")
    team = fm.get("team") or fm.get("department")
    if not (role or team):
        return None
    claim: Dict[str, Any] = {"display_name": name}
    if role:
        claim["role"] = role
    if team:
        claim["team"] = team
    return claim


def extract_note(
    fm: Dict[str, Any],
    sections: List[Tuple[str, List[str]]],
    raw_body: str,
) -> Dict[str, Any]:
    tags_raw = fm.get("tags")
    tags: List[str] = []
    if isinstance(tags_raw, str):
        # Comma- or whitespace-separated.
        tags = [t.strip().lstrip("#") for t in re.split(r"[,\s]+", tags_raw) if t.strip()]
    body_text = "\n".join(line for _, lines in sections for line in lines).strip()
    if not body_text:
        body_text = raw_body.strip()

    # Title preference order: frontmatter `title` → frontmatter `name` →
    # body-derived first-line slug (≤6 words, ~60 chars) → literal
    # "untitled". The body-derived path keeps untitled notes from
    # producing the unreadable `from-<submission-id>` entity ID
    # downstream.
    title = (
        fm.get("title")
        or _name_field(fm)
        or _title_from_body(body_text)
        or "untitled"
    )

    claim: Dict[str, Any] = {"title": title, "body": body_text}
    if tags:
        claim["tags"] = tags
    return claim


def _title_from_body(body: str) -> Optional[str]:
    """Derive a short, human-readable title from a body's first
    non-empty line. Caps at 6 words / 60 characters to keep the
    resulting projection filename navigable. Returns None when the
    body is empty or whitespace-only.

    Strips markdown-list and -heading prefixes — but only when
    they're FOLLOWED BY whitespace, so that an unstructured line
    like "919-428-4074" (which would otherwise get its leading
    digits eaten by an over-eager character-class strip) survives
    intact.
    """
    # Markdown prefixes: unordered list (`-`, `*`, `+`), ordered
    # list (`\d+.`), heading (`#`+), or block quote (`>`), each
    # required to be followed by at least one whitespace character
    # to count as a prefix.
    prefix_re = re.compile(r"^\s*(?:[-*+>]|#+|\d+\.)\s+")
    for line in body.splitlines():
        stripped = line.strip()
        stripped = prefix_re.sub("", stripped).strip()
        if not stripped:
            continue
        words = stripped.split()
        if not words:
            continue
        truncated = " ".join(words[:6])
        if len(truncated) > 60:
            truncated = truncated[:60].rsplit(" ", 1)[0]
        return truncated or None
    return None


# --------------------------------------------------------------------
# Schema validation
# --------------------------------------------------------------------


def _validate_claim_against_schema(claim: Dict[str, Any], schema: Dict[str, Any]) -> Optional[str]:
    """Minimal JSON-Schema subset validator.

    Handles ``type: object``, ``required``, ``properties.<k>.type`` for
    the limited shape the MVP predicate specs use. Returns ``None`` on
    pass or an error message on failure. Avoids the external
    ``jsonschema`` package so the scribe has zero pip dependencies.
    """
    if not isinstance(schema, dict):
        return None
    if schema.get("type") == "object" and not isinstance(claim, dict):
        return "claim must be an object"
    required = schema.get("required", [])
    for k in required:
        if k not in claim:
            return f"missing required field: {k}"
    props = schema.get("properties", {})
    for k, v in claim.items():
        spec = props.get(k)
        if not isinstance(spec, dict):
            continue
        expected = spec.get("type")
        if expected == "string" and not isinstance(v, str):
            return f"field {k} must be a string"
        if expected == "array" and not isinstance(v, list):
            return f"field {k} must be an array"
        if expected == "integer" and not isinstance(v, int):
            return f"field {k} must be an integer"
        if expected == "number" and not isinstance(v, (int, float)):
            return f"field {k} must be a number"
        if expected == "boolean" and not isinstance(v, bool):
            return f"field {k} must be a boolean"
    return None


def _fetch_schema(predicate_name: str) -> Optional[Dict[str, Any]]:
    """Ask the host for the predicate spec; pluck its claim_schema.

    Returns None on host error so the caller can demote to a `note`
    proposal instead of failing the whole submission.
    """
    try:
        spec = query("predicate.inspect", {"name": predicate_name})
    except FfsSkillError as e:
        log("warn", f"predicate.inspect({predicate_name}) failed: {e}")
        return None
    if isinstance(spec, dict):
        return spec.get("claim_schema") if isinstance(spec.get("claim_schema"), dict) else None
    return None


# --------------------------------------------------------------------
# Top-level handler
# --------------------------------------------------------------------


def _content_hash_hex(content: bytes) -> str:
    """BLAKE3-of-content as a hex string. The Rust daemon recomputes
    its multihash form server-side; the scribe just attaches a stable
    integrity tag.
    """
    try:
        import blake3  # type: ignore  # optional; sha256 fallback below.
        return blake3.blake3(content).hexdigest()
    except ImportError:
        return hashlib.sha256(content).hexdigest()


def _make_proposal(
    predicate: str,
    claim: Dict[str, Any],
    source_uri: str,
    content_hash_hex: str,
    rationale: str,
) -> Dict[str, Any]:
    return {
        "predicate": predicate,
        "claim": claim,
        "provenance": [
            {
                "kind": "ingest",
                "uri": source_uri,
                "hash_hex": content_hash_hex,
            }
        ],
        "rationale": rationale,
    }


def handle(inp: Any) -> Dict[str, Any]:
    """Top-level scribe entry point.

    `inp` shape::

        {"source_uri": "file:///...", "content": "...markdown..."}

    Returns ``{"proposals": [...], "warnings": [...]}``.
    """
    if not isinstance(inp, dict):
        return {"proposals": [], "warnings": ["input must be an object"]}
    source_uri = str(inp.get("source_uri") or "unknown:")
    content = inp.get("content") or ""
    if isinstance(content, bytes):
        content_bytes = content
        content_text = content.decode("utf-8", errors="replace")
    else:
        content_text = str(content)
        content_bytes = content_text.encode("utf-8")
    content_hash = _content_hash_hex(content_bytes)

    fm, sections, warnings = parse_markdown(content_text)
    proposals: List[Dict[str, Any]] = []

    # Conflict detection: if frontmatter name disagrees with body name,
    # emit a note proposal flagging the conflict.
    if conflicting := _conflicting_name(fm, content_text):
        proposals.append(
            _make_proposal(
                "note",
                {
                    "title": f"name conflict for {_name_field(fm)}",
                    "body": (
                        f"frontmatter says name={_name_field(fm)!r}; "
                        f"body says name={conflicting!r}. Reconcile manually."
                    ),
                    "tags": ["scribe-ambiguity"],
                },
                source_uri,
                content_hash,
                "structural-ambiguity: conflicting name claims",
            )
        )

    # Try contact.person first.
    if (claim := extract_contact_person(fm, sections)) is not None:
        proposals.append(
            _make_proposal(
                "contact.person",
                claim,
                source_uri,
                content_hash,
                "extracted display_name + contact fields from frontmatter and `Notes` section",
            )
        )
    # Then person.generic (only if contact.person didn't fire).
    elif (claim := extract_person_generic(fm, sections)) is not None:
        proposals.append(
            _make_proposal(
                "person.generic",
                claim,
                source_uri,
                content_hash,
                "extracted display_name + role/team from frontmatter",
            )
        )
    # Unstructured contact (no frontmatter, body looks like a
    # contact). Only fires when at least 2 distinct signals fire AND
    # a capitalized name is extractable from the body.
    elif (
        not _name_field(fm)
        and (unstructured := extract_contact_person_unstructured(content_text)) is not None
    ):
        unstructured_claim, unstructured_signals = unstructured
        proposals.append(
            _make_proposal(
                "contact.person",
                unstructured_claim,
                source_uri,
                content_hash,
                "matched "
                + str(len(unstructured_signals))
                + " unstructured-contact signals: "
                + ", ".join(unstructured_signals),
            )
        )

    # Always emit a note for unexplained content. If we extracted a
    # contact/person above, only emit a separate note when there's body
    # text beyond what the structured extraction captured.
    if not proposals or (proposals and content_text.strip()):
        note_claim = extract_note(fm, sections, content_text)
        # Avoid duplicating a tiny doc as both contact and note: only
        # add the note proposal if there's body text beyond
        # frontmatter.
        body_text = note_claim.get("body", "")
        already_have_structured = any(p["predicate"] != "note" for p in proposals)
        if not already_have_structured or (body_text and not _body_only_in_notes_section(sections)):
            proposals.append(
                _make_proposal(
                    "note",
                    note_claim,
                    source_uri,
                    content_hash,
                    "fallback note from raw markdown body",
                )
            )

    # Validate each proposal against its predicate's schema. Drop or
    # demote failures.
    validated: List[Dict[str, Any]] = []
    for p in proposals:
        schema = _fetch_schema(p["predicate"])
        if schema is None:
            # Host couldn't supply a schema — keep the proposal but
            # surface a warning so the auditor flags it.
            warnings.append(f"no schema for {p['predicate']}; proposal kept un-validated")
            validated.append(p)
            continue
        err = _validate_claim_against_schema(p["claim"], schema)
        if err is None:
            validated.append(p)
        else:
            log("info", f"dropping {p['predicate']} proposal: {err}")
            warnings.append(f"validation failed for {p['predicate']}: {err}")

    return {"proposals": validated, "warnings": warnings}


def _body_only_in_notes_section(sections: List[Tuple[str, List[str]]]) -> bool:
    """True if every non-empty body line lives inside a `## Notes`
    section — meaning the contact.person already captured everything
    in its `notes` list and a separate `note` proposal would be a
    duplicate.
    """
    for name, lines in sections:
        if name.lower() == "notes":
            continue
        for line in lines:
            if line.strip():
                return False
    return True


if __name__ == "__main__":
    run(handle)
