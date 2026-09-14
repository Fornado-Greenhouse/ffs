# ADR governance index

This index binds code paths to the architecture decision records that govern them, in the manner of okf-agent-memory's `code_refs` + `governance` practice (ADR-027). It is the repo's "what governs this file?" lookup and the one-sentence-description layer for progressive disclosure: read this table, then open only the ADRs the task actually touches.

## Tiers

| Tier | Meaning for an agent or contributor editing the referenced paths |
|---|---|
| **constraint** | Mandatory guardrail. You may edit the code, but the ADR's decision binds the edit. A change that contradicts the ADR needs a new ADR that supersedes it first. |
| **hold** | Execution freeze. Do not modify the referenced paths without explicit human approval in the current session. |
| **context** | Advisory background. Read it to understand why the code is shaped the way it is; it does not gate the edit. |

**How to use:** before substantial work under a path, find its row below and read the governing ADRs. Respect the tier. If a path is not listed, `ARCHITECTURE.md` is the governing document and the tier is `constraint` for anything under § Invariants or § Stability commitments.

## Active holds

No active holds.

## Path → governing ADRs → tier

| Path(s) | Governing ADRs | Tier |
|---|---|---|
| `crates/ffs-core/src/atom.rs`, `crates/ffs-core/src/multihash.rs`, `crates/ffs-core/src/multibase.rs` | ADR-017 (JCS envelope, `v` field is the migration knob), ADR-018 (Ed25519 / BLAKE3 / multibase), ADR-001 (records, not files), ADR-030 (entity ids are opaque and permanent, never derived from a name) | constraint |
| `crates/ffs-core/src/capability/` | ADR-007 (personal federation, capability atoms), ADR-013 (capability checks at the MCP boundary delegate here), ADR-020 (source-side evaluation for federation pulls), ADR-029 (`Accept` action + `max_per_day` scope for auto-filing) | constraint |
| `crates/ffs-core/src/predicate/`, `starter/predicates/` | ADR-021 (TOML + embedded JSON Schema spec format), ADR-014 (reverse-map annotations feed the fast-path), ADR-028 (path families declared in specs; business-graph predicate set), ADR-031 (`affiliation` role atoms; `participants[]` / `mentions[]` as entity references; optional `[ontology]` annotation), ADR-030 (`entity.same_as` / `entity.different_from` predicates) | constraint |
| `crates/ffs-core/src/projection/`, `crates/ffs-core/src/working_set.rs`, `starter/templates/` | ADR-005 (editor-agnostic materialized working set), ADR-011 (three path families), ADR-014 (reverse-map-annotated output), ADR-028 (spec-declared path families; wikilinked templates), ADR-030 (path-to-entity index; human basenames with parenthetical disambiguation and redirect stubs; `[[target\|display]]` links), ADR-031 (affiliation sections rendered by reverse lookup), ADR-034 (Proposed: derived status `current / unconfirmed / stale / disputed / deprecated` rendered as frontmatter and an "as of" line; "Also filed by" section; never stored) | constraint |
| `crates/ffs-core/src/store/` (`sqlite.rs`, `schema.rs`, `migrations.rs`, `mem.rs`) | ADR-016 (single SQLite DB per substrate, normalized schema, SQLCipher) | constraint |
| `crates/ffs-core/src/store/keyring.rs`, `crates/ffs-core/src/store/keyring_macos.rs` | ADR-023 (keychain access group; intent upheld), ADR-025 (profile delivery via `.app` bundle) | constraint |
| `crates/ffs-core/src/quarantine.rs`, `crates/ffs-core/src/quarantine_sqlite.rs` | ADR-013 (proposal quarantine as the human gate for agent writes), ADR-027 (proposal ≠ persisted; agents report "proposed"), ADR-029 (additive/conflict classifier; auto-accept only for additive proposals), ADR-034 (Proposed: owner accept auto-emits an `attestation`; peer-derived atoms enter as proposals with `kind: federation`; cross-peer disputes route here) | constraint |
| `crates/ffs-core/src/suppress.rs`, `crates/ffs-fastpath/` | ADR-014 (minimum-viable fast-path scope), ADR-005 (edits from any editor route to atoms or ingest), ADR-032 (Proposed: the `inbox_decision` edit kind and inbox-file suppression, if spike task_44 accepts it) | constraint |
| `crates/ffs-daemon/src/` | ADR-015 (minimal Rust daemon), ADR-019 (UDS / named pipe + JSON-RPC 2.0 method set), ADR-022 (`$FFS_DATA_DIR` is the vault root), ADR-024 (no Windows ACL code in the daemon), ADR-025 (daemon is the only binary inside `FFS.app`), ADR-030 (opaque id minting; the three-outcome resolver; merge RPC) | constraint |
| `crates/ffs-daemon/src/scribe.rs`, `crates/ffs-daemon/src/ingest_watcher.rs` | ADR-026 (scribe v2 engine seam; provenance carries engine + model), ADR-009 (stdlib-only skill bundles) | constraint |
| `crates/ffs-cli/` | ADR-006 (`ffs://` scheme is a stable noun, three address modes), ADR-019 (same JSON-RPC client as the plugin and MCP server) | constraint |
| `crates/ffs-federation/` | ADR-020 (mTLS over HTTPS, pull-based, fingerprint-pinned), ADR-007 (bilateral personal federation), ADR-012 (pairwise only; no multi-peer aggregation), ADR-018 (cert from the Ed25519 key), ADR-033 (Accepted: no new federation work until the intake pipeline reaches daily use; keep tests green; respect peer version skew), ADR-034 (Proposed: per-(peer, predicate) subscriptions, `predicates` filter, article dedup on pull, peer-derived atoms as proposals, `Attest` action; task_47) | constraint |
| `crates/ffs-mcp/` | ADR-013 (thin pass-through; capability checks on the daemon side), ADR-008 (speak MCP at the boundary), ADR-027 (eight-tool catalog; `ffs_search`, `ffs_list_path`, proposal wording), ADR-006 (`ffs_resolve_url` address modes), ADR-030 (`ffs_search` v2 as the reconciliation-shaped candidate generator) | constraint |
| `crates/ffs-skills-host/`, `skills/` (including `skills/courier/`, task_40) | ADR-009 (`SKILL.md`-shaped, claw-portable, zero pip deps; names the courier), ADR-026 (extraction engines live inside the scribe bundle), ADR-015 (subprocess + stdio hosting), ADR-031 (scribe emits affiliation proposals and entity-referencing mentions / participants; auditor reads role changes from affiliation atoms), ADR-034 (Proposed: auditor counts status per predicate and briefs "Past their window"; courier checks pulled article keys and emits `courier.run`) | constraint |
| `obsidian-plugin/` | ADR-005 (plugin is one editor among many; substrate is canonical), ADR-022 (vault root = `$FFS_DATA_DIR`), ADR-019 (UDS / named-pipe client), ADR-028 (new path families and wikilinks surface in the vault), ADR-032 (Proposed: the panel keeps five urgent items and links to the inbox file, if spike task_44 accepts it) | constraint |
| `installer/`, `bundle/`, `entitlements/`, `scripts/codesign-macos.sh`, `.github/workflows/release.yml` | ADR-022 (installer seeds `$FFS_DATA_DIR/.obsidian/`), ADR-023 (Developer ID signing + notarization intent), ADR-024 (`icacls` at install time on Windows), ADR-025 (`FFS.app` wraps only the daemon) | constraint |
| `docs/agent-memory/` | ADR-027 (the FFS Agent Memory Convention and the `ffs-memory` skill) | constraint |
| `docs/onboarding/` | ADR-022 (open `~/.ffs/` as the vault), ADR-025 (bundle-aware install and diagnostics), ADR-026 (engine configuration and privacy posture) | context |
| `docs/research/` | ADR-030 (entity-resolution memo: Wikidata identity, Fellegi-Sunter, entity linking), ADR-031 (top-level-ontologies memo: ISO/IEC 21838, BFO, TUpper, IAO) | context |
| `docs/ontology-alignment.md` | ADR-031 (the `[ontology]` annotation vocabulary and the per-predicate BFO / CCO / IAO mapping) | context |
| `.compozy/tasks/ffs-mvp/adrs/` (this directory) | `CLAUDE.md` § ADRs (numbering, when a new ADR is required), ADR-027 (keep this index current) | context |
| `ARCHITECTURE.md`, `README.md` | ADR-001 through ADR-004 (product shape), ADR-013 and ADR-027 (MCP surface described there) | context |

