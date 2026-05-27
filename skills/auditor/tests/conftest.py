"""Shared pytest setup for auditor tests.

Adds the auditor directory and `skills/_lib` to `sys.path` so the
`audit` and `ffs_skill` modules import cleanly when the tests
run from any working directory.
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_AUDITOR = os.path.abspath(os.path.join(_HERE, os.pardir))
_FFS_LIB = os.path.abspath(os.path.join(_AUDITOR, os.pardir, "_lib"))

for path in (_AUDITOR, _FFS_LIB):
    if path not in sys.path:
        sys.path.insert(0, path)
