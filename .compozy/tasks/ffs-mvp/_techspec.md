# Foley File System (FFS) MVP — Technical Specification

**Status:** Draft
**Date:** 2026-05-05
**Source PRD:** [`_prd.md`](_prd.md)

## Executive Summary

FFS MVP is a Rust-based local substrate that stores signed, classified, bitemporal atoms in a single SQLite database per user, exposes them through a long-running daemon over a Unix-domain-socket / Windows-named-pipe JSON-RPC API, materializes opinionated projection paths to disk for any editor to open, and federates bilaterally with peer substrates over mTLS HTTPS. The architecture splits into seven Rust crates (core types, daemon, CLI, MCP server, federation, fast-path, skills host) plus a Python skill bundle (scribe, librarian, auditor) and a TypeScript Obsidian plugin. A canonical-JSON atom envelope (RFC 8785 JCS) hashed with BLAKE3 and signed with Ed25519 is the single substrate-interop contract; everything else — local API, federation transport, predicate spec format, plugin protocol — composes around it.

The primary technical trade-off is heterogeneity for predictability: three toolchains (Rust, TypeScript, Python) instead of one, in exchange for a static-binary CLI on every platform, sub-200ms fast-path latency without GC jitter, and a familiar agent-skill shape that remains portable to OpenClaw later. A secondary trade-off is a single-writer SQLite store (vs. a journal-and-views design) that simplifies MVP correctness at the cost of a future scale ceiling that Phase 3+ work addresses.

## System Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                          FFS Substrate                              │
│                                                                     │
│  ┌──────────────┐                                                   │
│  │  ffs CLI     │                                                   │
│  │  (Rust)      │ ──────┐                                           │
│  └──────────────┘       │                                           │
│                         ▼                                           │
│  ┌──────────────┐    ┌────────────────────────────────────────┐     │
│  │  Obsidian    │    │           ffs-daemon (Rust)            │     │
│  │  plugin (TS) │ ──▶│                                        │     │
│  └──────────────┘    │  ┌─────────────────────────────────┐   │     │
│                      │  │  JSON-RPC 2.0 dispatcher        │   │     │
│  ┌──────────────┐    │  │  (UDS / named pipe)             │   │     │
│  │  ffs-mcp     │ ──▶│  └─────────────────────────────────┘   │     │
│  │  (Rust)      │    │  ┌─────────────────────────────────┐   │     │
│  └──────────────┘    │  │  ffs-core: atom store,          │   │     │
│         ▲            │  │  capability evaluator,          │   │     │
│         │            │  │  projection renderer,           │   │     │
│  MCP-aware           │  │  predicate-spec loader          │   │     │
│  external agents     │  └─────────────────────────────────┘   │     │
│                      │  ┌──────────────┐  ┌───────────────┐   │     │
│                      │  │ FS watcher + │  │ skills-host:  │   │     │
│                      │  │ fast-path    │  │ scribe, lib., │   │     │
│                      │  │ classifier   │  │ auditor (Py)  │   │     │
│                      │  └──────────────┘  └───────────────┘   │     │
│                      │  ┌─────────────────────────────────┐   │     │
│                      │  │  ffs-federation: HTTPS+mTLS,    │   │     │
│                      │  │  bridge handshake, pull sync    │   │     │
│                      │  └─────────────────────────────────┘   │     │
│                      └─────────────┬──────────────────────────┘     │
│                                    │                                │
│                              ┌─────▼────────┐                       │
│                              │  store.db    │                       │
│                              │  (SQLite +   │                       │
│                              │   SQLCipher) │                       │
│                              └──────────────┘                       │
│                                                                     │
│   Filesystem surfaces:                                              │
│     ~/.ffs/ingest/        (write-anything firehose)                 │
│     ~/.ffs/contacts/      (projection — materialized working set)   │
│     ~/.ffs/people/        (projection)                              │
│     ~/.ffs/notes/         (projection)                              │
│     ~/.ffs/config/        (predicate specs, templates, capability   │
│                            templates, all git-versioned)            │
└─────────────────────────────────────────────────────────────────────┘
                                  │ mTLS HTTPS
                                  ▼
                       Other FFS substrates (peers)