## All ADRs, one line each

| ADR | Status | One sentence |
|---|---|---|
| [ADR-001](adr-001.md) | Accepted | FFS is records-shaped: the substrate stores signed, classified atoms; files are surfaces only. |
| [ADR-002](adr-002.md) | Accepted | Developers and end users are both first-class audiences; neither surface is deferred. |
| [ADR-003](adr-003.md) | Accepted | Substrate-first MVP: ship the primitives with functional-not-polished surfaces and validate with a small community. |
| [ADR-004](adr-004.md) | Accepted | All three motivating scenarios (contact sovereignty, home-claw absorption, clone-and-collaborate) ship in MVP. |
| [ADR-005](adr-005.md) | Accepted | Projections are a materialized working set of real files any editor can open; no kernel driver, no Obsidian lock-in. |
| [ADR-006](adr-006.md) | Accepted | `ffs://<graph>/<address>` is a public, stable URL scheme with path, atom, and entity address modes. |
| [ADR-007](adr-007.md) | Accepted | MVP ships personal (bilateral, capability-atom) federation; organizational federation is Phase 3. |
| [ADR-008](adr-008.md) | Accepted | Speak MCP and A2A at boundaries; FFS-native semantics between FFS substrates. |
| [ADR-009](adr-009.md) | Accepted | FFS agents are `SKILL.md`-shaped, claw-portable, stdlib-only bundles; FFS does not reimplement the claw. |
| [ADR-010](adr-010.md) | Superseded by ADR-013 | Original deferral of the MCP server to Phase 2; kept for the historical record. |
| [ADR-011](adr-011.md) | Accepted | The path library starts at three families: `contacts/`, `people/`, `notes/`. |
| [ADR-012](adr-012.md) | Accepted | Federation is pairwise in MVP; multi-peer aggregation is Phase 2. |
| [ADR-013](adr-013.md) | Accepted (supersedes ADR-010) | The MCP server is MVP with six tools; capability checks are delegated to the daemon's evaluator. |
| [ADR-014](adr-014.md) | Accepted | A minimum-viable fast-path handles single-line, frontmatter, and additive-section edits; everything else routes to ingest. |
| [ADR-015](adr-015.md) | Accepted | The daemon, CLI, and MCP server are Rust; the plugin is TypeScript; skills are Python subprocesses. |
| [ADR-016](adr-016.md) | Accepted | One SQLite database per substrate, normalized atom schema, SQLCipher with the DEK in the OS keychain. |
| [ADR-017](adr-017.md) | Accepted | The atom envelope is RFC 8785 canonical JSON; content address is `multihash(blake3(jcs_bytes))`. |
| [ADR-018](adr-018.md) | Accepted | Ed25519 signing, ChaCha20-Poly1305 at rest, BLAKE3 hashing, base58btc multibase encoding. |
| [ADR-019](adr-019.md) | Accepted | Local IPC is a UDS / Windows named pipe carrying newline-delimited JSON-RPC 2.0 with verb-object methods. |
| [ADR-020](adr-020.md) | Accepted | Federation transport is mTLS over HTTPS, pull-based, fingerprint-pinned, capability-filtered at the source. |
| [ADR-021](adr-021.md) | Accepted | Predicate specs are TOML files with an embedded JSON Schema `[claim_schema]` and reverse-map rules. |
| [ADR-022](adr-022.md) | Accepted | `$FFS_DATA_DIR` (default `~/.ffs/`) is the Obsidian vault root; there is no separate vault path. |
| [ADR-023](adr-023.md) | Accepted (implementation superseded by ADR-025) | Code-signed macOS binaries share the `3S9R9K2L38.com.ffs.shared` keychain access group. |
| [ADR-024](adr-024.md) | Accepted | Windows ACL hardening happens in the installer via `icacls`, not in the daemon. |
| [ADR-025](adr-025.md) | Accepted | Only the daemon ships inside `FFS.app` (so AMFI honors the keychain entitlement); CLI and MCP server are standalone Mach-Os. |
| [ADR-026](adr-026.md) | Accepted | Scribe v2: predicate-schema-driven extraction behind an engine seam (`heuristic` default, `llm` opt-in), stdlib-only. |
| [ADR-027](adr-027.md) | Accepted | Adopt the OKF Agent Memory Convention's behavior for agents using FFS as memory; add `ffs_search` and `ffs_list_path`; dogfood via this index. |
| [ADR-028](adr-028.md) | Accepted; amended by ADR-030, ADR-031 | Path families are declared in predicate specs; the business-graph predicate set (`org.company`, `source.article`, `event.business`) ships with wikilinked templates. |
| [ADR-029](adr-029.md) | Accepted | Capability-gated auto-filing of additive proposals via an `Accept` action; conflicting proposals always route to review. |
| [ADR-030](adr-030.md) | Accepted | Entity ids are opaque and permanent; a Fellegi-Sunter three-outcome resolver (`existing` / `new` / `ambiguous`, ambiguous always reviewed); `entity.same_as` / `entity.different_from` merge atoms undoable by supersession; a notability-style NIL policy for minting. |
| [ADR-031](adr-031.md) | Accepted | Roles are `affiliation` atoms with a bearer, an organization, and a bitemporal window; event participants and article mentions are entity references with display and role or context; optional BFO / CCO / IAO annotations on predicate specs; no OWL reasoning in the substrate. |
| [ADR-032](adr-032.md) | Proposed (decided by spike task_44) | Review as markdown: the quarantine is projected as `inbox/<date>.md` with checkbox decisions the fast path applies; the panel keeps its five urgent items. |
| [ADR-033](adr-033.md) | Accepted | Federation stays shipped and tested but receives no new work until the intake pipeline reaches daily use (task_41 in use for two weeks). |
| [ADR-034](adr-034.md) | Proposed | Shared intake: `source.article` is the dedup-after-the-fact queue over pull (per-peer, per-predicate subscriptions, automatic `same_as` for articles, courier pre-fetch check, `courier.run` announce). Shared accuracy: `attestation` predicate, k-of-N of independent sources, derived status current / unconfirmed / stale / disputed / deprecated. |
