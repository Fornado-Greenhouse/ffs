"""The article ingest contract (task_40 § 40.5, ADR-031).

Parses the sections a courier (or an owner during the morning read)
writes into an ingest file and turns them into structured claim
fields and secondary proposals:

- ``## Mentions`` bullets ``- <Name> — <context>`` become
  ``mentions[]`` items ``{display, context}`` on the article claim and
  one person-or-organization proposal per bullet.
- ``## Events`` bullets ``- <kind>: <description> | <Name> (<role>), ...``
  become event proposals with ``participants[]`` items ``{display, role}``.
- ``## References`` bullets ``- [[basename|title]]`` or ``- <url>``
  populate a note's ``references[]``.

Nothing here names a predicate directly: the article predicate is
whatever the ``predicate:`` hint said, the secondary predicates are
found in the registry by schema shape (a ``mentions`` object array, a
``participants`` object array, a ``person``/``organization`` pair), and
the enumerations for event kinds and participant roles are read from
the registered schemas with a fallback to ADR-028 / ADR-031 vocabulary.
Stdlib only.
"""

from __future__ import annotations

import hashlib
import os
import re
from typing import Any, Dict, List, Optional, Tuple

from engine import (
    Submission,
    assign_local_refs,
    attach_source_article,
    bind_refs,
    make_proposal,
)

# Fallback vocabularies (ADR-028 kinds, ADR-031 roles). The registered
# schema's enum wins when present.
FALLBACK_EVENT_KINDS: Tuple[str, ...] = (
    "funding", "acquisition", "hire", "departure", "expansion", "opening",
    "closing", "award", "partnership", "other",
)
FALLBACK_PARTICIPANT_ROLES: Tuple[str, ...] = (
    "acquirer", "target", "investor", "investee", "hire", "employer", "departing",
    "landlord", "tenant", "developer", "winner", "partner", "other",
)

DEFAULT_BODY_LIMIT = 4000

_BULLET_RE = re.compile(r"^\s*[-*+]\s+(.*\S)\s*$")
_MENTION_SEP_RE = re.compile(r"\s+—\s+|\s+–\s+|\s+-\s+|:\s+")
_WIKILINK_RE = re.compile(r"\[\[([^\]|]+)(?:\|[^\]]*)?\]\]")
_URL_RE = re.compile(r"https?://[^\s<>()\[\]\"']+")
_PARTICIPANT_RE = re.compile(r"^\s*(.+?)\s*\(([^()]*)\)\s*$")

# Words in a mention's context that mark a person's role rather than
# an organization's description.
_ROLE_WORDS = frozenset(
    """
    ceo cfo coo cto cio cmo president vice chair chairman chairwoman chief officer
    founder cofounder co-founder partner principal owner director managing manager
    head lead executive vp svp evp counsel attorney lawyer analyst engineer editor
    reporter professor dean mayor councilman councilwoman councilmember commissioner
    senator representative broker agent developer(person) treasurer secretary
    spokesperson spokesman spokeswoman coach player chef designer architect
    """.split()
)
# Tokens that mark an organization name.
_ORG_TOKENS = frozenset(
    """
    inc inc. llc l.l.c. corp corp. corporation co co. company group holdings partners
    capital ventures bank university college county city council authority board
    foundation association institute hospital health realty properties development
    trust fund school district department agency office commission committee league
    club church center centre museum library airport network systems technologies
    """.split()
)


def body_limit() -> int:
    raw = os.environ.get("FFS_SCRIBE_BODY_LIMIT", "").strip()
    try:
        n = int(raw) if raw else DEFAULT_BODY_LIMIT
    except ValueError:
        n = DEFAULT_BODY_LIMIT
    return max(200, n)


def truncate_body(text: str, limit: Optional[int] = None) -> Tuple[str, Optional[str]]:
    """Cut ``text`` at ``limit`` characters. Returns ``(text, warning)``;
    the warning is the ``parse-warning`` line for the rationale."""
    limit = body_limit() if limit is None else limit
    if len(text) <= limit:
        return text, None
    cut = text[:limit].rstrip() + " (truncated)"
    return cut, f"parse-warning: body truncated from {len(text)} to {limit} characters"


