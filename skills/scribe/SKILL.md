---
name: scribe
kind: scribe
entry_point: extraction.py
python: python3
timeout_ms: 15000
---

# Scribe

The absorption agent. Reads any markdown blob — typed in Obsidian,
written by an AI agent, dropped in `~/.ffs/ingest/` by any tool —
infers structured claims about contacts, people, and notes, and
returns proposed atoms with provenance pointing back to the source.

Proposals land in the ingest quarantine. The user reviews them in the
daily-health-summary; accepted proposals become signed atoms.

The scribe tolerates malformed input. Anything it can't classify
becomes a `note` proposal so nothing is lost; structural ambiguities
(conflicting frontmatter, multiple plausible entities in one file)
surface as `note` proposals with a `parse-warning` rationale.

## Wire shape

Input from the host (`invoke.input`):

```json
{
  "source_uri": "file:///home/user/.ffs/ingest/2026-05-26-meeting-notes.md",
  "content": "---\nname: Sara Chen\n---\n..."
}
```

Output back to the host (`result.output`):

```json
{
  "proposals": [
    {
      "predicate": "contact.person",
      "claim": { "display_name": "Sara Chen", "notes": ["..."] },
      "provenance": [
        { "kind": "ingest", "uri": "file://.../2026...md", "hash": "..." }
      ],
      "rationale": "extracted display_name from frontmatter"
    }
  ],
  "warnings": ["malformed YAML at line 4"]
}
```

## ADRs

- ADR-009 — Claw integration via OpenClaw or Hermes pattern (skill packaging shape).
- ADR-011 — Path library starts at three (contacts, people, notes).
