"""The scribe's `ExtractionEngine` seam (task_36, ADR-026).

One submission in, one `EngineResult` out. Two engines implement it:

- ``heuristic`` (``heuristic.py``): today's regex and frontmatter code,
  always available, fully offline, the default.
- ``llm`` (``llm.py``): a stdlib-``urllib`` client to a configurable
  backend, strictly opt-in via ``FFS_SCRIBE_ENGINE=llm``.

Nothing leaves the machine and no new runtime dependency exists unless
the user opts in: ``select_engine`` imports ``llm.py`` lazily and falls
back to the heuristic engine if that import or its construction fails.

The `predicate:` frontmatter hint (task_36 § 36.4) is applied here, after
the engine runs, so both engines get identical hint semantics.
"""

from __future__ import annotations

import os
import re
import urllib.parse
from dataclasses import dataclass, field
from typing import Any, Dict, List, Mapping, Optional, Protocol, Tuple

from ffs_skill import log

# ---------------------------------------------------------------------
# Submission
# ---------------------------------------------------------------------


@dataclass
class Submission:
    source_uri: str
    content_text: str
    content_bytes: bytes = b""
    content_hash_hex: str = ""
    filename: Optional[str] = None
    frontmatter: Dict[str, Any] = field(default_factory=dict)
    sections: List[Tuple[str, List[str]]] = field(default_factory=list)
    parse_warnings: List[str] = field(default_factory=list)
    predicate_hint: Optional[str] = None

    @classmethod
    def from_input(cls, inp: Dict[str, Any]) -> "Submission":
        # Imported lazily: extraction imports this module at load time.
        from extraction import _content_hash_hex, parse_markdown

        source_uri = str(inp.get("source_uri") or "unknown:")
        content = inp.get("content") or ""
        if isinstance(content, bytes):
            content_bytes = content
            content_text = content.decode("utf-8", errors="replace")
        else:
            content_text = str(content)
            content_bytes = content_text.encode("utf-8")
        fm, sections, warnings = parse_markdown(content_text)
        hint_raw = fm.get("predicate")
        hint = str(hint_raw).strip().lower() if isinstance(hint_raw, str) and hint_raw.strip() else None
        return cls(
            source_uri=source_uri,
            content_text=content_text,
            content_bytes=content_bytes,
            content_hash_hex=_content_hash_hex(content_bytes),
            filename=filename_from_uri(source_uri),
            frontmatter=fm,
            sections=sections,
            parse_warnings=list(warnings),
            predicate_hint=hint,
        )

    def body_text(self) -> str:
        return "\n".join(line for _, lines in self.sections for line in lines).strip()


def filename_from_uri(source_uri: str) -> Optional[str]:
    """Basename without extension for a ``file://`` URI (percent
    decoded), else ``None``."""
    if not source_uri.startswith("file:"):
        return None
    path = urllib.parse.unquote(urllib.parse.urlparse(source_uri).path)
    base = os.path.basename(path)
    if not base:
        return None
    stem, _ext = os.path.splitext(base)
    return stem or None


# ---------------------------------------------------------------------
# Engine protocol
# ---------------------------------------------------------------------


@dataclass
class EngineResult:
    proposals: List[Dict[str, Any]] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)


class ExtractionEngine(Protocol):
    name: str
    model: str

    def extract(self, submission: Submission, registry: Any) -> EngineResult: ...


def select_engine(env: Optional[Mapping[str, str]] = None) -> ExtractionEngine:
    """Pick the engine named by ``FFS_SCRIBE_ENGINE`` (default
    ``heuristic``). The llm engine is imported lazily so the default
    path never touches ``llm.py``; any failure falls back to heuristic."""
    from heuristic import HeuristicEngine  # lazy: avoids an import cycle

    if env is None:
        env = os.environ
    choice = (env.get("FFS_SCRIBE_ENGINE") or "heuristic").strip().lower()
    if choice == "heuristic":
        return HeuristicEngine()
    if choice == "llm":
        try:
            from llm import LlmEngine  # type: ignore

            return LlmEngine.from_env(env)
        except Exception as e:  # noqa: BLE001 - never let opt-in config break ingest
            log("warn", f"llm engine unavailable ({type(e).__name__}: {e}); using heuristic")
            return HeuristicEngine()
    log("warn", f"unknown FFS_SCRIBE_ENGINE={choice!r}; using heuristic")
    return HeuristicEngine()


# ---------------------------------------------------------------------
# Proposal construction
# ---------------------------------------------------------------------


