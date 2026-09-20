"""Out-of-band corpus scoring for the ``llm`` engine (task_36).

Not collected by pytest (no ``test_`` prefix; guarded by ``__main__``).
Runs the ``llm`` engine over the golden corpus and prints per-predicate
precision and recall via the shared scorer in ``corpus_scorer.py``.
Exits 0 with a "skipping" message when no backend is reachable, so it
is safe to invoke from any script.

Invocation (from the repo root)::

    # Ollama at the default URL, model from the env or llama3.1:8b
    .venv/bin/python skills/scribe/tests/score_llm.py

    # Anthropic Messages API
    FFS_SCRIBE_LLM_URL=https://api.anthropic.com \\
    FFS_SCRIBE_LLM_MODEL=claude-sonnet-5 \\
    FFS_SCRIBE_ANTHROPIC_KEY=... \\
    .venv/bin/python skills/scribe/tests/score_llm.py

    # A private corpus of real articles that must not enter git
    FFS_SCRIBE_CORPUS_DIR=$HOME/.ffs/spikes/corpus \\
    .venv/bin/python skills/scribe/tests/score_llm.py

Requires the seam modules from the heuristic workstream (``engine``,
``registry``, ``validate``, ``heuristic``) and ``corpus_scorer.py``;
prints a clear message and exits 0 if either is missing.
"""

from __future__ import annotations

import os
import sys
import urllib.error
import urllib.parse
import urllib.request

_HERE = os.path.dirname(os.path.abspath(__file__))
_SCRIBE = os.path.abspath(os.path.join(_HERE, os.pardir))
_LIB = os.path.abspath(os.path.join(_SCRIBE, os.pardir, "_lib"))
for _p in (_SCRIBE, _LIB, _HERE):
    if _p not in sys.path:
        sys.path.insert(0, _p)


def _backend_reachable(env) -> tuple[bool, str]:
    url = env.get("FFS_SCRIBE_LLM_URL") or "http://localhost:11434"
    host = urllib.parse.urlparse(url).hostname or ""
    if host.lower() == "api.anthropic.com":
        if env.get("FFS_SCRIBE_ANTHROPIC_KEY"):
            return True, "anthropic key present"
        return False, "anthropic selected but FFS_SCRIBE_ANTHROPIC_KEY is absent"
    try:
        with urllib.request.urlopen(url.rstrip("/") + "/api/tags", timeout=3):  # noqa: S310
            return True, f"ollama reachable at {url}"
    except (urllib.error.URLError, OSError, TimeoutError) as e:
        return False, f"ollama not reachable at {url}: {e}"


def main() -> int:
    env = os.environ
    ok, why = _backend_reachable(env)
    if not ok:
        print(f"backend unreachable, skipping ({why})")
        return 0
    try:
        import corpus_scorer  # type: ignore  # written by the corpus workstream
    except ImportError:
        print("corpus_scorer.py not found next to this script; nothing to score (exit 0)")
        return 0
    try:
        from llm import LlmEngine
    except ImportError as e:
        print(f"llm engine or its seam modules not importable ({e}); exit 0")
        return 0

    corpus_dir = env.get("FFS_SCRIBE_CORPUS_DIR") or os.path.join(_HERE, "corpus")
    engine = LlmEngine.from_env(env)
    print(f"scoring llm engine ({why}); model={engine.model}; corpus={corpus_dir}")
    report = corpus_scorer.score_engine(engine, corpus_dir)
    print(corpus_scorer.format_report(report) if hasattr(corpus_scorer, "format_report") else report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
