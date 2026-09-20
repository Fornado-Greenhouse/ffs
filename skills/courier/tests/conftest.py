"""Shared pytest setup for courier tests: sys.path for the bundle, its
adapters package, and `skills/_lib`; an isolated FFS_DATA_DIR."""

import os
import sys

import pytest

_HERE = os.path.dirname(os.path.abspath(__file__))
_COURIER = os.path.abspath(os.path.join(_HERE, os.pardir))
_LIB = os.path.abspath(os.path.join(_COURIER, os.pardir, "_lib"))
for path in (_COURIER, _LIB):
    if path not in sys.path:
        sys.path.insert(0, path)

FIXTURES = os.path.join(_HERE, "fixtures")
STARTER = os.path.abspath(os.path.join(_COURIER, os.pardir, os.pardir, "starter", "config"))


@pytest.fixture(autouse=True)
def _isolated_env(tmp_path, monkeypatch):
    monkeypatch.setenv("FFS_DATA_DIR", str(tmp_path))
    for var in ("FFS_COURIER_DRY_RUN", "FFS_COURIER_MAIL_PASSWORD", "FFS_COURIER_MAIL_HOST"):
        monkeypatch.delenv(var, raising=False)
    (tmp_path / "config").mkdir()
    (tmp_path / "ingest").mkdir()
    return tmp_path


@pytest.fixture
def fixtures_dir():
    return FIXTURES


@pytest.fixture
def starter_dir():
    return STARTER
