import ffs_skill


def test_keychain_secret_env_override_wins(monkeypatch):
    monkeypatch.setenv("FFS_KEYCHAIN_FFS_COURIER_IMAP_EXAMPLE_COM", "s3cret")
    assert ffs_skill.keychain_secret("ffs.courier.imap.example.com") == "s3cret"


def test_keychain_secret_missing_never_raises(monkeypatch):
    monkeypatch.delenv("FFS_KEYCHAIN_FFS_COURIER_NOPE", raising=False)
    monkeypatch.setattr(ffs_skill, "__name__", "ffs_skill")
    # On macOS this shells out to `security` and finds nothing; elsewhere it returns None directly.
    assert ffs_skill.keychain_secret("ffs.courier.nope") is None
