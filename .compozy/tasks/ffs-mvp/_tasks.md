# FFS-MVP — Task List

## Tasks

| # | Title | Status | Complexity | Dependencies |
|---|-------|--------|------------|--------------|
| 01 | Cargo workspace + cross-platform CI scaffolding | completed | medium | — |
| 02 | Atom envelope: JCS canonicalization + Ed25519 sign/verify + BLAKE3 multihash | completed | high | task_01 |
| 03 | Predicate spec loader: TOML + JSON Schema + reverse-map rule parsing | completed | medium | task_02 |
| 04 | SQLite atom store with SQLCipher and bitemporal indexes | completed | high | task_02 |
| 05 | Capability evaluator: action × scope × bitemporal window | completed | high | task_02, task_04 |
| 06 | Projection renderer with Tera templates and reverse-map-annotated output | completed | medium | task_03, task_04, task_05 |
| 07 | JSON-RPC 2.0 dispatcher in ffs-daemon over UDS / Windows named pipe | completed | high | task_04, task_05, task_06 |
| 08 | ffs CLI: argv parser, `ffs://` URL resolver, static binaries for Linux/macOS/Windows | completed | medium | task_07 |
| 09 | ffs-fastpath: filesystem watcher + diff classifier + supersession-or-route-to-ingest | completed | high | task_03, task_04, task_06, task_07 |
| 10 | ffs-skills-host: subprocess host + stdio bridging for Python skills | completed | medium | task_07 |
| 11 | Scribe skill (Python): markdown to proposed atoms with provenance | completed | medium | task_03, task_07, task_10 |
| 12 | Librarian skill (Python): working-set manager and drift watcher | completed | low | task_04, task_06, task_07, task_10 |
| 13 | Auditor skill (Python): daily health summary atom authoring | completed | medium | task_04, task_05, task_07, task_10 |
| 14 | Federation transport: mTLS HTTPS server/client, cert-from-Ed25519, bridge handshake | completed | critical | task_02, task_04, task_05, task_07 |
| 15 | Federation pull sync: watermarks, capability-filtered serving, intersection, revocation | completed | critical | task_14 |
| 16 | ffs-mcp: six MVP MCP tools wrapping the daemon's JSON-RPC | completed | medium | task_07 |
| 17 | Obsidian plugin: scaffolding + UDS / named pipe client + event subscription | completed | medium | task_07 |
| 18 | Obsidian plugin: paginated folder enumeration + projection rendering on open + edit routing | completed | medium | task_17 |
| 19 | Obsidian plugin: daily health summary panel + entity-name search hook | completed | medium | task_17 |
| 20 | Starter predicate-spec library (contact.person, person.generic, note) | completed | low | task_03 |
| 21 | Starter Tera template library for the three MVP predicate types | completed | low | task_06, task_20 |
| 22 | Cross-platform installer scripts for Linux, macOS, Windows | completed | medium | task_08, task_17 |
| 23 | Onboarding documentation: technical-friend checklist and first-use guide | completed | low | task_22 |
| 24 | Wire SQLite atom store as the daemon binary's default store | completed | low | task_04, task_22 |
| 25 | Working-set materializer: render projection files to disk on atom commit | completed | medium | task_06, task_07, task_22, task_24 |
| 26 | Scribe subprocess + ingest watcher wired into the daemon binary | completed | medium | task_10, task_11, task_22, task_25 |
| 27 | OS keychain integration for owner signing key and SQLCipher DEK | completed | low | task_22, task_24, task_33, task_35 |
| 28 | Obsidian plugin polish: unsubscribe handles and render-on-demand fallback | completed | low | task_17, task_19 |
| 29 | SQLite-backed quarantine: persist pending submissions across daemon restarts | completed | medium | task_24, task_26 |
| 30 | Substrate-is-vault: $FFS_DATA_DIR is the Obsidian vault root | completed | low | task_22, task_25 |
| 31 | Ingest stability window: let users write a note over time before scribe consumes it | completed | low | task_26 |
| 32 | Scribe heuristics: recognize unstructured contacts and produce friendlier entity IDs | completed | medium | task_11, task_26 |
| 33 | macOS code signing + keychain-access-groups so task_27 works under launchd | completed | high | task_01, task_22, task_27 |
| 34 | Windows daemon path correctness: fastpath path normalization + scribe budget + named-pipe e2e | completed | high | task_07, task_09, task_11, task_22 |
| 35 | macOS .app bundle wrapping so the keychain entitlement actually works | completed | high | task_22, task_27, task_33 |
| 36 | Scribe v2: predicate-schema-driven extraction with pluggable engines | completed | high | task_11, task_26, task_32, task_42 |
| 37 | Agent memory convention: adopt okf-agent-memory practices (ADR-027) | completed | medium | task_16, task_23 |
| 38 | Filing cabinet: registry-declared path families + business-graph predicates and wikilinked templates (ADR-028) | completed | high | task_20, task_21, task_25, task_36, task_37 |
| 39 | Auto-file policy: Accept capability action + additive/conflict routing in the quarantine (ADR-029) | completed | high | task_29, task_38, task_44, task_45 |
| 40 | Courier: deterministic email-and-feeds intake + ffs_search v2 + URL dedup (pointers only for terms-restricted publishers) | completed | medium | task_37, task_38, task_43, task_45 |
| 41 | Morning briefing: movers-and-shakers summary from the auditor | pending | medium | task_13, task_38, task_39, task_40 |
| 42 | Spike: extraction quality on real business-press articles (gates task_36) | completed | low | task_11 |
| 43 | Spike: intake reality, does the courier need a browser or an agent (gates task_40) | completed | low | task_26 |
| 44 | Spike: review load and the review surface (gates task_39) | completed | low | task_19, task_29 |
| 45 | Scribe v3: multi-entity proposals + entity resolver (ADR-030, ADR-031) | completed | high | task_36, task_38 |
| 46 | Attestations, derived status, and staleness (ADR-034 local half) | pending | medium | task_38, task_41 |
| 47 | Shared intake and shared accuracy over federation (ADR-034 federation half) | pending | high | task_15, task_40, task_45, task_46 |
| 48 | Morning read: the owner-present reading session skill (ADR-035) | pending | medium | task_37, task_40, task_46 |
| 49 | Wire the fast-path watcher and inbox decisions into the production daemon (dependency inversion) | pending | medium | task_09, task_38, task_39 |

