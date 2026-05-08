---
status: pending
title: Starter Tera template library for the three MVP predicate types
type: chore
complexity: low
dependencies:
  - task_06
  - task_20
---

# Task 21: Starter Tera template library for the three MVP predicate types

## Overview
Author the Tera markdown templates referenced by the starter predicate specs. These templates define how a contact, a generic person, and a note are rendered into the projection markdown that any editor opens. The templates' output structure (sections, frontmatter fields, additive bullet lists) must align with the reverse-map annotations declared by their predicate specs.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST author `contact-person.md.tera` rendering a contact with frontmatter (display_name, work_email, tier) plus body sections (Notes, Organizations, History).
- MUST author `person-generic.md.tera` rendering a person reference.
- MUST author `note.md.tera` rendering a note with title, tags, and body.
- MUST align template output with each predicate spec's reverse-map annotations (so fast-path classifies edits correctly).
- MUST handle empty / missing fields gracefully (no template errors when optional fields are absent).
- MUST emit deterministic output (stable ordering of sections and bullets so render-hash stability holds).
- SHOULD include comments in templates explaining each section's role.
</requirements>

## Subtasks
- [ ] 21.1 Author `contact-person.md.tera`.
- [ ] 21.2 Author `person-generic.md.tera`.
- [ ] 21.3 Author `note.md.tera`.
- [ ] 21.4 Verify each template renders cleanly via the projection renderer with canonical fixture atoms.
- [ ] 21.5 Verify templates align with predicate-spec reverse-map annotations.

## Implementation Details
Create `starter/templates/contact-person.md.tera`, `starter/templates/person-generic.md.tera`, `starter/templates/note.md.tera`. These files are bundled with the installer (task 22) and copied to `~/.ffs/config/templates/` on first run. The renderer (task 06) loads templates by name as referenced from predicate specs (task 20).

See ADR-021 for the rendering-template integration and ADR-005 (root) for the editor-agnostic markdown commitment.

### Relevant Files
- `starter/templates/contact-person.md.tera` (new).
- `starter/templates/person-generic.md.tera` (new).
- `starter/templates/note.md.tera` (new).
- `starter/predicates/*.toml` (task_20) — references these templates.
- `crates/ffs-core/src/projection.rs` (task_06) — engine that renders them.

### Dependent Files
- Cross-platform installers (task_22) — bundle these files.

### Related ADRs
- [ADR-021: Predicate spec format](adrs/adr-021.md) — Templates referenced by specs.
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Markdown is the wire format to editors.
- [ADR-014: Minimum-viable fast-path](adrs/adr-014.md) — Output structure must align with reverse-map.

## Deliverables
- Three Tera markdown templates covering the MVP predicate types.
- Templates aligned with predicate-spec reverse-map annotations.
- Deterministic output ensuring render-hash stability.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to template rendering against canonical fixtures.
- Integration tests for end-to-end render-and-classify-and-roundtrip **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `contact-person.md.tera` renders a canonical contact atom into expected markdown (golden file).
  - [ ] `person-generic.md.tera` renders a canonical person atom into expected markdown.
  - [ ] `note.md.tera` renders a canonical note atom into expected markdown.
  - [ ] Empty optional fields (e.g., a contact with no `personal_email`) render without error and without empty section headers.
  - [ ] Rendering the same atom twice produces byte-identical output.
- Integration tests:
  - [ ] End-to-end: insert a contact atom → render projection → output matches golden markdown.
  - [ ] Edit a frontmatter value in the rendered markdown → fast-path classifies correctly using the matching reverse-map rule.
  - [ ] Add a bullet to the Notes section → classifies as additive_section.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- The three templates produce coherent, human-readable markdown for each canonical fixture atom.
- A round-trip (render → edit → fast-path → re-render) preserves user intent for all three edit categories on each predicate.