def make_proposal(
    predicate: str,
    claim: Dict[str, Any],
    submission: Submission,
    rationale: str,
    engine_name: str,
    model: str,
    *,
    local_ref: Optional[str] = None,
    valid_from: Optional[str] = None,
    valid_to: Optional[str] = None,
    ends_role: bool = False,
    extra_provenance: Optional[List[Dict[str, Any]]] = None,
) -> Dict[str, Any]:
    """Build one wire proposal.

    The keyword-only extras are the task_45 multi-entity conventions.
    Every one of them is optional and omitted from the dict when unset,
    so a single proposal without them is the degenerate case and the
    wire stays backward compatible:

    - ``local_ref``: a token unique within one submission's set
      (``"org-1"``, ``"person-2"``, ``"article"``) that other proposals'
      ``refs`` point at.
    - ``valid_from`` / ``valid_to``: ISO dates for the claim's validity
      window (an affiliation's start, or its end when ``ends_role``).
    - ``ends_role``: the proposal ends an existing role; the daemon
      turns it into a supersession of the matching affiliation head
      rather than a new atom.
    - ``extra_provenance``: further provenance entries (the
      ``source_article`` entry that ties every proposal in an article
      set to the article url).
    """
    provenance: List[Dict[str, Any]] = [
        {
            "kind": "ingest",
            "uri": submission.source_uri,
            "hash_hex": submission.content_hash_hex,
        }
    ]
    if extra_provenance:
        provenance.extend(dict(e) for e in extra_provenance)
    out: Dict[str, Any] = {
        "predicate": predicate,
        "claim": claim,
        "provenance": provenance,
        "rationale": rationale,
        "engine": engine_name,
        "model": model,
    }
    if local_ref:
        out["local_ref"] = local_ref
    if valid_from:
        out["valid_from"] = valid_from
    if valid_to:
        out["valid_to"] = valid_to
    if ends_role:
        out["ends_role"] = True
    return out


def source_article_provenance(submission: Submission, url: str) -> Dict[str, Any]:
    """The provenance entry that ties a proposal to the article it came
    from (task_45). ``hash_hex`` is the submission's content hash so the
    entry is stable across every proposal in the set."""
    return {"kind": "source_article", "uri": url, "hash_hex": submission.content_hash_hex}


# ---------------------------------------------------------------------
# Cross-references within one submission's proposal set (task_45)
# ---------------------------------------------------------------------

# Where a proposal may name another proposal by display string. The
# field list is a wire convention, not a predicate list: any predicate
# whose claim carries these fields participates. `display_name` and
# `title` are what a proposal is *called*; the rest are where it is
# *referenced*.
_NAME_FIELDS: Tuple[str, ...] = ("display_name", "title")
_SCALAR_REF_FIELDS: Tuple[str, ...] = ("organization", "person", "target", "other")
_OBJECT_LIST_REF_FIELDS: Tuple[str, ...] = ("mentions", "participants")


def normalize_display(value: Any) -> str:
    """Case- and whitespace-insensitive key for display matching."""
    return re.sub(r"\s+", " ", str(value)).strip().casefold()


def _proposal_names(proposal: Dict[str, Any]) -> List[str]:
    claim = proposal.get("claim") or {}
    names: List[str] = []
    for f in _NAME_FIELDS:
        v = claim.get(f)
        if isinstance(v, str) and v.strip():
            names.append(v)
    aliases = claim.get("aliases")
    if isinstance(aliases, list):
        names.extend(str(a) for a in aliases if str(a).strip())
    return names


def assign_local_refs(proposals: List[Dict[str, Any]]) -> None:
    """Give every proposal a ``local_ref`` unique within the set. Refs
    the engine or model already supplied are kept when unique; the
    rest are derived from the predicate's last dotted segment plus a
    counter (``org-1``, ``person-2``, ``article-1``)."""
    seen: set = set()
    for p in proposals:
        ref = p.get("local_ref")
        if isinstance(ref, str) and ref.strip() and ref not in seen:
            p["local_ref"] = ref.strip()
            seen.add(p["local_ref"])
        else:
            p.pop("local_ref", None)
    counters: Dict[str, int] = {}
    for p in proposals:
        if p.get("local_ref"):
            continue
        stem = str(p.get("predicate", "item")).split(".")[-1].replace("_", "-") or "item"
        while True:
            counters[stem] = counters.get(stem, 0) + 1
            candidate = f"{stem}-{counters[stem]}"
            if candidate not in seen:
                break
        p["local_ref"] = candidate
        seen.add(candidate)