def _bullets(lines: List[str]) -> List[str]:
    out: List[str] = []
    for line in lines:
        m = _BULLET_RE.match(line)
        if m:
            out.append(m.group(1).strip())
    return out


def _section(submission: Submission, name: str) -> Optional[List[str]]:
    for sec_name, lines in submission.sections:
        if sec_name.strip().lower() == name.lower():
            return lines
    return None


def parse_mentions(lines: List[str]) -> List[Dict[str, str]]:
    """``- <Name> — <context>`` bullets to ``{display, context}`` items,
    both verbatim. A bullet without a separator is a bare display."""
    items: List[Dict[str, str]] = []
    for text in _bullets(lines):
        parts = _MENTION_SEP_RE.split(text, maxsplit=1)
        display = parts[0].strip()
        context = parts[1].strip() if len(parts) > 1 else ""
        if not display:
            continue
        item = {"display": display}
        if context:
            item["context"] = context
        items.append(item)
    return items


def parse_references(lines: List[str]) -> List[str]:
    """``- [[basename|title]]`` yields the basename; ``- <url>`` yields
    the url; deduped in order."""
    refs: List[str] = []
    for text in _bullets(lines):
        m = _WIKILINK_RE.search(text)
        value = m.group(1).strip() if m else None
        if value is None:
            u = _URL_RE.search(text)
            value = u.group(0).rstrip(".,;") if u else None
        if value and value not in refs:
            refs.append(value)
    return refs


def parse_events(
    lines: List[str],
    kinds: Tuple[str, ...] = FALLBACK_EVENT_KINDS,
    roles: Tuple[str, ...] = FALLBACK_PARTICIPANT_ROLES,
) -> Tuple[List[Dict[str, Any]], List[str]]:
    """``- <kind>: <description> | <Name> (<role>), <Name> (<role>)``.
    Unknown kinds and roles map to ``other`` with a warning."""
    events: List[Dict[str, Any]] = []
    warnings: List[str] = []
    for text in _bullets(lines):
        head, _, tail = text.partition("|")
        kind_raw, sep, description = head.partition(":")
        if not sep:
            kind_raw, description = "other", head
            warnings.append(f"event bullet without a kind prefix; kind set to other: {head.strip()[:60]!r}")
        kind = kind_raw.strip().lower()
        if kind not in kinds:
            warnings.append(f"unknown event kind {kind!r}; kind set to other")
            kind = "other"
        participants: List[Dict[str, str]] = []
        for chunk in tail.split(","):
            chunk = chunk.strip()
            if not chunk:
                continue
            m = _PARTICIPANT_RE.match(chunk)
            if m:
                display, role = m.group(1).strip(), m.group(2).strip().lower()
            else:
                display, role = chunk, "other"
            if role not in roles:
                warnings.append(f"unknown participant role {role!r} for {display!r}; role set to other")
                role = "other"
            if display:
                participants.append({"display": display, "role": role})
        events.append(
            {"kind": kind, "description": description.strip(), "participants": participants}
        )
    return events, warnings


def looks_like_org(display: str) -> bool:
    tokens = [t.strip(".,").lower() for t in display.split()]
    return any(t in _ORG_TOKENS for t in tokens)


def looks_like_person(display: str) -> bool:
    if looks_like_org(display):
        return False
    tokens = display.replace("-", " ").split()
    if not 2 <= len(tokens) <= 4:
        return False
    if any(ch.isdigit() for ch in display):
        return False
    return all(t[0].isupper() for t in tokens if t and t[0].isalpha())


def _has_role_word(context: str) -> bool:
    return any(t.strip(".,;()").lower() in _ROLE_WORDS for t in context.split())


def classify_mention(display: str, context: str) -> str:
    """``"person"`` or ``"org"`` for a mention bullet."""
    if looks_like_org(display):
        return "org"
    if _has_role_word(context) or looks_like_person(display):
        return "person"
    return "org"


