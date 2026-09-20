"""Mailbox tick (task_40): the owner's own inbox, read deterministically.

Connect over IMAP SSL, select the folder, find unseen messages that
match a configured source (sender and optional subject filter), parse
each digest into items (headline, blurb, link), decode tracking
wrappers, normalize the url, drop links outside the publisher's
domains, skip what the ledger or the substrate already knows, and
write one ingest file per new item plus the day's digest note. A
second tick over the same message writes zero files.

The IMAP client and the substrate query are injected so tests use a
FakeImap over ``.eml`` fixtures and never touch the network.
"""

from __future__ import annotations

import email
import email.policy
import imaplib
import re
from datetime import datetime
from email.message import EmailMessage
from html.parser import HTMLParser
from typing import Any, Callable, Dict, List, Optional, Tuple
from urllib.parse import urlsplit

import urlnorm  # skills/_lib

from config import CourierConfig, Mailbox, MailboxSource, Publisher, Sources, mailbox_secret
from ledger import Ledger
from writer import Item, Output

ImapFactory = Callable[[Mailbox], Any]
SubstrateQuery = Callable[[str, Dict[str, Any]], Any]


class MailboxError(RuntimeError):
    pass


# --------------------------------------------------------------------
# IMAP
# --------------------------------------------------------------------


def default_imap_factory(mb: Mailbox) -> Any:
    if mb.auth == "oauth":
        raise MailboxError(
            "oauth is not implemented in this release; use app_password with a Google "
            "Workspace app password (requires 2-step verification)"
        )
    secret = mailbox_secret(mb.host)
    if not secret:
        raise MailboxError(
            f"no mailbox secret for {mb.host}: store it in the OS keychain under service "
            f"ffs.courier.{mb.host} (or set FFS_COURIER_MAIL_PASSWORD for tests)"
        )
    client = imaplib.IMAP4_SSL(mb.host, mb.port)
    client.login(mb.user, secret)
    return client


class FakeImap:
    """A minimal IMAP stand-in over parsed .eml messages (tests)."""

    def __init__(self, messages: List[bytes]) -> None:
        self.messages = messages
        self.flags: Dict[int, set] = {i + 1: set() for i in range(len(messages))}
        self.selected: Optional[str] = None
        self.logged_out = False

    def select(self, folder: str, readonly: bool = False) -> Tuple[str, List[bytes]]:
        self.selected = folder
        return "OK", [str(len(self.messages)).encode()]

    def search(self, charset: Any, *criteria: str) -> Tuple[str, List[bytes]]:
        unseen_only = any(c.upper() == "UNSEEN" for c in criteria)
        nums = [str(n) for n, flags in self.flags.items() if not (unseen_only and "\\Seen" in flags)]
        return "OK", [" ".join(nums).encode()]

    def fetch(self, num: str, parts: str) -> Tuple[str, List[Any]]:
        raw = self.messages[int(num) - 1]
        return "OK", [(f"{num} (RFC822 {{{len(raw)}}}".encode(), raw), b")"]

    def store(self, num: str, op: str, flag: str) -> Tuple[str, List[bytes]]:
        if "+FLAGS" in op:
            self.flags[int(num)].add(flag)
        return "OK", []

    def logout(self) -> None:
        self.logged_out = True


# --------------------------------------------------------------------
# Digest parsing
# --------------------------------------------------------------------


