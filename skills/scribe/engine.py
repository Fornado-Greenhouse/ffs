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
) -> Dict[str, Any]:
    return {
        "predicate": predicate,
        "claim": claim,
        "provenance": [
            {
                "kind": "ingest",
                "uri": submission.source_uri,
                "hash_hex": submission.content_hash_hex,
            }
        ],
        "rationale": rationale,
        "engine": engine_name,
        "model": model,
    }


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
            proposal = make_proposal(
                hint,
                built,
                submission,
                f"predicate hint {hint!r}: frontmatter supplied the schema's required fields; "
                "claim built from the frontmatter keys the schema declares",
                engine_name,
                model,
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
