---
status: pending
title: Federation pull sync — watermarks, capability-filtered serving, intersection, revocation
type: backend
complexity: critical
dependencies:
  - task_14
---

# Task 15: Federation pull sync — watermarks, capability-filtered serving, intersection, revocation

## Overview
Implement the operational federation behaviors that ride on top of the secured transport: capability-filtered atom serving, watermark-based pull-sync, capability-aware intersection queries, and revocation propagation. This delivers the five MVP federation features from the PRD: peer mounting, tier-based selective sharing, intersection computation, and revocation flow.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST implement `GET /federation/v1/atoms?since=<tx_time>&capability=<hash>` returning capability-filtered atoms after the watermark.
- MUST implement `GET /federation/v1/atom/<hash>` for individual atom fetch.
- MUST implement `GET /federation/v1/projection/<path>` for capability-filtered remote projection rendering.
- MUST implement `GET /federation/v1/intersection/<entity>` for capability-aware intersection checks.
- MUST implement `POST /federation/v1/revocation-notice` for opt-in immediate-revocation pushes.
- MUST schedule pulls on a configurable heartbeat (default 60s, user-tunable to 10s) plus on-demand triggers.
- MUST persist `federation_peers.last_pull` watermark and advance only after successful atom verification.
- MUST verify every pulled atom (signature + content hash) before insert (delegates to task 04's store).
- MUST detect revocation: a previously-yielding pull that now returns nothing triggers unmounting of the `from/<peer>/` view.
- MUST materialize peer atoms into `from/<peer>/` projection paths.
- MUST evaluate intersection queries via the local capability evaluator (task 05) plus a peer-side capability check.
- SHOULD support exponential backoff (1s → 60s) on failed pulls.
</requirements>

## Subtasks
- [ ] 15.1 Implement the five federation HTTPS endpoints.
- [ ] 15.2 Implement the pull scheduler (heartbeat + on-demand) inside the daemon.
- [ ] 15.3 Implement watermark-advance with verification.
- [ ] 15.4 Implement revocation detection and unmount flow.
- [ ] 15.5 Implement `from/<peer>/` projection mounting.
- [ ] 15.6 Implement capability-aware intersection query.
- [ ] 15.7 Wire `federation.pull` JSON-RPC method for on-demand triggers.

## Implementation Details
Extend `crates/ffs-federation/` with submodules `endpoints.rs`, `scheduler.rs`, `intersection.rs`, `mount.rs`. Pull scheduling runs as a tokio task that wakes per heartbeat or on `federation.pull` invocation. Revocation propagation latency is bounded by heartbeat unless the optional `revocation-notice` push is enabled.

See ADR-020 and PRD § Core Features § Personal federation between FFS graphs for behavioral semantics.

### Relevant Files
- `crates/ffs-federation/src/endpoints.rs` (new) — HTTP handlers.
- `crates/ffs-federation/src/scheduler.rs` (new) — pull scheduler.
- `crates/ffs-federation/src/intersection.rs` (new) — intersection query logic.
- `crates/ffs-federation/src/mount.rs` (new) — `from/<peer>/` mounting.
- `crates/ffs-federation/src/lib.rs` (task_14) — extends transport.
- `crates/ffs-core/src/capability.rs` (task_05) — filtering.

### Dependent Files
- Obsidian plugin (task_18) — navigates `from/<peer>/` paths and intersection paths.
- CLI (task_08) — `ffs ls ffs://my-graph/contacts/intersection/with/<peer>/` invokes intersection.

### Related ADRs
- [ADR-020: Federation transport — mTLS over HTTPS with pull-based sync](adrs/adr-020.md) — Pull semantics.
- [ADR-007: Personal federation in MVP](adrs/adr-007.md) — Five MVP federation features.
- [ADR-012: Bilateral federation in MVP, multi-peer aggregation in Phase 2](adrs/adr-012.md) — Bilateral scope.

## Deliverables
- The five federation HTTPS endpoints implemented and capability-filtered.
- Pull scheduler with heartbeat + on-demand triggers.
- Watermark-based incremental sync.
- Revocation detection and unmount.
- `from/<peer>/` peer mounting.
- Intersection query implementation.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Scenario tests with two-daemon harness covering all MVP federation behaviors **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Pulled atom with bad signature is rejected; watermark does not advance.
  - [ ] Pulled atom outside the capability scope is filtered out at the source (rejected before transport).
  - [ ] Intersection query returns only entities both sides have at the access tier each grants.
  - [ ] Pull scheduler triggers a pull after exactly one heartbeat tick.
  - [ ] Exponential backoff on failed pulls follows 1s, 2s, 4s, ..., 60s.
  - [ ] Revocation detection: a pull that returns empty after previously returning atoms triggers unmount.
- Scenario tests (two-daemon harness):
  - [ ] Peer A grants `existence`-tier capability to peer B; peer B pulls and sees only existence-classified atoms.
  - [ ] Peer A grants `work_email` capability; peer B sees existence + work_email atoms but not personal_email.
  - [ ] Intersection: A and B both have entity X; intersection path lists X.
  - [ ] Intersection: A has entity Y, B does not; intersection path does not list Y.
  - [ ] Revocation: A supersedes the capability atom; on next pull B's `from/A/` view empties; A's audit trail records the revocation.
  - [ ] Performance: federation pull of 100 atoms completes in under 5s.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- All five MVP federation features (capability authoring, peer mounting, tier-based sharing, intersection, revocation) demonstrably work in a two-daemon scenario test.
- Revocation propagation latency is within the heartbeat configured (default 60s); with `revocation-notice` push enabled, under 5s.