```

**Components**:

- **`ffs-core`** (Rust library): atom envelope, signing/verification, multihash addressing, capability evaluator, predicate-spec loader, projection renderer, SQLite store. The shared brain used by every binary.
- **`ffs-daemon`** (Rust binary): long-running per-user process. Owns `store.db`, the filesystem watchers, the fast-path classifier, the skill subprocess host, the federation HTTPS server, and the local JSON-RPC server.
- **`ffs-cli`** (Rust binary, statically linked): single binary distributed for Linux/macOS/Windows. Resolves `ffs://` URLs by talking to the local daemon over UDS / named pipe.
- **`ffs-mcp`** (Rust binary): MCP server exposing the six MVP tools (`ffs_query`, `ffs_render_projection`, `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`, `ffs_audit_query`). Thin wrapper translating MCP tool calls to JSON-RPC against the daemon.
- **`ffs-federation`** (Rust crate, embedded in daemon): mTLS HTTPS server and client. Owns the bridge handshake, capability-filtered atom serving, and pull-sync scheduling.
- **`ffs-fastpath`** (Rust crate, embedded in daemon): reverse-map classifier consuming projection file diffs and producing supersession atoms or routing to ingest.
- **`ffs-skills-host`** (Rust crate, embedded in daemon): subprocess host for Python skills. Routes scribe/librarian/auditor invocations and brokers their substrate access through the daemon's JSON-RPC layer.
- **Python skills bundle**: scribe (markdown → proposed atoms), librarian (working-set / drift watcher), auditor (daily health summary). Each is a `SKILL.md`-shaped directory under `~/.ffs/skills/`.
- **Obsidian plugin** (TypeScript): folder-enumeration interception, projection rendering on open, edit routing, daily-health-summary panel, entity-name search.

**Data flow**:

