"""On-disk predicate registry for the scribe (task_36 § 36.2).

Reads every ``*.toml`` under ``$FFS_DATA_DIR/config/predicates/`` with
stdlib ``tomllib`` (Python 3.11+). The registry is the single source
of truth the prompt builder renders from and the validator re-reads,
so registering a new predicate spec extends extraction with zero
scribe code changes (ADR-026).

When no specs are on disk (an older install, or a test with an empty
data dir) the registry falls back to the host's ``predicate.inspect``
query through the optional ``fallback`` callable, which keeps the
pre-task_36 daemon-hosted behavior working unchanged.
"""

from __future__ import annotations

import os
from typing import Any, Callable, Dict, List, Optional

try:  # Python 3.11+
    import tomllib
except ImportError:  # pragma: no cover - the bundle targets 3.11+
    tomllib = None  # type: ignore

SchemaFallback = Callable[[str], Optional[Dict[str, Any]]]


def _leading_comment(text: str) -> str:
    """First paragraph of a TOML file's leading ``#`` comment block,
    with the comment markers stripped. Empty when the file does not
    start with comments."""
    lines: List[str] = []
    for raw in text.splitlines():
        s = raw.strip()
        if s.startswith("#"):
            body = s.lstrip("#").strip()
            if not body and lines:
                break  # blank comment line ends the first paragraph
            if body:
                lines.append(body)
            continue
        if not s and not lines:
            continue
        break
    return " ".join(lines)


class PredicateRegistry:
    def __init__(
        self,
        specs: Optional[Dict[str, Dict[str, Any]]] = None,
        descriptions: Optional[Dict[str, str]] = None,
        fallback: Optional[SchemaFallback] = None,
    ) -> None:
        self._specs: Dict[str, Dict[str, Any]] = dict(specs or {})
        self._descriptions: Dict[str, str] = dict(descriptions or {})
        self._fallback = fallback
        self._cache: Dict[str, Optional[Dict[str, Any]]] = {}
        self.warnings: List[str] = []
        self.source_dir: Optional[str] = None

    # -- construction -------------------------------------------------

    @classmethod
    def from_specs(cls, specs: Dict[str, Dict[str, Any]]) -> "PredicateRegistry":
        return cls(specs=specs)

    @classmethod
    def load(
        cls,
        data_dir: Optional[str] = None,
        fallback: Optional[SchemaFallback] = None,
    ) -> "PredicateRegistry":
        """Load specs from ``<data_dir>/config/predicates/``.

        ``data_dir`` defaults to ``$FFS_DATA_DIR`` then ``~/.ffs``.
        Malformed files or specs without a ``name`` are skipped with a
        warning collected in ``registry.warnings``.
        """
        if data_dir is None:
            data_dir = os.environ.get("FFS_DATA_DIR") or os.path.expanduser("~/.ffs")
        reg = cls(fallback=fallback)
        spec_dir = os.path.join(data_dir, "config", "predicates")
        reg.source_dir = spec_dir
        if tomllib is None or not os.path.isdir(spec_dir):
            return reg
        for fname in sorted(os.listdir(spec_dir)):
            if not fname.endswith(".toml"):
                continue
            path = os.path.join(spec_dir, fname)
            try:
                with open(path, "r", encoding="utf-8") as f:
                    text = f.read()
                spec = tomllib.loads(text)
            except Exception as e:  # noqa: BLE001 - one bad file must not sink the registry
                reg.warnings.append(f"skipped predicate spec {fname}: {type(e).__name__}: {e}")
                continue
            name = spec.get("name") if isinstance(spec, dict) else None
            if not isinstance(name, str) or not name:
                reg.warnings.append(f"skipped predicate spec {fname}: no `name` field")
                continue
            reg._specs[name] = spec
            desc = _leading_comment(text)
            if desc:
                reg._descriptions[name] = desc
        return reg

    # -- queries ------------------------------------------------------

    def names(self) -> List[str]:
        return sorted(self._specs)

    def has(self, name: str) -> bool:
        """Registered on disk, or resolvable through the host fallback
        to a non-empty schema (an unknown predicate resolves to nothing)."""
        if name in self._specs:
            return True
        if self._fallback is None:
            return False
        return bool(self.schema(name))

    def raw(self, name: str) -> Dict[str, Any]:
        return self._specs.get(name, {})

    def description(self, name: str) -> str:
        """A spec's own ``description`` field when present, else the
        first paragraph of its leading comment block, else the name."""
        spec = self._specs.get(name) or {}
        d = spec.get("description")
        if isinstance(d, str) and d.strip():
            return d.strip()
        return self._descriptions.get(name) or name

    def schema(self, name: str) -> Optional[Dict[str, Any]]:
        """The predicate's ``claim_schema``; from disk when registered,
        else via the host fallback (cached per name), else ``None``."""
        spec = self._specs.get(name)
        if spec is not None:
            cs = spec.get("claim_schema")
            return cs if isinstance(cs, dict) else None
        if name in self._cache:
            return self._cache[name]
        result: Optional[Dict[str, Any]] = None
        if self._fallback is not None:
            try:
                result = self._fallback(name)
            except Exception:  # noqa: BLE001 - the fallback must never raise into extraction
                result = None
            if not isinstance(result, dict):
                result = None
        self._cache[name] = result
        return result
