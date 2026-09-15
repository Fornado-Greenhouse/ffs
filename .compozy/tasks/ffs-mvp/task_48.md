---
status: pending
title: "Morning read: the owner-present reading session skill (ADR-035)"
type: docs
complexity: medium
dependencies:
  - task_37
  - task_40
  - task_46
---

# Task 48: Morning read: the owner-present reading session skill (ADR-035)

## Overview
ADR-035 splits the newspaper goal in two. The courier (task_40) is deterministic and files pointers and permitted feeds on a schedule. The morning read is the other half: an interactive session in which the owner is present, invokes the read, and works through the day's agenda with an assistant at human pace. The assistant opens one article at a time in the owner's own signed-in browser when the owner says so, summarizes it in the session, proposes the people, organizations, affiliations, and events worth remembering, and files only what the owner chooses, as the owner's own note. The article text is never stored. Because the owner decides live, filing from the read is the owner's accept, and each accept emits an ADR-034 attestation.

This task delivers that session as an instructional skill for Claude Code (with Claude in Chrome) and for claw hosts with a browser tool, in the same shape as `docs/agent-memory/skill/ffs-memory/SKILL.md`. It also settles whether the read needs any new MCP surface. The preference is none: `ffs_author_atom` submits, the existing `ingest.accept` RPC accepts, and task_46's attestation is emitted on accept. If the owner-present accept needs a flag the tools do not carry, that flag is the only addition.

