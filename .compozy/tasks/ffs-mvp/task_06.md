---
status: pending
title: Projection renderer with Tera templates and reverse-map-annotated output
type: backend
complexity: medium
dependencies:
  - task_03
  - task_04
  - task_05
---

# Task 06: Projection renderer with Tera templates and reverse-map-annotated output

## Overview
Render a projection path (e.g., `contacts/by-name/S/Sarah_Chen.md`) into markdown by loading capability-filtered atoms, evaluating the predicate's rendering convention, applying Tera templates, and emitting the produced markdown plus a reverse-map annotation that maps each output element back to its source atom field. This is the read path that every editor sees and the input format the fast-path classifier depends on.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST take a `(ProjectionPath, as_of?)` request and return `{ markdown, render_hash, source_atoms, reverse_map_annotations }`.
- MUST resolve the path's underlying entity and predicate set via the predicate-spec registry.
- MUST apply capability filtering (via task 05's evaluator) before passing atoms to the renderer.
- MUST instantiate Tera templates referenced by predicate specs (`[rendering].template`).
- MUST emit reverse-map annotations linking each rendered output element to its source atom field.
- MUST support the three MVP path families: `contacts/`, `people/`, `notes/`.
- MUST support pagination strategies: alphabetical-first-letter, recency, by-org.
- MUST surface a stable `render_hash` so the daemon can detect when a re-render produces unchanged output.
- SHOULD bound rendering latency to satisfy the PRD's <500ms projection-open target.
</requirements>

## Subtasks
- [ ] 6.1 Define `ProjectionRequest`, `ProjectionResponse`, `ReverseMapAnnotation` types.
- [ ] 6.2 Wire the Tera template engine and load templates from `~/.ffs/config/templates/`.
- [ ] 6.3 Implement the path-to-query resolution (path family + sub-path → atom selection).
- [ ] 6.4 Apply capability filtering to selected atoms before rendering.
- [ ] 6.5 Render markdown via Tera using predicate-spec rendering convention.
- [ ] 6.6 Emit reverse-map annotations alongside the markdown.
- [ ] 6.7 Compute `render_hash` so the daemon can short-circuit unchanged re-renders.

## Implementation Details
Create `crates/ffs-core/src/projection.rs` with submodules. Tera templates are referenced by predicate specs (TechSpec § Implementation Design § Core Interfaces describes `[rendering].template`). Reverse-map annotations are emitted in a parallel data structure (not embedded in markdown) so editors render plain markdown and the daemon's fast-path consults the annotations separately.

See TechSpec § Implementation Design § API Endpoints (`projection.render` method) and ADR-021 for predicate-spec rendering conventions.

### Relevant Files
- `crates/ffs-core/src/projection.rs` (new) — primary module.
- `crates/ffs-core/src/projection/path.rs` (new) — path-family resolution.
- `crates/ffs-core/src/projection/render.rs` (new) — Tera integration.
- `crates/ffs-core/src/predicate.rs` (task_03) — rendering convention source.
- `crates/ffs-core/src/store/mod.rs` (task_04) — atom retrieval.
- `crates/ffs-core/src/capability.rs` (task_05) — filtering.

### Dependent Files
- `crates/ffs-daemon` (task_07) — `projection.render` RPC method.
- `crates/ffs-fastpath` (task_09) — consumes reverse-map annotations.
- `crates/ffs-federation` (task_15) — capability-filtered remote projection.
- Starter Tera templates (task_21) — referenced by starter predicate specs.

### Related ADRs
- [ADR-021: Predicate spec format](adrs/adr-021.md) — Rendering conventions and reverse-map.
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Why projections render to markdown.
- [ADR-011: Path library starts at three](adrs/adr-011.md) — Path families to support.

## Deliverables
- `ProjectionRenderer` that produces markdown + reverse-map annotations + render hash.
- Tera template engine wired with templates from `~/.ffs/config/templates/`.
- Pagination strategies for the three MVP path families.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against the full predicate + store + capability stack **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Rendering a `contacts/by-name/S/Sarah_Chen.md` projection produces frontmatter + body sections matching the predicate spec.
  - [ ] An entity hidden by capability filtering does not appear in the rendered output.
  - [ ] Two consecutive renders of the same projection produce identical `render_hash`.
  - [ ] An atom inserted between renders changes the `render_hash`.
  - [ ] Reverse-map annotations point each output element to a defined atom field.
  - [ ] Path resolver returns paginated entries for `contacts/by-name/S/` (alphabetical strategy).
  - [ ] Path resolver returns recency-ordered entries for `contacts/recent/`.
- Integration tests:
  - [ ] End-to-end: insert atoms via store, render path, capability-filter, verify output matches golden markdown.
  - [ ] Rendering a `notes/recent/` path with 100 atoms completes in under 500ms.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Rendering each of the three MVP path families produces the expected markdown for the canonical fixture atoms.
- Reverse-map annotations enable a downstream fast-path classifier (task 09) to map a single-line text edit back to the correct atom field.
