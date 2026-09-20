---
status: completed
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
ADR-035 splits the newspaper goal in two. The courier (task_40) is deterministic and files pointers and permitted feeds on a schedule. The morning read is the other half: an interactive session in which the owner is present, invokes the read, and works through the day's agenda with an assistant at human pace. The assistant opens one article at a time in the owner's own signed-in browser when the owner says so, summarizes it in the session, proposes the people, organizations, affiliations, and events worth remembering, and files only what the owner chooses, as the owner's own note. Article text is stored only when the owner says "clip this": then the article is filed as a `source.article` with its body under the `clip` classification tier (local library, never federated unless a capability names the tier), with `morning_read` provenance and an attestation (ADR-035 amendment, 2026-09-15). Because the owner decides live, filing from the read is the owner's accept, and each accept emits an ADR-034 attestation.

Publisher policy is the owner's `sources.toml` (task_40). A publisher with `fetch = "session"` is fetched only during the read and only on the owner's cue; the read is the one place such a publisher is opened by the assistant. A publisher with `fetch = "off"` is never fetched by the assistant; the owner opens it and pastes what matters. The read's refusals are behavioral (bulk, prefetch, crawling), never about a domain.

This task delivers that session as an instructional skill for Claude Code (with Claude in Chrome) and for claw hosts with a browser tool, in the same shape as `docs/agent-memory/skill/ffs-memory/SKILL.md`. It also settles whether the read needs any new MCP surface. The preference is none: `ffs_author_atom` submits, the existing `ingest.accept` RPC accepts, and task_46's attestation is emitted on accept. If the owner-present accept needs a flag the tools do not carry, that flag is the only addition.