def _org_from_context(context: str, org_displays: List[str]) -> Optional[str]:
    """The organization a person mention's context names: an org
    display from the same set found inside the context, else the text
    after ``at`` / ``of`` / ``with``."""
    low = context.lower()
    for org in org_displays:
        if org.lower() in low:
            return org
    m = re.search(r"\b(?:at|of|with|for)\s+([A-Z][^,;.]*)", context)
    if m:
        return m.group(1).strip()
    return None


def _role_from_context(context: str) -> Optional[str]:
    if not _has_role_word(context):
        return None
    role = re.split(r"\s+(?:at|of|with|for)\s+|,", context, maxsplit=1)[0].strip()
    return role or None


def _find_predicate(registry: Any, predicate_filter) -> Optional[str]:
    try:
        names = registry.names()
    except Exception:  # noqa: BLE001
        return None
    for name in names:
        try:
            schema = registry.schema(name) or {}
        except Exception:  # noqa: BLE001
            continue
        if predicate_filter(name, schema):
            return name
    return None


def _has_object_array(schema: Dict[str, Any], key: str) -> bool:
    props = schema.get("properties") or {}
    spec = props.get(key)
    if not isinstance(spec, dict) or spec.get("type") != "array":
        return False
    items = spec.get("items")
    return isinstance(items, dict) and items.get("type") == "object"


def _enum(schema: Dict[str, Any], *path: str) -> Optional[Tuple[str, ...]]:
    node: Any = schema
    for key in path:
        if not isinstance(node, dict):
            return None
        node = node.get(key)
    if isinstance(node, dict) and isinstance(node.get("enum"), list):
        return tuple(str(v) for v in node["enum"])
    return None


def _person_predicate(registry: Any) -> Optional[str]:
    # A person predicate: display_name required, no `person`/`organization`
    # pair (that is a role), and a `role` or `organization` property.
    def f(name: str, schema: Dict[str, Any]) -> bool:
        props = schema.get("properties") or {}
        required = set(schema.get("required") or [])
        return (
            "display_name" in required
            and not {"person", "organization"} <= required
            and ("role" in props or "organization" in props)
            and "industry" not in props
            and "work_email" not in props
        )

    return _find_predicate(registry, f)


def _org_predicate(registry: Any) -> Optional[str]:
    def f(name: str, schema: Dict[str, Any]) -> bool:
        props = schema.get("properties") or {}
        required = set(schema.get("required") or [])
        return "display_name" in required and "industry" in props

    return _find_predicate(registry, f)


def _event_predicate(registry: Any) -> Optional[str]:
    def f(name: str, schema: Dict[str, Any]) -> bool:
        return _has_object_array(schema, "participants") and "kind" in (schema.get("properties") or {})

    return _find_predicate(registry, f)


def article_supports_mentions(registry: Any, predicate: str) -> bool:
    try:
        schema = registry.schema(predicate) or {}
    except Exception:  # noqa: BLE001
        return False
    return _has_object_array(schema, "mentions")


def _fm_str(submission: Submission, key: str) -> Optional[str]:
    v = (submission.frontmatter or {}).get(key)
    return v.strip() if isinstance(v, str) and v.strip() else None


