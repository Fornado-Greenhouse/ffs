---
name: scribe
kind: scribe
entry_point: extraction.py
python: python3
timeout_ms: 180000
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

## Engines (task_36, ADR-026)

Extraction runs behind an `ExtractionEngine` seam (`engine.py`):

- `heuristic` (`heuristic.py`): the regex and frontmatter extractors in
  `extraction.py`. Always available, fully offline, zero configuration.
  The default and the fallback.
- `llm` (`llm.py`): a stdlib-`urllib` client to a configurable backend
  (Ollama chat API or the Anthropic Messages API). Strictly opt-in;
  nothing leaves the machine until the user sets it.

Environment (arrives through the daemon and skills host, never from a
config file inside the bundle):

| Variable | Default | Meaning |
|---|---|---|
| `FFS_SCRIBE_ENGINE` | `heuristic` | `heuristic` or `llm`; unknown values fall back to heuristic |
| `FFS_SCRIBE_LLM_URL` | `http://localhost:11434` | Backend URL; `api.anthropic.com` selects the Anthropic adapter |
| `FFS_SCRIBE_LLM_MODEL` | backend default | Model name |
| `FFS_SCRIBE_ANTHROPIC_KEY` | unset | API key for the Anthropic adapter (or the OS keychain) |
| `FFS_DATA_DIR` | `~/.ffs` | Where `config/predicates/*.toml` is read from |
| `FFS_SCRIBE_CORPUS_DIR` | unset | External golden corpus for the scorer (real articles never enter git) |

Predicate specs are read from `$FFS_DATA_DIR/config/predicates/*.toml`
by `registry.py` (stdlib `tomllib`); when none are on disk the registry
falls back to the host's `predicate.inspect` query. Registering a new
spec extends extraction with no scribe code change. Claims are checked
against the spec's `claim_schema` by `validate.py`.

A `predicate:` frontmatter key hints the target: a registered predicate
narrows extraction to it; `source.article` or any unregistered predicate
lands as a `note` that keeps the title, url, and summary (readable in the
vault before task_38 registers those predicates). Every proposal carries
top-level `engine` and `model` (empty for heuristic) so the review UI can
show what produced it.

Heuristic hygiene (the "Bones Occupation" fixes): the filename is a name
candidate, `Label: value` cards are parsed field by field instead of the
bigram scan, and field-label words never become a name.

## Timeout

`timeout_ms` is 180000 (three minutes). The heuristic engine answers in
milliseconds; the budget exists for the opt-in `llm` engine, where a
local model can take one to two minutes per note (spike task_42). The
host kills and restarts the skill past this budget, and the submission
is marked failed rather than left pending.

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
      "rationale": "extracted display_name from frontmatter",
      "engine": "heuristic",
      "model": ""
    }
  ],
  "warnings": ["malformed YAML at line 4"]
}
```

## ADRs

- ADR-009 — Claw integration via OpenClaw or Hermes pattern (skill packaging shape).
- ADR-011 — Path library starts at three (contacts, people, notes).