def bind_refs(proposals: List[Dict[str, Any]]) -> None:
    """Record, on each proposal, which other proposals in the same set
    its display references resolve to::

        "refs": [{"field": "organization", "local_ref": "org-1"},
                 {"field": "mentions[2].entity", "local_ref": "person-1"}]

    Matching is by display string (case and whitespace insensitive)
    against the other proposals' names and aliases. A display that
    matches nothing gets no entry; the daemon resolves those against
    the substrate. A proposal never references itself. Requires
    ``assign_local_refs`` to have run.
    """
    index: Dict[str, str] = {}
    for p in proposals:
        ref = p.get("local_ref")
        if not ref:
            continue
        for name in _proposal_names(p):
            index.setdefault(normalize_display(name), ref)
    for p in proposals:
        me = p.get("local_ref")
        claim = p.get("claim") or {}
        refs: List[Dict[str, str]] = []
        for f in _SCALAR_REF_FIELDS:
            v = claim.get(f)
            if isinstance(v, str) and v.strip():
                target = index.get(normalize_display(v))
                if target and target != me:
                    refs.append({"field": f, "local_ref": target})
        for f in _OBJECT_LIST_REF_FIELDS:
            items = claim.get(f)
            if not isinstance(items, list):
                continue
            for i, item in enumerate(items):
                if not isinstance(item, dict):
                    continue
                d = item.get("display")
                if isinstance(d, str) and d.strip():
                    target = index.get(normalize_display(d))
                    if target and target != me:
                        refs.append({"field": f"{f}[{i}].entity", "local_ref": target})
        if refs:
            p["refs"] = refs
        else:
            p.pop("refs", None)


def article_url_for_set(submission: Submission, proposals: List[Dict[str, Any]]) -> Optional[str]:
    """The article url a proposal set belongs to: the submission's
    frontmatter ``url`` when present, else the first proposal whose
    claim carries a string ``url``."""
    fm_url = (submission.frontmatter or {}).get("url")
    if isinstance(fm_url, str) and fm_url.strip():
        return fm_url.strip()
    for p in proposals:
        u = (p.get("claim") or {}).get("url")
        if isinstance(u, str) and u.strip():
            return u.strip()
    return None


def attach_source_article(submission: Submission, proposals: List[Dict[str, Any]]) -> Optional[str]:
    """Add the ``source_article`` provenance entry to every proposal in
    the set when an article url is known. Idempotent. Returns the url."""
    url = article_url_for_set(submission, proposals)
    if not url:
        return None
    entry = source_article_provenance(submission, url)
    for p in proposals:
        prov = p.setdefault("provenance", [])
        if not any(e.get("kind") == "source_article" and e.get("uri") == url for e in prov if isinstance(e, dict)):
            prov.append(dict(entry))
    return url


# ---------------------------------------------------------------------
# predicate: frontmatter hint
# ---------------------------------------------------------------------

_URL_RE = re.compile(r"https?://[^\s<>()\[\]\"']+")
_HEADING_RE = re.compile(r"^\s*#{1,6}\s+(.+?)\s*$")


def _first_heading(text: str) -> Optional[str]:
    for line in text.splitlines():
        m = _HEADING_RE.match(line)
        if m:
            return m.group(1).strip()
    return None


def _article_note_claim(submission: Submission, hint: str) -> Dict[str, Any]:
    fm = submission.frontmatter
    body = submission.body_text()
    def _fm_str(key: str) -> Optional[str]:
        v = fm.get(key)
        return v.strip() if isinstance(v, str) and v.strip() else None

    # An unregistered predicate's submission usually names its entity
    # (`name:` for an org, `display_name:` for a person); keep that as
    # the note title so the file is findable before task_38 lands.
    title = (
        _fm_str("title")
        or _fm_str("display_name")
        or _fm_str("name")
        or _first_heading(body)
        or submission.filename
        or "untitled"
    )
    parts: List[str] = []
    summary = fm.get("summary")
    if isinstance(summary, str) and summary.strip():
        parts.append(summary.strip())
    if body:
        parts.append(body)
    references: List[str] = []
    url = fm.get("url")
    if isinstance(url, str) and url.strip():
        references.append(url.strip().rstrip(".,;"))
    for m in _URL_RE.finditer(body):
        u = m.group(0).rstrip(".,;")
        if u not in references:
            references.append(u)
    tags: List[str] = []
    tags_raw = fm.get("tags")
    if isinstance(tags_raw, list):
        tags = [str(t).strip() for t in tags_raw if str(t).strip()]
    elif isinstance(tags_raw, str) and tags_raw.strip():
        # The minimal frontmatter parser hands `tags: [a, b]` through as
        # a string; strip the YAML flow-list brackets before splitting.
        cleaned = tags_raw.strip().strip("[]")
        tags = [t.strip().lstrip("#") for t in re.split(r"[,\s]+", cleaned) if t.strip()]
    # Any unregistered hint is tagged uniformly (dots to dashes), so a
    # `predicate: source.article` drop is findable as `source-article`
    # and a `predicate: widget.thing` drop as `widget-thing`. No
    # predicate or source is special-cased here.
    hint_tag = hint.replace(".", "-") if hint else ""
    if hint_tag and hint_tag not in tags:
        tags.append(hint_tag)
    claim: Dict[str, Any] = {"title": str(title), "body": "\n\n".join(parts)}
    if references:
        claim["references"] = references
    if tags:
        claim["tags"] = tags
    author = fm.get("byline") or fm.get("author") or fm.get("publication")
    if isinstance(author, str) and author.strip():
        claim["author"] = author.strip()
    return claim



