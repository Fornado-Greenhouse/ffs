---
status: pending
title: Starter predicate-spec library — contact.person, person.generic, note
type: chore
complexity: low
dependencies:
  - task_03
---

# Task 20: Starter predicate-spec library — contact.person, person.generic, note

## Overview
Author the three TOML predicate-spec files that ship as the MVP starter library. These specs define the substrate's vocabulary out of the box: contact-related person predicates with tier classification, a generic person predicate, and a generic note predicate. Reverse-map annotations on each spec are the load-bearing input to the fast-path edit classifier.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST author `contact.person.toml` with claim_schema covering display_name, work_email, personal_email, phone, organization, notes (each with appropriate tier classification).
- MUST author `person.generic.toml` covering display_name, role, team for non-contact person references.
- MUST author `note.toml` covering title, tags, body for the generic note path.
- MUST include reverse-map annotations on each spec for the three MVP edit categories (single-line text, frontmatter value, additive section).
- MUST cover ~15-20 reverse-map rules total across the three specs (per ADR-014 estimate).
- MUST define rendering convention referencing the matching Tera template (delivered in task 21).
- MUST define pagination strategy for path families (`alphabetical_first_letter`, `recency`, `by_org`).
- SHOULD include sub-predicates as needed for tier-based classification (e.g., `contact.person.work_email` may be a sub-predicate inheriting from `contact.person`).
</requirements>

## Subtasks
- [ ] 20.1 Author `contact.person.toml` with full claim_schema, rendering, reverse-map, pagination.
- [ ] 20.2 Author `person.generic.toml`.
- [ ] 20.3 Author `note.toml`.
- [ ] 20.4 Verify all three specs load cleanly via the predicate-spec loader (task 03).
- [ ] 20.5 Verify reverse-map rules cover the three MVP edit categories for each predicate.
- [ ] 20.6 Document each predicate's intended use in a `README.md` alongside the specs.

## Implementation Details
Create `starter/predicates/contact.person.toml`, `starter/predicates/person.generic.toml`, `starter/predicates/note.toml`, and `starter/predicates/README.md`. These files are bundled with the installer (task 22) and copied to `~/.ffs/config/predicates/` on first run.

See ADR-021 for the spec format and ADR-011 (root) for the three-MVP-predicates decision.

### Relevant Files
- `starter/predicates/contact.person.toml` (new).
- `starter/predicates/person.generic.toml` (new).
- `starter/predicates/note.toml` (new).
- `starter/predicates/README.md` (new) — usage documentation.
- `crates/ffs-core/src/predicate.rs` (task_03) — loader that validates these specs.

### Dependent Files
- Starter Tera template library (task_21) — templates referenced by these specs.
- Cross-platform installers (task_22) — bundle these files.

### Related ADRs
- [ADR-021: Predicate spec format](adrs/adr-021.md) — Format authority.
- [ADR-011: Path library starts at three](adrs/adr-011.md) — Three-predicate scope.
- [ADR-014: Minimum-viable fast-path](adrs/adr-014.md) — Reverse-map rules drive fast-path.

## Deliverables
- Three TOML predicate-spec files covering contact.person, person.generic, note.
- ~15-20 reverse-map rules across all three.
- Rendering convention referencing the Tera templates from task 21.
- Pagination strategies appropriate to each path family.
- README documenting each predicate.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to the loader's validation against these specs.
- Integration tests verifying end-to-end load via task 03 loader **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `contact.person.toml` parses without errors via the predicate-spec loader.
  - [ ] `person.generic.toml` parses without errors.
  - [ ] `note.toml` parses without errors.
  - [ ] Each spec's claim_schema is a valid JSON Schema (Draft 2020-12).
  - [ ] Each spec's reverse-map rules reference defined rendering elements.
  - [ ] `validate_claim("contact.person", canonical_payload)` succeeds.
- Integration tests:
  - [ ] Loading all three specs into the registry succeeds.
  - [ ] Reverse-map rules for each spec cover at least one rule per edit category (single_line_text, frontmatter_value, additive_section).
  - [ ] Total reverse-map rule count across the three specs is between 15 and 25.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- The three specs load without warnings on first daemon startup with a fresh `~/.ffs/`.
- Fast-path edits to a contact's name, work email, and additive notes section all classify correctly using these specs' reverse-map rules.
