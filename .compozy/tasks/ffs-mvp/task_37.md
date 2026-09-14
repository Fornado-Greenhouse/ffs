---
status: completed
title: Agent memory convention — adopt okf-agent-memory practices (ADR-027)
type: backend
complexity: medium
dependencies:
  - task_16
  - task_23
---

# Task 37: Agent memory convention — adopt okf-agent-memory practices (ADR-027)

## Overview
FFS is a memory substrate, but nothing told an MCP agent how to *behave* when using it as memory. Read from an agent's seat, the six MVP tools had three gaps: `ffs_author_atom` reads like "save" when its result is a quarantined proposal the owner has not yet accepted; there was no search tool, so "search before write" was impossible over MCP and duplicate entities were the default outcome; and there was no listing tool, so an agent orienting itself had to guess projection paths or dump entity queries. Meanwhile [okf-memory/okf-agent-memory](https://github.com/okf-memory/okf-agent-memory) published a domain-neutral Agent Memory Convention (search before write, no blanket scans, generated ≠ verified, preserve uncertainty, human override, review after task, never claim persistence, `constraint`/`hold`/`context` governance bound to code via `code_refs`). Nearly all of it is already enforced by FFS primitives — the quarantine is the generated/verified boundary, provenance is `sources`, supersession is history, capabilities are governance-as-data. ADR-027 records the decision to adopt the convention's *behavior* (not the OKF file format), add the two tools that make the behavior possible over MCP, ship an instructional skill, and dogfood the practice in this repo with an ADR governance index and a CLAUDE.md knowledge-discipline section.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST publish `docs/agent-memory/CONVENTION.md`: the FFS Agent Memory Convention, restating the okf-agent-memory minimal agent contract in FFS terms with an explicit OKF-concept → FFS-primitive mapping table (generated = quarantined proposal; verified = owner-accepted signed atom; sources = provenance; status/stale_after = bitemporal window + supersession; type = predicate; index = projection listing; governance = capability atoms). It MUST state that the OKF file format is not adopted for the substrate and why.
- MUST add two MCP tools to `crates/ffs-mcp`: `ffs_search` (→ daemon `entity.search`; `query` required, `limit` optional with default 10 and ceiling 50) and `ffs_list_path` (→ daemon `path.list`; `path` required, `page` optional). Both forward to existing, already capability-filtered RPC methods; no daemon changes.
- MUST reword the `ffs_author_atom` description so the result is described as a proposal with a `submission_id`, nothing persisted until the owner accepts it, reported as "proposed" never "saved", with search-first and qualify-inferences guidance. The signature MUST NOT change.
- MUST document the catalog order (`ffs_query`, `ffs_search`, `ffs_list_path`, `ffs_render_projection`, `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`, `ffs_audit_query`) and update every "six tools" statement in README.md, ARCHITECTURE.md (stability commitments), the ffs-mcp crate docs, and the integration test to eight.
- MUST ship `docs/agent-memory/skill/ffs-memory/SKILL.md`, a `SKILL.md`-shaped instructional bundle for MCP-aware agent hosts: read-before-write loop, proposal semantics, end-of-task review. It MUST NOT carry an `entry_point` and MUST NOT be installed under `$FFS_DATA_DIR/skills/`.
- MUST add `.compozy/tasks/ffs-mvp/adrs/README.md`: a governance index with tier definitions, an explicit active-holds section, a path → governing-ADRs → tier table covering every crate, `skills/`, `obsidian-plugin/`, installer/signing assets, and `docs/agent-memory/`, plus a one-line description and status for every ADR (001–027). Every ADR number in the table MUST be verified against the ADR's content.
- MUST add a `## Knowledge discipline (ADR-027)` section to `CLAUDE.md`: repo memory = ARCHITECTURE.md + ADRs + task files; read governing ADRs before substantial work (tiers respected); progressive disclosure; end-of-task knowledge review checklist; persistence honesty.
- MUST record ADR-027 with the standard shape (Status, Date, Context, Decision, Alternatives with why-rejected, Consequences, Risks, Implementation Notes, References).
- SHOULD link the convention from README.md § MCP agents and from ARCHITECTURE.md § Where to find more.
</requirements>

## Subtasks
- [x] 37.1 ADR-027 written with the four-part decision (convention doc, two tools + rewording, instructional skill, repo dogfooding) and three alternatives (OKF `knowledge/` bundle wholesale; BM25 full-text now; leave it to agent prompts).
- [x] 37.2 `docs/agent-memory/CONVENTION.md` with the OKF → FFS mapping table and open questions (full-text search, `status: "proposed"` in the `ffs_author_atom` payload).
- [x] 37.3 `ffs_search` + `ffs_list_path` translators, `SEARCH_DEFAULT_LIMIT` / `SEARCH_MAX_LIMIT`, `ffs_author_atom` rewording; unit tests for translation, limit defaulting/clamping, missing-argument errors; catalog test asserts eight tools in order.
- [x] 37.4 Integration test: `ffs_search` end to end through the in-process dispatcher returns capability-filtered hits; `tools/list` returns eight.
- [x] 37.5 `docs/agent-memory/skill/ffs-memory/SKILL.md` (no `entry_point`; not a daemon-hosted bundle).
- [x] 37.6 README.md § MCP agents and status table, ARCHITECTURE.md § Stability commitments and § Where to find more updated to eight tools + convention link.
- [x] 37.7 `.compozy/tasks/ffs-mvp/adrs/README.md` governance index (tiers, "No active holds.", path table, all-ADR table).
- [x] 37.8 `CLAUDE.md` § Knowledge discipline (ADR-027).
- [x] 37.9 `_tasks.md` row; `compozy tasks validate --name ffs-mvp` reports all tasks valid.