def _claim_from_frontmatter(submission: Submission, predicate: str, registry: Any) -> Optional[Dict[str, Any]]:
    """Schema-driven claim from frontmatter for a hinted, registered
    predicate. Copies only keys the claim_schema declares as properties,
    requires every schema-required key to be present, and drops the body
    text into the first declared string property named ``description``,
    ``body``, or ``summary`` when that key is absent. No predicate is
    named here; the schema is the contract. Returns None when the
    frontmatter cannot satisfy the schema.
    """
    try:
        schema = registry.schema(predicate)
    except Exception:  # noqa: BLE001
        return None
    if not isinstance(schema, dict):
        return None
    props = schema.get("properties")
    if not isinstance(props, dict) or not props:
        return None
    fm = submission.frontmatter or {}
    claim: Dict[str, Any] = {}
    for key, spec in props.items():
        if key == "predicate" or key not in fm:
            continue
        value = fm[key]
        expected = spec.get("type") if isinstance(spec, dict) else None
        if expected == "array":
            if isinstance(value, list):
                claim[key] = [str(v).strip() for v in value if str(v).strip()]
            elif isinstance(value, str) and value.strip():
                cleaned = value.strip().strip("[]")
                claim[key] = [t.strip() for t in re.split(r"[,]+", cleaned) if t.strip()]
        elif expected in (None, "string"):
            claim[key] = str(value).strip()
        else:
            claim[key] = value
    for required in schema.get("required", []):
        if required not in claim:
            return None
    body = submission.body_text().strip() if hasattr(submission, "body_text") else ""
    if body:
        for candidate in ("description", "body", "summary"):
            spec = props.get(candidate)
            if isinstance(spec, dict) and spec.get("type") == "string" and candidate not in claim:
                claim[candidate] = body
                break
    from validate import validate_claim  # local import keeps module load order simple

    if validate_claim(claim, schema) is not None:
        return None
    return claim

def apply_hint(
    submission: Submission,
    result: EngineResult,
    registry: Any,
    engine_name: str,
    model: str,
) -> EngineResult:
    """Honor a ``predicate:`` frontmatter hint. Never raises."""
    hint = submission.predicate_hint
    if not hint:
        return result
    try:
        registered = bool(registry.has(hint))
    except Exception as e:  # noqa: BLE001 - a broken registry must not sink the submission
        log("warn", f"registry lookup failed for hint {hint!r}: {e}")
        registered = False
    if registered:
        kept = [p for p in result.proposals if p.get("predicate") == hint]
        if kept:
            return EngineResult(proposals=kept, warnings=list(result.warnings))
        built = _claim_from_frontmatter(submission, hint, registry)
        if built is not None:
            url = built.get("url") if isinstance(built.get("url"), str) else None
            proposal = make_proposal(
                hint,
                built,
                submission,
                f"predicate hint {hint!r}: frontmatter supplied the schema's required fields; "
                "claim built from the frontmatter keys the schema declares",
                engine_name,
                model,
                local_ref="hint",
                extra_provenance=[source_article_provenance(submission, url)] if url else None,
            )
            return EngineResult(proposals=[proposal], warnings=list(result.warnings))
        return EngineResult(
            proposals=list(result.proposals),
            warnings=list(result.warnings)
            + [f"predicate hint {hint!r} produced no {hint} proposal; kept the engine's output"],
        )
    try:
        claim = _article_note_claim(submission, hint)
    except Exception as e:  # noqa: BLE001
        claim = {"title": submission.filename or "untitled", "body": submission.content_text}
        log("warn", f"article note fallback degraded: {e}")
    reason = f"{hint} is not a registered predicate"
    proposal = make_proposal(
        "note",
        claim,
        submission,
        f"predicate hint {hint!r} fell back to note ({reason}); title, url, and summary preserved",
        engine_name,
        model,
    )
    return EngineResult(proposals=[proposal], warnings=list(result.warnings))
