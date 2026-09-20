"""Shared pytest setup for the `_lib` helper tests."""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIB = os.path.abspath(os.path.join(_HERE, os.pardir))
if _LIB not in sys.path:
    sys.path.insert(0, _LIB)
