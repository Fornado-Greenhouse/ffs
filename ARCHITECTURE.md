# Architecture

The Foley File System (FFS) is a records-shaped substrate for personal knowledge that you own, classify, and share at sub-record granularity with peers. This document is the architectural orientation: what FFS is, how it's structured, what invariants the implementation maintains, how concurrency and security work, and where to look for more depth.

This doc is meant to be self-contained on first read. The full product specification, technical specification, and 21 architecture decision records live under [`.compozy/tasks/ffs-mvp/`](.compozy/tasks/ffs-mvp/) and are linked by name throughout.

---

## At a glance

FFS MVP is a Rust-based local substrate that:

- Stores **signed, classified, bitemporal records** (called *atoms*) in a per-user SQLite database.
- Exposes them through a long-running **daemon** over a Unix domain socket / Windows named pipe with a JSON-RPC 2.0 API.
- Materializes opinionated **projection paths** to disk so any editor can navigate the substrate as files.
- **Federates bilaterally** with peer substrates over mTLS HTTPS, with capability-checked atom serving and pull-based sync.

The MVP target is a small community (5–20 trusted peers) using federation in real workflow. Phase 2/3 expand on that base; nothing in MVP is throwaway scaffolding for later phases.

The primary technical trade-off is heterogeneity for predictability: three toolchains (Rust, TypeScript, Python) instead of one, in exchange for a static-binary CLI on every platform, sub-200ms fast-path latency without GC jitter, and a familiar agent-skill shape that remains portable to future hosts.

---

## The shape

```
┌───────────────────────────────────────────────────────────────────────┐
│                            FFS Substrate                              │
│                                                                       │
│  ┌──────────────┐     ┌───────────────────────────────────────────┐   │
│  │  ffs CLI     │ ──▶ │           ffs-daemon (Rust)               │   │
│  └──────────────┘     │  ┌─────────────────────────────────┐      │   │
│  ┌──────────────┐     │  │  JSON-RPC 2.0 dispatcher        │      │   │
│  │  Obsidian    │ ──▶ │  │  (UDS / named pipe)             │      │   │
│  │  plugin (TS) │     │  └─────────────────────────────────┘      │   │
│  └──────────────┘     │  ┌─────────────────────────────────┐      │   │
│  ┌──────────────┐     │  │  ffs-core: atom store,          │      │   │
│  │  ffs-mcp     │ ──▶ │  │  capability evaluator,          │      │   │
│  │  (Rust)      │     │  │  projection renderer            │      │   │
│  └──────────────┘     │  └─────────────────────────────────┘      │   │
│      ▲                │  ┌──────────────┐  ┌────────────────┐     │   │
│      │                │  │ FS watcher + │  │ skills-host:   │     │   │
│  MCP-aware            │  │ fast-path    │  │ scribe / lib./ │     │   │
│  external agents      │  │ classifier   │  │ auditor (Py)   │     │   │
│                       │  └──────────────┘  └────────────────┘     │   │
│                       │  ┌─────────────────────────────────┐      │   │
│                       │  │  ffs-federation: HTTPS + mTLS,  │      │   │
│                       │  │  bridge handshake, pull sync    │      │   │
│                       │  └─────────────────────────────────┘      │   │
│                       └─────────────┬─────────────────────────────┘   │
│                                     ▼                                 │
│                               ┌──────────┐                            │
│                               │ store.db │ (SQLite + SQLCipher)       │
│                               └──────────┘                            │
│                                                                       │
│  Filesystem surfaces:                                                 │
│    ~/.ffs/ingest/      (write-anything firehose)                      │
│    ~/.ffs/contacts/    (projections — materialized working set)       │
│    ~/.ffs/people/      (projections)                                  │
│    ~/.ffs/notes/       (projections)                                  │
│    ~/.ffs/config/      (predicate specs, templates — git-versioned)   │
└───────────────────────────────────────────────────────────────────────┘
                                  │ mTLS HTTPS
                                  ▼
                        Other FFS substrates (peers)
```

