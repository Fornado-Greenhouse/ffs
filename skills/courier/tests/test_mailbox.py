import email
import email.policy
import os

import mailbox as mbx
from helpers import courier_cfg, fetcher_from, imap_factory_for, load_eml, sources_cfg, SOURCES_TOML
from ledger import Ledger
from writer import Output


def _msg(name):
    return email.message_from_bytes(load_eml(name)[0], policy=email.policy.default)


def test_courier_parses_html_digest_into_items():
    cfg = courier_cfg()
    items = mbx.message_items(_msg("pointer_digest.eml"), cfg.wrappers)
    urls = [i["url"] for i in items]
    assert "https://news.example-ledger.test/news/2026/09/14/widget-maker-second-plant.html" in urls
    assert all("utm_" not in u for u in urls)
    heads = [i["headline"] for i in items]
    assert "Widget maker breaks ground on second plant" in heads
    assert not any(h.lower().startswith("read full story") for h in heads)


def test_courier_parses_text_digest_into_items():
    cfg = courier_cfg()
    items = mbx.message_items(_msg("text_digest.eml"), cfg.wrappers)
    assert [(i["headline"], i["url"]) for i in items] == [
        ("Port authority approves warehouse lease", "https://news.example-ledger.test/news/2026/09/15/port-warehouse-lease.html"),
        ("Startup raises $12M seed round", "https://news.example-ledger.test/news/2026/09/15/startup-seed-round.html"),
    ]


def test_tracking_link_decodes_to_canonical_url():
    cfg = courier_cfg()
    items = mbx.message_items(_msg("pointer_digest.eml"), cfg.wrappers)
    assert all(not i["url"].startswith("https://go.example-ledger.test") for i in items)


def _run(tmp_path, *emls, sources_text=SOURCES_TOML, dry=False, query=None):
    cfg = courier_cfg()
    sources = sources_cfg(sources_text)
    ledger = Ledger(str(tmp_path))
    out = Output(str(tmp_path / "ingest"), str(tmp_path / "ingest" / ".courier" / "dry-run" / "t") if dry else None)
    factory = imap_factory_for(*emls)
    res = mbx.mailbox_tick(cfg, sources, ledger, out, imap_factory=factory, query=query)
    return res, factory.fake, ledger


def test_courier_writes_one_ingest_file_per_item_and_one_digest(tmp_path):
    res, fake, ledger = _run(tmp_path, "pointer_digest.eml")
    ingest = tmp_path / "ingest"
    articles = sorted(p.name for p in ingest.glob("example-ledger-2026-09-14-*.md") if "-digest-" not in p.name)
    assert len(articles) == 3
    assert (ingest / "example-ledger-digest-2026-09-14.md").exists()
    assert res["items_seen"] == 3 and res["files_written"] == 4 and res["last_error"] is None
    for name in articles:
        text = (ingest / name).read_text(encoding="utf-8")
        assert text.startswith("---\npredicate: source.article\ntitle: ")
        assert "\nintake: pointer\n" in text and "\nfetch: session\n" in text
        assert text.rstrip().endswith("---"), "pointer files have an empty body"
        assert "## Mentions" not in text
    assert "\\Seen" in fake.flags[1]
    assert len(ledger) == 3


def test_pointer_mode_writes_title_url_date_and_empty_body_byte_for_byte(tmp_path, fixtures_dir):
    _run(tmp_path, "pointer_digest.eml")
    got = (tmp_path / "ingest" / "example-ledger-2026-09-14-widget-maker-breaks-ground-on-second-plant.md").read_bytes()
    exp = open(os.path.join(fixtures_dir, "expected", "example-ledger-2026-09-14-widget-maker-breaks-ground-on-second-plant.md"), "rb").read()
    assert got == exp
    got_d = (tmp_path / "ingest" / "example-ledger-digest-2026-09-14.md").read_bytes()
    exp_d = open(os.path.join(fixtures_dir, "expected", "example-ledger-digest-2026-09-14.md"), "rb").read()
    assert got_d == exp_d


def test_blurb_mode_carries_reported_by_from_parenthetical(tmp_path):
    res, _, _ = _run(tmp_path, "clip_newsletter.eml")
    ingest = tmp_path / "ingest"
    bakery = (ingest / "metro-morning-2026-09-14-new-bakery-opens-on-main.md").read_text(encoding="utf-8")
    assert "\nintake: clip\n" in bakery and "\nfetch: off\n" in bakery
    assert "reported_by: The Example Ledger" in bakery
    assert "Crumb & Co., a new bakery from owners Dana Field and Lee Park, is now open on Main Street." in bakery
    assert "(The Example Ledger)" not in bakery.split("---")[2]
    transit = (ingest / "metro-morning-2026-09-14-council-to-vote-on-transit-lanes.md").read_text(encoding="utf-8")
    assert "reported_by" not in transit
    assert "Mira Cole" in transit
    assert res["items_seen"] == 3