class _DigestParser(HTMLParser):
    """Collect anchors (text, href) and the text blocks between them."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.events: List[Tuple[str, str]] = []  # ("a", href) / ("t", text) / ("aend", text)
        self._href: Optional[str] = None
        self._buf: List[str] = []
        self._skip = 0

    def handle_starttag(self, tag: str, attrs: List[Tuple[str, Optional[str]]]) -> None:
        if tag in ("script", "style"):
            self._skip += 1
            return
        if tag == "a":
            self._flush_text()
            self._href = dict(attrs).get("href") or ""
            self._buf = []
        elif tag in ("p", "div", "br", "td", "tr", "li", "h1", "h2", "h3", "h4"):
            self._flush_text()

    def handle_endtag(self, tag: str) -> None:
        if tag in ("script", "style"):
            self._skip = max(0, self._skip - 1)
            return
        if tag == "a" and self._href is not None:
            text = " ".join("".join(self._buf).split())
            self.events.append(("a", self._href))
            self.events.append(("aend", text))
            self._href = None
            self._buf = []
        elif tag in ("p", "div", "td", "tr", "li", "h1", "h2", "h3", "h4"):
            self._flush_text()

    def handle_data(self, data: str) -> None:
        if self._skip:
            return
        self._buf.append(data)

    def _flush_text(self) -> None:
        if self._href is None:
            text = " ".join("".join(self._buf).split())
            if text:
                self.events.append(("t", text))
            self._buf = []

    def close(self) -> None:
        super().close()
        self._flush_text()


_ATTRIB_RE = re.compile(r"\(([^()]{2,60})\)\s*$")
_NOISE_TEXT = re.compile(r"^(read full story|read more|learn more|view in browser|unsubscribe|manage preferences|see more)\W*$", re.I)


def _attribution(blurb: str) -> Tuple[str, Optional[str]]:
    m = _ATTRIB_RE.search(blurb)
    if not m:
        return blurb, None
    outlet = m.group(1).strip()
    if len(outlet.split()) > 8 or outlet.lower().startswith("http"):
        return blurb, None
    return blurb[: m.start()].rstrip(), outlet


def parse_html_digest(html: str, wrappers: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Return raw items: {headline, url (normalized), blurb}."""
    p = _DigestParser()
    p.feed(html)
    p.close()
    items: List[Dict[str, Any]] = []
    ev = p.events
    i = 0
    while i < len(ev):
        kind, val = ev[i]
        if kind == "a":
            href = val
            text = ev[i + 1][1] if i + 1 < len(ev) and ev[i + 1][0] == "aend" else ""
            i += 2
            # the next text block after the anchor is the blurb candidate
            blurb = ""
            j = i
            while j < len(ev):
                k2, v2 = ev[j]
                if k2 == "t":
                    if not _NOISE_TEXT.match(v2):
                        blurb = v2
                    break
                if k2 == "a":
                    break
                j += 1
            if href and text and not _NOISE_TEXT.match(text) and href.startswith(("http://", "https://")):
                items.append({"headline": text, "url": urlnorm.normalize_url(href, wrappers or None), "blurb": blurb})
            continue
        i += 1
    return items


_TEXT_LINK_RE = re.compile(r"https?://\S+")


