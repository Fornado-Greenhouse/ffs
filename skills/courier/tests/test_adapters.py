import json

from adapters import charlotte_legistar, meck_permits, rss, sec_edgar
from config import Feed
from helpers import fetcher_from, read_fixture, sources_cfg, courier_cfg
from ledger import Ledger
from writer import Output
import courier


def _feed(adapter, **kw):
    return Feed(name=kw.pop("name", adapter), adapter=adapter, publication=kw.pop("publication", adapter), **kw)


def test_meck_permits_adapter_maps_records_to_event_files(tmp_path):
    feed = _feed("meck_permits", publication="Mecklenburg County permits", endpoint="https://gis.example.test/FeatureServer/0/query", filters={"min_cost": 1000000})
    fetcher = fetcher_from({"https://gis.example.test/FeatureServer/0/query": (200, read_fixture("meck_permits.json"))})
    recs = meck_permits.tick(feed, "2026-09-01", fetcher, {})
    assert [r["id"] for r in recs] == ["BLDG-2026-018812", "BLDG-2026-018900"], "the record with no permit number is skipped"
    url, _ = fetcher.calls[0]
    # Live field names (2026-09-20): date filtered on the server with a DATE
    # literal; the cost field is a string on the layer, so min_cost is applied
    # client-side and never appears in the query.
    assert "issue_date+%3E%3D+DATE+%272026-09-01%27" in url and "orderByFields=issue_date+DESC" in url
    assert "min_cost" not in url and "building_construction_cost_customer" not in url
    item = meck_permits.to_item(recs[1], feed, "scheduled")
    assert item.intake == "feed" and item.publication == "Mecklenburg County permits"
    assert item.mentions == [("Digital Moores Chapel LLC", "permit owner, Data center upfit")]
    kind, desc, parts = item.events[0]
    assert kind == "opening" and "41,100,000" in desc and parts == [("Digital Moores Chapel LLC", "developer")]
    assert "courier-structured" in item.tags
    text = __import__("writer").render_article(item)
    assert "## Mentions\n- Digital Moores Chapel LLC — permit owner, Data center upfit" in text
    assert "## Events\n- opening: " in text and "| Digital Moores Chapel LLC (developer)" in text


def test_charlotte_legistar_adapter_extracts_petitioners():
    feed = _feed("charlotte_legistar", publication="Charlotte City Council", endpoint="https://api.legistar.test/v1/charlottenc")
    fetcher = fetcher_from({
        "https://api.legistar.test/v1/charlottenc/events": (200, read_fixture("legistar_events.json")),
        "https://api.legistar.test/v1/charlottenc/matters": (200, read_fixture("legistar_matters.json")),
    })
    recs = charlotte_legistar.tick(feed, "2026-09-01", fetcher, {})
    assert [r["id"] for r in recs] == ["event-4401", "matter-9101", "matter-9102"]
    assert recs[1]["petitioners"] == ["Barnhardt Manufacturing Co."]
    assert recs[2]["petitioners"] == ["Metro Transit Partners"]
    item = charlotte_legistar.to_item(recs[1], feed, "scheduled")
    assert item.mentions == [("Barnhardt Manufacturing Co.", "petitioner before Charlotte City Council")]
    assert any("$filter=" in u for u, _ in fetcher.calls)


def test_sec_edgar_adapter_declares_user_agent_and_respects_max_rps():
    feed = _feed("sec_edgar", publication="SEC EDGAR", endpoint="https://efts.example.test/search-index", filters={"q": "Charlotte", "forms": ["8-K"]})
    fetcher = fetcher_from({"https://efts.example.test/search-index": (200, read_fixture("edgar_search.json"))})
    clock = {"t": 0.0}
    sleeps = []
    limiter = sec_edgar.RateLimiter(3, clock=lambda: clock["t"], sleep=lambda s: (sleeps.append(s), clock.__setitem__("t", clock["t"] + s)))
    ctx = {"edgar_user_agent": "Owner owner@example.test", "limiter": limiter}
    recs = sec_edgar.tick(feed, "2026-09-01", fetcher, ctx)
    assert fetcher.calls[0][1]["User-Agent"] == "Owner owner@example.test"
    assert recs[0]["officer"] == "Jordan Reyes" and recs[0]["kind"] == "hire"
    assert recs[1]["officer"] is None
    item = sec_edgar.to_item(recs[0], feed, "scheduled")
    assert ("Jordan Reyes", "officer named in 8-K item 5.02, Example Bancorp (EXB)") in item.mentions
    assert item.events[0][0] == "hire" and ("Jordan Reyes", "hire") in item.events[0][2]
    # limiter: 3 per second; the fourth acquisition in the same second sleeps
    for _ in range(3):
        limiter.acquire()
    assert sleeps and sleeps[-1] > 0
    # never more than max_rps stamps inside any rolling second
    assert len(limiter.stamps) <= 3
    import pytest

    with pytest.raises(RuntimeError, match="User-Agent"):
        sec_edgar.tick(feed, "2026-09-01", fetcher, {"edgar_user_agent": ""})


