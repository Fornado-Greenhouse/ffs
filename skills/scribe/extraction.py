"""Scribe: markdown → proposed atoms, behind an `ExtractionEngine` seam.

Entry point for the skills host. Since task_36 (ADR-026) the pipeline
per invocation is:

1. Build a ``Submission`` (``engine.py``): tolerant frontmatter parse,
   ``## Heading`` sections, content hash, filename, ``predicate:`` hint.
2. Load the ``PredicateRegistry`` (``registry.py``) from
   ``$FFS_DATA_DIR/config/predicates/*.toml``; fall back to the host's
   ``predicate.inspect`` when no specs are on disk.
3. Run the selected engine (``FFS_SCRIBE_ENGINE``: ``heuristic`` by
   default, ``llm`` opt-in). The heuristic engine (``heuristic.py``)
   is this file's regex and frontmatter extractors:
   - frontmatter `name` + (`email`/`phone`/`org` or a `Notes` section)
     → `contact.person`.
   - frontmatter `name` + (`role`/`team`) without contact hints
     → `person.generic`.
   - unstructured body with a name plus a second contact signal
     → `contact.person` (card shape, filename, phone, email, venue).
   - everything else → `note`.
4. Apply the ``predicate:`` hint (registered predicate targets it;
   ``source.article`` or an unregistered predicate falls back to a
   `note` that keeps title, url, and summary).
5. Validate each claim against its predicate's ``claim_schema``
   (``validate.py``). Rejected claims are dropped with a warning.
6. Every proposal carries provenance (source URI + content hash) and
   the ``engine`` / ``model`` that produced it.
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
for _p in (_LIB, _HERE):
    if _p not in sys.path:
        sys.path.insert(0, _p)

from ffs_skill import FfsSkillError, log, query, run  # noqa: E402

from engine import Submission, apply_hint, select_engine  # noqa: E402
from registry import PredicateRegistry  # noqa: E402
from validate import validate_claim  # noqa: E402


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
    """Pull a display name from frontmatter, in preference order.

    A frontmatter ``title`` is a note title, never a person's name
    (task_36 hygiene): "title: Grocery list" used to mint a
    person.generic named "Grocery list". Only ``display_name`` and
    ``name`` seed a person.
    """
    for key in ("display_name", "name"):
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


# Field-label words (task_36 hygiene fix 3). These are the labels of a
# key-value contact card ("Nickname: Bones", "Occupation: UFC fighter")
# and must never be taken for a person's name, which is exactly how
# "Bones Occupation" was minted on 2026-06-21 (ADR-026).
_FIELD_LABEL_WORDS = frozenset(
    [
        "nickname", "occupation", "phone", "email", "address", "first",
        "last", "name", "title", "company", "organization", "employer",
        "mobile", "cell", "website", "notes", "note", "birthday", "fax",
        "job", "role", "telephone",
    ]
)


def extract_capitalized_name(text: str) -> Optional[str]:
    """Find a two-word capitalized name in `text`. Skips a small
    set of English grammar function words, common past-tense
    interaction verbs, and contact-card field labels (the false
    positives that are universally safe to reject). The caller is
    expected to pre-mask venue spans, so we don't need a venue
    stop-list here.

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
        lf, ll = first.lower(), last.lower()
        if lf in _NAME_STOPWORDS or ll in _NAME_STOPWORDS:
            continue
        if lf in _FIELD_LABEL_WORDS or ll in _FIELD_LABEL_WORDS:
            continue
        return f"{first} {last}"
    return None


# --------------------------------------------------------------------
# task_36 hygiene: filename as a name candidate + card-shape parsing
# --------------------------------------------------------------------

_PERSON_WORD_RE = re.compile(r"^[A-Z][a-z]+$")


def filename_as_person_name(filename: Optional[str]) -> Optional[str]:
    """Return ``"First Last"`` when a file's stem looks like a person's
    name: two or three capitalized words after ``_``/``-`` become
    spaces, none of them a stop word or a field label. Otherwise
    ``None`` (dates, slugs, lowercase names, "Phone List")."""
    if not filename:
        return None
    words = [w for w in re.split(r"[\s_\-]+", filename.strip()) if w]
    if not 2 <= len(words) <= 3:
        return None
    for w in words:
        if not _PERSON_WORD_RE.match(w):
            return None
        lw = w.lower()
        if lw in _NAME_STOPWORDS or lw in _FIELD_LABEL_WORDS:
            return None
    return " ".join(words)


