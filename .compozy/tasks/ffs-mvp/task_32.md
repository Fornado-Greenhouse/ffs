---
status: completed
title: Scribe heuristics — recognize unstructured contacts and produce friendlier entity IDs
type: backend
complexity: medium
dependencies:
  - task_11
  - task_26
---

# Task 32: Scribe heuristics — recognize unstructured contacts and produce friendlier entity IDs

## Overview
The first-use rehearsal showed the scribe defaulting to `note` for content that read clearly as a contact: "919-428-4074 / January 18 / Met at Ballantyne Country Club" became a `note` predicate with title "untitled" and entity ID `from-sub-00000002-zgW1oF6Y` (the dispatcher's `from-<submission-id>` fallback). Two related gaps: (a) the scribe's heuristics require frontmatter to fire `contact.person`, missing unstructured contact-like body text; (b) when no `display_name`/`title` is extracted, the substrate falls back to a non-human-readable entity ID that becomes the projection filename.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST extend `skills/scribe/extraction.py` to recognize unstructured contact signals when frontmatter is absent: capitalized-name patterns ("Met at … with Sara Chen"), phone-number patterns (`\d{3}-\d{3}-\d{4}` and other common shapes), email-in-body patterns (`<word>@<word>`), and "Met at X" / "saw X at" venue patterns. Emit a `contact.person` proposal when ≥2 of these signals fire AND a capitalized name is extractable.
- MUST extend the same path to fall back to a `note` predicate with a body-derived title when only weaker signals are present — current behavior, but the title is no longer literally "untitled".
- MUST update the dispatcher's `ingest_accept` entity-ID generation (`crates/ffs-daemon/src/dispatch.rs::397`) to produce a human-readable slug from the proposal's claim:
  - For `contact.person`: slugify `display_name` (existing behavior).
  - For `note`: slugify `title` if present; else a 4-word slug of the body's first non-empty line; else fall back to `note-YYYY-MM-DD-HHMM` based on `tx_time`.
  - For `person.generic`: slugify `display_name`.
- MUST keep the existing `from-<submission-id>` path as the last-resort fallback when no signals are extractable AND no body content can be sloganed (e.g., binary or whitespace-only content) — but this case should be rare after the above.
- MUST NOT regress the existing `scribe_integration.rs` or `scribe.rs` unit tests; extend them with new fixtures covering the unstructured-contact cases.
- SHOULD emit a `rationale` string on each proposal that explains which signals fired (e.g., "matched 2 contact signals: phone number + capitalized name"), so the daily-summary panel can show the user why scribe made a given choice.

</requirements>

## Subtasks
- [x] 32.1 Add heuristic functions to `skills/scribe/extraction.py` for phone/email/name/venue patterns; gate the `contact.person` proposal on a signal-count threshold. *(Approach: venue spans are detected first and masked from the text before the capitalized-name detector runs, so "Met at Ballantyne Country Club" doesn't get misclassified as a person named "Ballantyne Country". The stop list is intentionally narrow — grammar function words + a small set of past-tense interaction verbs ("Met", "Saw", "Spoke", etc.) — NOT month names, day names, or venue words, which would falsely reject real people like "April Johnson".)*
- [x] 32.2 Improve note-fallback so the proposal carries a meaningful `title` derived from the body (first line slug, capped at ~6 words). *(Markdown-list-prefix stripper now requires the prefix to be followed by whitespace, so unstructured content like "919-428-4074" doesn't get its leading digits eaten.)*
- [x] 32.3 Update `crates/ffs-daemon/src/dispatch.rs::ingest_accept` to derive a human-readable entity ID from the proposal's claim using a shared `slugify` helper. *(Pure helper `slug_for_proposal` + `slugify` + `format_tx_time_slug` implemented in `dispatch.rs` with 10 unit tests covering every branch.)*
- [x] 32.4 Add unit tests covering: unstructured contact (phone + name) → contact.person; ambiguous body → note with first-line title; truly empty body → fallback to `note-YYYY-MM-DD-HHMM`; multiple contacts in one ingest file → multiple proposals. *(12 new Python tests + 10 new Rust tests.)*
- [x] 32.5 Extend the e2e ingest test with an unstructured-contact fixture proving the contact.person path lands. *(`unstructured_body_text_produces_contact_person_proposal` exercises the full pipeline with the rehearsal's "Met Sara Chen … Phone 919-428-4074" fixture.)*

## Implementation Details
The scribe's existing `extract_contact_person` (in `skills/scribe/extraction.py`) reads only frontmatter. Add a sibling `extract_contact_person_unstructured` that walks the body for signals. The threshold function counts signals and returns the proposal when the count is high enough.

For the entity-ID generation, the dispatcher currently has (paraphrasing `dispatch.rs:397`):
```rust
entity: claim.get("display_name")
    .and_then(|v| v.as_str())
    .map(|s| EntityId::new(s.replace(' ', "_")))
    .unwrap_or_else(|| EntityId::new(format!("from-{}", &sub.id))),
```

Generalize this to:
```rust
let slug = slug_for_proposal(&proposal); // shared helper
EntityId::new(slug)
```

The helper inspects `proposal.predicate` and the claim shape to pick the right field, with `from-<submission-id>` as the last-resort fallback.

### Relevant Files
- `skills/scribe/extraction.py` — main extraction logic.
- `skills/scribe/tests/` — Python tests already exist for the existing heuristics; extend.
- `crates/ffs-daemon/src/dispatch.rs` — `ingest_accept` entity-ID generation.
- `crates/ffs-daemon/src/scribe.rs` — wire-format unit tests.

### Dependent Files
- `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` — new fixture for unstructured contact.
- `docs/onboarding/first-use-guide.md` — update the "capture a contact" section to show that unstructured text also works.

### Related ADRs
- [ADR-009: Claw integration via OpenClaw or Hermes pattern](adrs/adr-009.md) — Scribe is a claw-shaped skill; heuristic improvements are skill-side and don't change the protocol.

## Deliverables
- Updated `skills/scribe/extraction.py` with unstructured-contact recognition.
- Human-readable entity-ID generation in `dispatch.rs`.
- Unit tests with 80%+ coverage **(REQUIRED)** for the new heuristics and entity-ID helper.
- Integration test for the unstructured-contact path **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Body "Met Sara Chen at the conference. Phone 919-428-4074." → `contact.person` proposal with `display_name: "Sara Chen"`.
  - [ ] Body "919-428-4074" alone → `note` proposal (one signal, below threshold), title "919-428-4074" or "Untitled note from <date>".
  - [ ] Body "Met at Ballantyne Country Club" without a name → `note` proposal, title "Met at Ballantyne Country Club" (first-line slug).
  - [ ] Frontmatter `name: Sara Chen` (existing behavior) still produces `contact.person` — no regression.
  - [ ] `slug_for_proposal(contact.person, {"display_name": "Sara Chen"})` → "Sara_Chen".
  - [ ] `slug_for_proposal(note, {"title": "Tuesday standup notes"})` → "Tuesday_standup_notes".
  - [ ] `slug_for_proposal(note, {})` against a submission with body "Met at the office" → "Met_at_the_office".
  - [ ] `slug_for_proposal(note, {})` against an empty body → "note-2026-06-02-0730" (tx_time-based).
- Integration tests:
  - [ ] Drop a markdown file containing only "919-428-4074, January 18, Met at Ballantyne Country Club" (the exact rehearsal fixture). Assert `ingest.list_pending` surfaces a `contact.person` proposal (or, at the very least, a `note` proposal whose entity ID is human-readable).
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- The first-use rehearsal's unstructured-text fixture produces a meaningful proposal predicate and a file the user can navigate to by name, not by submission ID.
- Users who write contact notes in their natural style (no frontmatter, free-form body) see them classified correctly without having to learn the scribe's preferred shape.
