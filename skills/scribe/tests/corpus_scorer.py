"""Golden-corpus loader and scorer for the scribe (task_36).

A fixture is a directory holding ``input.md`` (the note as it would
be dropped into ``ingest/``) and ``expected.json``::

    {
      "filename": "Jon Jones.md",
      "proposals": [
        {"predicate": "contact.person",
         "claim": {"display_name": "Jon Jones", "phone": "919-428-4074"},
         "soft": {"role": "UFC fighter"}}
      ],
      "forbid": ["person.generic"],
      "required": true,
      "awaits": "hygiene fixes 36.8",
      "expected_when_registered": [...],
      "notes": "why this fixture exists"
    }

Only the fields listed under ``claim`` are scored. Strings match
after whitespace normalization and case folding; arrays are treated
as sets, and every expected element must be present; numbers must be
equal; a value of ``{"$contains": "text"}`` matches when the actual
string contains the text. ``soft`` fields are reported but never
scored. ``forbid`` lists predicates that must not appear at all.
``expected_when_registered`` is ignored by the scorer; it documents
how the expectation flips once a later task registers a predicate.

The corpus directory is ``$FFS_SCRIBE_CORPUS_DIR`` when set (so real
articles can be scored locally without entering git) and the in-repo
``tests/corpus/`` otherwise.

Stdlib only, per ADR-009.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
import tomllib
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Tuple

_HERE = os.path.dirname(os.path.abspath(__file__))
_SCRIBE = os.path.abspath(os.path.join(_HERE, os.pardir))
_REPO = os.path.abspath(os.path.join(_SCRIBE, os.pardir, os.pardir))
IN_REPO_CORPUS = os.path.join(_HERE, "corpus")
STARTER_PREDICATES = os.path.join(_REPO, "starter", "predicates")


# --------------------------------------------------------------------
# Fixtures
# --------------------------------------------------------------------


@dataclass
class Fixture:
    id: str
    path: str
    filename: str
    input: str
    expected: List[Dict[str, Any]]
    forbid: List[str] = field(default_factory=list)
    required: bool = False
    awaits: Optional[str] = None
    notes: str = ""
    expected_when_registered: Optional[List[Dict[str, Any]]] = None

    @property
    def source_uri(self) -> str:
        return f"file:///ingest/{self.filename}"


def corpus_dir() -> str:
    """The corpus directory in effect: the env override or the repo."""
    override = os.environ.get("FFS_SCRIBE_CORPUS_DIR")
    if override:
        return os.path.abspath(os.path.expanduser(override))
    return IN_REPO_CORPUS


def load_corpus(directory: Optional[str] = None) -> List[Fixture]:
    """Load every fixture directory under ``directory`` (default:
    :func:`corpus_dir`). A directory without ``expected.json`` or
    ``input.md`` is skipped with a warning on stderr.
    """
    root = directory or corpus_dir()
    fixtures: List[Fixture] = []
    if not os.path.isdir(root):
        print(f"corpus: directory not found: {root}", file=sys.stderr)
        return fixtures
    for name in sorted(os.listdir(root)):
        path = os.path.join(root, name)
        if not os.path.isdir(path):
            continue
        exp_path = os.path.join(path, "expected.json")
        in_path = os.path.join(path, "input.md")
        if not (os.path.isfile(exp_path) and os.path.isfile(in_path)):
            print(f"corpus: skipping {name}: missing expected.json or input.md", file=sys.stderr)
            continue
        with open(exp_path, encoding="utf-8") as f:
            exp = json.load(f)
        with open(in_path, encoding="utf-8") as f:
            inp = f.read()
        fixtures.append(
            Fixture(
                id=name,
                path=path,
                filename=str(exp.get("filename") or f"{name}.md"),
                input=inp,
                expected=list(exp.get("proposals") or []),
                forbid=list(exp.get("forbid") or []),
                required=bool(exp.get("required", False)),
                awaits=exp.get("awaits"),
                notes=str(exp.get("notes") or ""),
                expected_when_registered=exp.get("expected_when_registered"),
            )
        )
    return fixtures


# --------------------------------------------------------------------
# Running an engine
# --------------------------------------------------------------------


def run_engine(handle_fn: Callable[[Any], Dict[str, Any]], fixture: Fixture) -> List[Dict[str, Any]]:
    """Invoke the scribe's ``handle`` on a fixture and return its
    proposals. ``source_uri`` carries the fixture's filename so the
    filename-as-name-source hygiene fix has something to read.
    """
    result = handle_fn({"source_uri": fixture.source_uri, "content": fixture.input})
    if not isinstance(result, dict):
        return []
    proposals = result.get("proposals") or []
    return [p for p in proposals if isinstance(p, dict)]


# --------------------------------------------------------------------
# Schema sources for tests: starter TOMLs on disk, or a query stub
# --------------------------------------------------------------------


def load_starter_schemas() -> Dict[str, Dict[str, Any]]:
    """Parse ``starter/predicates/*.toml`` with tomllib and return
    ``{predicate_name: claim_schema}``.
    """
    out: Dict[str, Dict[str, Any]] = {}
    if not os.path.isdir(STARTER_PREDICATES):
        return out
    for name in sorted(os.listdir(STARTER_PREDICATES)):
        if not name.endswith(".toml"):
            continue
        with open(os.path.join(STARTER_PREDICATES, name), "rb") as f:
            spec = tomllib.load(f)
        pname = spec.get("name")
        schema = spec.get("claim_schema")
        if isinstance(pname, str) and isinstance(schema, dict):
            out[pname] = schema
    return out


def starter_registry_env(tmp_path: Any, monkeypatch: Any) -> str:
    """Copy the starter predicate TOMLs into ``<tmp>/config/predicates``
    and point ``FFS_DATA_DIR`` at ``<tmp>`` so a TOML-backed registry
    loads the three MVP predicates. Returns the data dir.
    """
    data_dir = os.path.join(str(tmp_path), "ffs-data")
    pred_dir = os.path.join(data_dir, "config", "predicates")
    os.makedirs(pred_dir, exist_ok=True)
    for name in os.listdir(STARTER_PREDICATES):
        if name.endswith(".toml"):
            shutil.copy(os.path.join(STARTER_PREDICATES, name), pred_dir)
    monkeypatch.setenv("FFS_DATA_DIR", data_dir)
    return data_dir


def install_query_stub(monkeypatch: Any, schemas: Optional[Dict[str, Dict[str, Any]]] = None) -> None:
    """Replace ``query`` in every scribe module that imported it so
    ``predicate.inspect`` is served from the starter TOML schemas
    without a host. Unknown predicates raise ``FfsSkillError`` the way
    the host would, which is what drives the note fallback.
    """
    schemas = schemas if schemas is not None else load_starter_schemas()
    import ffs_skill  # type: ignore  # conftest put _lib on sys.path

    def fake_query(method: str, params: Any) -> Any:
        if method != "predicate.inspect":
            raise ffs_skill.FfsSkillError(f"stub: unsupported method {method}")
        name = params.get("name") if isinstance(params, dict) else None
        if name not in schemas:
            raise ffs_skill.FfsSkillError(f"predicate not registered: {name}")
        return {"name": name, "claim_schema": schemas[name]}

    monkeypatch.setattr(ffs_skill, "query", fake_query)
    for mod in list(sys.modules.values()):
        f = getattr(mod, "__file__", None) or ""
        if f and os.path.abspath(f).startswith(_SCRIBE + os.sep) and hasattr(mod, "query"):
            monkeypatch.setattr(mod, "query", fake_query)


# --------------------------------------------------------------------
# Scoring
# --------------------------------------------------------------------


def _norm(s: str) -> str:
    return re.sub(r"\s+", " ", s).strip().casefold()


def field_matches(expected: Any, actual: Any) -> bool:
    """One expected field value against the actual value."""
    if isinstance(expected, dict) and "$contains" in expected:
        return isinstance(actual, str) and _norm(str(expected["$contains"])) in _norm(actual)
    if isinstance(expected, str):
        return isinstance(actual, str) and _norm(expected) == _norm(actual)
    if isinstance(expected, list):
        if not isinstance(actual, list):
            return False
        have = {_norm(str(a)) for a in actual}
        return all(_norm(str(e)) in have for e in expected)
    if isinstance(expected, bool):
        return isinstance(actual, bool) and expected == actual
    if isinstance(expected, (int, float)):
        return isinstance(actual, (int, float)) and not isinstance(actual, bool) and expected == actual
    return expected == actual


@dataclass
class ProposalMatch:
    predicate: str
    expected_fields: int
    matched_fields: int
    misses: List[str]
    soft_hits: List[str]
    soft_misses: List[str]
    matched_index: Optional[int]

    @property
    def complete(self) -> bool:
        return self.expected_fields > 0 and self.matched_fields == self.expected_fields


@dataclass
class Score:
    fixture_id: str
    matches: List[ProposalMatch]
    extras: List[str]
    forbidden_hits: List[str]

    @property
    def recall(self) -> float:
        exp = sum(m.expected_fields for m in self.matches)
        return 1.0 if exp == 0 else sum(m.matched_fields for m in self.matches) / exp

    @property
    def all_matched(self) -> bool:
        return all(m.complete for m in self.matches) and not self.forbidden_hits


def score(expected: List[Dict[str, Any]], actual: List[Dict[str, Any]], forbid: Optional[List[str]] = None,
          fixture_id: str = "") -> Score:
    """Match each expected proposal to the best actual proposal of the
    same predicate (most matched fields, each actual used once) and
    count field-level hits and misses.
    """
    used: set = set()
    matches: List[ProposalMatch] = []
    for exp in expected:
        pred = str(exp.get("predicate"))
        claim = exp.get("claim") or {}
        soft = exp.get("soft") or {}
        best: Tuple[int, Optional[int], List[str]] = (-1, None, [])
        for i, act in enumerate(actual):
            if i in used or act.get("predicate") != pred:
                continue
            aclaim = act.get("claim") or {}
            misses = [k for k, v in claim.items() if not field_matches(v, aclaim.get(k))]
            hits = len(claim) - len(misses)
            if hits > best[0]:
                best = (hits, i, misses)
        hits, idx, misses = best
        if idx is None:
            matches.append(ProposalMatch(pred, len(claim), 0, list(claim.keys()), [], list(soft.keys()), None))
            continue
        used.add(idx)
        aclaim = actual[idx].get("claim") or {}
        soft_hits = [k for k, v in soft.items() if field_matches(v, aclaim.get(k))]
        soft_misses = [k for k in soft if k not in soft_hits]
        matches.append(ProposalMatch(pred, len(claim), hits, misses, soft_hits, soft_misses, idx))
    extras = [str(a.get("predicate")) for i, a in enumerate(actual) if i not in used]
    forbidden = [str(a.get("predicate")) for a in actual if a.get("predicate") in set(forbid or [])]
    return Score(fixture_id, matches, extras, forbidden)


def summarize(scores: List[Score]) -> Dict[str, Dict[str, float]]:
    """Per-predicate precision and recall.

    recall = matched expected fields / expected fields.
    precision = complete expected proposals / (complete + extra
    proposals of that predicate); extras are actual proposals no
    expectation claimed, counted as one false positive each.
    """
    acc: Dict[str, Dict[str, float]] = {}

    def slot(p: str) -> Dict[str, float]:
        return acc.setdefault(p, {"expected_fields": 0, "matched_fields": 0, "complete": 0, "extras": 0, "fixtures": 0})

    for s in scores:
        seen: set = set()
        for m in s.matches:
            d = slot(m.predicate)
            d["expected_fields"] += m.expected_fields
            d["matched_fields"] += m.matched_fields
            d["complete"] += 1 if m.complete else 0
            seen.add(m.predicate)
        for e in s.extras:
            slot(e)["extras"] += 1
            seen.add(e)
        for p in seen:
            slot(p)["fixtures"] += 1
    out: Dict[str, Dict[str, float]] = {}
    for p, d in acc.items():
        recall = 1.0 if d["expected_fields"] == 0 else d["matched_fields"] / d["expected_fields"]
        denom = d["complete"] + d["extras"]
        precision = 1.0 if denom == 0 else d["complete"] / denom
        out[p] = {**d, "recall": round(recall, 3), "precision": round(precision, 3)}
    return out


def format_table(summary: Dict[str, Dict[str, float]]) -> str:
    rows = ["| predicate | fixtures | precision | recall | matched/expected fields | extras |", "|---|---|---|---|---|---|"]
    for p in sorted(summary):
        d = summary[p]
        rows.append(
            f"| {p} | {int(d['fixtures'])} | {d['precision']:.2f} | {d['recall']:.2f} | "
            f"{int(d['matched_fields'])}/{int(d['expected_fields'])} | {int(d['extras'])} |"
        )
    return "\n".join(rows)


def score_corpus(handle_fn: Callable[[Any], Dict[str, Any]], fixtures: List[Fixture]) -> List[Score]:
    return [score(f.expected, run_engine(handle_fn, f), f.forbid, f.id) for f in fixtures]


if __name__ == "__main__":
    # Out-of-band scoring: honors FFS_SCRIBE_ENGINE and FFS_SCRIBE_CORPUS_DIR.
    # Requires a running host for predicate.inspect unless the engine reads
    # TOML specs from $FFS_DATA_DIR/config/predicates; see task_36.
    sys.path.insert(0, _SCRIBE)
    sys.path.insert(0, os.path.join(_REPO, "skills", "_lib"))
    import extraction  # type: ignore

    fx = load_corpus()
    results = score_corpus(extraction.handle, fx)
    for r in results:
        flag = "ok " if r.all_matched else "MISS"
        print(f"{flag} {r.fixture_id}: recall={r.recall:.2f} extras={r.extras} forbidden={r.forbidden_hits}")
    print()
    print(format_table(summarize(results)))