def parse_text_digest(text: str, wrappers: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Plain-text digests: a headline line followed by a link line."""
    items: List[Dict[str, Any]] = []
    lines = [ln.strip() for ln in text.splitlines()]
    last_headline = ""
    for idx, ln in enumerate(lines):
        m = _TEXT_LINK_RE.search(ln)
        if m:
            url = urlnorm.normalize_url(m.group(0).rstrip(".,)>"), wrappers or None)
            headline = last_headline or ln[: m.start()].strip() or url
            blurb = ""
            for nxt in lines[idx + 1 : idx + 4]:
                if nxt and not _TEXT_LINK_RE.search(nxt) and not _NOISE_TEXT.match(nxt):
                    blurb = nxt
                    break
            items.append({"headline": headline, "url": url, "blurb": blurb})
            last_headline = ""
        elif ln and not _NOISE_TEXT.match(ln):
            last_headline = ln
    return items


def message_items(msg: EmailMessage, wrappers: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    html_part = msg.get_body(preferencelist=("html",))
    if html_part is not None:
        return parse_html_digest(html_part.get_content(), wrappers)
    text_part = msg.get_body(preferencelist=("plain",))
    if text_part is not None:
        return parse_text_digest(text_part.get_content(), wrappers)
    return []


def message_date(msg: EmailMessage) -> str:
    try:
        dt = email.utils.parsedate_to_datetime(msg.get("Date", ""))
        if dt is not None:
            return dt.date().isoformat()
    except (TypeError, ValueError):
        pass
    return datetime.utcnow().date().isoformat()


def _sender_matches(msg: EmailMessage, sender: str) -> bool:
    _, addr = email.utils.parseaddr(msg.get("From", ""))
    addr = addr.lower()
    s = sender.lower()
    if "@" in s:
        return addr == s
    return addr.endswith("@" + s) or addr.endswith("." + s)


def match_source(msg: EmailMessage, sources: List[MailboxSource]) -> Optional[MailboxSource]:
    subject = msg.get("Subject", "") or ""
    for src in sources:
        if not _sender_matches(msg, src.sender):
            continue
        if src.subject_filter and not re.search(src.subject_filter, subject):
            continue
        return src
    return None


def _host(url: str) -> str:
    try:
        return (urlsplit(url).hostname or "").lower()
    except ValueError:
        return ""


_YEAR_SEG = re.compile(r"/(19|20)\d{2}(/|$)")


def looks_like_article(url: str, path_filter: Optional[str] = None) -> bool:
    """Generic shape test for an article link, or the source's own
    regex. Digests carry account, index, and sponsor links on the
    publisher's own host; a year segment, an .html suffix, or a path
    at least four segments deep is what an article link looks like."""
    try:
        path = urlsplit(url).path or "/"
    except ValueError:
        return False
    if path_filter:
        return re.search(path_filter, path) is not None
    if _YEAR_SEG.search(path):
        return True
    if path.lower().endswith((".html", ".htm")):
        return True
    return len([s for s in path.split("/") if s]) >= 4


def _in_domains(url: str, pub: Optional[Publisher]) -> bool:
    if pub is None or not pub.domains:
        return True
    h = _host(url)
    return any(h == d or h.endswith("." + d) for d in pub.domains)


# --------------------------------------------------------------------
# The tick
# --------------------------------------------------------------------


def substrate_knows(query: Optional[SubstrateQuery], url: str) -> bool:
    if query is None:
        return False
    try:
        res = query("entity.search", {"query": url, "predicate": "source.article", "limit": 1})
    except Exception:  # noqa: BLE001 - absent or refusing host means ledger only
        return False
    hits = res.get("results") if isinstance(res, dict) else None
    return bool(hits)


def items_for_message(
    msg: EmailMessage,
    src: MailboxSource,
    cfg: CourierConfig,
    sources: Sources,
    warnings: List[str],
) -> List[Item]:
    pub = sources.get(src.publisher)
    if pub is None:
        warnings.append(f"publisher {src.publisher!r} is not in sources.toml; filing as pointer / session")
        intake, fetch, pub_name = "pointer", "session", src.publisher
    else:
        intake, fetch, pub_name = pub.intake, pub.fetch, pub.name
    day = message_date(msg)
    out: List[Item] = []
    seen_urls: set = set()
    for raw in message_items(msg, cfg.wrappers):
        url = raw["url"]
        if url in seen_urls or not _in_domains(url, pub) or not looks_like_article(url, src.path_filter):
            continue
        seen_urls.add(url)
        body = ""
        reported_by = None
        if intake == "clip" and raw.get("blurb"):
            body, reported_by = _attribution(raw["blurb"])
        out.append(
            Item(
                title=raw["headline"],
                url=url,
                publication=pub_name,
                published_at=day,
                intake=intake,
                fetch=fetch,
                body=body,
                reported_by=reported_by,
            )
        )
    return out


def mailbox_tick(
    cfg: CourierConfig,
    sources: Sources,
    ledger: Ledger,
    output: Output,
    imap_factory: ImapFactory = default_imap_factory,
    query: Optional[SubstrateQuery] = None,
) -> Dict[str, Any]:
    result: Dict[str, Any] = {
        "tick": "mailbox",
        "dry_run": output.dry_run,
        "items_seen": 0,
        "files_written": 0,
        "files": [],
        "would_submit": [],
        "fetch_requests": [],
        "fetch_failures": 0,
        "skipped_seen": 0,
        "warnings": [],
        "last_error": None,
        "new_items": [],  # Item objects for the scheduled-fetch pass (not serialized)
    }
    mb = cfg.mailbox
    if mb is None:
        result["warnings"].append("no [mailbox] configured")
        return result
    try:
        client = imap_factory(mb)
    except Exception as e:  # noqa: BLE001
        result["last_error"] = f"mailbox connect: {e}"
        return result
    try:
        client.select(mb.folder)
        criteria = ["UNSEEN"] if mb.mark_seen else ["ALL"]
        typ, data = client.search(None, *criteria)
        nums = data[0].split() if typ == "OK" and data and data[0] else []
        refs_by_pub: Dict[Tuple[str, str], List[Tuple[str, str]]] = {}
        for num in nums:
            num_s = num.decode() if isinstance(num, bytes) else str(num)
            typ, parts = client.fetch(num_s, "(RFC822)")
            raw = b""
            for part in parts:
                if isinstance(part, tuple) and len(part) >= 2 and isinstance(part[1], (bytes, bytearray)):
                    raw = bytes(part[1])
                    break
            if not raw:
                continue
            msg = email.message_from_bytes(raw, policy=email.policy.default)
            src = match_source(msg, mb.sources)
            if src is None:
                continue
            items = items_for_message(msg, src, cfg, sources, result["warnings"])
            result["items_seen"] += len(items)
            all_written = True
            for item in items:
                if ledger.has(item.url) or substrate_knows(query, item.url):
                    result["skipped_seen"] += 1
                    continue
                try:
                    path = output.write_item(item)
                except OSError as e:
                    all_written = False
                    result["warnings"].append(f"write failed for {item.url}: {e}")
                    continue
                result["files"].append(path)
                result["new_items"].append(item)
                refs_by_pub.setdefault((item.publication, item.published_at), []).append((item.basename, item.title))
                if not output.dry_run:
                    ledger.add(item.url, item.published_at)
            if all_written and mb.mark_seen and not output.dry_run:
                client.store(num_s, "+FLAGS", "\\Seen")
        for (pub_name, day), refs in refs_by_pub.items():
            path = output.write_or_append_digest(pub_name, day, refs, cfg.digest_title_prefix)
            if path and path not in result["files"]:
                result["files"].append(path)
        if not output.dry_run:
            ledger.save()
    except Exception as e:  # noqa: BLE001
        result["last_error"] = f"mailbox tick: {e}"
    finally:
        try:
            client.logout()
        except Exception:  # noqa: BLE001
            pass
    result["files_written"] = 0 if output.dry_run else len(result["files"])
    if output.dry_run:
        result["would_submit"] = list(result["files"])
    return result
