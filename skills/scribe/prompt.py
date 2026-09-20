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
    '"rationale": "<why, with inferences marked>"}], '
    '"summary": "<one paragraph>"}'
)

RULES: Tuple[str, ...] = (
    "State only what the text states. Never invent names, dates, amounts, roles, or contact details.",
    "Emit no field the text does not support; omit unknown fields rather than guessing.",
    "If you infer something rather than read it, say so in rationale, not in the claim.",
    "One proposal per distinct entity or event; do not split one person or organization across proposals.",
    "Use display names and titles exactly as printed in the text.",
    "Attribution to other publications in parentheses names a source, not a subject.",
    "Output exactly one JSON object matching the envelope, with no prose and no markdown fences.",
)


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