**Three folder spaces, three semantics:**

- `~/.ffs/ingest/` is a write-anything firehose. Humans, AI agents, and CLI tools all write here. The scribe absorbs.
- `~/.ffs/contacts/`, `~/.ffs/people/`, `~/.ffs/notes/` are *projections* — virtual paths backed by the substrate, materialized to disk on demand for the working set.
- `~/.ffs/config/` holds predicate specs, templates, and capability templates. Edited by humans, git-versioned, hot-reloaded by the daemon.

The seven Rust crates and their responsibilities are documented in [TechSpec § System Architecture](.compozy/tasks/ffs-mvp/_techspec.md) and [ADR-015](.compozy/tasks/ffs-mvp/adrs/adr-015.md).

---

## Core abstractions

### The atom envelope

An *atom* is a signed, content-addressed record about an entity. It carries a predicate name, a structured claim, an author, bitemporal validity, classification, supersession link, and provenance.

```rust
pub struct AtomEnvelope {
    pub v: u32,                         // schema version, currently 1
    pub entity: EntityId,               // multibase-encoded
    pub predicate: PredicateName,       // e.g. "contact.person"
    pub claim: serde_json::Value,       // validated against predicate's claim_schema
    pub author: PublicKey,              // Ed25519 multibase
    pub valid_from: Iso8601,
    pub valid_to: Option<Iso8601>,
    pub tx_time: Iso8601,
    pub classification: Tier,           // "existence" | "work_email" | ...
    pub supersedes: Option<Multihash>,  // None for root atom in a chain
    pub provenance: Vec<Provenance>,
    pub signature: Signature,           // Ed25519 over JCS bytes (sig field elided)
}
```

