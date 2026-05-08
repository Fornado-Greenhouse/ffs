---
status: pending
title: Predicate spec loader — TOML + JSON Schema + reverse-map rule parsing
type: backend
complexity: medium
dependencies:
  - task_02
---

# Task 03: Predicate spec loader — TOML + JSON Schema + reverse-map rule parsing

## Overview
Implement the predicate-spec loader that reads TOML predicate definitions from `~/.ffs/config/predicates/`, validates each spec's internal consistency, and exposes the parsed structure (claim schema, rendering convention, reverse-map rules, pagination strategy, frontmatter convention) to the rest of the substrate. The loader is the input point for the substrate's vocabulary.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST parse TOML predicate-spec files using a strict parser (reject unknown top-level fields).
- MUST validate the embedded `[claim_schema]` section as a syntactically valid JSON Schema (Draft 2020-12) using the `jsonschema` crate.
- MUST parse `[[reverse_map]]` rules into a typed structure with `output`, `atom_field`, `edit_kind` fields.
- MUST validate that every reverse-map `output` references a defined rendering element (frontmatter field or template-defined section).
- MUST reject specs that reference predicate ancestors (`parent_predicate`) that are not loaded.
- MUST hot-reload predicate specs when files in the config folder change.
- MUST expose a `validate_claim(predicate, claim_value) -> Result<(), ValidationError>` function used by atom signing and federation pulls.
- SHOULD return a structured registry that allows callers to look up a predicate spec by name.
</requirements>

## Subtasks
- [ ] 3.1 Define the typed predicate-spec model (`PredicateSpec`, `ReverseMapRule`, `RenderingConvention`, `Pagination`).
- [ ] 3.2 Implement TOML parsing with deny-unknown-fields strictness.
- [ ] 3.3 Integrate JSON Schema validation against the embedded `[claim_schema]`.
- [ ] 3.4 Implement reverse-map rule parsing and internal-consistency validation.
- [ ] 3.5 Implement parent-predicate resolution against the registry.
- [ ] 3.6 Implement hot-reload by watching the `~/.ffs/config/predicates/` directory.
- [ ] 3.7 Expose a `validate_claim` function consumed by atom-authoring callers.

## Implementation Details
Create `crates/ffs-core/src/predicate.rs` and helpers. Use the `toml` crate for parsing and `jsonschema` for validation. The `notify` crate (already needed for fast-path watching in task 09) reloads specs when files change. Predicate specs are themselves authored as substrate atoms (predicate-spec atoms with `predicate = "predicate.spec"`); the loader is the bootstrap path that loads them from disk before the substrate is fully online.

See ADR-021 for the spec format and reverse-map rule shape.

### Relevant Files
- `crates/ffs-core/src/predicate.rs` (new) — primary module.
- `crates/ffs-core/src/predicate/registry.rs` (new) — predicate registry.
- `crates/ffs-core/src/predicate/reverse_map.rs` (new) — reverse-map rule types and validation.
- `crates/ffs-core/src/atom.rs` (task_02) — claim payloads validated via this module.

### Dependent Files
- `crates/ffs-core/src/projection.rs` (task_06) — renderer reads rendering convention.
- `crates/ffs-fastpath` (task_09) — classifier consumes reverse-map rules.
- Starter library (task_20) — first three predicate-spec files.

### Related ADRs
- [ADR-021: Predicate spec format — TOML with embedded JSON Schema](adrs/adr-021.md) — Format and reverse-map structure.
- [ADR-014: Minimum-viable fast-path for trivial projection edits in MVP](adrs/adr-014.md) — Why reverse-map rules exist.

## Deliverables
- A typed predicate-spec model and registry.
- TOML loader with strict field validation.
- JSON Schema validator wired to atom signing and federation.
- Reverse-map rule parser with internal-consistency checks.
- Hot-reload via filesystem watcher.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests for end-to-end TOML-to-validated-claim **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] A canonical `contact.person.toml` parses into the expected structure.
  - [ ] Unknown top-level TOML fields produce a parse error with field name.
  - [ ] An `[claim_schema]` section that is not valid JSON Schema rejects with a structured error.
  - [ ] A reverse-map rule whose `output` doesn't match any rendering element rejects.
  - [ ] A spec with a `parent_predicate` that isn't loaded rejects with a helpful error.
  - [ ] `validate_claim("contact.person", {display_name: "Sara"})` returns Ok.
  - [ ] `validate_claim("contact.person", {})` rejects because `display_name` is required.
  - [ ] `validate_claim("contact.person", {display_name: 1})` rejects because of type mismatch.
- Integration tests:
  - [ ] Hot-reload: writing a new spec file results in registry update within 2s.
  - [ ] Parent-predicate inheritance: a spec inheriting from `person.generic` allows fields defined on the parent.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- The three starter MVP predicate specs (`contact.person`, `person.generic`, `note`) load cleanly when added in task 20.
- A reverse-map rule producing a sample diff classification matches the expected `edit_kind`.
