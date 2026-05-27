---
status: completed
title: Scribe skill (Python) — markdown to proposed atoms with provenance
type: backend
complexity: medium
dependencies:
  - task_03
  - task_07
  - task_10
---

# Task 11: Scribe skill (Python) — markdown to proposed atoms with provenance

## Overview
The scribe is the absorption agent: it reads markdown files dropped into `~/.ffs/ingest/` (by humans typing in Obsidian, by AI agents writing markdown, by any tool), infers structured claims about entities, and produces proposed atoms with provenance pointing back to the source files. Proposals land in the ingest quarantine; the user accepts them via the daily health summary, at which point they become signed atoms.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST be a `SKILL.md`-shaped Python directory hosted by ffs-skills-host (task 10).
- MUST tolerate malformed markdown: accept any input without throwing; surface ambiguity rather than fail.
- MUST infer atoms for the three MVP predicate types: `contact.person`, `person.generic`, `note`.
- MUST attach provenance pointing back to the source file path and content hash.
- MUST validate inferred claims against the predicate's JSON Schema (via the daemon's `predicate.inspect` proxy).
- MUST submit proposals to the ingest quarantine via the daemon's `ingest.submit` JSON-RPC method (capability-checked).
- MUST surface structural ambiguity (e.g., conflicting claims) as items the user reviews in the daily summary.
- SHOULD use a small LLM for natural-language extraction; the LLM is not embedded — the skill calls out to whatever model the user has configured.
</requirements>

## Subtasks
- [x] 11.1 Author the `SKILL.md` metadata describing scribe's identity, capabilities, and entry point.
- [x] 11.2 Implement markdown ingestion: read the file, parse frontmatter, segment body sections.
- [x] 11.3 Implement extraction for `contact.person` predicates (name, email, phone, org, notes).
- [x] 11.4 Implement extraction for `person.generic` predicates (name, role, team).
- [x] 11.5 Implement extraction for `note` predicates (title, tags, body).
- [x] 11.6 Validate inferred claims against the predicate's JSON Schema before submission.
- [x] 11.7 Submit proposals via `ingest.submit` with provenance and rationale.

## Implementation Details
Create `skills/scribe/SKILL.md`, `skills/scribe/extraction.py`, `skills/scribe/prompts/`, and `skills/scribe/tests/`. The skill consumes the `ffs_skill` Python helper library (task 10) for the stdio protocol. LLM access is configured by the user externally; the scribe accepts a configured model client.

See PRD § Core Features § Scribe and ingest absorption and ADR-009 (root) for the skill packaging convention.

### Relevant Files
- `skills/scribe/SKILL.md` (new) — skill metadata.
- `skills/scribe/extraction.py` (new) — primary logic.
- `skills/scribe/prompts/` (new) — LLM prompts (editable).
- `skills/scribe/tests/` (new) — pytest golden-file tests.
- `skills/scribe/definition.atom.json` (new) — FFS agent definition atom.

### Dependent Files
- Daily health summary (task 19, task 13) — surfaces scribe proposals for user review.

### Related ADRs
- [ADR-009: Claw integration](adrs/adr-009.md) — Skill packaging.
- [ADR-011: Path library starts at three](adrs/adr-011.md) — Three predicate types in MVP.

## Deliverables
- A working scribe skill packaged in `SKILL.md` form, hosted by ffs-skills-host.
- Extraction logic for the three MVP predicate types.
- Provenance attached to every proposal.
- Pytest golden-file test suite.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against the real daemon `ingest.submit` flow **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Markdown frontmatter (`---\nname: Sara\n---`) yields a `contact.person` claim with `display_name: "Sara"`.
  - [ ] Body section "Notes:\n- Met at conference" yields a note bullet in the contact's claim.
  - [ ] Conflicting claims (`name: Sara` and `name: Sarah` in two sources) surface as a structural-ambiguity item.
  - [ ] Malformed YAML frontmatter does not throw; the skill emits a partial claim and a parse-warning.
  - [ ] Inferred claim that fails JSON Schema validation is logged and not submitted.
- Integration tests:
  - [ ] End-to-end: write a markdown file to `~/.ffs/ingest/`, watch the proposal land in the quarantine.
  - [ ] Provenance on the proposal points to the source file path and a stable content hash.
  - [ ] Capability denial: submitting under an identity without `Write` capability returns a structured error.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Golden-file tests cover the canonical fixture markdown inputs producing the expected proposal outputs.
- A user-curated demo input (a sample contact card markdown) produces a reasonable proposal that a reasonable user would accept.
