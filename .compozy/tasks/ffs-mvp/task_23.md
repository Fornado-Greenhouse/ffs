---
status: pending
title: Onboarding documentation — technical-friend checklist and first-use guide
type: docs
complexity: low
dependencies:
  - task_22
---

# Task 23: Onboarding documentation — technical-friend checklist and first-use guide

## Overview
Author the onboarding artifacts that close the loop on the MVP's technical-friend-helping-non-technical-peer scenario. The checklist walks the technical friend through installation, keychain setup, identity initialization, federation handshake, and first-use verification. The first-use guide walks the end-user peer through their first day of capture, navigation, and review without terminal use.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST produce `docs/onboarding/technical-friend-checklist.md` covering the friend's setup steps from initial install through first federation handshake with another peer.
- MUST produce `docs/onboarding/first-use-guide.md` covering the end-user peer's first daily-use flow: capture in ingest, navigate projections, review the daily summary, accept proposals, edit a contact, set up federation.
- MUST produce `docs/onboarding/troubleshooting.md` covering the common failure modes flagged in TechSpec § Known Risks: SQLCipher cross-platform issues, Windows named-pipe quirks, federation handshake friction, skill subprocess hangs.
- MUST include screenshots or annotated examples for the Obsidian-side flows.
- MUST be readable end-to-end in under 30 minutes (PRD § User Experience: total onboarding under one hour).
- SHOULD link to the PRD, TechSpec, and ADRs for users who want deeper context.
</requirements>

## Subtasks
- [ ] 23.1 Author the technical-friend checklist (install → keychain → identity → first run → federation).
- [ ] 23.2 Author the first-use guide (capture → navigate → review → edit → federate).
- [ ] 23.3 Author the troubleshooting guide covering known-risk failure modes.
- [ ] 23.4 Capture screenshots of the Obsidian plugin's daily-summary panel, projection navigation, and federation setup.
- [ ] 23.5 Cross-reference the docs from the installer scripts (print "see docs/onboarding/...").

## Implementation Details
Create `docs/onboarding/technical-friend-checklist.md`, `docs/onboarding/first-use-guide.md`, `docs/onboarding/troubleshooting.md`, and a `docs/onboarding/screenshots/` directory. Documentation is markdown, readable both in a text editor and rendered. Update the project's top-level `README.md` to link to these guides.

See PRD § User Experience § Onboarding by a technical friend for the audience and tone.

### Relevant Files
- `docs/onboarding/technical-friend-checklist.md` (new).
- `docs/onboarding/first-use-guide.md` (new).
- `docs/onboarding/troubleshooting.md` (new).
- `docs/onboarding/screenshots/` (new directory).
- `README.md` (new at repo root) — links to the onboarding docs.
- `installer/install.sh`, `installer/install.ps1` (task_22) — print pointers to the onboarding docs.

### Dependent Files
- None — this task closes the MVP loop.

### Related ADRs
- [ADR-002: Both audiences first-class](adrs/adr-002.md) — Documentation supports both audiences.
- [ADR-007: Personal federation in MVP](adrs/adr-007.md) — Federation handshake is part of onboarding.

## Deliverables
- Three onboarding markdown documents covering the technical-friend, first-use, and troubleshooting flows.
- Annotated screenshots for Obsidian-side flows.
- Top-level `README.md` linking to the onboarding docs.
- Installer scripts printing pointers to the docs on completion.
- Unit tests with 80%+ coverage **(REQUIRED)** — for any embedded code samples that must remain runnable.
- Integration tests for end-to-end onboarding rehearsal **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Embedded code samples (e.g., a sample `ffs cat` command) execute without error against a freshly-installed substrate.
  - [ ] Markdown lints cleanly (no broken internal links; valid heading hierarchy).
- Integration tests:
  - [ ] Onboarding rehearsal: an unfamiliar developer follows the technical-friend checklist on a clean VM and reaches a working FFS substrate within 1 hour.
  - [ ] First-use rehearsal: a non-technical user (or simulated user via scripted flow) follows the first-use guide and successfully captures a contact, navigates to it, and reviews a daily summary.
  - [ ] Troubleshooting guide covers each issue flagged in TechSpec Known Risks (verify by checklist).
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A reviewer who has never seen FFS before can install and use it for an hour following only the onboarding docs and the installer.
- The PRD's success criterion of technical-friend-onboarding-in-under-an-hour is achievable from these documents.
