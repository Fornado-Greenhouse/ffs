"""Shared pytest setup for scribe tests.

Adds the scribe directory and `skills/_lib` to `sys.path` so the
`extraction` and `ffs_skill` modules import cleanly when the tests
run from any working directory.
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_SCRIBE = os.path.abspath(os.path.join(_HERE, os.pardir))
_LIB = os.path.abspath(os.path.join(_SCRIBE, os.pardir, "_lib"))

for path in (_SCRIBE, _LIB):
    if path not in sys.path:
        sys.path.insert(0, path)