1. **Read path** (CLI / plugin / MCP server → daemon → store.db): client sends `path.list` or `projection.render` JSON-RPC request → daemon dispatches → `ffs-core` loads atoms, evaluates capabilities, runs the predicate's render template → returns markdown + metadata.
2. **Ingest write path** (any writer → ingest folder → scribe → user accepts → atom): files dropped in `~/.ffs/ingest/` trigger a filesystem event; the daemon hands the file to the scribe subprocess; scribe returns proposed atoms; daemon stores proposals in a quarantine table; user accepts via daily health summary; accepted proposals become signed atoms.
3. **Fast-path edit** (editor save → FS watcher → classifier → atom): editor writes a projection file; FS watcher fires; daemon diffs the file against its last rendered hash; classifier matches the diff to a reverse-map rule; daemon authors a supersession atom and re-renders the projection. Latency budget: ~200ms.
4. **Federation pull** (peer's daemon → my daemon → my store.db): on heartbeat, daemon's federation client opens an mTLS connection to each peer, presents the bridge capability, requests atoms after the watermark, verifies signatures, inserts into the local store.
5. **MCP write** (external agent → MCP server → daemon → ingest quarantine): agent calls `ffs_author_atom`; MCP server validates the agent's identity against the agent's capability; daemon writes the proposed atom to ingest with provenance pointing to the agent.

**External system interactions**:

- **OS keychain** (macOS Keychain / Windows Credential Store / Linux Secret Service): holds DEK and author signing keys.
- **Operating-system filesystem watchers**: inotify (Linux), kqueue (macOS), ReadDirectoryChangesW (Windows). The `notify` crate normalizes them.
- **Git** (out-of-process): users `git clone` substrates; the daemon ignores git internals and reads the cloned files on startup.
- **Obsidian** (TypeScript runtime): hosts the plugin; the plugin connects to the daemon via UDS / named pipe.
- **Peer FFS substrates**: mTLS HTTPS over the public internet (or Tailscale, port-forward, etc.).

## Implementation Design

### Core Interfaces

The atom envelope and the local API surface are the load-bearing contracts. Two Rust types capture them.

```rust
// crates/ffs-core/src/atom.rs

/// The signed, content-addressed unit of substrate state.
/// Serialized as canonical JSON (RFC 8785 JCS) per ADR-017.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AtomEnvelope {
    pub v: u32,                          // schema version, currently 1
    pub entity: EntityId,                // multibase-encoded
    pub predicate: PredicateName,        // e.g. "contact.person"
    pub claim: serde_json::Value,        // validated against predicate's claim_schema
    pub author: PublicKey,               // Ed25519 multibase
    pub valid_from: Iso8601,
    pub valid_to: Option<Iso8601>,
    pub tx_time: Iso8601,
    pub classification: Tier,            // "existence" | "work_email" | "personal_email" | "notes" | ...
    pub supersedes: Option<Multihash>,   // None for root atom in a chain
    pub provenance: Vec<Provenance>,
    pub signature: Signature,            // Ed25519 over JCS bytes (sig field elided)
}

impl AtomEnvelope {
    pub fn content_hash(&self) -> Multihash;     // multihash(blake3(jcs_bytes))
    pub fn verify(&self) -> Result<(), VerifyError>;
    pub fn sign(template: AtomTemplate, key: &SigningKey) -> Result<Self, SignError>;
}
```

```rust
// crates/ffs-core/src/api.rs

/// The JSON-RPC method dispatch table. Each variant maps to a single method name.
/// Wire serialization uses serde with method tags; per ADR-019.
#[derive(Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    #[serde(rename = "atom.get")]         AtomGet { hash: Multihash },
    #[serde(rename = "atom.list")]        AtomList { entity: Option<EntityId>, predicate: Option<PredicateName>, as_of: Option<Iso8601> },
    #[serde(rename = "projection.render")] ProjectionRender { path: ProjectionPath, as_of: Option<Iso8601> },
    #[serde(rename = "path.list")]        PathList { path: ProjectionPath, page: Option<u32> },
    #[serde(rename = "ingest.submit")]    IngestSubmit { source_uri: String, content: Vec<u8> },
    #[serde(rename = "fastpath.submit")]  FastPathSubmit { projection_path: ProjectionPath, new_content: Vec<u8> },
    #[serde(rename = "capability.evaluate")] CapabilityEvaluate { agent: PublicKey, action: Action, target: Target },
    #[serde(rename = "federation.peer.add")]  FederationPeerAdd { endpoint: String, fingerprint: Multihash },
    #[serde(rename = "federation.pull")]      FederationPull { peer: PublicKey },
    #[serde(rename = "predicate.inspect")] PredicateInspect { name: PredicateName },
    #[serde(rename = "health.summary")]   HealthSummary,
}
```

### Data Models

**`AtomEnvelope`** (above) — the canonical substrate record.

**Capability atom** (a specialization of `AtomEnvelope` with `predicate = "capability.grant"`):

```
claim: {
  grantee: <peer_public_key>,
  actions: ["read"] | ["read", "supersede"] | ...,
  scope: {
    predicates: ["contact.person", ...],
    entities: <optional list>,
    classifications: ["existence", "work_email"],
    tier: "introducible" | "discreet" | ...
  },
  conditions: <evaluator-readable predicates>,
  valid_from, valid_to: <bitemporal window>
}
```

**Predicate spec atom** (`predicate = "predicate.spec"`): claim payload is the TOML predicate spec content (per ADR-021) embedded as a string field plus a parsed-and-cached structured copy.

**SQLite schema** (per ADR-016):

| Table | Key columns | Purpose |
|-------|-------------|---------|
| `atoms` | `content_hash` PK, `(entity_id, predicate, tx_time)` index, `(predicate, tx_time)` index, `(supersedes)` index | Primary atom store |
| `classifications` | `(atom_hash, tier)` | Per-atom classification claims |
| `capabilities` | denormalized current view | Fast capability evaluation |
| `provenance` | `(atom_hash, source_kind)` | Source-traceback for every atom |
| `entities` | `entity_id` PK | Canonical labels and entity-level metadata |
| `claims_fts` | FTS5 virtual table | Full-text search over claim payloads |
| `federation_peers` | `peer_id` PK | Bridge state, watermarks, fingerprints |
| `working_set` | `projection_path` PK | Materialized projection metadata |
| `ingest_quarantine` | `submission_id` PK | Pending scribe proposals awaiting user acceptance |

**`Action`** (capability action set): `Read`, `Write`, `Supersede`, `Erase`, `Classify`, `Federate`.

### API Endpoints

**Local JSON-RPC 2.0** (UDS / named pipe; per ADR-019):

| Method | Params | Result |
|--------|--------|--------|
| `atom.get` | `hash` | `AtomEnvelope` |
| `atom.list` | `entity?`, `predicate?`, `as_of?` | `[AtomEnvelope]` |
| `projection.render` | `path`, `as_of?` | `{ markdown, render_hash, source_atoms }` |
| `path.list` | `path`, `page?` | `{ entries, next_page? }` |
| `ingest.submit` | `source_uri`, `content` | `{ submission_id }` |
| `fastpath.submit` | `projection_path`, `new_content` | `{ kind: "applied" \| "routed_to_ingest", atom_hash? }` |
| `capability.evaluate` | `agent`, `action`, `target` | `{ allowed, reason }` |
| `federation.peer.add` | `endpoint`, `fingerprint` | `{ peer_id }` |
| `federation.peer.list` | — | `[FederationPeer]` |
| `federation.pull` | `peer` | `{ atoms_pulled }` |
| `predicate.inspect` | `name` | `PredicateSpec` |
| `health.summary` | — | `{ proposals, questions, drift_flags }` |

**Notifications** (server → client): `event.atom.committed`, `event.projection.invalidated`, `event.fastpath.applied`, `event.federation.peer.changed`.

**Federation HTTPS** (mTLS; per ADR-020):

| Method + Path | Purpose |
|---------------|---------|
| `POST /federation/v1/handshake` | Bridge establishment |
| `GET /federation/v1/atoms?since=<tx_time>&capability=<hash>` | Capability-filtered pull |
| `GET /federation/v1/atom/<hash>` | Single atom fetch |
| `GET /federation/v1/projection/<path>` | Capability-filtered projection render |
| `GET /federation/v1/intersection/<entity>` | Intersection check for `intersection/with/<peer>/` |
| `POST /federation/v1/revocation-notice` | Optional immediate-revocation push |

**MCP tools** (six MVP tools per PRD § Core Features § FFS-MCP server): `ffs_query`, `ffs_render_projection`, `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`, `ffs_audit_query`. Each tool is a thin translator: validate agent capability → translate to JSON-RPC method → call daemon → translate response.

## Integration Points

| System | Purpose | Auth | Error/retry |
|--------|---------|------|-------------|
| **OS keychain** (macOS / Windows / Linux) | DEK and author signing key storage | Process-user keychain access | On lookup miss, daemon refuses to start; user re-runs setup |
| **Filesystem watchers** (inotify / kqueue / ReadDirectoryChangesW) | Detect ingest writes and projection edits | OS-level | Watcher reset on missed events; full directory scan reconciles |
| **Peer FFS substrates** | Federation pull sync, bridge handshake | mTLS with Ed25519-derived certs, fingerprint-pinned | Exponential backoff on connection failure (1s → 60s); revocation detected as empty pull result |
| **Git** (clone-time only) | Substrate cloning per day-one-clone-and-collaborate | None (substrate trusts cloned files at user direction) | Daemon validates atom signatures on first read; invalid atoms quarantined |
| **MCP-aware agents** (external) | Read/write substrate via MCP server | Agent identity bound to FFS author key; capability checks on every tool call | Structured MCP errors on capability denial |
| **Python skill subprocesses** (scribe, librarian, auditor) | Markdown absorption, drift watching, daily reports | Inherits daemon trust (subprocess of daemon) | Skill crash auto-restarts; output schema-validated before substrate effect |

## Impact Analysis

This is a greenfield project; no existing code is modified. The table records new components introduced and risks for each.

| Component | Impact Type | Description and Risk | Required Action |
|-----------|-------------|---------------------|-----------------|
| `ffs-core` crate | New | Foundational library; bugs propagate everywhere. Risk: medium | Property tests for atom canonicalization, signing, capability evaluation |
| `ffs-daemon` binary | New | Long-running process; crashes affect all clients. Risk: medium | Robust error handling, supervised subprocess host, structured logging |
| `ffs-cli` binary | New | Single static binary, three OSes, three target triples. Risk: low | Cross-compile in CI; smoke-test on each platform |
| `ffs-mcp` binary | New | Capability-check correctness gates external agent writes. Risk: high | Integration tests with simulated agents; capability tests with rejected and accepted scopes |
| `ffs-federation` crate | New | mTLS handshake and pull sync correctness gates federation security. Risk: high | Scenario tests with two-daemon harness; certificate-rotation tests; revocation tests |
| `ffs-fastpath` crate | New | Misclassified diffs could author wrong atoms or lose user edits. Risk: high | Reverse-map golden tests for every MVP predicate; manual review of edge cases |
| `ffs-skills-host` crate | New | Subprocess management for skills; hangs or zombie processes possible. Risk: medium | Process supervision with timeouts and restart policy |
| Python skills bundle | New | Scribe extraction quality affects user trust. Risk: medium | Pytest with golden markdown inputs and expected proposal outputs |
| Obsidian plugin | New | End-user-facing surface; UX bugs shape adoption. Risk: medium | Vitest unit tests; manual cross-platform smoke tests in Obsidian on Linux/macOS/Windows |
| `~/.ffs/` directory layout | New convention | Three folder spaces, no overlap. Risk: low | Structural enforcement in daemon startup |
| `store.db` SQLite schema | New | Schema migration burden grows over time. Risk: low at MVP | `schema_version` table; daemon refuses unknown future versions |
| `~/.ffs/config/predicates/*.toml` | New convention | Predicate-spec format is a long-lived contract. Risk: medium | Format documented in ADR-021; reverse-map invariants validated on load |

## Testing Approach

### Unit Tests

**Strategy**: every module in `ffs-core` has co-located `#[cfg(test)]` unit tests. Component boundaries get isolated tests with the dependency injected.

**Mock requirements**:

- `AtomStore` is a trait; tests inject an in-memory implementation backed by a `BTreeMap` instead of SQLite.
- The skills-host trait abstracts over subprocess spawning; tests inject a stub that returns canned scribe outputs.
- Filesystem watchers are abstracted behind a `WatchSource` trait; tests inject synthetic event streams.
- Federation transport is abstracted behind a `FederationClient` trait; tests pair two in-memory clients without network.

**Critical scenarios per crate**:

- `ffs-core::atom`: canonical-JSON byte stability across permutations of input; signature roundtrip; multihash round-trip; supersession-chain head resolution; bitemporal point queries (atom valid at `t` with two superseding chains).
- `ffs-core::capability`: action-against-scope evaluation for all six actions and every tier combination; bitemporal capability windows; revocation supersession.
- `ffs-core::predicate`: TOML predicate spec parsing; JSON Schema validation against canonical and adversarial claim payloads; reverse-map rule-table consistency.
- `ffs-core::store` (against in-memory backend): atom insert + lookup; supersession resolution; classification updates; FTS5 query.
- `ffs-fastpath::classifier`: diff classification for each reverse-map kind (single-line text, frontmatter value, additive list); fall-through to slow-path on ambiguous diffs.
- `ffs-federation::handshake`: handshake state machine; certificate-pin updates; capability-filter request construction.

**Property tests** (via `proptest`):

- Round-trip: any signed `AtomEnvelope` re-canonicalizes to the same bytes and same content hash.
- Capability monotonicity: superseding a capability never broadens its scope.
- FTS5 indexes are consistent with `atoms.envelope` after arbitrary insert / supersession sequences.

### Integration Tests

**Strategy**: a `tests/` harness in each binary crate spins up the real daemon against a tmpdir-rooted `~/.ffs/`. Tests exercise the full flow including SQLite, filesystem watchers, JSON-RPC, and skill subprocesses.

**Components co-tested**:

- Daemon + SQLite + filesystem watcher: write a markdown file to `~/.ffs/ingest/`; expect a quarantined proposal within 2s; accept the proposal via JSON-RPC; expect an atom in the store and a re-rendered projection on disk.
- Daemon + plugin (headless): connect a fake JSON-RPC client; subscribe to events; trigger a fast-path edit; expect an `event.fastpath.applied` notification with the new atom hash.
- Daemon + MCP server: spawn a stub MCP-aware agent; have it call `ffs_query`; expect capability-filtered atoms back; have it call `ffs_author_atom` with an out-of-scope claim; expect a structured capability error.
- Daemon + federation transport: stand up two daemons in separate tmpdirs; perform bridge handshake; pull atoms across; verify capability filtering rejects out-of-tier atoms; revoke; verify next pull returns nothing.

**Test data requirements**:

- A canonical fixture set of contact / person / note atoms used across tests.
- Golden markdown inputs for scribe extraction tests, with paired expected proposal outputs.
- A fixture predicate-spec library (`contact.person.toml`, `person.generic.toml`, `note.toml`) checked into the repo.

**Environment dependencies**:

- SQLite + SQLCipher built into the test binary (cargo feature `bundled-sqlcipher`).
- Python 3.11+ for skill subprocess tests.
- A test-only Ed25519 key in tmpdir keychain shim (the keychain integration is wrapped behind a trait; tests use a tmpfile-backed implementation).
- No network: federation tests use loopback; mTLS uses test-generated certificates.

## Development Sequencing

### Build Order

The build order respects the dependency graph from `ffs-core` outward. Each step states which prior steps it depends on.

1. **`ffs-core::atom`**: atom envelope type, JCS canonicalization, Ed25519 signing/verification, BLAKE3 multihash content addressing. — *no dependencies*.
2. **`ffs-core::predicate`**: TOML predicate-spec loader, JSON Schema validator, reverse-map rule parser. — *depends on step 1 (claim payloads embed in atoms)*.
3. **`ffs-core::store`**: SQLite schema + SQLCipher integration; atom write/read; supersession chain resolution; FTS5 indexing. — *depends on step 1 (envelope blob storage)*.
4. **`ffs-core::capability`**: capability evaluator (action × scope × bitemporal window). — *depends on steps 1, 3 (capabilities are atoms in the store)*.
5. **`ffs-core::projection`**: projection renderer (Tera templates per predicate spec); reverse-map–annotated render output. — *depends on steps 2, 3, 4 (renderer applies capability filtering and uses predicate specs)*.
6. **JSON-RPC dispatcher (in `ffs-daemon`)**: UDS / named pipe server; method dispatch; notification publisher. — *depends on steps 1-5 (dispatcher returns substrate state)*.
7. **`ffs-cli`**: argv parser; URL resolver; JSON-RPC client. Distributable as a static binary on its own. — *depends on step 6 (CLI talks to daemon)*.
8. **`ffs-fastpath`**: filesystem watcher integration; diff classifier against reverse-map rules; supersession-or-route-to-ingest decision. — *depends on steps 2, 3, 5, 6 (uses predicate specs, reads/writes atoms, calls dispatcher)*.
9. **`ffs-skills-host`**: subprocess host for Python skills; stdio bridging. — *depends on step 6 (skills call daemon JSON-RPC)*.
10. **Python skills bundle (`scribe`, `librarian`, `auditor`)**: extract atoms from markdown, watch for drift, produce daily reports. — *depends on steps 6, 9 (skills run inside the host and call the daemon)*.
11. **`ffs-federation`**: mTLS HTTPS server and client; bridge handshake; capability-filtered atom serving; pull-sync scheduler. — *depends on steps 4, 6 (capability evaluator and dispatcher)*.
12. **`ffs-mcp`**: MCP server binary translating tool calls to JSON-RPC. — *depends on step 6 (calls daemon)*.
13. **Obsidian plugin**: TypeScript plugin with UDS / named pipe client, projection-folder enumeration interception, daily-health-summary panel, fast-path-aware editing UX. — *depends on steps 6, 8 (uses dispatcher + fast-path events)*.
14. **Onboarding artifacts**: installer scripts, technical-friend-onboarding checklist, predicate-spec starter library files, Tera template starter library. — *depends on steps 1-13 (whole stack must work end-to-end)*.

### Technical Dependencies

Blocking dependencies that must be resolved before each major step:

- **Step 1**: Ed25519, ChaCha20-Poly1305, BLAKE3, multihash, JCS Rust crates available and version-pinned. *All available today*.
- **Step 3**: SQLCipher source bundled and cross-compilable for Linux/macOS/Windows. *Verify cross-compilation matrix early; SQLCipher headers per target are needed*.
- **Step 6**: Tokio + UDS / named pipe support. *Available; named pipe support on Windows requires `tokio` `windows` feature flag*.
- **Step 8**: `notify` crate with debounced event mode. *Available; Windows ReadDirectoryChangesW path-too-long bug needs CI verification*.
- **Step 11**: `rcgen` for cert generation; `axum` + `rustls`; Ed25519 cipher suite in TLS 1.3. *Available*.
- **Step 13**: Obsidian plugin API stability; UDS / named pipe support in Node. *UDS supported via `net.createConnection({ path })`; Windows named pipes have known Node quirks; CI verification required*.

External infrastructure: none. The MVP runs entirely on personal hardware. No cloud services, registries, or shared infrastructure are required for delivery. Optional deployment-time tools (Tailscale, port-forward) are documented but not implemented by FFS.

## Monitoring and Observability

The MVP is single-user, personal-hardware. Observability is local: structured logs the user can inspect, an auditor-produced daily summary atom, and explicit health-summary RPCs the plugin surfaces.

**Key metrics** (recorded as substrate atoms by the auditor on a daily cadence):

- Atom-author rate per author per day.
- Fast-path apply count vs. slow-path-routed count (user-visible signal of fast-path coverage).
- Federation pull success rate per peer.
- Capability-evaluation denials per agent (anomaly detection for misconfigured skills or compromised keys).
- Working-set size; ingest-quarantine queue depth.
- Median and p95 latencies for `projection.render`, `path.list`, `fastpath.submit`.

**Log events** (structured JSON via `tracing`):

- `atom_committed`: atom_hash, entity, predicate, classification, author, source_kind.
- `fastpath_applied`: projection_path, source_atom, new_atom, latency_ms.
- `fastpath_routed_to_ingest`: projection_path, reason, submission_id.
- `federation_pull`: peer_id, atoms_pulled, latency_ms.
- `federation_pull_failed`: peer_id, error_kind.
- `capability_denied`: agent, action, target, reason.
- `predicate_loaded`: predicate_name, version, file_hash.
- `skill_crashed`: skill_name, exit_code, restart_count.

**Alerting thresholds and escalation** (surfaced via daily health summary, not paging):

- More than 10 capability denials per agent per day → flag in summary as "agent X attempted out-of-scope writes".
- Federation pull failure rate above 50% over 24h for any peer → flag in summary as "bridge with peer X is unhealthy; check connectivity".
- Ingest-quarantine queue depth above 100 → flag in summary as "you have a backlog of scribe proposals; consider reviewing".
- Fast-path slow-path ratio inverts (more slow-path than fast-path) → flag as "consider whether predicate specs need additional reverse-map coverage".

There is no cloud-side observability and no SLO. The auditor and daily-health-summary surface are the operational instrumentation.

## Technical Considerations

### Key Decisions

| Decision | Rationale | Trade-off | Alternatives rejected |
|----------|-----------|-----------|------------------------|
| Minimal Rust daemon (vs. OpenClaw) | Static binaries, predictable latency, mature crate ecosystem for FS watchers / crypto / SQLite | Rust learning curve; OpenClaw integration becomes secondary | OpenClaw as primary; both with OpenClaw as secondary |
| Single SQLite DB per substrate | Simplicity at MVP scale; backup is a single file; clone-via-git works trivially | Single-writer constraint; SQLCipher build dependency | Per-predicate DBs; append-only log + materialized views |
| Canonical JSON (JCS) atom envelope | Human readable; debuggable; mature implementations; portable verification | 1.3-2x larger than CBOR/protobuf | Deterministic CBOR; canonical protobuf |
| Ed25519 + ChaCha20-Poly1305 + BLAKE3 | Modern defaults; constant-time; mature audited libraries; multihash gives forward migration | BLAKE3 less universal than SHA-256 | AES-GCM + SHA-256; XChaCha20 + SHA-256; ECDSA-P256 |
| UDS / named pipe + JSON-RPC 2.0 | Zero network exposure; user-permission gating; simple framing | TS named-pipe quirks on Windows | Localhost HTTP/2; gRPC over UDS |
| mTLS pull-based federation | Cryptographic identity at transport; no bearer-token replay; works around NAT one-side-reachable | Self-signed certs unfamiliar to TLS-terminating proxies; revocation latency bounded by heartbeat | Bearer tokens over plain TLS; push-based with relay; A2A wire format |
| TOML + JSON Schema predicate specs | Hand-editable; strict types; reverse-map first-class; JSON Schema is well-tooled | Two formats inside the substrate (TOML + JSON) | JSON-only; YAML-only; Rust-defined predicates |
| Layered testing (unit + property + integration + scenario) | Catches subtle bitemporal/capability bugs; federation correctness validated under multi-process | Longer CI time; scenario harness is real infrastructure work | Unit + integration only; contract-tests-only |

### Known Risks

- **Reverse-map rule mistakes silently mis-author atoms.** Reverse-map TOML changes affect what user edits get saved as. *Mitigation*: golden-file tests for every rule; auditor flags rapid same-field supersessions; predicate-spec atoms are bitemporal so a bad spec is correctable without data loss.
- **SQLCipher cross-compilation friction.** The bundled-sqlcipher cargo feature works but each target needs verified builds. *Mitigation*: CI matrix on three OSes; release-blocking smoke test that opens an encrypted store on each target.
- **Obsidian plugin's Windows named-pipe path.** Node's `net.createConnection` to `\\.\pipe\<name>` has historical quirks. *Mitigation*: CI smoke test on Windows Obsidian; CLI-subprocess fallback if direct IPC fails.
- **Federation handshake UX is unforgiving.** Out-of-band fingerprint exchange is a friction surface for non-technical peers. *Mitigation*: technical-friend onboarding checklist covers handshake; daily-health-summary surfaces handshake failures clearly; Phase 2 may add QR-code fingerprint exchange.
- **Skill subprocess hangs.** Long-running scribe / librarian / auditor invocations can wedge. *Mitigation*: process supervision with per-call timeouts; auto-restart with exponential backoff; auditor flags repeated crashes.
- **Working-set policy is wrong.** PRD § Open Questions flags this. MVP ships "most-recently-touched + user-pinned" as a starter heuristic. *Mitigation*: working-set decisions are easily reversible; refinement informed by Phase 2 telemetry.
- **Capability evaluator subtle bugs.** Bitemporal evaluation × scope intersection × tier classification is non-trivial. *Mitigation*: property tests for monotonicity, no-broadening-on-supersession, and time-window correctness; capability denials surfaced in daily summary.
- **MCP capability-check correctness.** ADR-013 commits to "capability checks fire at the MCP boundary." A bug here lets agents author out-of-scope atoms. *Mitigation*: the MCP server delegates capability evaluation to `ffs-core::capability` rather than implementing its own; integration tests target capability denial cases specifically.

## Architecture Decision Records

The complete ADR set lives in `adrs/`. ADR-001 through ADR-014 originated with the PRD and capture product-shape decisions. ADR-015 through ADR-021 originated with this TechSpec and capture technical-implementation decisions.

PRD-level ADRs:

- [ADR-001: Records-shaped substrate, not file-shaped](adrs/adr-001.md) — Substrate is in the iCloud Notes lineage, not OneDrive.
- [ADR-002: Both audiences first-class](adrs/adr-002.md) — Developer and end-user audiences are equally primary.
- [ADR-003: Substrate-First MVP](adrs/adr-003.md) — Working surfaces over polish; breadth over depth.
- [ADR-004: Three motivating scenarios all in MVP](adrs/adr-004.md) — Contact-graph sovereignty, home-claw absorption, day-one-clone-and-collaborate.
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Real files on disk for any editor; fast-path for trivial edits.
- [ADR-006: `ffs://` URL scheme as public stable contract](adrs/adr-006.md) — Universal addressing modeled on `s3://`.
- [ADR-007: Personal federation in MVP, organizational deferred](adrs/adr-007.md) — Same primitive at two scales; MVP ships personal scale.
- [ADR-008: Speak MCP and A2A at boundaries](adrs/adr-008.md) — Standards integration over invention.
- [ADR-009: Claw integration via OpenClaw or Hermes pattern](adrs/adr-009.md) — FFS contributes agent definitions in claw-shape format.
- [ADR-010: MCP server deferred to Phase 2](adrs/adr-010.md) — *Superseded by ADR-013.*
- [ADR-011: Path library starts at three (contacts/people/notes)](adrs/adr-011.md) — Decisions, projects, questions, action-items, policies are Phase 2.
- [ADR-012: Bilateral federation in MVP, multi-peer aggregation in Phase 2](adrs/adr-012.md) — MVP proves the primitive; aggregation builds on it.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — Six tools, capability-checked. Supersedes ADR-010.
- [ADR-014: Minimum-viable fast-path for trivial projection edits in MVP](adrs/adr-014.md) — Reverse-map annotations on the three MVP predicate types.

TechSpec-level ADRs:

- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Resolves PRD open question; commits the language stack.
- [ADR-016: Single SQLite database per substrate with normalized atom store](adrs/adr-016.md) — Schema layout, indexing strategy, SQLCipher integration.
- [ADR-017: Canonical JSON (RFC 8785 JCS) atom envelope](adrs/adr-017.md) — Long-lived interoperability contract for signed atoms.
- [ADR-018: Cryptographic primitives — Ed25519, ChaCha20-Poly1305, BLAKE3](adrs/adr-018.md) — Algorithm commitments and key management.
- [ADR-019: Local IPC via Unix domain socket / Windows named pipe with JSON-RPC 2.0](adrs/adr-019.md) — Local API transport and method shape.
- [ADR-020: Federation transport — mTLS over HTTPS with pull-based sync](adrs/adr-020.md) — Bridge handshake, transport endpoints, revocation propagation.
- [ADR-021: Predicate spec format — TOML with embedded JSON Schema](adrs/adr-021.md) — Predicate spec authoring format and reverse-map annotations.
