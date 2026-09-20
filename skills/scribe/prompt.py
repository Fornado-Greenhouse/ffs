"""Prompt builder for the scribe's ``llm`` engine (task_36, ADR-026).

The extraction prompt is rendered from the registered predicate specs,
never hand-written per predicate: each predicate contributes its name,
description, and ``claim_schema``. Registering a new predicate spec
therefore extends extraction with no scribe code change. The shape
below is the one spike task_42 scored (zero invalid proposals on
Sonnet 5), tightened to the seam interface.

Stdlib only (ADR-009).
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Tuple

from engine import Submission
from registry import PredicateRegistry

ENVELOPE_CONTRACT = (
    '{"proposals": [{"predicate": "<name>", "claim": {...}, '
    '"rationale": "<why, with inferences marked>", '
    '"local_ref": "<token unique in this output, e.g. org-1, person-2, event-1, article-1>", '
    '"valid_from": "<YYYY-MM-DD, optional>", "valid_to": "<YYYY-MM-DD, optional>", '
    '"ends_role": <true only when the proposal ends an existing role, optional>}], '
    '"summary": "<one paragraph>"}'
)

RULES: Tuple[str, ...] = (
    "State only what the text states. Never invent names, dates, amounts, roles, or contact details.",
    "Emit no field the text does not support; omit unknown fields rather than guessing.",
    "If you infer something rather than read it, say so in rationale, not in the claim.",
    "One proposal per distinct entity or event; do not split one person or organization across proposals.",
    "Use display names and titles exactly as printed in the text.",
    "Attribution to other publications in parentheses names a source, not a subject.",
    "Give every proposal a local_ref token unique within this output.",
    "When one proposal refers to another (a person's organization, an item in an object-valued list such as mentions or participants), write the other entity's display name exactly as printed. Never invent or guess identifiers: never emit an entity key.",
    "Object-valued list items carry the printed display name plus their own descriptive fields (context, role) and nothing else.",
    "Dates are ISO YYYY-MM-DD. Put a claim's start date in valid_from when the text states one.",
    "A role ending (stepped down, departs, leaves, retires) is a proposal with ends_role true and valid_to set to the stated or published date; never describe an ending as a new role.",
    "Output exactly one JSON object matching the envelope, with no prose and no markdown fences.",
)


def _role_predicates(registry: PredicateRegistry) -> List[str]:
    """Predicates whose schema requires both a ``person`` and an
    ``organization``: a role held by a person at an organization. Found
    from the registry, never named in code."""
    out: List[str] = []
    for name in registry.names():
        schema = registry.schema(name) or {}
        required = set(schema.get("required") or [])
        if {"person", "organization"} <= required:
            out.append(name)
    return out


def role_guidance(registry: PredicateRegistry) -> List[str]:
    """Extra rules rendered only when a role predicate is registered."""
    lines: List[str] = []
    for name in _role_predicates(registry):
        schema = registry.schema(name) or {}
        props = schema.get("properties") or {}
        kind = props.get("kind") if isinstance(props, dict) else None
        enum = kind.get("enum") if isinstance(kind, dict) else None
        enum_text = f" with kind one of {json.dumps(enum)}" if isinstance(enum, list) else ""
        lines.append(
            f"When the text states or implies that a person holds a role at an organization, also emit one {name} "
            f"proposal{enum_text}: person and organization are the display names as printed, title is the role as "
            "printed, valid_from is the stated start date if any (else omit it), and source is the document url."
        )
    return lines


def render_predicate(registry: PredicateRegistry, name: str) -> str:
    """One predicate block: name, description, compact claim_schema JSON."""
    description = registry.description(name).strip() or "(no description)"
    schema = registry.schema(name)
    schema_json = json.dumps(schema, separators=(",", ":"), sort_keys=True)
    return f"### {name}\n{description}\nclaim_schema: {schema_json}"


def build_system(registry: PredicateRegistry, hint: str | None = None) -> str:
    parts: List[str] = [
        "You are the scribe for a personal records substrate. "
        "Extract structured proposals from ONE submitted document.",
        "Rules:",
    ]
    parts.extend(f"- {r}" for r in RULES)
    parts.extend(f"- {r}" for r in role_guidance(registry))
    parts.append("Registered predicates (name, description, JSON claim_schema):")
    for name in registry.names():
        parts.append(render_predicate(registry, name))
    parts.append("Envelope (output exactly this shape): " + ENVELOPE_CONTRACT)
    if hint and registry.has(hint):
        parts.append(f"Target predicate: {hint}")
    return "\n\n".join(parts)


def build_user(submission: Submission) -> str:
    meta: Dict[str, Any] = {
        "source_uri": submission.source_uri,
        "filename": submission.filename or "",
    }
    header = "Document metadata: " + json.dumps(meta, separators=(",", ":"))
    return header + "\n\nDocument text:\n<<<\n" + submission.content_text + "\n>>>"


def build_prompt(registry: PredicateRegistry, submission: Submission) -> Tuple[str, str]:
    """Return ``(system, user)`` for the backend call."""
    return build_system(registry, submission.predicate_hint), build_user(submission)
