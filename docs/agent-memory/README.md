# FFS as agent memory

This directory holds the behavioral contract for AI agents that use an FFS substrate as persistent memory, and an installable skill that teaches it.

| File | What it is |
|---|---|
| [`CONVENTION.md`](CONVENTION.md) | The FFS Agent Memory Convention v0.1: when and how an agent should read from and propose to the substrate. Adopted by ADR-027. |
| [`skill/ffs-memory/SKILL.md`](skill/ffs-memory/SKILL.md) | An agent skill (SKILL.md shape) that condenses the convention into a tool reference, a workflow, and an end-of-task review checklist. |

The convention adapts the [OKF Agent Memory Convention](https://github.com/okf-memory/okf-agent-memory) onto FFS. The OKF file format is not adopted; FFS atoms already carry provenance, signatures, classification, and bitemporal history. The behavioral rules are: search before write, progressive disclosure, honest provenance, and never claiming a proposal was saved.

## Installing the skill

The `ffs-memory` skill is instructional only. It has no `entry_point` and no `definition.atom.json`, so it is **not** a daemon-hosted skill bundle. Do not copy it under `~/.ffs/skills/`; the skills host would reject it. Install it where your agent loads skills from:

```sh
# Claude Code (per-project)
mkdir -p .claude/skills
cp -R docs/agent-memory/skill/ffs-memory .claude/skills/ffs-memory

# Claude Code (per-user)
mkdir -p ~/.claude/skills
cp -R docs/agent-memory/skill/ffs-memory ~/.claude/skills/ffs-memory

# Claw hosts (OpenClaw, Hermes) and other SKILL.md-aware agents
mkdir -p .agents/skills
cp -R docs/agent-memory/skill/ffs-memory .agents/skills/ffs-memory
```

A symlink works as well as a copy if your host follows symlinks.

## Wiring the MCP server

The skill assumes the eight `ffs-mcp` tools are available. Register the server with your agent. For Claude Code, the `mcpServers` block is:

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

`FFS_DAEMON_SOCKET` defaults to `$FFS_DATA_DIR/run/ffs.sock` (or `~/.ffs/run/ffs.sock`) when unset. `FFS_AGENT_IDENTITY` sets the identity URI stamped onto proposals when the agent supplies no `source_uri`; it defaults to `mcp-agent:local`. See the `ffs-mcp` binary's docblock in `crates/ffs-mcp/src/main.rs` for the full environment surface.

Without the MCP server, the skill falls back to the `ffs` CLI (`ffs ls`, `ffs cat`, `ffs get`, `ffs predicate inspect`) and to dropping Markdown into `~/.ffs/ingest/`. Search and audit queries have no CLI equivalent in the MVP.

## The eight tools

| Tool | Purpose |
|---|---|
| `ffs_search` | Find entities by display name or note title. Lightweight hits, capability-filtered. |
| `ffs_list_path` | Enumerate a projection listing such as `contacts/by-name/S/` or `notes/recent/`. |
| `ffs_render_projection` | Render one projection path to Markdown. |
| `ffs_query` | List the atoms behind an entity, optionally by predicate and `as_of`. |
| `ffs_resolve_url` | Resolve an `ffs://` URL to an atom, entity, or projection. |
| `ffs_inspect_predicate` | Return a predicate spec: claim schema, rendering, reverse-map rules. |
| `ffs_author_atom` | Submit Markdown to the ingest quarantine. Returns a `submission_id` for a proposal. |
| `ffs_audit_query` | Return recent auditor daily-summary atoms. |

Capability checks run at the daemon on every call (ADR-013). A denial is returned as a tool-level error, not a transport failure, and the convention says the agent stops there.

## Related

- [`ARCHITECTURE.md`](../../ARCHITECTURE.md) for the substrate's invariants and security model.
- [`docs/onboarding/technical-friend-checklist.md`](../onboarding/technical-friend-checklist.md) for installing the daemon and MCP server.
- ADR-013 (MCP server in MVP), ADR-026 (scribe engines and provenance), ADR-027 (this convention).
