"""Minimal JSON-Schema subset validator for scribe proposals.

Zero dependencies (ADR-009): the external ``jsonschema`` package is
deliberately not used. The subset covers what the predicate specs in
``starter/predicates/*.toml`` actually express today plus the small
extensions task_38's business-graph shapes need:

- ``type: object`` with ``required`` and ``properties``;
- primitive ``type`` on properties (string, integer, number, boolean,
  array, object);
- ``enum`` on string properties;
- ``items.type`` for arrays, one level deep (arrays of strings or of
  objects; object items may carry ``required`` and ``properties``);
- nested ``type: object`` properties with ``required``, one level deep.

Returns ``None`` when the claim passes, else a short error message.
Unknown keys in the claim are tolerated (the spec's ``claim_schema``
is a contract on what is known, not a whitelist), matching the
pre-task_36 behavior the daemon relies on.
"""

from __future__ import annotations

from typing import Any, Dict, Optional


def _check_type(value: Any, expected: Optional[str], path: str) -> Optional[str]:
    if expected == "string" and not isinstance(value, str):
        return f"field {path} must be a string"
    if expected == "array" and not isinstance(value, list):
        return f"field {path} must be an array"
    if expected == "integer" and (not isinstance(value, int) or isinstance(value, bool)):
        return f"field {path} must be an integer"
    if expected == "number" and (not isinstance(value, (int, float)) or isinstance(value, bool)):
        return f"field {path} must be a number"
    if expected == "boolean" and not isinstance(value, bool):
        return f"field {path} must be a boolean"
    if expected == "object" and not isinstance(value, dict):
        return f"field {path} must be an object"
    return None


def _check_object(claim: Dict[str, Any], schema: Dict[str, Any], path: str, depth: int) -> Optional[str]:
    for k in schema.get("required", []) or []:
        if k not in claim:
            where = f" in {path}" if path else ""
            return f"missing required field: {k}{where}"
    props = schema.get("properties", {}) or {}
    for k, v in claim.items():
        spec = props.get(k)
        if not isinstance(spec, dict):
            continue
        sub_path = f"{path}.{k}" if path else k
        expected = spec.get("type")
        err = _check_type(v, expected, sub_path)
        if err:
            return err
        enum = spec.get("enum")
        if isinstance(enum, list) and enum and v not in enum:
            return f"field {sub_path} is not one of {enum}"
        if expected == "array":
            items = spec.get("items")
            if isinstance(items, dict) and items.get("type"):
                for i, item in enumerate(v):
                    item_path = f"{sub_path}[{i}]"
                    err = _check_type(item, items.get("type"), item_path)
                    if err:
                        return err
                    if items.get("type") == "object" and depth < 1:
                        err = _check_object(item, items, item_path, depth + 1)
                        if err:
                            return err
        elif expected == "object" and depth < 1:
            err = _check_object(v, spec, sub_path, depth + 1)
            if err:
                return err
    return None


def validate_claim(claim: Any, schema: Any) -> Optional[str]:
    """Validate ``claim`` against ``schema``; ``None`` on pass, else a
    message. A non-dict schema (missing, ``None``) always passes so a
    host that cannot supply a schema never blocks a proposal."""
    if not isinstance(schema, dict):
        return None
    if schema.get("type") == "object" and not isinstance(claim, dict):
        return "claim must be an object"
    if not isinstance(claim, dict):
        return None
    return _check_object(claim, schema, "", 0)