## Build order (as of 2026-09-20)

1. **Tasks 42, 43, 44** (done 2026-09-14 to 2026-09-20). Spikes that gated everything below: extraction quality decides whether the LLM engine can carry the pipeline, intake reality decides the courier's shape, review load decides the review surface and the auto-file cap. Findings notes only, no production code.
2. **Task 36, slim.** The tracer bullet: the `llm` engine against the three existing predicates, so a readable daily digest lands in the vault before any refactor. No dependency on task_38.
3. **Task 38.** Filing cabinet and identity (ADR-028, ADR-030, ADR-031): registry-declared path families, opaque ids, the business-graph predicates, wikilinked templates. The big refactor, now informed by real extraction output from task_36.
4. **Task 45.** Scribe v3: multi-entity proposals and the three-outcome resolver, split out of task_36 because it needs both the LLM engine and the new predicates.
5. **Task 40.** The courier as a deterministic stdlib skill bundle on a daemon schedule: mailbox pointers and clips plus the county permit, Council, EDGAR, and RSS feeds; per-publisher fetch and clip policy in `sources.toml`, owner-set, default pointer/session for terms-restricted publishers (ADR-035 as amended); `ffs_search` v2 and URL dedup.
5b. **Task 48.** The morning read, as soon as the courier files its first agenda: the owner-present session that opens articles one at a time and files the owner's notes (ADR-035).
6. **Task 39.** Auto-file policy (ADR-029) with the review surface ADR-032 accepted from task_44: the inbox file grouped by source article, panel counts and link only; max_per_day default 50.
6b. **Task 49.** The fast-path watcher and inbox ticks in the production daemon; found during task 39: the watcher never ran in the binary because of a crate dependency cycle.
7. **Task 41.** The morning briefing, last, because it reads everything the others write.
8. **Task 46.** Attestations and staleness, local (ADR-034 local half): makes "is it still accurate" a question the substrate answers before any peer exists; the owner's accept is the first attestation, windows per predicate, the briefing nags about facts past their window.
9. **Task 47.** Shared intake and shared accuracy over federation (ADR-034 federation half), when ADR-033 lifts: subscriptions, article dedup by key, peer-derived atoms as proposals, peer attestations counted with source independence.

### Known gaps carried

- Org-to-org relationships (subsidiaries, parents) are not modeled; affiliation is person-to-org only.
- Permit owners are often LLC vehicles; an org-to-org "vehicle of" relationship is needed sooner than planned (from the task_43 primary-source probe).
- Press reports announcement dates, not start dates, so an affiliation's `valid_from` is the article date.
- Real article text never enters git; corpus fixtures are paraphrased or kept under `$FFS_DATA_DIR`.
- Atom volume of about 36,000 a year fits the envelope for roughly three years, after which the PRD's indefinite-accumulation question is due.
- `entity.same_as` across federated peers: two substrates mint different ids for the same person by design; ADR-034 (proposed) covers articles automatically and people via dispute review.
- Federation itself is deferred behind the intake pipeline per ADR-033 (accepted); ADR-034 (proposed) is the reason it resumes.
