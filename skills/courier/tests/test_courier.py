import json
import os

import courier
from helpers import COURIER_TOML, SOURCES_TOML, courier_cfg, imap_factory_for, sources_cfg


def _write_configs(tmp_path):
    (tmp_path / "config" / "courier.toml").write_text(COURIER_TOML, encoding="utf-8")
    (tmp_path / "config" / "sources.toml").write_text(SOURCES_TOML, encoding="utf-8")


def test_run_tick_dry_run_reports_would_submit_and_touches_nothing(tmp_path):
    _write_configs(tmp_path)
    res = courier.run_tick({"dry_run": True}, base_dir=str(tmp_path), imap_factory=imap_factory_for("pointer_digest.eml", "clip_newsletter.eml"))
    assert res["dry_run"] is True and res["last_error"] is None
    r = res["results"][0]
    assert set(r.keys()) == set(courier.RESULT_KEYS)
    assert r["files_written"] == 0 and len(r["would_submit"]) == 3 + 1 + 3 + 1
    assert not (tmp_path / "ingest" / ".courier" / "last_run.json").exists()
    assert list((tmp_path / "ingest").glob("*.md")) == []


def test_run_tick_real_run_writes_last_run_and_status_reads_it(tmp_path):
    _write_configs(tmp_path)
    res = courier.run_tick({}, base_dir=str(tmp_path), imap_factory=imap_factory_for("pointer_digest.eml"))
    r = res["results"][0]
    assert r["files_written"] == 4 and r["tick"] == "mailbox"
    st = courier.status(str(tmp_path))
    assert st["files_written"] == 4 and st["items_seen"] == 3 and st["last_run"].endswith("Z") and st["last_error"] is None
    # second real run: nothing new, status reflects it
    res2 = courier.run_tick({}, base_dir=str(tmp_path), imap_factory=imap_factory_for("pointer_digest.eml"))
    assert res2["results"][0]["files_written"] == 0 and courier.status(str(tmp_path))["files_written"] == 0


def test_run_tick_missing_config_is_a_clear_error(tmp_path):
    res = courier.run_tick({}, base_dir=str(tmp_path))
    assert "courier.toml not found" in res["last_error"]


def test_second_tick_same_day_appends_to_existing_digest(tmp_path):
    _write_configs(tmp_path)
    courier.run_tick({}, base_dir=str(tmp_path), imap_factory=imap_factory_for("pointer_digest.eml"))
    # a later message the same day from the same publisher
    import email, email.policy
    from helpers import load_eml

    import base64

    real = "https://news.example-ledger.test/news/2026/09/14/widget-maker-third-plant.html"
    seg = base64.urlsafe_b64encode(real.encode()).decode().rstrip("=")
    html = f'<html><body><a href="https://go.example-ledger.test/click/9.9/{seg}/ffff">Widget maker breaks ground on third plant</a></body></html>'
    raw2 = load_eml("pointer_digest.eml")[0].split(b"\n\n", 1)[0] + b"\n\n" + html.encode()
    from mailbox import FakeImap

    fake = FakeImap([raw2])
    courier.run_tick({}, base_dir=str(tmp_path), imap_factory=lambda _mb: fake)
    digest = (tmp_path / "ingest" / "example-ledger-digest-2026-09-14.md").read_text(encoding="utf-8")
    refs = [ln for ln in digest.splitlines() if ln.startswith("- [[")]
    assert len(refs) == 4 and len(set(refs)) == 4
    assert digest.count("## References") == 1


def test_standalone_cli_dry_run_prints_json(tmp_path, monkeypatch, capsys):
    _write_configs(tmp_path)
    import mailbox as mbx

    monkeypatch.setattr(courier, "default_imap_factory", imap_factory_for("pointer_digest.eml"))
    monkeypatch.setattr("sys.argv", ["courier.py", "run", "--dry-run"])
    res = courier.run_tick({"dry_run": True}, base_dir=str(tmp_path), imap_factory=courier.default_imap_factory)
    out = json.dumps(res)
    assert "would_submit" in out and '"dry_run": true' in out


def test_agent_hosted_skill_doc_embeds_the_exact_expected_fixtures():
    """docs/agent-memory/skill/ffs-courier/SKILL.md must carry the bundle's
    byte-exact output for the worked example (task_40 success criterion)."""
    import os

    here = os.path.dirname(os.path.abspath(__file__))
    repo = os.path.abspath(os.path.join(here, os.pardir, os.pardir, os.pardir))
    doc = open(os.path.join(repo, "docs", "agent-memory", "skill", "ffs-courier", "SKILL.md"), encoding="utf-8").read()
    expected_dir = os.path.join(here, "fixtures", "expected")
    for name in sorted(os.listdir(expected_dir)):
        body = open(os.path.join(expected_dir, name), encoding="utf-8").read().rstrip("\n")
        assert body in doc, f"{name} is not embedded verbatim in the agent-hosted skill doc"
