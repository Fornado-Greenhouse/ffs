<!-- omit in toc -->
# FFS — Foley File System

![Status](https://img.shields.io/badge/status-pre--alpha-orange)
![License](https://img.shields.io/badge/license-Blue_Oak_1.0.0-blue.svg)
[![CI](https://github.com/Fornado-Greenhouse/ffs/actions/workflows/ci.yml/badge.svg)](https://github.com/Fornado-Greenhouse/ffs/actions/workflows/ci.yml)

> **Records-shaped, not file-shaped.** A substrate where what your knowledge says, who said it, and who's allowed to see it are all first-class.

> ⚠️ **Pre-alpha.** Not yet installable. The substrate's atom envelope, predicate-spec loader, and Cargo workspace are in. The daemon, CLI, plugin, and federation transport are not. See [Status](#status) for what's shipped and [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design.

---

## Why this exists

The SaaS-and-files era is good at neither of two things knowledge workers need: **ownership of their data**, and **sub-record access control**. CRMs, file shares, knowledge bases, contact managers each handle one or two of these concerns — and only inside walls the user does not control.

FFS is a substrate that gives you both at the layer below applications. It treats knowledge as **records**, not files. Each record is a signed claim about something — a contact, a decision, a note — with provenance, bitemporal history, and tier-based classification. Records are content-addressed and capability-gated, so two FFS instances can federate bilaterally: "I can see existence and work email for your contacts; you can see full notes for mine; either of us can revoke at any time."

In the iCloud Notes / Datomic lineage, not the Dropbox / OneDrive lineage.

## What FFS gives you

**Ownership.** FFS runs on your hardware, encrypted at rest with keys held by your OS keychain. There is no provider in the middle. There is no terms-of-service that changes. A `git clone` of your substrate produces a working FFS instance — your knowledge is portable in a way SaaS data never is.

**Sub-record control.** A contact decomposes into atoms — existence, work email, personal email, notes — each classified independently. Tell one peer "you can see existence and work email"; tell another "full access"; revoke either at any time. Capabilities are themselves signed records, queryable and auditable.

**Durable history.** Every claim is signed, timestamped, and immutable. Change happens through supersession; nothing is overwritten. You can ask "what did I know about Sarah on April 15th, before she changed teams?" and get a real answer.

## A glimpse

The Sarah Chen contact below is what you'd see in `~/.ffs/contacts/by-name/S/Sarah_Chen.md`. Open it in Obsidian, vim, Notepad, VS Code — whatever. Edit it. The substrate absorbs the change.

```markdown
---
display_name: Sarah Chen
work_email: sarah@acme.com
tier: introducible
---

## Notes
- Met at distributed-systems conference, June 2025
- Interested in records-shaped knowledge tools
```

The file is a *projection* — rendered on demand from the underlying atoms. Here's one of the atoms behind it (the work-email claim, signed by Wes):

```json
{
  "v": 1,
  "entity": "z6MkrSarahChen001...",
  "predicate": "contact.person.work_email",
  "claim": {"value": "sarah@acme.com"},
  "author": "z6MkhaXgWesIdentity...",
  "valid_from": "2026-05-09T00:00:00Z",
  "valid_to": null,
  "tx_time": "2026-05-09T14:23:11.421Z",
  "classification": "work_email",
  "supersedes": null,
  "provenance": [{"kind": "ingest_file", "uri": "file:///.ffs/ingest/notes-2025-06-12.md", "hash": "z58s..."}],
  "signature": "z58sQv...Ed25519signature..."
}
```

Sarah's full contact is the head of supersession chains for several atoms like this — display name, work email, personal email, notes — each separately classifiable and shareable. The markdown file you see in the editor is the rendered view; the atoms are canonical.

## Status

| Component | Status | What it does |
|---|---|---|
| Cargo workspace + cross-platform CI | ✅ shipped | Seven Rust crates, Linux/macOS/Windows build matrix |
| Atom envelope (signing, hashing, JCS) | ✅ shipped | The interoperability contract: Ed25519 + BLAKE3 + canonical JSON |
| Predicate spec loader (TOML + JSON Schema + hot-reload) | ✅ shipped | The substrate's vocabulary, parsed and validated at load |
| SQLite atom store with SQLCipher | ⏳ planned | Per-substrate encrypted storage with bitemporal indexes |
| Capability evaluator | ⏳ planned | Action × scope × time → Allow/Deny |
| Projection renderer (Tera templates) | ⏳ planned | Atoms → markdown for any editor |
| Daemon JSON-RPC dispatcher (UDS / named pipe) | ⏳ planned | Long-running per-user process |
| `ffs` CLI + `ffs://` URL resolver | ⏳ planned | Static binary for shell pipelines |
| Filesystem watcher + fast-path classifier | ⏳ planned | Sub-200ms editor edits become atoms |
| Skills host (scribe, librarian, auditor) | ⏳ planned | Python agents under daemon supervision |
| Federation transport (mTLS pull-based) | ⏳ planned | Bilateral peer-to-peer, fingerprint-pinned |
| MCP server (six MVP tools) | ⏳ planned | AARM-conformant boundary for AI agents |
| Obsidian plugin | ⏳ planned | The end-user surface |

The full 23-task plan in dependency order: [`.compozy/tasks/ffs-mvp/_tasks.md`](.compozy/tasks/ffs-mvp/_tasks.md).

## Architecture, briefly

```
        editors / CLI / MCP agents
                   │
                   ▼
            ┌──────────────┐         mTLS HTTPS
            │  ffs-daemon  │◀───────────────────────▶ peer FFS
            │  (Rust)      │
            └──────┬───────┘
                   ▼
            ┌──────────────┐
            │ store.db     │  (SQLite + SQLCipher)
            └──────────────┘
```

Seven Rust crates plus a Python skill bundle plus a TypeScript Obsidian plugin. The atom envelope (canonical JSON, BLAKE3 hash, Ed25519 signature) is the single substrate-interop contract; everything else composes around it.

Full design: [`ARCHITECTURE.md`](ARCHITECTURE.md). Decision-by-decision history: [`adrs/`](.compozy/tasks/ffs-mvp/adrs/) (21 ADRs). Product spec: [`_prd.md`](.compozy/tasks/ffs-mvp/_prd.md). Implementation spec: [`_techspec.md`](.compozy/tasks/ffs-mvp/_techspec.md).

## MCP agents

FFS ships `ffs-mcp`, a Model Context Protocol server that any MCP-aware agent (Claude Code, ChatGPT desktop, framework-agnostic agents) can spawn as a subprocess. It exposes six tools, each translating to a daemon JSON-RPC call with capability checks at the daemon boundary:

| Tool | Purpose |
|---|---|
| `ffs_query` | List atoms about an entity (capability-filtered) |
| `ffs_render_projection` | Render a projection path to markdown |
| `ffs_resolve_url` | Resolve `ffs://` URLs (atom / entity / path) |
| `ffs_author_atom` | Submit content for scribing into the ingest quarantine |
| `ffs_inspect_predicate` | Return a predicate spec (schema + reverse-map rules) |
| `ffs_audit_query` | Return recent auditor.daily_summary atoms |

Sample Claude Code `mcpServers` block:

```jsonc
{
  "mcpServers": {
    "ffs": {
      "command": "ffs-mcp",
      "args": [],
      "env": {
        "FFS_DAEMON_SOCKET": "/Users/you/.ffs/run/ffs.sock",
        "FFS_AGENT_KEY": "/Users/you/.ffs/keys/claude.ed25519"
      }
    }
  }
}
```

Per ADR-013, capability checks live on the daemon side; the MCP server is a thin pass-through. A capability denial comes back as an MCP tool-level error (`isError: true`) with the typed reason in `details.kind`, not a JSON-RPC error — so the agent surfaces "the substrate refused this" cleanly without treating it as a transport break.

## Philosophy: substrate, not application

FFS doesn't tell you what your knowledge is *for*. It gives you the primitives — atoms, capabilities, federation, projection — and lets applications above the substrate inherit them. The first application planned on top is **Folodex**, a contact-graph tool that exercises FFS for the relationship-holder use case (salespeople, journalists, founders, community organizers who own networks they currently don't control).

But Folodex is one application. The same primitives handle general knowledge capture, AI-agent output absorption, and team collaboration. That breadth is intentional: tools that solve one of these concerns well already exist; what's missing is the substrate underneath that handles all of them coherently.

**FFS aligns with [AARM](https://aarm.dev)** — Autonomous Action Runtime Management — an open specification for securing AI-driven actions at runtime. The mapping is by construction, not retrofit: capability-as-data is AARM's policy engine; atoms are the context accumulator; the proposal-quarantine flow is the approval service; signed-and-multihashed atoms are the receipt generator. See [`ARCHITECTURE.md` § Security model](ARCHITECTURE.md#security-model-aarm-conformant) for the component-by-component breakdown.

## Follow along

This project is being built in the open. There is nothing to install yet; the most useful things you can do today are:

- ⭐ Star the repo — signals interest and helps surface it.
- 👀 Watch for releases — first usable build is what we're heading toward.
- 📖 Read [`ARCHITECTURE.md`](ARCHITECTURE.md) and the [PRD](.compozy/tasks/ffs-mvp/_prd.md). They're the substantive design.
- 💬 Open a [Discussion](https://github.com/Fornado-Greenhouse/ffs/discussions) — especially with use-case signal: *"here's the network I'd want to share — does FFS fit?"*
- 🐛 File an [Issue](https://github.com/Fornado-Greenhouse/ffs/issues) for design feedback, technical concerns, or "I'd like to use this for X."

`CONTRIBUTING.md` and `SECURITY.md` are forthcoming. For security concerns in the meantime, contact the maintainer directly. Operational norms for working in this repo (test runner, shell discipline, ADR numbering) are documented in [`CLAUDE.md`](CLAUDE.md).

## License and credit

FFS is licensed under the [Blue Oak Model License 1.0.0](LICENSE) — a modern, plain-English permissive license with an explicit patent grant. You may use, copy, modify, and redistribute it freely; please retain the license notice.

Project lead: **Wes Foley**.

Built with the records-not-files lineage of [Datomic](https://www.datomic.com/), [iCloud Notes](https://support.apple.com/en-us/HT205773), and the local-first community's push toward user-owned knowledge. The substrate's storage layer rests on [SQLite](https://www.sqlite.org/), and we carry forward its blessing:

> *May you do good and not evil.*
> *May you find forgiveness for yourself and forgive others.*
> *May you share freely, never taking more than you give.*