The read doubles as the review. ADR-032's inbox file (or `ingest.list_pending` where the inbox is not yet built) is the agenda: courier pointers, CLTtoday extractions, permit and Council items, ambiguous resolutions, and role changes are walked in the same sitting.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST write `docs/agent-memory/skill/ffs-morning-read/SKILL.md` with YAML frontmatter (`name: ffs-morning-read`, a `description` that triggers on "morning read", "read the paper with me", "what's in today's digest", and never on an unattended or scheduled request) and a body that states ADR-035's rules as refusals before any workflow: one article at a time on the owner's cue; no prefetching, background, scheduled, or bulk opening; no storing of article bodies anywhere; summaries spoken, only chosen notes filed; inferences marked; persistence claims limited to what the tools returned.
- MUST describe the session workflow: (1) open the day's agenda from ADR-032's `inbox/<date>.md` when it exists, else from `ingest.list_pending` via the daemon, grouping items as pointers to read, extractions to confirm, ambiguous resolutions, and role changes; (2) walk items in order and, for a pointer, wait for the owner's cue, then open exactly that URL in the owner's browser session (name the capability, "open one URL in the owner's session and read its page text", not a vendor tool) and summarize aloud; (3) propose entities and affiliations with `display` and `context` in the ADR-031 shapes, and say which are stated in the article and which are inferred; (4) file the owner's chosen notes via `ffs_author_atom` with `source_uri` set to the article url and content that carries a `provenance: morning_read` marker the scribe preserves (see the provenance requirement below); (5) where the owner says "file it", accept in-session through the existing accept path so the atom lands and task_46's attestation is emitted with basis `owner_knowledge` or `re_read_same_source`; (6) handle "these are different people", "same person as", and merges through the task_39 actions; (7) refuse bulk requests with the reason; (8) end with the day's digest note (the `[[target|display]]` list of what was read and filed) and a two-line "what we filed" recap that names only what the tools confirmed as filed, proposed, or attested.
- MUST define the provenance of a read-filed atom: `kind: morning_read`, `uri` = the article url, `actor` = the assistant's identity from the MCP server's configured identity, `owner_present: true`, plus the session id. The scribe (`crates/ffs-daemon/src/scribe.rs`) and the quarantine MUST carry these fields through unchanged; if the ingest contract from task_40 cannot express `owner_present`, add it to the contract's frontmatter in this task.
- MUST decide the in-session accept path and record the decision in the skill and in this task's Implementation Details: prefer `ffs_author_atom` followed by the existing `ingest.accept` RPC (reachable from the CLI or a thin MCP pass-through) with no new tool; if a new MCP tool is unavoidable, it is a single `ffs_accept_proposal { submission_id, owner_present: true }` that translates to `ingest.accept`, is added to the catalog per ADR-027's rules, and is documented as valid only in an owner-present session.
- MUST write a session log: one `note` per read titled `Morning read <date>` whose body lists each article opened (url, time), each proposal made and its outcome (filed, proposed, skipped), and each refusal; filed at the end of the session through the same accept path. This is the audit trail ADR-035 relies on for the behavioral line.
- MUST include a worked example in the skill: a three-item agenda (one CBJ pointer, one CLTtoday extraction with two mentions, one ambiguous resolution), the exact tool calls in order, the owner's cues, and the recap wording; the example MUST use invented names and a placeholder url, never real newsletter or article text.
- MUST add a docs test (Rust or Python, whichever the repo already runs over docs; `crates/ffs-mcp` has the catalog) that every MCP tool named in the skill exists in `tool_catalog()`, and a second check that the skill contains the five refusal rules verbatim (a fixed list in the test) so a later edit cannot drop them silently.
- MUST update `docs/agent-memory/README.md` to list the morning-read skill beside `ffs-memory` and `ffs-courier`, with one paragraph on which host tools it needs (an MCP client with the ffs tools, and a browser tool in the owner's session) and what it degrades to without a browser tool (the owner opens the article and pastes what matters).
- MUST update `docs/agent-memory/CONVENTION.md` § Human override with one sentence: an owner present in a session is the human gate, and presence relaxes nothing about persistence claims or inference marking.
- MUST NOT add any fetch, cache, or scheduling logic anywhere; MUST NOT store article text in any atom, note, log, or fixture; MUST NOT reference bizjournals.com, the Observer, or Axios except as pointer sources the owner opens.
- SHOULD add a "Read the paper with me" section to `docs/onboarding/first-use-guide.md` describing a morning: the courier ran, the inbox file is the agenda, you open the assistant, and what you say to start.
</requirements>

## Subtasks
- [ ] 48.1 Decide the in-session accept path (existing `ingest.accept` vs a single pass-through tool); record it; add the pass-through only if unavoidable, with tests.
- [ ] 48.2 Provenance fields for read-filed atoms (`kind: morning_read`, `owner_present`, session id) through the ingest contract, scribe, and quarantine; unit tests that they survive to the atom.
- [ ] 48.3 Write `docs/agent-memory/skill/ffs-morning-read/SKILL.md`: refusal rules first, workflow, provenance, accept path, merges and different-from, digest and recap, worked example.
- [ ] 48.4 Session log note shape and its filing at session end; test that a log with one refusal renders as expected.
- [ ] 48.5 Docs tests: every named tool exists in the catalog; the five refusal rules are present verbatim.
- [ ] 48.6 `docs/agent-memory/README.md`, `CONVENTION.md` § Human override, first-use-guide section.
- [ ] 48.7 Live validation: one real morning read on the project lead's Mac with the courier's agenda from that day; record the outcome in this task's Result section.

## Implementation Details
Current structure: `docs/agent-memory/skill/ffs-memory/SKILL.md` is the shape (frontmatter, minimal contract, tool table, workflow, checklist). `crates/ffs-mcp/src/tools.rs` holds `tool_catalog()`; `ffs_author_atom` translates to `ingest.submit` and stamps `source_uri` from the caller or the agent identity. `ingest.accept` exists in `crates/ffs-daemon/src/dispatch.rs` and is what the plugin's panel calls; it signs with the daemon's owner key. task_46 adds the attestation emitted on accept; task_40 adds the ingest contract and the courier-filed agenda; ADR-032 (decided by task_44) adds the inbox file.

The behavioral line is enforced three ways: the skill's refusal rules, the session log that records every open and every refusal, and the absence of any fetch code path the skill could call. There is nothing for an unattended process to run.

### Relevant Files
- `docs/agent-memory/skill/ffs-morning-read/SKILL.md`: new.
- `docs/agent-memory/README.md`, `docs/agent-memory/CONVENTION.md`: listings and the human-override sentence.
- `crates/ffs-daemon/src/scribe.rs`, `crates/ffs-core/src/quarantine.rs`: provenance fields carried through.
- `crates/ffs-mcp/src/tools.rs`, `crates/ffs-mcp/tests/`: only if the pass-through accept tool is unavoidable; the docs test lives here either way.
- `docs/onboarding/first-use-guide.md`: "Read the paper with me".

### Dependent Files
- `docs/agent-memory/skill/ffs-courier/SKILL.md` (task_40): the courier's agenda is this skill's input.
- `obsidian-plugin/src/summary.ts` (task_39): the same accept path the panel uses.

### Related ADRs
- [ADR-035](adrs/adr-035.md): the decision this task implements.
- [ADR-027](adrs/adr-027.md): persistence honesty and inference marking, unchanged by presence.
- [ADR-029](adrs/adr-029.md): role changes and ambiguous items are walked in the read.
- [ADR-032](adrs/adr-032.md): the inbox file as the agenda.
- [ADR-034](adrs/adr-034.md): the attestation emitted on in-session accept.

## Deliverables
- `docs/agent-memory/skill/ffs-morning-read/SKILL.md` with refusal rules, workflow, provenance, worked example **(REQUIRED)**.
- In-session accept path decided and, if a tool was added, tested **(REQUIRED)**.
- `morning_read` provenance carried end to end with tests **(REQUIRED)**.
- Session log note **(REQUIRED)**.
- Docs tests for tool names and refusal rules **(REQUIRED)**.
- README, CONVENTION, and first-use-guide updates.

## Tests
- Unit tests:
  - [ ] `morning_read_provenance_survives_to_atom`: an ingest file with the read's provenance markers yields an accepted atom whose provenance entry has `kind: morning_read`, the article url, and `owner_present: true`.
  - [ ] `in_session_accept_emits_attestation`: accepting a read-filed proposal emits one attestation with the expected basis (depends on task_46).
  - [ ] `session_log_note_renders_opens_outcomes_and_refusals`.
  - [ ] `skill_names_only_catalog_tools`: every backticked `ffs_*` name in the skill is in `tool_catalog()`.
  - [ ] `skill_contains_refusal_rules_verbatim`: the five rules from ADR-035 § (2) are present.
  - [ ] If added: `ffs_accept_proposal_translates_to_ingest_accept_with_owner_present`.
- Integration tests:
  - [ ] `read_filed_note_lands_in_vault_with_wikilinks`: `ingest_pipeline_e2e`: a read-filed note with two mentions, accepted, materializes with links and an attestation.
- All tests must pass

## Success Criteria
- One real morning read, end to end, files three notes, one affiliation, and one attestation with `kind: morning_read` provenance, and the session refused a bulk-open request and logged the refusal.
- No article text exists anywhere in the substrate, the session log, or the repository after the read.
- The recap at the end of the read names only what the tools confirmed; nothing is described as saved that the accept path did not return.