The read doubles as the review. ADR-032's inbox file (accepted 2026-09-20; or `ingest.list_pending` where the inbox is not yet built) is the agenda, and its source order is the read's order: each source's derived proposals (record, people, organizations, affiliations, events, ambiguous resolutions, role changes) are decided together, and "accept all under this article" is the per-source affirmation the owner may give aloud. The agenda items are: courier pointers, CLTtoday extractions, permit and Council items, ambiguous resolutions, and role changes are walked in the same sitting.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST write `docs/agent-memory/skill/ffs-morning-read/SKILL.md` with YAML frontmatter (`name: ffs-morning-read`, a `description` that triggers on "morning read", "read the paper with me", "what's in today's digest", and never on an unattended or scheduled request) and a body that states ADR-035's rules as refusals before any workflow: one article at a time on the owner's cue; no bulk opening ("open all of today's links" is refused as bulk behavior, whatever the publisher), no prefetching or background opening in the read, only articles named in the owner's own digest, never crawling; no storing of article bodies except on the owner's "clip this"; summaries spoken, only chosen notes and clips filed; inferences marked; persistence claims limited to what the tools returned. The skill MUST say that publisher policy is the owner's `sources.toml` and that the assistant honors `fetch = "session"` (open only in the read, on cue) and `fetch = "off"` (never open; the owner opens and pastes).
- MUST describe the session workflow: (1) open the day's agenda from ADR-032's `inbox/<date>.md` when it exists, else from `ingest.list_pending` via the daemon, walking it in the file's source order so that one article's pointer, extractions, ambiguous resolutions, and role changes are handled together, and speaking every candidate for an ambiguous item aloud rather than only the first (spike 44 showed the owner picks the first-listed candidate); (2) walk items in order and, for a pointer, wait for the owner's cue, then open exactly that URL in the owner's browser session (name the capability, "open one URL in the owner's session and read its page text", not a vendor tool) and summarize aloud; (3) propose entities and affiliations with `display` and `context` in the ADR-031 shapes, and say which are stated in the article and which are inferred; (4) file the owner's chosen notes via `ffs_author_atom` with `source_uri` set to the article url and content that carries a `provenance: morning_read` marker the scribe preserves (see the provenance requirement below); (5) where the owner says "file it", accept in-session through the existing accept path so the atom lands and task_46's attestation is emitted with basis `owner_knowledge` or `re_read_same_source`; (6) handle "these are different people", "same person as", and merges through the task_39 actions; (6b) on the owner's "clip this", file the open article as a `source.article` with its body, `intake: morning_read`, classification tier `clip`, `content_hash` of the page text, provenance `kind: morning_read` with the url and `owner_present: true`, accept it in-session, and emit the attestation; the clip is the owner's library copy and is never federated unless a capability names the `clip` tier; (7) refuse bulk requests with the reason, as bulk behavior and not as a rule about the publisher; (8) end with the day's digest note (the `[[target|display]]` list of what was read and filed) and a two-line "what we filed" recap that names only what the tools confirmed as filed, proposed, or attested.
- MUST define the provenance of a read-filed atom: `kind: morning_read`, `uri` = the article url, `actor` = the assistant's identity from the MCP server's configured identity, `owner_present: true`, plus the session id. The scribe (`crates/ffs-daemon/src/scribe.rs`) and the quarantine MUST carry these fields through unchanged; if the ingest contract from task_40 cannot express `owner_present`, add it to the contract's frontmatter in this task.
- MUST decide the in-session accept path and record the decision in the skill and in this task's Implementation Details: prefer `ffs_author_atom` followed by the existing `ingest.accept` RPC (reachable from the CLI or a thin MCP pass-through) with no new tool; if a new MCP tool is unavoidable, it is a single `ffs_accept_proposal { submission_id, owner_present: true }` that translates to `ingest.accept`, is added to the catalog per ADR-027's rules, and is documented as valid only in an owner-present session.
- MUST write a session log: one `note` per read titled `Morning read <date>` whose body lists each article opened (url, time), each proposal made and its outcome (filed, proposed, skipped), and each refusal; filed at the end of the session through the same accept path. This is the audit trail ADR-035 relies on for the behavioral line.
- MUST include a worked example in the skill: a three-item agenda (one CBJ pointer, one CLTtoday extraction with two mentions, one ambiguous resolution), the exact tool calls in order, the owner's cues, and the recap wording; the example MUST use invented names and a placeholder url, never real newsletter or article text.
- MUST add a docs test (Rust or Python, whichever the repo already runs over docs; `crates/ffs-mcp` has the catalog) that every MCP tool named in the skill exists in `tool_catalog()`, and a second check that the skill contains the five refusal rules verbatim (a fixed list in the test) so a later edit cannot drop them silently.
- MUST update `docs/agent-memory/README.md` to list the morning-read skill beside `ffs-memory` and `ffs-courier`, with one paragraph on which host tools it needs (an MCP client with the ffs tools, and a browser tool in the owner's session) and what it degrades to without a browser tool (the owner opens the article and pastes what matters).
- MUST update `docs/agent-memory/CONVENTION.md` § Human override with one sentence: an owner present in a session is the human gate, and presence relaxes nothing about persistence claims or inference marking.
- MUST NOT add any cache or scheduling logic to the read; MUST NOT store article text in any atom, note, log, or fixture except the `clip` atom the owner asked for; MUST NOT hard-code any publisher domain as allowed or refused: the skill reads the owner's `sources.toml` policy and the worked example uses a placeholder publisher. The `clip` tier MUST be added to the tier vocabulary and to the capability scope evaluator so that a federation pull never serves a `clip` atom unless the capability's `classifications` names `clip` explicitly.
- SHOULD add a "Read the paper with me" section to `docs/onboarding/first-use-guide.md` describing a morning: the courier ran, the inbox file is the agenda, you open the assistant, and what you say to start.
</requirements>

## Result (2026-09-20)

Two decisions were settled and recorded rather than deferred. The in-session accept path is one new MCP tool, `ffs_accept_proposal { submission_id, owner_present (must be literally true), choices?, resolved_entity? }`, translating to `ingest.accept`; the catalog is now nine tools. An agent host conducting the read may have no shell for the CLI, and the owner's live decision is what the flag records. Provenance stays inside the frozen envelope: a read-filed atom carries two entries, `morning_read` with the article url and `session` with an `ffs-session://` uri carrying the assistant identity, the session id, and `owner_present=true`, backed by two new `SourceKind` variants. The contract's frontmatter gained `owner_present`, `session`, and `actor`, which the scribe turns into those entries and never into claim fields.

The `clip` tier is the one evaluator change: a target classified `clip` matches a capability only when the scope names the tier. The owner's own any-scope grant still reads their clips, since the library is theirs; a peer's any-scope grant does not, so a federation pull never serves a clipped article unless a capability names `clip`. Both cases are tested at the evaluator and at the pull-serving seam.

The skill states the five refusal rules verbatim before any workflow, and a docs test asserts both that every `ffs_*` name it mentions exists in the catalog and that the five sentences are present, so a later edit cannot quietly drop them. The session log renders opens, proposal outcomes, refusals, and clips through a helper the hosts share.

Subtask 48.7 stays open. A real morning read needs the owner present and the courier's real agenda, and the mailbox app password is not yet in the keychain, so no live session has run. To start one: store the app password under `ffs.courier.imap.gmail.com`, run `ffs courier run --dry-run` and then without the flag, then open the assistant and say "let's do the morning read".

Verification: cargo nextest 651 passed; fmt clean; clippy 0 warnings; pytest 265 passed; vitest 86 passed.

## Subtasks
- [x] 48.1 Decide the in-session accept path (existing `ingest.accept` vs a single pass-through tool); record it; add the pass-through only if unavoidable, with tests.
- [x] 48.2 Provenance fields for read-filed atoms (`kind: morning_read`, `owner_present`, session id) through the ingest contract, scribe, and quarantine; unit tests that they survive to the atom.
- [x] 48.3 Write `docs/agent-memory/skill/ffs-morning-read/SKILL.md`: refusal rules first, workflow, provenance, accept path, merges and different-from, digest and recap, worked example.
- [x] 48.4 Session log note shape and its filing at session end; test that a log with one refusal and one clip renders as expected.
- [x] 48.4b "Clip this": `clip` tier in the tier vocabulary and the capability evaluator (served only when a capability names it), the clip atom's shape and provenance, in-session accept, attestation; unit test that a federation pull under a capability scoped to `existence` does not serve a `clip` atom and one naming `clip` does.
- [x] 48.5 Docs tests: every named tool exists in the catalog; the five refusal rules are present verbatim.
- [x] 48.6 `docs/agent-memory/README.md`, `CONVENTION.md` § Human override, first-use-guide section.
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
- [ADR-032](adrs/adr-032.md) (Accepted 2026-09-20): the inbox file, ordered by source, as the agenda; "accept all under this article" as the per-source affirmation.
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
  - [x] `morning_read_provenance_survives_to_atom`: an ingest file with the read's provenance markers yields an accepted atom whose provenance entry has `kind: morning_read`, the article url, and `owner_present: true`.
  - [x] `in_session_accept_emits_attestation`: accepting a read-filed proposal emits one attestation with the expected basis (depends on task_46).
  - [x] `session_log_note_renders_opens_outcomes_refusals_and_clips`.
  - [x] `clip_this_stores_body_under_clip_tier_with_morning_read_provenance_and_attestation`.
  - [x] `federation_pull_does_not_serve_clip_tier_without_capability_naming_it` / `federation_pull_serves_clip_tier_when_capability_names_it`.
  - [x] `skill_names_only_catalog_tools`: every backticked `ffs_*` name in the skill is in `tool_catalog()`.
  - [x] `skill_contains_refusal_rules_verbatim`: the five rules from ADR-035 § (2) are present.
  - [x] If added: `ffs_accept_proposal_translates_to_ingest_accept_with_owner_present`.
- Integration tests:
  - [x] `read_filed_note_lands_in_vault_with_wikilinks`: `ingest_pipeline_e2e`: a read-filed note with two mentions, accepted, materializes with links and an attestation.
- All tests must pass

## Success Criteria
- One real morning read, end to end, files three notes, one affiliation, one clipped article, and their attestations with `kind: morning_read` provenance; the session refused a bulk-open request as bulk behavior and logged the refusal.
- The clipped article carries classification `clip`, renders in the owner's vault, and is not served by a federation pull unless a capability names the tier; no other article text exists in the substrate, the session log, or the repository after the read.
- The recap at the end of the read names only what the tools confirmed; nothing is described as saved that the accept path did not return.
