"""URL normalization, article basenames, and content hashes (task_40).

Shared by the courier (which files articles) and the scribe (which
reads the files back). The Rust side (`ffs-core`) carries the same
rules and both are checked against `fixtures/urlnorm.json`, so the
two implementations cannot drift silently.

Stdlib only. Nothing here talks to the network.

Rules for `normalize_url`:

- A known tracking wrapper is decoded first (`link.bizjournals.com/
  click/<campaign>/<base64url>/<hash>` carries the real URL as a
  base64url path segment; no click is needed).
- scheme and host are lowercased; a default port is dropped;
- `utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid` query parameters are
  removed; other parameters keep their order;
- the fragment is dropped;
- a trailing slash on a non-root path is collapsed.

The article's entity id is opaque and minted once (ADR-030); the
basename `<publication-slug>-<YYYY-MM-DD>-<title-slug>` is a
projection file name and the resolver's blocking key, never the
identity.
"""

from __future__ import annotations

import base64
import hashlib
import re
from typing import Any, Dict, Iterable, List, Optional
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

_TRACKING_PARAMS = {"fbclid", "gclid", "mc_cid", "mc_eid", "dclid", "msclkid"}
_TRACKING_PREFIXES = ("utm_",)

# The default wrapper table. `segment_index` counts non-empty path
# segments from zero, so `/click/<campaign>/<b64>/<hash>` has the
# payload at index 2. Owners extend the table in `courier.toml`
# (`[[tracking.wrapper]]`); this default is what a fresh install knows.
DEFAULT_WRAPPERS: List[Dict[str, Any]] = [
    {
        "host_pattern": "link.bizjournals.com",
        "kind": "base64_path_segment",
        "segment_index": 2,
    }
]

_DEFAULT_PORTS = {"http": "80", "https": "443"}


def _b64url_decode(segment: str) -> Optional[str]:
    padded = segment + "=" * (-len(segment) % 4)
    try:
        raw = base64.urlsafe_b64decode(padded.encode("ascii"))
    except (ValueError, UnicodeEncodeError):
        return None
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if text.startswith(("http://", "https://")):
        return text
    return None


def decode_tracking_url(raw: str, wrappers: Optional[Iterable[Dict[str, Any]]] = None) -> str:
    """Return the real URL carried by a known tracking wrapper, or
    `raw` unchanged when no wrapper matches or the payload does not
    decode to an http(s) URL."""
    table = list(wrappers) if wrappers is not None else DEFAULT_WRAPPERS
    try:
        parts = urlsplit(raw.strip())
    except ValueError:
        return raw
    host = (parts.hostname or "").lower()
    if not host:
        return raw
    for w in table:
        pattern = str(w.get("host_pattern", "")).lower()
        if not pattern:
            continue
        if host != pattern and not host.endswith("." + pattern):
            continue
        if w.get("kind") != "base64_path_segment":
            continue
        segments = [s for s in parts.path.split("/") if s]
        try:
            idx = int(w.get("segment_index", 0))
        except (TypeError, ValueError):
            continue
        if idx < 0 or idx >= len(segments):
            continue
        decoded = _b64url_decode(segments[idx])
        if decoded:
            return decoded
    return raw


def _is_tracking_param(key: str) -> bool:
    k = key.lower()
    return k in _TRACKING_PARAMS or any(k.startswith(p) for p in _TRACKING_PREFIXES)


def normalize_url(raw: str, wrappers: Optional[Iterable[Dict[str, Any]]] = None) -> str:
    """Canonical form of an article URL. Idempotent."""
    url = decode_tracking_url(raw.strip(), wrappers)
    try:
        parts = urlsplit(url)
    except ValueError:
        return url
    scheme = (parts.scheme or "").lower()
    host = (parts.hostname or "").lower()
    if not scheme or not host:
        return url
    netloc = host
    if parts.port is not None and str(parts.port) != _DEFAULT_PORTS.get(scheme):
        netloc = f"{host}:{parts.port}"
    if parts.username:
        cred = parts.username + (":" + parts.password if parts.password else "")
        netloc = f"{cred}@{netloc}"
    path = parts.path or "/"
    if len(path) > 1 and path.endswith("/"):
        path = path.rstrip("/") or "/"
    kept = [(k, v) for k, v in parse_qsl(parts.query, keep_blank_values=True) if not _is_tracking_param(k)]
    query = urlencode(kept, doseq=True) if kept else ""
    return urlunsplit((scheme, netloc, path, query, ""))


_SLUG_KEEP = re.compile(r"[^a-z0-9]+")


def slug(text: str, max_len: int = 80) -> str:
    """Lowercase, non-alphanumerics to single hyphens, trimmed, capped."""
    s = _SLUG_KEEP.sub("-", text.strip().lower()).strip("-")
    if len(s) > max_len:
        s = s[:max_len].rstrip("-")
    return s


def article_basename(publication: str, date: str, title: str) -> str:
    """`<publication-slug>-<YYYY-MM-DD>-<title-slug>`. Deterministic for
    the same inputs; differs on any change; never used as an entity id."""
    date_part = date.strip()[:10]
    return f"{slug(publication, 40)}-{date_part}-{slug(title, 80)}"


_B58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def base58btc_encode(data: bytes) -> str:
    n = int.from_bytes(data, "big")
    out = ""
    while n > 0:
        n, rem = divmod(n, 58)
        out = _B58_ALPHABET[rem] + out
    pad = 0
    for b in data:
        if b == 0:
            pad += 1
        else:
            break
    return "1" * pad + out


def content_hash_multibase(data: bytes) -> str:
    """blake2b-256 of the bytes, base58btc with the multibase `z`
    prefix. The Rust side accepts either a blake3 or a blake2b digest
    for `content_hash`; the courier only has blake2b in the stdlib."""
    digest = hashlib.blake2b(data, digest_size=32).digest()
    return "z" + base58btc_encode(digest)