def build_article_set(
    submission: Submission,
    article_claim: Dict[str, Any],
    predicate: str,
    registry: Any,
    engine_name: str,
    model: str,
) -> Tuple[List[Dict[str, Any]], List[str]]:
    """Turn a hinted article submission into its proposal set: the
    article (with ``mentions[]``), one person or org proposal per
    mention, one event proposal per event bullet; local_refs assigned,
    refs bound, article provenance attached. Returns (proposals, warnings)."""
    warnings: List[str] = []
    fm = submission.frontmatter or {}
    schema = registry.schema(predicate) or {}
    props = schema.get("properties") or {}

    # Frontmatter fields that are provenance or policy, never claim data.
    intake = _fm_str(submission, "intake")
    fetch = _fm_str(submission, "fetch")
    reported_by = _fm_str(submission, "reported_by")
    record_id = _fm_str(submission, "record_id")
    policy_note = ", ".join(x for x in (f"intake: {intake}" if intake else "", f"fetch: {fetch}" if fetch else "") if x)
    if intake and "tags" in props:
        tags = list(article_claim.get("tags") or [])
        tag = f"intake-{intake}"
        if tag not in tags:
            tags.append(tag)
        article_claim["tags"] = tags
    structured = "courier-structured" in [str(t) for t in (article_claim.get("tags") or [])]

    extra_prov: List[Dict[str, Any]] = []
    if reported_by:
        extra_prov.append(
            {"kind": "reported_by", "uri": f"outlet:{reported_by}", "hash_hex": submission.content_hash_hex}
        )
    read_prov, clip_hint = read_session_provenance(
        submission,
        article_claim.get("url") if isinstance(article_claim.get("url"), str) else None,
        clippable=clippable_predicate(registry, predicate),
    )
    extra_prov.extend(read_prov)

    # Mentions.
    mention_lines = _section(submission, "Mentions")
    mentions = parse_mentions(mention_lines) if mention_lines else []
    if mentions and _has_object_array(schema, "mentions"):
        article_claim["mentions"] = [dict(m) for m in mentions]

    rationale = f"predicate hint {predicate!r}: claim built from the frontmatter keys the schema declares"
    if policy_note:
        rationale += f" ({policy_note})"
    proposals: List[Dict[str, Any]] = [
        make_proposal(
            predicate, article_claim, submission, rationale, engine_name, model,
            local_ref="article", extra_provenance=extra_prov or None,
            classification_hint=clip_hint,
        )
    ]

    person_pred = _person_predicate(registry)
    org_pred = _org_predicate(registry)
    kinds = [classify_mention(m["display"], m.get("context", "")) for m in mentions]
    org_displays = [m["display"] for m, k in zip(mentions, kinds) if k == "org"]
    hint_rationale = "courier-structured hint" if structured else "mention bullet from the article's `## Mentions` section"
    if record_id:
        hint_rationale += f" (record {record_id})"
    if mentions and (person_pred is None or org_pred is None):
        warnings.append(
            "mentions kept on the article only: the registry has no person or organization predicate to propose"
        )
    else:
        for m, kind in zip(mentions, kinds):
            display, context = m["display"], m.get("context", "")
            if kind == "person":
                claim: Dict[str, Any] = {"display_name": display}
                org = _org_from_context(context, org_displays)
                if org:
                    claim["organization"] = org
                role = _role_from_context(context)
                if role:
                    claim["role"] = role
                proposals.append(make_proposal(person_pred, claim, submission, hint_rationale, engine_name, model))
            else:
                claim = {"display_name": display}
                if context:
                    org_schema = registry.schema(org_pred) or {}
                    if "description" in (org_schema.get("properties") or {}):
                        claim["description"] = context
                proposals.append(make_proposal(org_pred, claim, submission, hint_rationale, engine_name, model))

    # Events.
    event_lines = _section(submission, "Events")
    if event_lines:
        event_pred = _event_predicate(registry)
        if event_pred is None:
            warnings.append("events dropped: the registry has no event predicate with participants")
        else:
            ev_schema = registry.schema(event_pred) or {}
            kinds_enum = _enum(ev_schema, "properties", "kind") or FALLBACK_EVENT_KINDS
            roles_enum = _enum(ev_schema, "properties", "participants", "items", "properties", "role") or FALLBACK_PARTICIPANT_ROLES
            events, ev_warnings = parse_events(event_lines, kinds_enum, roles_enum)
            warnings.extend(ev_warnings)
            ev_props = ev_schema.get("properties") or {}
            for ev in events:
                claim = {"kind": ev["kind"], "summary": ev["description"]}
                if "title" in ev_props:
                    claim["title"] = ev["description"][:80]
                if ev["participants"]:
                    claim["participants"] = ev["participants"]
                date = _fm_str(submission, "published_at")
                if date and "date" in ev_props:
                    claim["date"] = date
                url = article_claim.get("url")
                if isinstance(url, str) and url and "source" in ev_props:
                    claim["source"] = url
                proposals.append(
                    make_proposal(event_pred, claim, submission, "event bullet from the article's `## Events` section", engine_name, model)
                )

    assign_local_refs(proposals)
    bind_refs(proposals)
    attach_source_article(submission, proposals)
    return proposals, warnings

