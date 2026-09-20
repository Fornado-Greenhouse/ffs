import os

import pytest

import config
from helpers import COURIER_TOML, SOURCES_TOML, courier_cfg, sources_cfg


def test_sources_toml_loader_validates_enums_rejects_unknown_keys_and_has_no_domain_lists(starter_dir):
    for intake in config.INTAKE_VALUES:
        for fetch in config.FETCH_VALUES:
            s = sources_cfg(f'[[publisher]]\nkey = "k"\nname = "K"\ndomains = ["k.test"]\nintake = "{intake}"\nfetch = "{fetch}"\n')
            assert s.get("k").intake == intake and s.get("k").fetch == fetch
    with pytest.raises(config.ConfigError, match="unknown key"):
        sources_cfg('[[publisher]]\nkey = "k"\nname = "K"\ndomains = ["k.test"]\nblocklist = true\n')
    with pytest.raises(config.ConfigError, match="intake"):
        sources_cfg('[[publisher]]\nkey = "k"\nname = "K"\ndomains = ["k.test"]\nintake = "scrape"\n')
    with pytest.raises(config.ConfigError, match="fetch"):
        sources_cfg('[[publisher]]\nkey = "k"\nname = "K"\ndomains = ["k.test"]\nfetch = "always"\n')
    # starter file loads with the documented defaults
    with open(os.path.join(starter_dir, "sources.toml"), encoding="utf-8") as f:
        starter = sources_cfg(f.read())
    for key in ("bizjournals", "charlotteobserver", "axios"):
        assert starter.get(key).intake == "pointer" and starter.get(key).fetch == "session"
    assert starter.get("clttoday").intake == "clip" and starter.get("clttoday").fetch == "off"
    assert starter.get("meck_permits").fetch == "scheduled"
    assert starter.get("bizjournals").daily_cap == 20 and starter.get("bizjournals").min_pause_seconds == 8
    # no denylist or allowlist anywhere in the bundle's code
    bundle = os.path.abspath(os.path.join(os.path.dirname(__file__), os.pardir))
    hits = []
    for root, _dirs, files in os.walk(bundle):
        if "tests" in root.split(os.sep):
            continue
        for fn in files:
            if fn.endswith(".py"):
                with open(os.path.join(root, fn), encoding="utf-8") as f:
                    text = f.read().lower()
                for needle in ("bizjournals.com", "charlotteobserver.com", "axios.com", "denylist", "blocklist", "allowlist"):
                    if needle in text:
                        hits.append((fn, needle))
    assert hits == [], hits


def test_courier_reads_secret_from_keychain_never_from_toml(monkeypatch):
    with pytest.raises(config.ConfigError, match="secret-looking key"):
        courier_cfg(COURIER_TOML + '\n[mailbox]\npassword = "x"\n'.replace("[mailbox]\n", ""))  # duplicate table is a TOML error; use a nested key instead
    with pytest.raises(config.ConfigError, match="secret-looking key"):
        courier_cfg(COURIER_TOML.replace('user = "owner@example.test"', 'user = "owner@example.test"\napp_password_token = "x"'))
    monkeypatch.setenv("FFS_COURIER_MAIL_PASSWORD", "from-env")
    assert config.mailbox_secret("imap.example.test") == "from-env"
    monkeypatch.delenv("FFS_COURIER_MAIL_PASSWORD")
    monkeypatch.setenv("FFS_KEYCHAIN_FFS_COURIER_IMAP_EXAMPLE_TEST", "from-keychain")
    assert config.mailbox_secret("imap.example.test") == "from-keychain"


def test_courier_toml_parses_sources_feeds_wrappers_and_env_overrides():
    cfg = courier_cfg(COURIER_TOML, {"FFS_COURIER_MAIL_FOLDER": "Newsletters"})
    assert cfg.mailbox.host == "imap.example.test" and cfg.mailbox.folder == "Newsletters"
    assert [s.publisher for s in cfg.mailbox.sources] == ["example_ledger", "metro_morning"]
    assert cfg.wrappers == [{"host_pattern": "go.example-ledger.test", "kind": "base64_path_segment", "segment_index": 2}]
    assert cfg.edgar_max_rps == 8
    with pytest.raises(config.ConfigError, match="max_rps"):
        courier_cfg(COURIER_TOML.replace("max_rps = 8", "max_rps = 11"))
    with pytest.raises(config.ConfigError, match="user_agent is required"):
        courier_cfg(COURIER_TOML.replace('user_agent = "Test Owner test@example.test"', 'user_agent = ""') + '\n[[feed]]\nname = "e"\nadapter = "sec_edgar"\npublication = "SEC"\n[feed.filters]\nq = "x"\n')
    with pytest.raises(config.ConfigError, match="unknown key"):
        courier_cfg(COURIER_TOML + "\n[output]\ncolor = 1\n")


def test_starter_courier_toml_loads(starter_dir):
    with open(os.path.join(starter_dir, "courier.toml"), encoding="utf-8") as f:
        cfg = courier_cfg(f.read())
    assert cfg.mailbox.auth == "app_password"
    assert any(w["host_pattern"] == "link.bizjournals.com" for w in cfg.wrappers)
    assert cfg.feeds == []  # feeds ship commented out
    assert cfg.body_limit == 4000


def test_courier_user_agent_is_owner_phrased_from_toml_and_env(tmp_path, monkeypatch):
    from config import load_courier

    cfg_dir = tmp_path / "config"
    cfg_dir.mkdir(exist_ok=True)
    (cfg_dir / "courier.toml").write_text(
        '[mailbox]\nhost = "imap.example.test"\nuser = "me@example.test"\n\n[courier]\nuser_agent = "My courier (me@example.test)"\n',
        encoding="utf-8",
    )
    monkeypatch.delenv("FFS_COURIER_USER_AGENT", raising=False)
    cfg = load_courier(str(tmp_path))
    assert cfg.user_agent == "My courier (me@example.test)"
    monkeypatch.setenv("FFS_COURIER_USER_AGENT", "Override agent (x@y.z)")
    assert load_courier(str(tmp_path)).user_agent == "Override agent (x@y.z)"


def test_make_fetcher_sends_the_configured_user_agent(monkeypatch):
    import urllib.request

    import courier

    seen = {}

    class _Resp:
        status = 200

        def read(self):
            return b"{}"

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    def fake_urlopen(req, timeout=30):
        seen["ua"] = req.get_header("User-agent")
        return _Resp()

    monkeypatch.setattr(urllib.request, "urlopen", fake_urlopen)
    fetcher = courier.make_fetcher("Owner phrased (o@w.n)")
    assert fetcher("https://example.test/feed", {}) == (200, b"{}")
    assert seen["ua"] == "Owner phrased (o@w.n)"
    fetcher("https://example.test/feed", {"User-Agent": "EDGAR declared (e@d.g)"})
    assert seen["ua"] == "EDGAR declared (e@d.g)"
