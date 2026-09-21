import json
import os

import urlnorm

_FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir, "fixtures", "urlnorm.json")


def test_normalize_url_strips_tracking_params_and_fragment():
    raw = "HTTPS://Example.com/path/?utm_source=a&fbclid=b&gclid=c&keep=1#frag"
    assert urlnorm.normalize_url(raw) == "https://example.com/path?keep=1"


def test_normalize_url_is_idempotent():
    with open(_FIXTURE, encoding="utf-8") as f:
        pairs = json.load(f)["pairs"]
    for p in pairs:
        once = urlnorm.normalize_url(p["raw"])
        assert urlnorm.normalize_url(once) == once, p["raw"]


def test_shared_fixture_pairs_all_agree():
    with open(_FIXTURE, encoding="utf-8") as f:
        pairs = json.load(f)["pairs"]
    assert len(pairs) >= 12
    for p in pairs:
        assert urlnorm.normalize_url(p["raw"]) == p["normalized"], p["raw"]


def test_tracking_link_decodes_to_canonical_url():
    import base64

    real = "https://www.bizjournals.com/charlotte/news/2026/09/14/story.html?utm_source=st"
    seg = base64.urlsafe_b64encode(real.encode()).decode().rstrip("=")
    wrapped = f"https://link.bizjournals.com/click/1.2/{seg}/deadbeef"
    assert urlnorm.decode_tracking_url(wrapped) == real
    assert urlnorm.normalize_url(wrapped) == "https://www.bizjournals.com/charlotte/news/2026/09/14/story.html"


def test_custom_wrapper_table_is_honored_and_unknown_hosts_untouched():
    import base64

    real = "https://news.example.org/a"
    seg = base64.urlsafe_b64encode(real.encode()).decode().rstrip("=")
    wrapped = f"https://go.example-press.test/r/{seg}"
    assert urlnorm.decode_tracking_url(wrapped) == wrapped
    table = [{"host_pattern": "go.example-press.test", "kind": "base64_path_segment", "segment_index": 1}]
    assert urlnorm.decode_tracking_url(wrapped, table) == real


def test_article_basename_slug_is_deterministic():
    a = urlnorm.article_basename("Example Ledger", "2026-09-14", "Maker opens second plant: what's next?")
    b = urlnorm.article_basename("Example Ledger", "2026-09-14", "Maker opens second plant: what's next?")
    assert a == b == "example-ledger-2026-09-14-maker-opens-second-plant-what-s-next"
    assert urlnorm.article_basename("Example Ledger", "2026-09-15", "Maker opens second plant: what's next?") != a
    assert urlnorm.article_basename("Other Paper", "2026-09-14", "Maker opens second plant: what's next?") != a
    long_title = "word " * 40
    assert len(urlnorm.slug(long_title)) <= 80


def test_content_hash_multibase_is_blake2b_base58btc():
    h = urlnorm.content_hash_multibase(b"hello")
    assert h.startswith("z")
    assert h == urlnorm.content_hash_multibase(b"hello")
    assert h != urlnorm.content_hash_multibase(b"hello!")
    # base58btc alphabet only, after the prefix
    assert all(c in urlnorm._B58_ALPHABET for c in h[1:])
    # leading zero bytes become '1's
    assert urlnorm.base58btc_encode(b"\x00\x00\x01") == "112"


def test_campaign_params_are_stripped_so_a_resent_article_is_one_article():
    """Live finding 2026-09-21: the Journal's digest carries ana, j, and
    senddate. The normalized url is the article's identity key, so a story
    re-sent on a later day must collapse to the same url."""
    from urlnorm import normalize_url

    base = "https://www.bizjournals.com/charlotte/news/2025/10/01/story.html"
    day1 = normalize_url(base + "?ana=e_CH_EX&j=47558334&senddate=2026-09-19")
    day2 = normalize_url(base + "?ana=e_CH_EX&j=48112900&senddate=2026-10-02")
    assert day1 == day2 == base
    # A real query parameter survives.
    assert normalize_url(base + "?id=7&senddate=2026-09-19") == base + "?id=7"