# ---------------------------------------------------------------------
# Morning read (ADR-035, task_48)
# ---------------------------------------------------------------------

READ_SESSION_KEYS = ("intake", "owner_present", "session", "actor")


def read_session_provenance(
    submission: Submission, article_url: Optional[str], clippable: bool = True
) -> Tuple[List[Dict[str, Any]], Optional[str]]:
    """Provenance for a submission filed during an owner-present morning
    read. Returns (extra provenance entries, classification hint).

    The atom envelope's provenance entry is frozen at {kind, uri, hash},
    so the read is encoded as two entries: ``morning_read`` (uri = the
    article url the owner had open, hash = the submitted content's hash)
    and ``session`` (uri = ``ffs-session://<actor>/<session>?owner_present=true``,
    hash = the session id's digest). None of the four frontmatter keys
    ever becomes claim data. The hint is ``"clip"`` only when the owner
    clipped a body (intake morning_read, owner present, and a non-empty
    prose body); pointers never get it.
    """
    fm = submission.frontmatter or {}
    intake = str(fm.get("intake") or "").strip().lower()
    if intake != "morning_read":
        return [], None
    owner_present = str(fm.get("owner_present") or "").strip().lower() in ("true", "yes", "1")
    session = str(fm.get("session") or "").strip() or "unknown"
    actor = str(fm.get("actor") or "").strip() or "assistant"
    entries: List[Dict[str, Any]] = [
        {
            "kind": "morning_read",
            "uri": article_url or submission.source_uri,
            "hash_hex": submission.content_hash_hex,
        },
        {
            "kind": "session",
            "uri": f"ffs-session://{actor}/{session}?owner_present={'true' if owner_present else 'false'}",
            "hash_hex": hashlib.sha256(session.encode("utf-8")).hexdigest(),
        },
    ]
    body = submission.body_text().strip() if hasattr(submission, "body_text") else ""
    # Only an information-bearing record (an article, something with a
    # url) can be a clip; a person or org note filed from the read is a
    # note, whatever its body says.
    hint = "clip" if (clippable and owner_present and body) else None
    return entries, hint


def clippable_predicate(registry: Any, predicate: str) -> bool:
    """A predicate whose schema declares a ``url`` property is an
    information-bearing record and may be clipped (ADR-035)."""
    try:
        schema = registry.schema(predicate) or {}
    except Exception:  # noqa: BLE001
        return False
    props = schema.get("properties") or {}
    return isinstance(props, dict) and "url" in props


def render_session_log(
    date: str,
    opened: List[Tuple[str, str]],
    proposals: List[Tuple[str, str, str]],
    refusals: List[Tuple[str, str]],
    clips: List[Tuple[str, str]],
) -> str:
    """The morning read's audit trail (ADR-035): one ``note`` per read,
    filed at session end through the same accept path. ``opened`` is
    (time, url); ``proposals`` is (predicate, display, outcome) with
    outcome one of filed | proposed | skipped; ``refusals`` is (time,
    request); ``clips`` is (basename, title). Sections with no items are
    omitted so the log never claims an action that did not happen.
    """
    lines = [
        "---",
        "predicate: note",
        f"title: Morning read {date}",
        "tags: [morning-read, session-log]",
        "intake: morning_read",
        "---",
        "",
        f"Session log for the morning read of {date}.",
    ]
    if opened:
        lines += ["", "## Opened"] + [f"- {t} {u}" for t, u in opened]
    if proposals:
        lines += ["", "## Proposals"] + [f"- {p}: {d} ({o})" for p, d, o in proposals]
    if refusals:
        lines += ["", "## Refusals"] + [f"- {t} {r}: refused as bulk behavior" for t, r in refusals]
    if clips:
        lines += ["", "## Clips"] + [f"- [[{b}|{t}]]" for b, t in clips]
    return "\n".join(lines) + "\n"
