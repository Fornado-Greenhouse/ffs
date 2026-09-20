"""Shared pytest setup for scribe tests.

Adds the scribe directory and `skills/_lib` to `sys.path` so the
`extraction` and `ffs_skill` modules import cleanly when the tests
run from any working directory.

Also pins `FFS_DATA_DIR` to an empty temporary directory for every
test (autouse) so the predicate registry never reads the developer's
real `~/.ffs/config/predicates/`; tests that want on-disk specs write
their own TOMLs under the fixture's `config/predicates/`.
"""

import os
import sys

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
_SCRIBE = os.path.abspath(os.path.join(_HERE, os.pardir))
_LIB = os.path.abspath(os.path.join(_SCRIBE, os.pardir, "_lib"))

for path in (_SCRIBE, _LIB):
    if path not in sys.path:
        sys.path.insert(0, path)


@pytest.fixture(autouse=True)
def _isolated_data_dir(tmp_path, monkeypatch):
    """Point FFS_DATA_DIR at an empty scratch dir and clear the scribe
    engine env vars so each test starts from the heuristic default."""
    monkeypatch.setenv("FFS_DATA_DIR", str(tmp_path))
    for var in ("FFS_SCRIBE_ENGINE", "FFS_SCRIBE_LLM_URL", "FFS_SCRIBE_LLM_MODEL"):
        monkeypatch.delenv(var, raising=False)
    return tmp_path