## Implementation Details
No daemon changes. `entity.search` (task_19) already matches `display_name` / `title` case-insensitively and capability-filters each hit before returning `{entity, predicate, display_name}`; `path.list` (task_07) renders the listing form of a projection path. The MCP layer stays a thin pass-through per ADR-013: two new translators in `crates/ffs-mcp/src/tools.rs` forward to those methods and shape the response with the existing `forward` helper, so capability denials still surface as tool-level `isError` results with `details.kind = capability_denied`.

The behavioral content lives in three places with deliberately different lifetimes: tool descriptions (tested, ship with the binary), `CONVENTION.md` (the reasoning, versioned prose), and the skill (the workflow an agent host loads). The governance index is hand-maintained; CLAUDE.md's knowledge-review checklist is what keeps it current.

### Relevant Files
- `crates/ffs-mcp/src/tools.rs` — two new tools, constants, reworded `ffs_author_atom`, unit tests.
- `crates/ffs-mcp/src/lib.rs` — crate docs and `tools_list` test updated to eight.
- `crates/ffs-mcp/tests/mcp_integration.rs` — eight-tool assertion, `ffs_search` end-to-end test.
- `docs/agent-memory/CONVENTION.md`, `docs/agent-memory/skill/ffs-memory/SKILL.md` — new.
- `.compozy/tasks/ffs-mvp/adrs/adr-027.md`, `.compozy/tasks/ffs-mvp/adrs/README.md` — new.
- `CLAUDE.md`, `README.md`, `ARCHITECTURE.md` — sections updated.

### Dependent Files
- `docs/onboarding/first-use-guide.md` — no change required; MCP setup is documented in README.md.
- `crates/ffs-daemon/src/dispatch.rs` — read-only reference for `entity.search` and `path.list` param shapes.

### Related ADRs
- [ADR-027: Adopt the OKF Agent Memory Convention for agents using FFS as persistent memory](adrs/adr-027.md) — this task's foundation.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — the six-tool set being extended; capability checks stay daemon-side.
- [ADR-009: Claw integration via OpenClaw or Hermes pattern](adrs/adr-009.md) — the `SKILL.md` shape the instructional skill borrows.
- [ADR-008: Speak MCP and A2A at boundaries](adrs/adr-008.md) — why the convention is delivered over MCP tool descriptions.

## Deliverables
- ADR-027 and the ADR governance index **(REQUIRED)**.
- `docs/agent-memory/CONVENTION.md` and the `ffs-memory` skill **(REQUIRED)**.
- `ffs_search` + `ffs_list_path` with unit and integration tests; `ffs_author_atom` reworded **(REQUIRED)**.
- README.md, ARCHITECTURE.md, CLAUDE.md updates.
- `_tasks.md` row; `compozy tasks validate` clean.

## Tests
- Unit tests (`crates/ffs-mcp/src/tools.rs`, `crates/ffs-mcp/src/lib.rs`):
  - [x] `catalog_contains_the_eight_tools` — catalog is exactly the eight names in the ADR-027 order.
  - [x] `tools_list_returns_the_eight_tools` — `tools/list` advertises eight tools including `ffs_search` and `ffs_list_path`.
  - [x] `ffs_search_translates_to_entity_search_with_query_and_limit` — `query` and `limit` pass through to `entity.search`.
  - [x] `ffs_search_limit_defaults_to_ten` — omitted `limit` becomes `SEARCH_DEFAULT_LIMIT`.
  - [x] `ffs_search_limit_clamps_at_fifty` — `limit` above `SEARCH_MAX_LIMIT` is clamped.
  - [x] `ffs_search_missing_query_errors_without_calling_daemon` — no daemon call, `isError: true`.
  - [x] `ffs_list_path_translates_to_path_list_with_path_and_page` — `path` and optional `page` pass through to `path.list`.
  - [x] `ffs_author_atom_description_is_honest_about_persistence` — description contains "proposal" / "proposed" and "not" persisted wording.
- Integration tests (`crates/ffs-mcp/tests/mcp_integration.rs`):
  - [x] `tools_list_returns_eight_tools_end_to_end` — over the line-delimited transport.
  - [x] `ffs_search_returns_hit_for_inserted_contact_end_to_end` — inserted contact is found by substring; hit carries `entity`, `predicate`, `display_name`.
- Repo checks:
  - [x] `cargo nextest run --workspace --all-features` green; `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
  - [x] `compozy tasks validate --name ffs-mvp` reports all tasks valid.
- All tests must pass

## Success Criteria
- An MCP agent can complete the read-before-write loop over FFS using only the eight tools: list a path, search by name, inspect a hit, propose content, and report it as pending owner review.
- No "six tools" statement remains in README.md, ARCHITECTURE.md, crate docs, or tests.
- Every path row in the governance index cites only ADRs whose content actually governs that path; "No active holds." is explicit.
- The convention, the skill, and the tool descriptions agree on proposal semantics for `ffs_author_atom`.