def _names_agree(a: str, b: str) -> bool:
    """Two names agree when they are the same name, ignoring case and
    spacing. Sharing a surname is not agreement: "Haddad Imports" is
    not "Omar Haddad"."""
    return " ".join(a.lower().split()) == " ".join(b.lower().split())


# Card line: "Label: value" or "Label - value". The label is one
# capitalized word followed by any number of lowercase words, which is
# what "Phone numbe r- 9194284074" (typo included) looks like.
_CARD_LINE_RE = re.compile(r"^([A-Z][a-z]*(?:\s[a-z]+)*)\s*[:\-]\s*(.*)$")

# Canonical card keys. A label matches a key when their first four
# letters agree (typo-tolerant: "Phone numbe r" -> "phone") or the key
# starts with the whole label ("Nam" -> "name").
_CARD_KEYS = {
    "name": "display_name",
    "fullname": "display_name",
    "first": "first",
    "last": "last",
    "phone": "phone",
    "telephone": "phone",
    "mobile": "phone",
    "cell": "phone",
    "email": "email",
    "nickname": "nickname",
    "occupation": "role",
    "title": "role",
    "role": "role",
    "job": "role",
    "company": "organization",
    "organization": "organization",
    "employer": "organization",
    "address": "note",
    "website": "note",
    "notes": "note",
    "note": "note",
    "birthday": "note",
}

_PERSONAL_EMAIL_DOMAINS = frozenset(
    ["gmail.com", "yahoo.com", "hotmail.com", "outlook.com", "icloud.com", "me.com", "aol.com", "proton.me", "protonmail.com"]
)


def _card_key(label: str) -> Optional[str]:
    norm = re.sub(r"[^a-z]", "", label.lower())
    if len(norm) < 3:
        return None
    for key, field in _CARD_KEYS.items():
        if norm[:4] == key[:4] or key.startswith(norm):
            return field
    return None


def _normalize_phone(value: str) -> str:
    digits = re.sub(r"\D", "", value)
    if len(digits) == 11 and digits.startswith("1"):
        digits = digits[1:]
    if len(digits) == 10:
        return f"{digits[:3]}-{digits[3:6]}-{digits[6:]}"
    return value.strip()


def parse_card_lines(content_text: str) -> Optional[Dict[str, Any]]:
    """Parse a key-value contact card into contact.person fields.

    A card is at least two ``Label: value`` lines of which at least
    one label is a known contact field. Returns ``None`` when the text
    is not a card (prose, a single labelled line) so the caller can
    fall through to the bigram scan.
    """
    matches: List[Tuple[str, str]] = []
    for raw in content_text.splitlines():
        line = raw.strip()
        m = _CARD_LINE_RE.match(line)
        if m and m.group(2).strip():
            matches.append((m.group(1), m.group(2).strip()))
    if len(matches) < 2:
        return None
    claim: Dict[str, Any] = {}
    notes: List[str] = []
    first = last = None
    known = 0
    for label, value in matches:
        field = _card_key(label)
        if field is None:
            continue
        known += 1
        if field == "display_name":
            claim.setdefault("display_name", value)
        elif field == "first":
            first = value
        elif field == "last":
            last = value
        elif field == "phone":
            claim.setdefault("phone", _normalize_phone(value))
        elif field == "email":
            em = _EMAIL_PATTERN.search(value)
            if em:
                addr = em.group(0)
                domain = addr.rsplit("@", 1)[-1].lower()
                key = "personal_email" if domain in _PERSONAL_EMAIL_DOMAINS else "work_email"
                claim.setdefault(key, addr)
        elif field == "nickname":
            notes.append(f"Nickname: {value}")
        elif field == "role":
            claim.setdefault("role", value)
        elif field == "organization":
            claim.setdefault("organization", value)
        elif field == "note":
            notes.append(f"{label.strip().capitalize()}: {value}")
    if known == 0:
        return None
    if "display_name" not in claim and first and last:
        claim["display_name"] = f"{first} {last}"
    if notes:
        claim["notes"] = notes
    return claim


