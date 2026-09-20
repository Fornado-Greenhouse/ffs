"""The article ingest contract, rendered (task_40).

One markdown file per item; keys in a fixed order so the bundle, the
agent-hosted skill, the convention, and the scribe agree byte for
byte. The courier never writes an entity id (ADR-030).
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

import urlnorm  # skills/_lib

MENTION_SEP = " — "  # the contract's literal grammar: spaced em dash


@dataclass
class Item:
    title: str
    url: str  # normalized
    publication: str
    published_at: str  # YYYY-MM-DD
    intake: str  # pointer | clip | feed | morning_read
    fetch: str  # off | session | scheduled
    body: str = ""
    byline: Optional[str] = None
    tags: List[str] = field(default_factory=list)
    reported_by: Optional[str] = None
    content_hash: Optional[str] = None
    mentions: List[Tuple[str, str]] = field(default_factory=list)  # (name, context)
    events: List[Tuple[str, str, List[Tuple[str, str]]]] = field(default_factory=list)  # (kind, desc, [(name, role)])

    @property
    def basename(self) -> str:
        return urlnorm.article_basename(self.publication, self.published_at, self.title)


def _yaml_scalar(value: str) -> str:
    """Quote when YAML would otherwise misread the value."""
    v = value.replace("\n", " ").strip()
    if v == "":
        return '""'
    needs = (
        v[0] in "[]{}&*!|>'\"%@`#-?:," or ": " in v or " #" in v or v.endswith(":")
        or v.lower() in ("true", "false", "null", "yes", "no", "~")
        or re.fullmatch(r"[-+]?\d[\d_]*(\.\d+)?", v) is not None
    )
    if needs:
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    return v


def _flow_list(values: List[str]) -> str:
    return "[" + ", ".join(_yaml_scalar(v) for v in values) + "]"


def render_article(item: Item) -> str:
    lines = ["---", "predicate: source.article", f"title: {_yaml_scalar(item.title)}", f"url: {_yaml_scalar(item.url)}"]
    lines.append(f"publication: {_yaml_scalar(item.publication)}")
    lines.append(f"published_at: {_yaml_scalar(item.published_at)}")
    if item.byline:
        lines.append(f"byline: {_yaml_scalar(item.byline)}")
    if item.tags:
        lines.append(f"tags: {_flow_list(item.tags)}")
    lines.append(f"intake: {item.intake}")
    if item.reported_by:
        lines.append(f"reported_by: {_yaml_scalar(item.reported_by)}")
    if item.content_hash:
        lines.append(f"content_hash: {_yaml_scalar(item.content_hash)}")
    lines.append(f"fetch: {item.fetch}")
    lines.append("---")
    lines.append("")
    body = item.body.strip()
    if body:
        lines.append(body)
        lines.append("")
    if item.mentions:
        lines.append("## Mentions")
        for name, context in item.mentions:
            lines.append(f"- {name}{MENTION_SEP}{context}")
        lines.append("")
    if item.events:
        lines.append("## Events")
        for kind, desc, parts in item.events:
            who = ", ".join(f"{n} ({r})" for n, r in parts)
            lines.append(f"- {kind}: {desc} | {who}" if who else f"- {kind}: {desc}")
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"


def digest_basename(publication: str, day: str) -> str:
    return f"{urlnorm.slug(publication, 40)}-digest-{day[:10]}"


def render_digest(publication: str, day: str, refs: List[Tuple[str, str]], title_prefix: str = "") -> str:
    title = f"{title_prefix}{publication} digest {day[:10]}"
    lines = ["---", "predicate: note", f"title: {_yaml_scalar(title)}", "tags: [digest, courier]", "---", "", "Filed by the courier.", "", "## References"]
    for basename, display in refs:
        lines.append(f"- [[{basename}|{display}]]")
    return "\n".join(lines) + "\n"


_REF_RE = re.compile(r"^- \[\[([^\]|]+)\|([^\]]*)\]\]\s*$")


def append_to_digest(existing: str, refs: List[Tuple[str, str]]) -> str:
    """Add new reference bullets to an existing digest, skipping any
    basename already listed. Returns the new text."""
    present = set()
    for line in existing.splitlines():
        m = _REF_RE.match(line)
        if m:
            present.add(m.group(1))
    out = existing.rstrip("\n") + "\n"
    if "## References" not in existing:
        out += "\n## References\n"
    for basename, display in refs:
        if basename not in present:
            out += f"- [[{basename}|{display}]]\n"
            present.add(basename)
    return out


def write_text(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


class Output:
    """Where a tick writes: the real ingest folder, or a dry-run scratch dir."""

    def __init__(self, ingest_dir: str, dry_run_dir: Optional[str] = None) -> None:
        self.ingest_dir = ingest_dir
        self.dry_run_dir = dry_run_dir
        self.written: List[str] = []

    @property
    def dry_run(self) -> bool:
        return self.dry_run_dir is not None

    @property
    def target(self) -> str:
        return self.dry_run_dir or self.ingest_dir

    def write_item(self, item: Item) -> str:
        path = os.path.join(self.target, item.basename + ".md")
        write_text(path, render_article(item))
        self.written.append(path)
        return path

    def write_or_append_digest(self, publication: str, day: str, refs: List[Tuple[str, str]], title_prefix: str = "") -> Optional[str]:
        if not refs:
            return None
        path = os.path.join(self.target, digest_basename(publication, day) + ".md")
        if os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                text = append_to_digest(f.read(), refs)
        else:
            text = render_digest(publication, day, refs, title_prefix)
        write_text(path, text)
        if path not in self.written:
            self.written.append(path)
        return path
