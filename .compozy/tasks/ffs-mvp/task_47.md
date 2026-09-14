---
status: pending
title: Shared intake and shared accuracy over federation (the federation half of ADR-034)
type: backend
complexity: high
dependencies:
  - task_15
  - task_40
  - task_45
  - task_46
---

# Task 47: Shared intake and shared accuracy over federation (the federation half of ADR-034)

## Overview
Blocked by ADR-033 until the briefing (task_41) has been in daily use for two consecutive weeks. This task is the reason the deferral lifts.

The owner's two statements set the scope: "we don't need every agent on earth reading the same news stories every day," and "in any collection of N hosts, some percentage of N has to agree it's still accurate." ADR-034 and the three memos under `docs/research/` (federated feeds, agent work distribution, shared accuracy) turn them into a small set of additions to the federation that already ships (task_14, task_15): a per-(peer, predicate) subscription with its own watermark, dedup of `source.article` atoms after the fact by their exact key with automatic `entity.same_as` for articles only, a courier that checks pulled article keys before fetching, an announce atom per courier run, peer-derived people and orgs arriving as proposals in the local quarantine under a per-peer `Accept` grant, and attestations from peers counted toward the quorum task_46 introduced, with independence across sources so two readers of one article are one confirmation. No broker, no leases, no consensus, no push.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add a per-(peer, predicate) subscription (ADR-034 § Decision (6)): a `federation_subscriptions` record keyed by `(peer_id, predicate)` with its own `tx_time` watermark and cadence, stored alongside the existing per-capability `watermarks` in `ffs-core::federation_peers` (`FederationPeer.watermarks`), and a `predicates` query parameter on `GET /federation/v1/atoms` handled by `handle_pull_atoms` in `crates/ffs-federation/src/server.rs` that narrows the walk to the listed predicates while the pinned capability still bounds what is served. `tick_once_for_peer` in `scheduler.rs` MUST iterate subscriptions, pulling each predicate after its own watermark and advancing it only after verified insert, preserving the revocation signal (a subscription that previously yielded atoms and now yields zero). Daemon methods `federation.subscribe { peer, predicates, cadence_s }`, `federation.unsubscribe`, `federation.subscriptions` and a CLI `ffs federation subscribe <peer> --predicates source.article,event.business`. A peer with no subscriptions keeps today's behavior (pull everything the capability covers) so existing peers are unaffected.
- MUST dedup `source.article` atoms on pull by the article key (normalized url per task_40's rule, and `content_hash` when both sides carry one): when a pulled article's key matches a local article entity, the daemon writes an automatic, owner-signed `entity.same_as` from the pulled entity to the local one with `criterion: article_key`, undoable by supersession; the local head stays the local summary, and the projection renders an "Also filed by <peer>" section from the peer's atom. A `content_hash` disagreement under the same url MUST block the automatic link and route a review item instead.
- MUST make the courier (task_40) consult pulled `source.article` atoms before fetching: the ledger check includes `entity.search` over `predicate: source.article` for the normalized url across local and mounted peer atoms; on a hit the courier writes no ingest file and records provenance `kind: federation, peer, atom` on the local article record (creating it as a reference if it does not exist), so the article appears in the digest note with no fetch and no extraction.
- MUST emit a `courier.run` announce atom per courier tick (ADR-034 § Decision (6)): claim `{ source, issue, article_keys[], count, engine, model }`, signed by the substrate, `valid_to` = end of day; starter spec with no path family; peers that subscribe to `courier.run` and pull it before their own tick skip listed keys. The briefing reports "N peers also covered this issue" from these atoms.
- MUST NOT implement `courier.claim` unless spike task_43's findings note records that full fetch is rate-limited, bot-walled, or paid; if so, add it as an advisory lease atom (`valid_to` as TTL, per issue) in a separate subtask and document that expiry means anyone may proceed.
- MUST route a peer's derived atoms into the local quarantine as proposals (ADR-034 § Decision (7)): pulled atoms whose predicate is `person.generic`, `org.company`, `affiliation`, or `event.business` are not inserted as local heads; they are converted to proposals with provenance `kind: federation` plus the peer's original engine and model provenance intact, run through task_45's resolver, and classified by ADR-029's additive rule under an `Accept` grant whose grantee is the peer's substrate key (the grantee model from ADR-029 extended to peer identities). `source.article`, `attestation`, `courier.run`, and `auditor.*` atoms are pulled as atoms. A per-peer setting `rederive: true` re-runs the local scribe engine on the peer's article instead of using the peer's proposals (off by default). The existing `from/<peer>/` mount continues to show the peer's atoms as the peer's view.
- MUST add an `Attest` capability action (ADR-034 § Decision (8)) mirroring `Accept`: granted by an owner-signed `capability.grant` to a peer key, scoped to predicates. Pulled `attestation` atoms are stored regardless (peer-version-skew tolerance) but counted toward the derived status only when the attesting peer holds an `Attest` grant for the attested atom's predicate. `ffs capability grant --action attest --grantee <peer> --predicates affiliation,org.company`.
- MUST count quorum with independence across peers (ADR-034 § Decision (8)): task_46's status function receives attestations from all signers; with `independent = true`, attestations sharing a `source` url or `content_hash` collapse to one regardless of signer, so two peers who read the same article are one confirmation and a peer attesting from a second publication, a primary source, or owner knowledge is a second.
- MUST route cross-peer disputes to review: when a pulled proposal for an entity linked to a local entity by `entity.same_as` (owner-asserted, task_39) contradicts the local head (a different current affiliation, a different org scalar), the classifier marks it Conflicting, the daemon records a `contradicted_by` attestation on the local head with `source` = the peer atom's hash, the derived status shows `disputed` until the owner accepts one side, and nothing is auto-resolved. Three-strikes damping per peer per entity (stop auto-proposing after three rejections until re-enabled) is later and MUST be listed as such.
- MUST tolerate peer version skew: a peer without the `attestation`, `courier.run`, or subscription support pulls and serves as before; unknown predicates are stored and have no projection (verified in ADR-031); a `predicates` filter on a server that ignores it degrades to the full walk with a warning on the client.
- MUST document: `docs/onboarding/technical-friend-checklist.md` ("Sharing the reading with a peer": subscribe, grant accept and attest, what arrives where); `docs/onboarding/first-use-guide.md` ("Also filed by" and "confirmed by Alice" in projections); `docs/agent-memory/CONVENTION.md` § 12 (peer attestations and independence); ARCHITECTURE.md federation section and stability commitments (the `predicates` filter and subscription RPCs join the JSON-RPC method set).
</requirements>

## Subtasks
- [ ] 47.1 `federation_subscriptions` in `ffs-core::federation_peers` (in-memory and SQLite stores) with per-subscription watermarks; `federation.subscribe | unsubscribe | subscriptions` RPCs; CLI verb; tests.
- [ ] 47.2 `predicates` filter on `handle_pull_atoms` and `pull_atoms` in `client.rs`; `tick_once_for_peer` iterates subscriptions with per-subscription watermark advance and revocation detection; fallback to full walk when a peer has no subscriptions; skew fallback when the server ignores the filter; tests.
- [ ] 47.3 Article dedup on pull: key match, automatic owner-signed `entity.same_as` with `criterion: article_key`, "Also filed by" rendering, `content_hash` disagreement routes to review; tests.
- [ ] 47.4 Courier pre-fetch check over local and mounted `source.article` atoms; `kind: federation` provenance; digest note lists pulled articles; `courier.run` announce atom spec and emission; briefing "N peers also covered this issue"; tests with two in-process substrates.
- [ ] 47.5 Peer-derived atoms as proposals: pull-path routing by predicate, provenance carry-through, resolver and ADR-029 classification with the peer key as grantee, `rederive` setting; tests.
- [ ] 47.6 `Attest` capability action; grant CLI; quorum counting across signers with source independence; peer attestations stored but not counted without a grant; tests.
- [ ] 47.7 Cross-peer dispute routing with `contradicted_by` attestations and `disputed` status; tests.
- [ ] 47.8 Peer-version-skew tests (peer without attestation spec, peer ignoring `predicates`); docs; ARCHITECTURE.md and stability commitments; ADR-033 closed out with a reference to this task.
- [ ] 47.9 Only if spike task_43 records expensive fetch: `courier.claim` advisory lease atom with TTL; tests; docs.

## Implementation Details
The federation crate's shape is unchanged: pull over mTLS with a pinned capability, verify on insert, advance a watermark. The subscription is a narrowing of the walk and a second watermark table; the memo's queue mapping (predicate = topic, watermark = offset, capability = ACL, watermark advance = commit) holds line for line. Routing pulled atoms into the quarantine reuses the `ingest.submit` path with a pre-built proposal set rather than a scribe run, so the resolver and the additive rule apply unchanged.

Two-substrate tests use the in-process client and server from `crates/ffs-federation/tests/` (task_15) with two stores, two owner keys, and a shared clock.

### Relevant Files
- `crates/ffs-core/src/federation_peers.rs` (subscriptions), `crates/ffs-core/src/capability/mod.rs` (`Attest`), `crates/ffs-core/src/attestation.rs` (independence across signers, task_46).
- `crates/ffs-federation/src/server.rs` (`handle_pull_atoms` filter), `client.rs` (`pull_atoms` params), `scheduler.rs` (`tick_once_for_peer` over subscriptions).
- `crates/ffs-daemon/src/dispatch.rs`, `api.rs` (subscription RPCs, pull-path routing to the quarantine, article `same_as`, dispute attestations).
- `skills/courier/courier.py` (pre-fetch check, `courier.run`), `starter/predicates/courier.run.toml` (new).
- `skills/auditor/audit.py` (peers-also-covered, dispute counts).
- `crates/ffs-cli/src/commands.rs` (`federation subscribe`, `capability grant --action attest`).

### Dependent Files
- `crates/ffs-federation/tests/` (two-substrate e2e), `crates/ffs-daemon/tests/ingest_pipeline_e2e.rs` (peer proposals).
- `docs/onboarding/technical-friend-checklist.md`, `docs/onboarding/first-use-guide.md`, `docs/agent-memory/CONVENTION.md`, `ARCHITECTURE.md`.

### Related ADRs
- [ADR-034](adrs/adr-034.md): the decision; this task is its federation half.
- [ADR-033](adrs/adr-033.md): the deferral this task lifts; its four reopen questions.
- [ADR-029](adrs/adr-029.md): `Accept` grants with peer keys as grantees; the additive rule; `max_per_day` as the flood guard.
- [ADR-030](adrs/adr-030.md): `entity.same_as` semantics; why articles may be auto-linked and people may not.
- [ADR-031](adrs/adr-031.md): unknown-predicate tolerance; `source.article` key.
- [ADR-020](adrs/adr-020.md), [ADR-012](adrs/adr-012.md): pull-based, pairwise transport this task narrows rather than replaces.

## Deliverables
- Per-(peer, predicate) subscriptions with their own watermarks; `predicates` filter; RPCs and CLI **(REQUIRED)**.
- Article dedup on pull with automatic `same_as` and "Also filed by" **(REQUIRED)**.
- Courier pre-fetch check and `courier.run` announce **(REQUIRED)**.
- Peer-derived atoms as quarantine proposals under a per-peer `Accept` grant **(REQUIRED)**.
- `Attest` capability action; cross-signer quorum with source independence **(REQUIRED)**.
- Cross-peer dispute routing **(REQUIRED)**.
- Skew tests and docs **(REQUIRED)**.
- `courier.claim` only if spike 43 warrants it.
- Unit tests with 80%+ coverage on new modules **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Subscription store: create, list, per-subscription watermark advance, remove; SQLite and in-memory parity.
  - [ ] `handle_pull_atoms` with `predicates` returns only listed predicates and never more than the capability allows; without the parameter behaves as today.
  - [ ] `tick_once_for_peer` advances each subscription's watermark independently; revocation detected per subscription; a peer with no subscriptions falls back to the full walk.
  - [ ] Article key match writes an owner-signed `same_as` with `criterion: article_key`; `content_hash` disagreement blocks it and produces a review item.
  - [ ] Pull-path routing: a pulled `person.generic` becomes a proposal with `kind: federation` and the peer's engine/model provenance; a pulled `source.article` is inserted as an atom.
  - [ ] `Attest`: attestation from a peer without the grant is stored but not counted; with the grant it counts; two peers attesting from the same source url count once at `independent = true`.
  - [ ] Dispute: a peer proposal contradicting a `same_as`-linked local head yields a `contradicted_by` attestation and `disputed` status; owner accept of either side clears it.
  - [ ] Skew: a server ignoring `predicates` yields a full walk and a client warning; a peer without the `attestation` spec stores attestation atoms with no projection.
- Integration tests (two in-process substrates A and B):
  - [ ] A's courier files an article; B subscribes to A's `source.article`; B pulls it and B's courier tick over the same email writes zero ingest files and records `kind: federation` provenance.
  - [ ] A's derived person arrives in B's quarantine as a proposal, resolves against B's entities, and auto-files only if B granted A `Accept` for that predicate and the proposal is additive.
  - [ ] B attests A's affiliation with `basis: independent_source`; with `k = 2` on A, A's affiliation renders `current`; with both attestations from the same article url it stays `unconfirmed`.
  - [ ] A's `courier.run` atom is pulled by B before B's tick and B skips the listed keys; B's briefing reports one peer also covered the issue.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- Two substrates reading the same digest email fetch and extract each article once between them, with no lease and no coordination beyond pull.
- A peer's people and orgs never become local heads without passing the local resolver and the local human gate or a per-peer grant.
- A fact confirmed by two peers from the same article shows one confirmation; a second independent source makes it `current` at `k = 2`.
- Federation tests from task_14 and task_15 stay green; a pre-task_47 peer interoperates unchanged.
- All tests passing; coverage >= 80% on new modules.