def test_rss_adapter_dedups_by_guid():
    feed = _feed("rss", publication="Sample Energy newsroom", url="https://news.sample-energy.test/rss")
    fetcher = fetcher_from({"https://news.sample-energy.test/rss": (200, read_fixture("newsroom.rss"))})
    recs = rss.tick(feed, "", fetcher, {})
    assert [r["id"] for r in recs] == ["rel-2026-0914-1", "rel-2026-0912-1"]
    assert recs[0]["url"] == "https://news.sample-energy.test/releases/2026/09/14/rivera-president"
    assert recs[0]["summary"] == "Sample Energy today named Pat Rivera president of its Carolinas region."
    again = rss.tick(feed, "", fetcher, {"seen_guids": ["rel-2026-0914-1"]})
    assert [r["id"] for r in again] == ["rel-2026-0912-1"]
    since = rss.tick(feed, "2026-09-13", fetcher, {})
    assert [r["id"] for r in since] == ["rel-2026-0914-1"]
    item = rss.to_item(recs[0], feed, "scheduled")
    assert item.intake == "clip" and item.reported_by == "Sample Energy newsroom" and item.published_at == "2026-09-14"
    atom_feed = _feed("rss", publication="Sample Bank news", url="https://news.sample-bank.test/atom")
    fetcher2 = fetcher_from({"https://news.sample-bank.test/atom": (200, read_fixture("newsroom.atom"))})
    atom_recs = rss.tick(atom_feed, "", fetcher2, {})
    assert atom_recs[0]["title"] == "Sample Bank opens Uptown branch" and atom_recs[0]["date"] == "2026-09-14"


def test_feed_watermark_advances_only_after_all_files_written(tmp_path):
    cfg = courier_cfg()
    feed = _feed("rss", name="energy", publication="Sample Energy newsroom", url="https://news.sample-energy.test/rss", since="2026-09-01")
    fetcher = fetcher_from({"https://news.sample-energy.test/rss": (200, read_fixture("newsroom.rss"))})
    ledger = Ledger(str(tmp_path))
    out = Output(str(tmp_path / "ingest"))
    res = courier.feed_tick(feed, cfg, sources_cfg(), ledger, out, fetcher=fetcher)
    # two records on two different days: two article files plus one digest per publication per day
    assert res["items_seen"] == 2 and res["files_written"] == 4 and res["last_error"] is None
    assert ledger.watermark("energy") == "2026-09-14"
    # second tick: nothing new
    res2 = courier.feed_tick(feed, cfg, sources_cfg(), Ledger(str(tmp_path)), Output(str(tmp_path / "ingest")), fetcher=fetcher)
    # the watermark advanced to 09-14, so the 09-12 record is filtered by `since` and the 09-14 one is in the ledger
    assert res2["files_written"] == 0 and res2["items_seen"] == 1 and res2["skipped_seen"] == 1
    # malformed payload: skipped with an error in the tick result, watermark untouched
    bad = fetcher_from({"https://news.sample-energy.test/rss": (200, b"<not xml")})
    l3 = Ledger(str(tmp_path / "b"))
    res3 = courier.feed_tick(feed, cfg, sources_cfg(), l3, Output(str(tmp_path / "b" / "ingest")), fetcher=bad)
    assert res3["last_error"] and res3["files_written"] == 0
    assert l3.watermark("energy", "2026-09-01") == "2026-09-01"
    # a write failure keeps the watermark
    l4 = Ledger(str(tmp_path / "c"))
    out4 = Output(str(tmp_path / "c" / "ingest"))
    real_write = out4.write_item
    calls = {"n": 0}

    def flaky(item):
        calls["n"] += 1
        if calls["n"] == 2:
            raise OSError("disk full")
        return real_write(item)

    out4.write_item = flaky  # type: ignore[assignment]
    res4 = courier.feed_tick(feed, cfg, sources_cfg(), l4, out4, fetcher=fetcher)
    assert res4["files_written"] >= 1 and any("write failed" in w for w in res4["warnings"])
    assert l4.watermark("energy", "2026-09-01") == "2026-09-01"


def test_rss_adapter_reports_html_page_as_not_a_feed():
    from adapters import rss

    try:
        rss.parse_feed(b"<!DOCTYPE html><html><head><title>Newsroom</title></head><body></body></html>")
    except ValueError as e:
        assert "not an XML feed" in str(e)
    else:
        raise AssertionError("HTML page must not parse as a feed")


def test_sec_edgar_query_carries_both_ends_of_the_custom_date_range():
    from adapters import sec_edgar

    seen = {}

    def fetcher(url, headers=None):
        seen["url"] = url
        return 200, b'{"hits": {"hits": []}}'

    class Feed:
        endpoint = None
        filters = {"q": "Charlotte, North Carolina", "forms": ["8-K"]}
        name = "edgar"
        publication = "SEC EDGAR"

    sec_edgar.tick(Feed(), "2026-09-01", fetcher, {"edgar_user_agent": "t (x@y.z)", "today": "2026-09-20"})
    assert "startdt=2026-09-01" in seen["url"] and "enddt=2026-09-20" in seen["url"]