def test_courier_second_run_over_same_message_writes_nothing(tmp_path):
    res1, fake, ledger = _run(tmp_path, "pointer_digest.eml")
    assert res1["files_written"] == 4
    # second tick: the message is now seen (UNSEEN search skips it) and the ledger knows the urls
    cfg = courier_cfg()
    out = Output(str(tmp_path / "ingest"))
    res2 = mbx.mailbox_tick(cfg, sources_cfg(), Ledger(str(tmp_path)), out, imap_factory=lambda _mb: fake)
    assert res2["files_written"] == 0 and res2["files"] == []
    # and even with mark_seen off (the message is re-read), the ledger blocks every item
    fake.flags[1].clear()
    calls = []

    def query(method, params):
        calls.append((method, params))
        return {"results": []}

    res3 = mbx.mailbox_tick(cfg, sources_cfg(), Ledger(str(tmp_path)), Output(str(tmp_path / "ingest")), imap_factory=lambda _mb: fake, query=query)
    assert res3["files_written"] == 0 and res3["skipped_seen"] == 3
    # substrate dedup: an unknown-to-ledger url that the substrate already has is skipped
    fake.flags[1].clear()
    fresh = Ledger(str(tmp_path / "other"))
    res4 = mbx.mailbox_tick(cfg, sources_cfg(), fresh, Output(str(tmp_path / "other" / "ingest")), imap_factory=lambda _mb: fake, query=lambda m, p: {"results": [{"entity": "z1"}]})
    assert res4["skipped_seen"] == 3 and res4["files_written"] == 0


def test_courier_dry_run_writes_to_scratch_only(tmp_path):
    res, fake, ledger = _run(tmp_path, "pointer_digest.eml", dry=True)
    assert list((tmp_path / "ingest").glob("*.md")) == []
    scratch = tmp_path / "ingest" / ".courier" / "dry-run" / "t"
    assert len(list(scratch.glob("*.md"))) == 4
    assert res["dry_run"] is True and res["files_written"] == 0 and len(res["would_submit"]) == 4
    assert "\\Seen" not in fake.flags[1]
    assert len(ledger) == 0 and not (tmp_path / "ingest" / ".courier" / "seen.json").exists()


def test_courier_sender_and_subject_filters(tmp_path):
    res, _, _ = _run(tmp_path, "unrelated.eml")
    assert res["items_seen"] == 0 and res["files_written"] == 0
    cfg = courier_cfg()
    cfg.mailbox.sources[0].subject_filter = "^Morning Edition"
    fake = imap_factory_for("pointer_digest.eml", "text_digest.eml")
    res2 = mbx.mailbox_tick(cfg, sources_cfg(), Ledger(str(tmp_path)), Output(str(tmp_path / "ingest")), imap_factory=fake)
    assert res2["items_seen"] == 2, "only the Morning Edition digest matches the subject filter"


def test_pointer_publisher_flipped_to_clip_changes_output_without_code_change(tmp_path):
    flipped = SOURCES_TOML.replace('intake = "pointer"', 'intake = "clip"')
    _run(tmp_path, "pointer_digest.eml", sources_text=flipped)
    text = (tmp_path / "ingest" / "example-ledger-2026-09-14-widget-maker-breaks-ground-on-second-plant.md").read_text(encoding="utf-8")
    assert "\nintake: clip\n" in text


def test_unknown_publisher_files_as_pointer_with_warning(tmp_path):
    res, _, _ = _run(tmp_path, "clip_newsletter.eml", sources_text='[[publisher]]\nkey = "x"\nname = "X"\ndomains = ["x.test"]\n')
    assert any("not in sources.toml" in w for w in res["warnings"])
    text = (tmp_path / "ingest" / "metro-morning-2026-09-14-new-bakery-opens-on-main.md").read_text(encoding="utf-8")
    assert "\nintake: pointer\n" in text and "publication: metro_morning" in text


def test_oauth_connect_gives_clear_error(tmp_path):
    cfg = courier_cfg()
    cfg.mailbox.auth = "oauth"
    res = mbx.mailbox_tick(cfg, sources_cfg(), Ledger(str(tmp_path)), Output(str(tmp_path / "ingest")))
    assert "oauth is not implemented" in (res["last_error"] or "")


def test_published_at_prefers_the_articles_own_date_from_the_url():
    """Live finding 2026-09-21: a digest re-ran a 2025 story. Dating it by
    the send date would file a year-old article as today's news."""
    from mailbox import _published_at

    old = "https://www.bizjournals.com/charlotte/news/2025/10/01/story.html"
    assert _published_at(old, "2026-09-19") == "2025-10-01"
    # No date in the path: the digest's send date stands.
    assert _published_at("https://example.test/news/story.html", "2026-09-19") == "2026-09-19"
    # A future-dated path is not trusted over the send date.
    assert _published_at("https://x.test/news/2099/01/01/s.html", "2026-09-19") == "2026-09-19"


def test_reading_the_mailbox_never_marks_a_message_seen_by_itself(tmp_path):
    """Live finding 2026-09-21: the tick fetched with RFC822, which sets
    \\Seen as a side effect, so a dry run marked the owner's digests as
    read and the next tick's UNSEEN search found nothing. The fetch must
    peek; only a real tick whose files were all written marks the message.
    """
    _res, fake_dry, _ledger = _run(tmp_path, "pointer_digest.eml", dry=True)
    assert all("\\Seen" not in f for f in fake_dry.flags.values()), (
        "a dry run must leave every message unread"
    )

    _res2, fake_real, _l2 = _run(tmp_path / "real", "pointer_digest.eml")
    assert "\\Seen" in fake_real.flags[1], (
        "a real tick marks the message once every file was written"
    )
