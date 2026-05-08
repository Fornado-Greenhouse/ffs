---
status: pending
title: Capability evaluator — action × scope × bitemporal window
type: backend
complexity: high
dependencies:
  - task_02
  - task_04
---

# Task 05: Capability evaluator — action × scope × bitemporal window

## Overview
Implement the fixed capability evaluator that decides whether a given author may perform a given action against a given target at a given time. Capabilities are themselves atoms; the evaluator reads them from the store, applies their declarative conditions, and returns an Allow/Deny decision with structured reasons. Every read, write, federation pull, and MCP tool call passes through this evaluator.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST evaluate capability by intersecting requested `(action, target)` with each capability atom's `(actions, scope)`.
- MUST honor bitemporal validity windows: a capability is in force only when the evaluation tx_time falls within `valid_from..valid_to`.
- MUST treat revocation as supersession: a superseded capability atom does not grant access at the supersession's tx_time onward.
- MUST evaluate action set: `Read`, `Write`, `Supersede`, `Erase`, `Classify`, `Federate`.
- MUST evaluate scope dimensions: predicate, entity, classification, tier.
- MUST never broaden scope on supersession (a superseder can only narrow or maintain).
- MUST return a typed decision (`Allow` / `Deny`) with the matching capability atom hash on Allow and the reason on Deny.
- SHOULD evaluate against a denormalized `capabilities` view in the store for speed; refresh on capability writes.
</requirements>

## Subtasks
- [ ] 5.1 Define the `Action`, `Target`, `Decision`, `DenyReason` types.
- [ ] 5.2 Implement the evaluator against the `AtomStore` trait so it works in tests with `MemAtomStore`.
- [ ] 5.3 Implement bitemporal window resolution with `tx_time` and `valid_at`.
- [ ] 5.4 Implement scope intersection across predicate, entity, classification, tier.
- [ ] 5.5 Implement supersession-aware capability lookup (active capability set at a given tx_time).
- [ ] 5.6 Add property tests asserting that supersession monotonically narrows scope.
- [ ] 5.7 Document the deny reasons (`NotInScope`, `Expired`, `Revoked`, `NoCapabilityFound`, etc.).

## Implementation Details
Create `crates/ffs-core/src/capability.rs` and submodules. The capabilities table in the store (per ADR-016) is the input set. Capability atoms have `predicate = "capability.grant"` and the claim payload shape described in TechSpec § Implementation Design § Data Models.

See ADR-007 (root) for the personal-federation use cases that drive the evaluator's correctness.

### Relevant Files
- `crates/ffs-core/src/capability.rs` (new) — primary module.
- `crates/ffs-core/src/capability/scope.rs` (new) — scope intersection.
- `crates/ffs-core/src/capability/decision.rs` (new) — `Allow`/`Deny` types.
- `crates/ffs-core/src/atom.rs` (task_02) — capability atoms are envelopes.
- `crates/ffs-core/src/store/mod.rs` (task_04) — capability lookup.

### Dependent Files
- `crates/ffs-daemon` (task_07) — every dispatch path consults the evaluator.
- `crates/ffs-federation` (tasks 14, 15) — capability filtering at federation boundary.
- `crates/ffs-mcp` (task_16) — capability check on every MCP tool call.

### Related ADRs
- [ADR-007: Personal federation in MVP](adrs/adr-007.md) — Tier-based selective sharing depends on capability scopes.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — Capability checks fire at the MCP boundary.
- [ADR-001: Records-shaped substrate](adrs/adr-001.md) — Capability-as-data is part of the substrate's identity.

## Deliverables
- A pure capability evaluator that takes `(agent, action, target, as_of)` and returns `Decision`.
- Action set covering Read, Write, Supersede, Erase, Classify, Federate.
- Property tests for monotonicity-on-supersession and time-window correctness.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests against the SQLite-backed store **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] A capability granting Read on `contact.person` with scope tier `existence` allows Read against an atom with `classification = "existence"`.
  - [ ] The same capability denies Read against an atom with `classification = "personal_email"`.
  - [ ] A capability with `valid_from = 2026-06-01` denies Read at `as_of = 2026-05-15`.
  - [ ] A capability with `valid_to = 2026-04-30` denies Read at `as_of = 2026-05-15`.
  - [ ] Superseding a capability atom causes subsequent evaluations to deny.
  - [ ] An atom with no matching capability returns `Deny(NoCapabilityFound)`.
  - [ ] Multiple capabilities for the same agent: most permissive intersection of allowed scopes wins.
- Integration tests:
  - [ ] Against `SqliteAtomStore` with 1000 capabilities, evaluation completes in under 10ms.
  - [ ] Evaluator and store agree: capability inserted via `AtomStore` is honored by next evaluation.
- Property tests:
  - [ ] For any chain of capability supersessions, the active scope at any tx_time is a subset of the original capability's scope.
  - [ ] No combination of valid_from/valid_to/tx_time grants access outside the declared window.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- 10000 evaluations against a 1000-capability store complete in under 1 second.
- The five MVP federation features (capability authoring, peer mounting, tier-based sharing, intersection, revocation) all evaluate correctly with this module as their gate.