The envelope is serialized as canonical JSON ([RFC 8785 JCS](https://datatracker.ietf.org/doc/html/rfc8785)). The signature covers the JCS bytes with the `signature` field elided. The content address is `multihash(blake3(jcs_bytes))` with codec `0x1e`. All public keys, signatures, and hashes are encoded as base58btc multibase strings (prefix `z`).

This format is a long-lived public contract. Tools in any language can verify FFS atoms with a JSON parser, an Ed25519 verifier, a JCS implementation, and a BLAKE3 hasher. See [ADR-017](.compozy/tasks/ffs-mvp/adrs/adr-017.md) (envelope format) and [ADR-018](.compozy/tasks/ffs-mvp/adrs/adr-018.md) (crypto primitives).

### Capability as data

Authorization in FFS is data, not code. A *capability* is itself an atom, with `predicate = "capability.grant"` and a claim of the shape:

```json
{
  "grantee": "z6MkhaXg...",
  "actions": ["read", "supersede"],
  "scope": {
    "predicates": ["contact.person"],
    "entities": null,
    "classifications": ["existence", "work_email"],
    "tier": "introducible"
  },
  "valid_from": "2026-05-08T00:00:00Z",
  "valid_to": null
}
```

The capability is signed by the granting author and stored like any other atom. Revocation is supersession. The daemon's capability evaluator (`ffs-core::capability`) takes `(agent, action, target, as_of)` and returns `Allow` / `Deny` against the active capability set at `as_of`. There is no imperative policy code — policy lives in atoms and is queryable.

See [ADR-007](.compozy/tasks/ffs-mvp/adrs/adr-007.md) (personal federation in MVP) and [ADR-013](.compozy/tasks/ffs-mvp/adrs/adr-013.md) (MCP-boundary capability checks).

---

## Invariants

These properties hold in every supported configuration. Code, tests, and reviews defend them.

1. **Atoms are immutable.** Change happens through supersession (a new atom with `supersedes = <prior_hash>`). The substrate stores both. Erasure is a separate, structured operation per [PRD § Data privacy and security](.compozy/tasks/ffs-mvp/_prd.md).
2. **Every atom is signed by exactly one author.** Unsigned atoms never enter the store. The store rejects them at insert.
3. **Capability evaluation happens at the source substrate.** A federation pull is filtered by the source's capability set against the requester's identity; the receiver does not gate its own atoms on the receiver's capabilities.
4. **Projections are derived; the substrate is canonical.** Projection files on disk are a cache. The atom store is the source of truth. Projection drift resolves to the substrate.
5. **The atom envelope is a long-lived public contract.** Breaking change requires bumping the `v` field with a documented migration. The format is otherwise stable forever.
6. **The `ffs://` URL scheme is stable forever.** No `ffs2://`. New addressing modes extend the scheme without versioning the prefix.
7. **Federation atoms cross only when authorized by a capability atom.** The capability is signed by the source; the receiver records both the atom and the capability hash that authorized its arrival.
8. **Possession of an atom hash does not grant access to its content.** The substrate is content-addressed but not content-distributed. Capability checks gate retrieval at the source.

---

## Concurrency model

The substrate is local-first, eventually consistent across federation, and uses a single-writer model per substrate. The seven rules:

**1. Supersession chains are trees.** Two atoms can both point to the same parent (e.g., a local edit and a federation pull each authoring a successor before either sees the other). Head selection at `(entity, predicate, as_of)` is the unique non-superseded leaf; ties between multiple unsuperseded leaves resolve to the leaf with the latest `tx_time`, breaking further ties on the atom's content hash. Multi-leaf states surface in the auditor's daily summary; the user supersedes explicitly to disambiguate.

**2. Federation atoms can arrive before their `supersedes` referent.** Y supersedes X; if Y arrives first (asymmetric capability state, partial bootstrap), FFS accepts it. X-with-no-superseder is treated as a candidate head until X arrives. Dangling supersedes pointers are not rejection conditions.

**3. The daemon's own writes do not feed back as filesystem events.** Every daemon-induced file write records the file's expected post-write content hash; the FS watcher compares incoming events to the expected hash and ignores matches. Edits that beat the watcher's debounce window reconcile via the next render-hash check.

**4. Predicate-spec hot-reload does not block in-flight writes.** Spec swaps are atomic against the validator; in-flight writes complete with their pre-swap validator; new writes after the swap use the new validator. A spec change can briefly accept claims that would fail under the new spec — acceptable, since spec changes are user-initiated and rare.

**5. Multi-peer queries are best-effort, not transactional.** An intersection across peers A and B fetches each at slightly different moments; the result reflects what was visible at fetch time per peer, not a globally consistent snapshot.

**6. There is one logical writer per substrate.** Local edits, scribe-accepted proposals, fast-path supersessions, and federation pulls all enqueue to a single tokio writer task that funnels them through SQLite. **At any moment, exactly one atom commit is in progress.** Reads use SQLite WAL snapshot isolation and run concurrently. Write parallelism would be a Phase 3+ change with its own ADR.

**7. There is no cross-substrate causality tracking.** `tx_time`s from different substrates are not comparable; FFS does not implement vector clocks. Causality propagates only through `supersedes` pointers and bitemporal `valid_*` ranges. Cross-substrate "what did peer A know at time T" queries are not supported in MVP.

---

## Security model (AARM-conformant)

FFS treats security as runtime action mediation, not perimeter defense. The architecture aligns with [AARM](https://aarm.dev) — Autonomous Action Runtime Management, a specification for securing AI-driven actions at runtime — and the alignment is by construction, not retrofit.

### AARM components mapped to FFS

| AARM component | FFS subsystem |
|---|---|
| **Action Mediation** | `ffs-core::capability` evaluator at every RPC entry; MCP-server boundary; daemon dispatcher |
| **Context Accumulator** | The atom store: every action produces a signed atom with provenance |
| **Policy Engine** | `ffs-core::capability` evaluating capability atoms |
| **Approval Service** | `ingest_quarantine` + daily-health-summary panel for scribe and MCP-agent proposals |
| **Deferral Service** | Slow-path ingest routing for ambiguous fast-path edits; federation pull deferral on transient failure |
| **Receipt Generator** | Ed25519 signatures + BLAKE3 multihash + supersession chains |
| **Telemetry Exporter** | `tracing` JSON output; auditor daily-summary atoms; `ffs_audit_query` MCP tool |

### Action taxonomy

AARM classifies actions in four categories. FFS implements three fully and one partially:

- **Forbidden** — capability evaluator returns `Deny` for any action outside scope; daemon refuses with a structured error. Fully implemented.
- **Context-Dependent Allow** — scribe (or MCP agent) proposes; user reviews in daily summary; accepted proposal becomes a signed atom. The proposal-quarantine flow is exactly this. Fully implemented.
- **Context-Dependent Defer** — ambiguous fast-path classifications route to ingest as corrections; federation pulls during transport failure defer to next heartbeat. Fully implemented.
- **Context-Dependent Deny** — anomaly-driven runtime deny (e.g., "agent X authored 1000 atoms in 5 minutes; pause writes from X"). MVP currently produces auditor flags but does not act on them at runtime. **Phase 2 work.**

### Threat model

- **Trusted**: the user's hardware, OS keychain, daemon process.
- **Partially trusted**: federation peers — capability-checked and signature-verified, but their queries reveal what's being asked. Private set intersection (PSI) is Phase 2 for use cases that need it.
- **Untrusted**: network in transit (mTLS); claims authored by other agents (capability-checked + signature-verified before any local effect).
- **Out of scope for MVP**: malware on the user's host, shoulder-surfing, subpoenas of the host machine, side channels.
- **Acknowledged but unmitigated**: query-metadata leakage during federation pulls.

Key compromise is treated CA-style: the user supersedes the compromised key's identity atom, notifies peers out-of-band, and re-issues. There is no automatic revocation propagation in MVP beyond the receiving peer's next pull.

### Anti-patterns (organized by AARM component)

These are the ways an implementation slips out of conformance. Each rule cites the component it preserves.

**Action Mediation:**
- DON'T read `store.db` directly from another process. It's SQLCipher-encrypted and the schema may evolve. Go through the daemon's RPC surface.
- DON'T write to projection paths from outside the daemon's fast-path. You'll create a phantom diff at best, and bypass capability checks at worst.

**Policy Engine:**
- DON'T bypass the capability evaluator. Every read or write that touches user data must route through `ffs-core::capability`.
- DON'T treat capabilities as transitive. A grant does not authorize the grantee to re-grant unless the capability claim explicitly allows it.
- DON'T trust a predicate's filename. The `name` field inside the spec is authoritative.

**Receipt Generator:**
- DON'T author atoms outside the daemon. Atoms must enter through the writer task so the receipt chain stays intact.
- DON'T let a skill author atoms claiming a different agent's identity. Skill identity is bound at subprocess spawn and the daemon enforces signing-key isolation.
- DON'T conflate `tx_time` with "when it happened in the world." That's `valid_from` / `valid_to`. `tx_time` is when the substrate received the claim.

**Approval Service:**
- DON'T approve quarantined proposals in batches without per-item review. The approval is the human-in-the-loop fence.
- DON'T let agents auto-supersede their own atoms without human review when the supersession crosses a tier boundary.

**Telemetry Exporter:**
- DON'T expose `audit_query` results without re-evaluating capability. Telemetry must not leak across capability boundaries.

### Acknowledged gaps

These are real gaps, not anti-patterns to avoid:

1. **Anomaly-driven Context-Dependent Deny is not implemented in MVP.** The auditor produces threshold flags ("agent X had 11 capability denials") in the daily summary; the runtime does not act on them. Phase 2 work — the gap is honest but real.

2. **The Ed25519 signing key is shared between substrate authorship and federation mTLS identity** (per [ADR-020](.compozy/tasks/ffs-mvp/adrs/adr-020.md)). One identity, one key — but cross-protocol use of the same key is a category of risk that warrants explicit analysis. Pre-1.0 work: either justify the choice with a signing-context domain-separation argument, or rotate to two keys with a documented binding atom.

---

## Engineering envelope

### Performance budgets

The MVP architecture targets personal-scale workloads:

- **Folder navigation**: under 200ms for projection-path enumeration.
- **Projection rendering**: under 500ms on file open.
- **Fast-path edit acknowledgement**: under 200ms from save to atom commit.
- **CLI queries**: under 1s for `ffs cat` / `ffs ls` / `ffs get`.
- **Edit absorption (slow-path)**: under 1 minute from filesystem write to atom commit.
- **Federation revocation propagation**: bounded by heartbeat (default 60s, tunable to 10s).

Workload assumption: 10K–100K atoms per substrate, 5–20 federation peers, single user per substrate.

### Scaling envelope

The single-writer SQLite design is sized for personal scale. The cliff sits around 1M atoms or sustained write rates above ~50/sec, where SQLite's serialized writer becomes the bottleneck. Phase 3+ work addresses partitioning (per-predicate-family databases) or a non-SQLite backend if the architecture's reach grows beyond personal hardware.

### Stability commitments

Pre-1.0, breaking changes are possible everywhere with notice. Post-1.0, this surface is locked:

**Stable:**
- The `ffs://` URL scheme.
- The atom envelope shape (the `v` field is the migration knob).
- The JSON-RPC method set used by the CLI, Obsidian plugin, and MCP server.
- The six MVP MCP tool signatures.
- The TOML predicate-spec format.

**Internal (free to change):**
- The SQLite schema.
- The daemon's internal Rust modules.
- The Tera template macros and rendering internals.
- The skill-subprocess wire protocol.

### What's not built yet

MVP excludes, by design:

- Anomaly-driven runtime deny (Phase 2).
- Multi-peer federation aggregation (Phase 2).
- Private set intersection for federation queries (Phase 2).
- Expanded path library — decisions, projects, questions, action-items, policies (Phase 2).
- ZTM as federation transport (Phase 2).
- Organizational federation with formal contracts (Phase 3).
- TEE-hosted deployments (Phase 3).
- Cryptographic verbs beyond Ed25519/ChaCha20-Poly1305/BLAKE3 (Phase 3+).

Full roadmap: [PRD § Phased Rollout Plan](.compozy/tasks/ffs-mvp/_prd.md).

---

## Where to find more

- [Product Requirements Document](.compozy/tasks/ffs-mvp/_prd.md) — what FFS is for and who it's for.
- [Technical Specification](.compozy/tasks/ffs-mvp/_techspec.md) — implementation design, build order, integration points.
- [Architecture Decision Records](.compozy/tasks/ffs-mvp/adrs/) — 21 ADRs covering product (001–014) and technical (015–021) decisions.
- [Task breakdown](.compozy/tasks/ffs-mvp/_tasks.md) — 23-task implementation plan in dependency order.
- [AARM specification](https://aarm.dev) — Autonomous Action Runtime Management.

### Contributing

A few rules of the road:

- All public-type changes get an ADR.
- Cross-cutting changes (atom envelope, capability evaluator, federation transport) require a security-review note.
- Property tests are required for any logic with state-machine flavor; integration tests are required for cross-process flows.
- Format and lint gates are non-negotiable. CI runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo nextest run --workspace --all-features` on Linux/macOS/Windows.
- Federation-relevant changes consider peer-version skew: peers won't upgrade in lockstep.

`CLAUDE.md` covers operational discipline for agentic operators (test runner, shell tool conventions, ADR numbering). `CONTRIBUTING.md` (forthcoming) covers branch strategy, review expectations, release cadence.
