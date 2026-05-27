"""Shared pytest setup for librarian tests.

Adds the librarian directory and `skills/_lib` to `sys.path` so the
`watcher` and `ffs_skill` modules import cleanly when the tests
run from any working directory.
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB_DIR = os.path.abspath(os.path.join(_HERE, os.pardir))
_FFS_LIB = os.path.abspath(os.path.join(_LIB_DIR, os.pardir, "_lib"))

for path in (_LIB_DIR, _FFS_LIB):
    if path not in sys.path:
        sys.path.insert(0, path)