def extract_contact_person_unstructured(
    content_text: str,
    filename: Optional[str] = None,
) -> Optional[Tuple[Dict[str, Any], List[str]]]:
    """Walk the body for unstructured contact signals: a key-value
    card, the filename, phone, email, venue mention, capitalized
    name. Emit a `contact.person` claim when ≥2 distinct signals fire
    AND a name is available (from the card, the filename, or text
    where venue spans have been masked out).

    Returns `(claim, signals)` — `signals` is the list of signal
    names that fired (used for the proposal's `rationale` string).
    """
    fname_name = filename_as_person_name(filename)

    # Hygiene fix 2: a key-value card is parsed field by field and the
    # bigram scan is skipped entirely (it is what stitched "Bones
    # Occupation" together).
    card = parse_card_lines(content_text)
    if card is not None:
        signals: List[str] = ["key-value card"]
        claim: Dict[str, Any] = dict(card)
        body_name = claim.get("display_name")
        if fname_name and (not body_name or not _names_agree(body_name, fname_name)):
            claim["display_name"] = fname_name
            signals.append("filename")
        if not claim.get("display_name"):
            return None
        if claim.get("phone"):
            signals.append("phone number")
        if claim.get("work_email") or claim.get("personal_email"):
            signals.append("email address")
        if claim.get("role") or claim.get("organization") or claim.get("notes"):
            signals.append("card fields")
        if len(signals) < 2:
            return None
        return claim, signals

    # Detect venues FIRST. Venues are masked from the text before
    # we run the name detector so e.g. "Met at Ballantyne Country
    # Club" doesn't get misclassified as a person named "Ballantyne
    # Country".
    venues = detect_venue_mentions(content_text)
    masked = _mask_spans(content_text, [(s, e) for _, s, e in venues])

    name = extract_capitalized_name(masked)
    signals = ["capitalized name"]
    # Hygiene fix 1: the filename is a name candidate that wins when
    # the body yields nothing or a name that fails the cross-check.
    if fname_name and (not name or not _names_agree(name, fname_name)):
        name = fname_name
        signals = ["filename"]
    if not name:
        return None
    claim = {"display_name": name}

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


# The validator lives in ``validate.py`` since task_36; this alias keeps
# the pre-task_36 import path working for anything that used it.
_validate_claim_against_schema = validate_claim


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


def handle(inp: Any) -> Dict[str, Any]:
    """Top-level scribe entry point.

    `inp` shape::

        {"source_uri": "file:///...", "content": "...markdown..."}

    Returns ``{"proposals": [...], "warnings": [...]}``. Every proposal
    carries ``engine`` and ``model`` (ADR-026 provenance).
    """
    if not isinstance(inp, dict):
        return {"proposals": [], "warnings": ["input must be an object"]}

    submission = Submission.from_input(inp)
    registry = PredicateRegistry.load(fallback=_fetch_schema)
    for w in registry.warnings:
        log("warn", w)
    engine = select_engine()

    try:
        result = engine.extract(submission, registry)
    except Exception as e:  # noqa: BLE001 - ingest must never hard-fail on an engine
        from heuristic import HeuristicEngine

        log("warn", f"{engine.name} engine raised {type(e).__name__}: {e}; falling back to heuristic")
        fallback = HeuristicEngine()
        result = fallback.extract(submission, registry)
        result.warnings.append(f"{engine.name} engine failed ({type(e).__name__}); heuristic result emitted")
        engine = fallback

    result = apply_hint(submission, result, registry, engine.name, engine.model)

    # Validate each proposal against its predicate's schema. Drop
    # failures with a warning so nothing is silently accepted.
    warnings: List[str] = list(submission.parse_warnings) + list(result.warnings)
    validated: List[Dict[str, Any]] = []
    for p in result.proposals:
        p.setdefault("engine", engine.name)
        p.setdefault("model", engine.model)
        schema = registry.schema(p["predicate"])
        if schema is None:
            # No schema on disk or from the host: keep the proposal but
            # surface a warning so the auditor flags it.
            warnings.append(f"no schema for {p['predicate']}; proposal kept un-validated")
            validated.append(p)
            continue
        err = validate_claim(p["claim"], schema)
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
