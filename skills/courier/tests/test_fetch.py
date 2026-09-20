import urllib.request

import fetch as fetchmod
from helpers import SOURCES_TOML, courier_cfg, imap_factory_for, sources_cfg
from ledger import Ledger
from writer import Item, Output
import mailbox as mbx
import urlnorm

ARTICLE_HTML = b"<html><head><title>Widget maker breaks ground</title></head><body><nav>menu</nav><article><p>The maker broke ground Monday.</p><p>Jobs will follow next year.</p></article><footer>foot</footer></body></html>"


def _items():
    return [
        Item(title="A", url="https://news.example-ledger.test/news/2026/09/14/a.html", publication="Example Ledger", published_at="2026-09-14", intake="pointer", fetch="scheduled"),
        Item(title="B", url="https://news.example-ledger.test/news/2026/09/14/b.html", publication="Example Ledger", published_at="2026-09-14", intake="pointer", fetch="scheduled"),
        Item(title="C", url="https://news.example-ledger.test/news/2026/09/14/c.html", publication="Example Ledger", published_at="2026-09-14", intake="pointer", fetch="scheduled"),
    ]


def _opener(status_by_url, requests):
    def opener(req: urllib.request.Request, timeout: float):
        requests.append(req)
        return status_by_url.get(req.full_url, 404), ARTICLE_HTML if status_by_url.get(req.full_url) == 200 else b""

    return opener


def test_scheduled_fetch_requests_only_digest_urls_at_human_pace_under_daily_cap(tmp_path, monkeypatch):
    monkeypatch.setenv("FFS_COURIER_COOKIES_NEWS_EXAMPLE_LEDGER_TEST", "news.example-ledger.test\tTRUE\t/\tTRUE\t0\tsess\tabc123\n")
    scheduled = SOURCES_TOML.replace('fetch = "session"', 'fetch = "scheduled"\ndaily_cap = 2\nmin_pause_seconds = 5')
    sources = sources_cfg(scheduled)
    items = _items()
    requests = []
    sleeps = []
    ledger = Ledger(str(tmp_path))
    out = Output(str(tmp_path / "ingest"))
    for it in items:
        out.write_item(it)
    res = fetchmod.scheduled_fetch(items, sources, ledger, out, opener=_opener({i.url: 200 for i in items}, requests), sleep=sleeps.append, rand=lambda: 0.5, today="2026-09-14")
    assert [r.full_url for r in requests] == [items[0].url, items[1].url], "daily_cap 2 stops the third"
    assert all(r.full_url in {i.url for i in items} for r in requests), "only digest urls are ever requested"
    assert sleeps == [7.5], "one pause between the two requests, at least min_pause_seconds"
    assert requests[0].get_header("Cookie") == "sess=abc123"
    assert res["fetch_requests"] == [{"url": items[0].url, "status": 200}, {"url": items[1].url, "status": 200}]
    assert any("daily_cap" in w for w in res["warnings"])
    assert ledger.fetch_count("example_ledger", "2026-09-14") == 2


def test_scheduled_fetch_403_keeps_pointer_and_counts_failure(tmp_path):
    scheduled = SOURCES_TOML.replace('fetch = "session"', 'fetch = "scheduled"')
    items = _items()[:1]
    out = Output(str(tmp_path / "ingest"))
    path = out.write_item(items[0])
    before = open(path, encoding="utf-8").read()
    res = fetchmod.scheduled_fetch(items, sources_cfg(scheduled), Ledger(str(tmp_path)), out, opener=_opener({items[0].url: 403}, []), sleep=lambda s: None)
    assert res["fetch_failures"] == 1 and res["clipped"] == []
    assert open(path, encoding="utf-8").read() == before
    assert "intake: pointer" in before


def test_scheduled_fetch_200_rewrites_item_as_clip_with_content_hash(tmp_path):
    scheduled = SOURCES_TOML.replace('fetch = "session"', 'fetch = "scheduled"')
    items = _items()[:1]
    out = Output(str(tmp_path / "ingest"))
    path = out.write_item(items[0])
    res = fetchmod.scheduled_fetch(items, sources_cfg(scheduled), Ledger(str(tmp_path)), out, opener=_opener({items[0].url: 200}, []), sleep=lambda s: None)
    text = open(path, encoding="utf-8").read()
    assert res["clipped"] == [items[0].url]
    assert "\nintake: clip\n" in text and "content_hash: " + urlnorm.content_hash_multibase(ARTICLE_HTML) in text
    assert "The maker broke ground Monday.\n\nJobs will follow next year." in text
    assert "menu" not in text and "foot" not in text


def test_starter_sources_toml_never_schedules_terms_restricted_publishers(tmp_path, starter_dir):
    with open(f"{starter_dir}/sources.toml", encoding="utf-8") as f:
        starter = sources_cfg(f.read())
    items = [
        Item(title="x", url="https://www.bizjournals.com/charlotte/news/2026/09/14/x.html", publication="CBJ", published_at="2026-09-14", intake="pointer", fetch="session"),
        Item(title="y", url="https://www.charlotteobserver.com/news/business/article1.html", publication="Obs", published_at="2026-09-14", intake="pointer", fetch="session"),
        Item(title="z", url="https://www.axios.com/local/charlotte/2026/09/14/z", publication="Axios", published_at="2026-09-14", intake="pointer", fetch="session"),
    ]
    requests = []
    out = Output(str(tmp_path / "ingest"))
    res = fetchmod.scheduled_fetch(items, starter, Ledger(str(tmp_path)), out, opener=_opener({}, requests), sleep=lambda s: None)
    assert requests == [] and res["fetch_requests"] == []
    # the owner flips one publisher to scheduled: exactly its digest urls are requested
    with open(f"{starter_dir}/sources.toml", encoding="utf-8") as f:
        flipped = f.read().replace('key = "bizjournals"\nname = "Charlotte Business Journal"\ndomains = ["bizjournals.com"]\nintake = "pointer"\nfetch = "session"', 'key = "bizjournals"\nname = "Charlotte Business Journal"\ndomains = ["bizjournals.com"]\nintake = "pointer"\nfetch = "scheduled"')
    assert 'fetch = "scheduled"\ndaily_cap = 20' in flipped
    requests2 = []
    fetchmod.scheduled_fetch(items, sources_cfg(flipped), Ledger(str(tmp_path / "f")), Output(str(tmp_path / "f" / "ingest")), opener=_opener({}, requests2), sleep=lambda s: None)
    assert [r.full_url for r in requests2] == [items[0].url]


def test_run_tick_applies_scheduled_fetch_after_mailbox(tmp_path):
    scheduled = SOURCES_TOML.replace('fetch = "session"', 'fetch = "scheduled"\nmin_pause_seconds = 0')
    requests = []
    opener = _opener({"https://news.example-ledger.test/news/2026/09/14/widget-maker-second-plant.html": 200}, requests)
    import courier

    res = courier.run_tick({"ticks": ["mailbox"]}, base_dir=str(tmp_path), cfg=courier_cfg(), sources=sources_cfg(scheduled), imap_factory=imap_factory_for("pointer_digest.eml"), fetch_opener=opener)
    r = res["results"][0]
    assert len(r["fetch_requests"]) == 3 and r["fetch_failures"] == 2
    text = (tmp_path / "ingest" / "example-ledger-2026-09-14-widget-maker-breaks-ground-on-second-plant.md").read_text(encoding="utf-8")
    assert "intake: clip" in text
