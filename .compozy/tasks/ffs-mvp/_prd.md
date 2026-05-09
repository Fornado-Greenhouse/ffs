# Foley File System (FFS) — Product Requirements Document

**Status:** Draft
**Date:** 2026-05-05

## Overview

FFS is a substrate for records-shaped knowledge with provenance, classification, and capability. It exists because the SaaS-and-files era leaves knowledge workers without three things their work requires: ownership of their data, sub-record access control, and durable history with attribution. Existing tools — CRMs, file shares, knowledge bases, contact managers — handle one or two of these concerns at most, and only inside walls the user does not control.

FFS provides those three things at the substrate layer. Applications built on FFS inherit them. Users running FFS gain them whether or not specific applications exist yet for their use case.

The substrate's value is felt most acutely by relationship-holders whose livelihoods depend on networks they currently do not own — salespeople, journalists, founders, community organizers. The first application built on FFS, Folodex, addresses that audience directly. But FFS is not specific to contacts; the same substrate handles general knowledge capture, AI-agent output absorption, and team collaboration with the same primitives.

This document specifies FFS the substrate. Folodex and other applications built on it have their own product requirements; references to Folodex here are illustrative.

## Goals

The MVP succeeds when a small community of 5-20 trusted peers is using FFS for real, with personal federation working at small scale.

The success indicators, treated equally:

- **Retention.** The community is still using FFS daily 90 days after onboarding. Installs that don't translate into use don't count.
- **Real federation events.** Peers are actually authoring capabilities, mounting each other's graphs, computing intersections, and revoking — not just reading documentation about it.
- **Emergent value.** At least one introduction or collaboration happens through the intersection-computation feature that wouldn't have happened without FFS.

The MVP does not target ecosystem-scale adoption, public release, or commercial revenue. Those are later concerns. The bar is small-community validation.

There is no fixed timeline. Scope drives schedule.

## User Stories

### Primary persona: the relationship-holder (end user)

- As a salesperson, I want to own my contact graph independently of my employer's CRM, so that my network survives job changes.
- As a salesperson, I want to share specific contacts with a trusted peer at a tier I control (existence only, work email, full notes), so that I can collaborate without exposing my whole Rolodex.
- As a salesperson, I want to see who my peer and I both know, so that I can find paths to introductions I couldn't make alone.
- As a salesperson, I want to revoke a peer's access at any time, with the revocation taking effect immediately and being recorded in an audit trail.

### Primary persona: the application developer (developer)

- As a developer building an application like Folodex, I want a substrate that handles records, signatures, classification, and federation, so that I can build features on top rather than reinventing them.
- As a developer, I want a stable URL scheme (`ffs://`) for addressing atoms and projections, so that my application can reference substrate content from outside the substrate.
- As a developer, I want a CLI for substrate operations during development, so that I can inspect, query, and manipulate without writing application code.

### Secondary persona: the technically capable end user

- As a power user, I want to clone an FFS substrate from git and start contributing, so that joining a team's knowledge base is friction-free.
- As a power user, I want my AI agents to write markdown into a folder that the substrate absorbs and structures, so that I get queryable claims from agent output without manual organization.
- As a power user, I want to edit projections in any text editor (Obsidian, Notepad, Vim, VS Code) and have my edits absorbed by the substrate, so that my tool choice is mine.

### Secondary persona: the non-technical peer

- As a non-technical peer onboarded by a technical friend, I want FFS to install and run with light support and no terminal use during normal operation, so that I can participate in the community without being blocked on technical complexity.

## Core Features

### Atoms — the substrate of record

The substrate stores **atoms**, which are signed, time-stamped, classified records about entities. Each atom carries: an entity reference, a predicate identifying what kind of claim it makes, the claim content (which may be structured, prose, or both), an author identity, a cryptographic signature, valid time and transaction time, classification, and provenance pointing to source material.

Atoms are immutable. Change happens through **supersession**: a new atom asserts a corrected claim and references the prior one. Both remain queryable; the head of the chain is current state.

Atoms are content-addressed using the multihash format, allowing forward migration to new hash algorithms without breaking historical references.

### Three folder spaces

The file system, where it appears in FFS, is three distinct surfaces with non-overlapping semantics.

**Ingestion** (`~/.ffs/ingest/`) is a write-anything firehose. Humans typing in Obsidian, AI agents writing markdown, CLI tools dropping files all write here. The folder is tolerant of any markdown structure. The substrate watches the folder; the scribe agent absorbs new content into atoms with provenance pointing back to source files.

**Projection** (`~/.ffs/contacts/`, `~/.ffs/people/`, `~/.ffs/notes/`) is a navigable filing cabinet. Every folder is a query against the substrate. Every file is a rendered atom (or atoms) about an entity, computed on demand. The MVP path library covers contacts, people, and a generic notes path; richer libraries are Phase 2.

**Configuration** (`~/.ffs/config/`) holds predicate definitions, path-library customizations, and capability templates. Edited by humans; git-versioned.

The three surfaces never overlap; the naming convention makes this visible.

### Editor-agnostic working set

Projection files are real files on disk, materialized by a local daemon. Any editor that opens files — Obsidian, Notepad, Vim, VS Code, TextEdit — can open them. The daemon manages a working set of materialized projections (recently-touched, user-pinned), refreshing them when underlying atoms change. Operating-system-level filesystem watchers detect edits to projection files; edits made through any tool are absorbed into the ingest path as correction notebook entries with provenance linking back to the projection.

This makes FFS genuinely a file system rather than an Obsidian extension. The Notepad-and-Explorer workflow works.

### Capability as data

Authorization is data, not code. A **capability** is an atom that grants an author the right to perform a class of actions (read, write, supersede, erase, classify, federate) within a scope (predicates, entities, classifications, tiers). Capabilities have declarative conditions evaluated by a fixed evaluator, are bitemporal (with valid windows), and revocable (revocation is supersession).

There is no imperative policy code; policy lives in atoms and is queryable.

### Sub-record classification

An atom's classification is itself a claim, asserted by an authorized author. Multiple classification claims about the same atom can exist; conflicts surface as live questions. Sub-document granularity is structural — a contact entity decomposes into atoms with potentially different classifications (existence, work email, personal email, notes), enabling tier-based selective sharing.

### Personal federation between FFS graphs

Two FFS instances bridge bilaterally. Each side authors capability atoms granting the other tier-scoped views. The receiving substrate honors the capabilities at the source; only authorized atoms cross.

The MVP federation feature set:

- **Bilateral capability authoring.** I grant a peer a tier-scoped view; they grant me one in return.
- **Peer mounting.** The peer's contacts appear in my `~/.ffs/contacts/from/<peer>/` folder, navigable like my own.
- **Tier-based selective sharing.** Existence, work email, personal email, and notes are separately classifiable atoms; capabilities can grant any subset.
- **Intersection computation.** A path surfaces who-the-peer-and-I-both-know.
- **Revocation flow.** I supersede the capability; the peer's view disappears; the audit trail records it.

Multi-peer aggregation (one path showing intersection across many peers) is Phase 2.

### `ffs://` URL scheme

A universal addressing scheme modeled on `s3://`. The form is `ffs://<graph>/<address>[?<query>]` with three addressing modes:

- **Path** addressing: `ffs://my-graph/contacts/by-name/S/`
- **Atom** addressing: `ffs://my-graph/atom/<content-hash>`
- **Entity** addressing: `ffs://my-graph/entity/<entity-id>`

Bitemporal queries are URL parameters: `?as_of=2026-04-15`, `?valid_at=2026-Q1`.

The scheme is a public, stable contract. It addresses things; it does not perform operations. Tools apply verbs to URLs.

### `ffs` CLI tool

A single static binary per platform, distributed for Linux, macOS, and Windows. Resolves `ffs://` URLs in shell pipelines:

- `ffs cat ffs://my-graph/contacts/recent/` returns markdown.
- `ffs ls ffs://my-graph/projects/active/` returns a list.
- `ffs get ffs://my-graph/atom/<hash>` returns an atom as JSON.
- `ffs cat ffs://my-graph/decisions/?as_of=2026-04-15` returns historical state.

Shell-friendly: pipes, exit codes, plain output by default, `--json` for structured output.

### Obsidian plugin

The end-user-surface for the technical half of the mixed-capability community. Functional but minimal in MVP:

- Folder enumeration interception for projection paths (paginated structured listings, not flat directories of thousands).
- On-demand projection rendering when a user opens a projection file.
- Edit-of-projection routed to ingest as a correction notebook entry.
- Visual treatment distinguishing projections (read-with-care) from notebook entries (free editing).
- Entity-name search hooked into Obsidian's file finder.
- Daily health summary panel with five items maximum: pending scribe proposals, open questions, drift flags.

Richer plugin features (full substrate search, faceted search across predicates, working-set heuristics, real-time invalidation) are Phase 2.

### Scribe and ingest absorption

The scribe agent reads markdown from `~/.ffs/ingest/` and proposes atoms with provenance back to source files. It is tolerant of malformed input — accepting whatever shows up, inferring what it can, surfacing structural ambiguity for human review. Proposals are quarantined; they become signed atoms only on user acceptance.

Other AI agents (Claude Code, OpenClaw skills, Hermes) can also write to the ingest folder; the substrate absorbs from any writer, with the scribe handling the structure-inference work.

### Direct projection editing (minimum-viable fast-path)

Users editing projection files in any editor (Obsidian, Notepad, Vim, VS Code) see trivial edits reflected immediately rather than queued for review. The MVP fast-path handles a tightly-scoped set of edits:

- **Single-line text-field edits.** Changing the value of a field rendered from a single atom field — a name, an email, a phone number, a date — is detected via reverse-map annotations on predicate specs and authored as a supersession atom within milliseconds.
- **Frontmatter value edits.** Changing a frontmatter value (e.g., `tier: introducible` to `tier: discreet`) is detected and authored as a supersession atom on the corresponding classification or property.
- **Additive edits to designated sections.** Adding a new bullet to a section marked as additive in the predicate's rendering convention (e.g., a new note line in the Notes section of a contact) is authored as a new atom.

The fast-path applies only to projections rendered from the three MVP predicate types (contact-related person predicates, generic person predicates, note predicates). Reverse-map annotations on those predicate specs make fast-path inference reliable.

Edits the fast-path does not handle — multi-paragraph restructuring, substantial deletions, edits that span multiple atoms ambiguously, edits to federated projections (peer contacts), edits to projections rendered from predicates without reverse-map annotations — route to the ingest folder as correction notebook entries, the way the existing architecture handles all edits. The user reviews these corrections in their daily health summary; the scribe proposes atoms; on acceptance they become supersessions.

The user sees fast-path edits reflected within ~200ms (optimistic UI update with substrate confirmation). Slow-path edits surface as items in the daily health summary within minutes.

This makes the editor-agnostic commitment real for writes as well as reads. Alice fixing a typo in Notepad sees her fix reflected immediately. Alice reorganizing a contact's notes sees a "review pending" item in her daily summary. Both behave correctly; she's not surprised in either case.

### FFS-MCP server

A local MCP (Model Context Protocol) server exposes the substrate to any MCP-aware AI agent. Agents read and write the substrate without bespoke integration. The server is a thin wrapper over the substrate's existing API; the substantial work is the substrate, not the MCP layer.

The MVP MCP server exposes six tools:

- **`ffs_query`** — query the substrate by entity, predicate, or time range; capability-checked; returns matching atoms with metadata.
- **`ffs_render_projection`** — render a projection at a path; equivalent to `ffs cat` over MCP; returns markdown.
- **`ffs_resolve_url`** — resolve an `ffs://` URL; returns the underlying atoms or rendered content.
- **`ffs_author_atom`** — propose an atom into the ingest path with provenance; capability-checked at signing; returns the proposed atom's identifier for tracking through scribe review.
- **`ffs_inspect_predicate`** — return a predicate spec so an agent can understand the substrate's vocabulary before authoring or querying.
- **`ffs_audit_query`** — run audit queries (recent atoms, supersessions, classifications, capability events).

Capability checks fire at the MCP boundary; an agent receives only what its identity is entitled to. An agent attempting to author an atom outside its capability scope receives a structured error rather than silent failure.

The MCP server runs alongside the local daemon. Agents connect via the standard MCP transport (stdio or SSE per the agent's configuration). The Obsidian plugin and CLI continue to use the substrate's local API directly; the MCP server is for MCP-aware external agents.

This makes the home-claw absorption scenario fully bidirectional: agents writing to the ingest folder (via filesystem) and reading substrate state (via MCP) work in the same MVP.

### Daemon

A long-running local process that hosts the FFS agents, exposes the substrate to the Obsidian plugin and CLI, manages the working set of materialized projection files, watches the filesystem for edits, and coordinates federation operations with peers.

The daemon's implementation as either an OpenClaw/Hermes skill set or a minimal FFS-specific process is an implementation choice, not a product specification.

## User Experience

### Onboarding by a technical friend

A non-technical peer (a salesperson, for example) onboards with help from a technical friend. The friend installs FFS on the peer's hardware, sets up the daemon, configures the keychain, runs through a first-use checklist with them. Total time: under an hour. After onboarding, the peer uses FFS through Obsidian without further terminal use.

### Daily use

The peer captures contacts and notes by typing in Obsidian into their `~/.ffs/ingest/` vault. They review scribe proposals at their pace through the daily health summary. They navigate `/contacts/recent/`, `/contacts/by-org/AcmeCorp/`, etc., to find existing contacts. They edit a projection in Obsidian to fix a typo; the fast-path detects the edit, authors a supersession atom, and the projection re-renders within ~200ms — they see the corrected name immediately. If they make a more substantial change (reorganizing notes, deleting paragraphs), the edit routes to ingest as a correction; the user reviews and accepts the change in their next daily health summary check.

### Personal federation

The peer sets up a federation bridge with a trusted friend. They walk through a tier-selection flow — what to share at each tier (existence only, work email, full notes), what to keep private, what to mark introducible. They see the friend's contacts appear in `~/.ffs/contacts/from/<friend>/`. They navigate the intersection at `~/.ffs/contacts/intersection/with/<friend>/` to find shared contacts. When the friendship cools or trust changes, they revoke the capability; the friend's view disappears.

### Editor-agnostic baseline

A user who opens Windows Explorer, navigates to `C:\Users\<them>\ffs\contacts\by-name\S\`, and double-clicks `Sarah_Chen.md` in Notepad sees the rendered contact card. They can read it. They can edit it. A typo fix is reflected within seconds — the daemon detects the save, the fast-path authors the supersession atom, the projection re-renders, the file on disk updates. Notepad on Windows 11 prompts to reload; on Windows 10, the user reopens to see the canonical version. The same flow works in Vim, VS Code, TextEdit, or any other editor that opens files.

### AI agent workflow

An AI agent (Claude Code, an OpenClaw skill, Hermes) running on the user's hardware reads and writes the substrate via the FFS-MCP server. The agent queries `ffs_query` to find recent decisions about a project; renders projections with `ffs_render_projection` to understand context; authors new atoms with `ffs_author_atom` when it has structured output to commit. The agent's identity is bound to a capability scope; it sees and writes only what's authorized. The user reviews proposed atoms in the daily health summary the same way they'd review scribe proposals.

### Developer workflow

A developer building an application against FFS uses the `ffs` CLI to inspect the substrate during development. They construct `ffs://` URLs to reference specific atoms and projections. They write code that reads from and writes to the substrate via the local API or via the FFS-MCP server (depending on whether their application is MCP-native). Their code is testable against a real local FFS instance.

### Accessibility and discoverability

The MVP does not commit to specific accessibility features beyond what the underlying tools (Obsidian, terminal, native file browsers) provide. Discoverability of the projection paths comes from the path library's opinionated structure — users browsing `~/.ffs/` see folders that name what's in them.

## High-Level Technical Constraints

### Required integrations

The MVP must work with:

- **Obsidian** as the primary end-user editor surface.
- **OpenClaw or Hermes** as the agent host for FFS agents, packaged as `SKILL.md` artifacts. The implementation choice between an OpenClaw integration and a minimal FFS daemon is operational.
- **Git** for substrate cloning (the day-one-clone-and-collaborate scenario).
- **Operating-system-level filesystem watchers** for editor-agnostic edit detection (ReadDirectoryChangesW on Windows, kqueue on macOS, inotify on Linux).
- **AI agents** writing markdown to `~/.ffs/ingest/` (no specific agent required; the substrate is tolerant of any writer).

ZTM (Zero Trust Mesh), MCP servers, and direct projection editing are not MVP integrations; see Phased Rollout Plan.

### Performance from a user perspective

- Folder navigation in projection paths must feel responsive — under 200ms for typical folder enumeration.
- Opening a projection file must return rendered markdown in under 500ms.
- Edits to projections must be acknowledged within seconds and absorbed into atoms within minutes.
- The CLI must complete simple queries (`ffs cat`, `ffs ls`, `ffs get`) in under one second for typical-size graphs.

### Data privacy and security

- Atoms are encrypted at rest using per-cohort data encryption keys; the keys are managed via the OS keychain on each user's hardware.
- Cryptographic deletion (destroying the DEK) is the default erasure mode; tombstoning and physical deletion are alternative modes for specific cases.
- Possession of an atom hash does not grant access to content. Capability checks gate retrieval at the source substrate. There is no global content layer.
- Federation queries reveal what is being asked to the receiving peer. This is an accepted tradeoff in MVP; private set intersection (PSI) is Phase 2 work for use cases that need it.

### Compliance

The MVP does not target specific regulatory regimes (HIPAA, SOC 2, GDPR audits). The architecture's commitments — bitemporal history, capability audit trail, erasure with three modes, signed authorship — make compliance work tractable in later phases when specific regimes become relevant. MVP users with compliance concerns are responsible for their own evaluation.

## Non-Goals (Out of Scope)

The MVP does not include:

- **Fast-path edit handling for ambiguous edits.** The MVP fast-path covers single-line text-field edits, frontmatter value edits, and additive edits to designated sections — for the three MVP predicate types only. Multi-paragraph restructuring, substantial deletions, and edits spanning multiple atoms ambiguously route to ingest as corrections (the existing slow-path). Phase 2 adds the staged-edit pattern with explicit user review for ambiguous cases, plus fast-path coverage for Phase 2 predicate types.
- **MCP server tools beyond the six MVP tools.** The MVP MCP server exposes `ffs_query`, `ffs_render_projection`, `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`, and `ffs_audit_query`. Richer tool surfaces (federation operations, capability authoring via MCP, real-time subscription to atom updates) are Phase 2 informed by what agents actually need.
- **Path library beyond contacts/people/notes.** Decisions, projects, questions, action-items, policies are Phase 2.
- **Multi-peer federation aggregation.** Bilateral pair-by-pair federation works in MVP; aggregation across many peers in one path is Phase 2.
- **Organizational federation with formal contracts.** MVP federation is between individuals. Organizational federation between firms with legal teams is Phase 3.
- **Cryptographic verbs beyond AEAD and signatures.** PSI, ZK, FHE, MPC are named as future work; PSI is the most likely first addition in Phase 2 for the contact-graph intersection use case.
- **ZTM as federation transport.** MVP uses direct HTTPS for federation; ZTM is Phase 2.
- **TEE-hosted secure-enclave deployments.** Phase 3 or later.
- **Hosted FFS deployments and multi-tenant operations.** MVP is personal-hardware only.
- **A2A protocol bridging to non-FFS agents.** MVP federation is FFS-to-FFS only.
- **Cloud-native editor for direct atom authoring.** Authoring is via markdown plus scribe absorption.
- **Email ingestion.** Defer until permitted environments and explicit demand exist.
- **RuVector / RVF as substrate alternative.** SQLite is the MVP storage; alternatives are Phase 3+.
- **Real-time collaborative editing within a document.** That is an editor-layer concern, not a substrate concern.
- **Hermes-style autonomous curator.** MVP librarian is a simple watcher.
- **Public release, marketing, commercial offerings.** Small-community validation only.

## Phased Rollout Plan

### MVP (Phase 1)

The core features described above, sufficient to validate all three motivating scenarios with a 5-20 person community.

**Deliverables:**

- Atom store, signing, classification, capability evaluation, supersession, erasure.
- Three folder spaces (ingest, projection, config) with the three-path starter library (contacts, people, notes).
- Daemon managing the working set, watching the filesystem, hosting the FFS agents.
- Scribe with tolerant ingestion; auditor producing daily reports; librarian watching for drift.
- `ffs` CLI tool with `ffs://` URL resolution, distributed as static binaries for Linux/macOS/Windows.
- Obsidian plugin with the minimum-viable feature set described above.
- Direct projection editing (minimum-viable fast-path) — single-line text-field edits, frontmatter value edits, and additive edits to designated sections, for the three MVP predicate types. Slow-path for ambiguous edits routes to ingest as before.
- FFS-MCP server with the six MVP tools (`ffs_query`, `ffs_render_projection`, `ffs_resolve_url`, `ffs_author_atom`, `ffs_inspect_predicate`, `ffs_audit_query`), capability-checked at the boundary.
- Personal federation: bilateral capability authoring, peer mounting, tier-based selective sharing, intersection computation, revocation.
- Onboarding documentation aimed at the technical-friend-helping-non-technical-peer workflow.

**Success criteria to proceed to Phase 2:**

- 5-20 peers using FFS daily; 90-day retention.
- Real federation events occurring in normal workflow.
- At least one emergent introduction made through intersection computation.
- Architecture's commitments (cloning, capability, classification, federation, projection, editor-agnostic editing, MCP integration) all demonstrated working in the community's actual usage.

### Phase 2

Pre-committed high-leverage features plus reserved capacity for adoption-driven additions.

**Pre-committed deliverables:**

- **Fast-path edit handling for ambiguous cases.** The staged-edit pattern with explicit user review for substantial restructuring, deletions, and edits spanning multiple atoms ambiguously. Plus fast-path coverage for Phase 2 predicate types as the path library expands.
- **Expanded path library:** decisions, projects, questions, action-items, policies (the deferred path families from MVP), with reverse-map annotations enabling fast-path edits on each.
- **Richer MCP tool surface.** Federation operations, capability authoring via MCP, real-time subscription to atom updates, and additional tools informed by what MVP MCP usage reveals as needed.
- **Multi-peer federation aggregation:** intersections and unions across multiple peers in one path.
- **ZTM as federation transport** when peers prefer it over HTTPS.
- **PSI for federation queries** — private set intersection, allowing peers to compute who-they-both-know without revealing full sets.

**Reserved capacity** for features the community asks for that we haven't anticipated. Adoption surfaces what we couldn't predict at design time.

**Success criteria to proceed to Phase 3:** community grows beyond the original 5-20 (organic word-of-mouth or friend-of-friend invitations), or adoption-driven additions reveal genuine ecosystem need.

### Phase 3

Long-arc and ambition features that depend on broader adoption or specific environmental conditions.

- Organizational federation with formal contracts between firms with legal teams.
- Hosted FFS deployments and multi-tenant operations.
- TEE-hosted secure-enclave deployments for compliance-conscious users.
- Cryptographic verbs beyond PSI: ZK proofs, FHE, MPC where use cases pull them in.
- Email ingestion and other multi-source ingestion paths.
- A2A protocol bridging to non-FFS agents.
- Cloud-native editor for direct atom authoring.
- RuVector or other vector-and-graph-aware substrate alternatives.
- Hermes-style autonomous curator for the predicate library.

## Success Metrics

### User engagement (the core success indicators)

- **90-day retention.** 5-20 peers still using FFS daily three months after onboarding. Measured by atom-authoring activity in their substrates.
- **Federation event rate.** Peers authoring capabilities, mounting peers, computing intersections, revoking. Measured by counting federation operations across the community.
- **Emergent introductions.** Introductions or collaborations made through the intersection-computation feature that wouldn't have happened otherwise. Measured by self-reported attribution from community members.

### Performance benchmarks

- Folder navigation under 200ms for typical projection paths.
- Projection rendering under 500ms on open.
- CLI queries under one second for typical operations.
- Edit absorption from filesystem write to atom commit under one minute.

### Quality attributes

- The substrate is genuinely cloneable: a `git clone` of one peer's substrate produces a working FFS instance for the cloning peer (with capability-filtered visibility).
- Editor-agnostic editing works: opening a projection in Notepad, vim, VS Code, and Obsidian all behave correctly.
- Federation revocation is immediate: a revoked capability removes the peer's view within seconds, not minutes.

## Risks and Mitigations

### Adoption risk

**Risk:** The 5-20 peer community installs but doesn't adopt for daily use. Onboarding succeeds but retention drops; federation events don't happen because nobody's authoring real capabilities.

**Mitigations:**

- The technical-friend-onboarding model means each new peer has a person invested in their success, not just installation.
- Folodex (or whatever the first contact-graph application becomes) gives users a daily-use reason to engage with the substrate, not just an "interesting architecture" reason.
- The MVP commitment to all three motivating scenarios means users can use FFS for personal knowledge alongside contact federation; if one use case doesn't stick, others may.
- Honest success criteria with retention measured at 90 days mean we'll know whether adoption is real, not just installation rates.

### Network-effects risk

**Risk:** Federation requires both peers to install FFS. Even within a 5-20 person community, if a meaningful subset doesn't onboard, the federation features never get exercised in real use.

**Mitigations:**

- The substrate delivers value at scale of one. Users who install but whose peers haven't can still use FFS for personal knowledge, contact capture, and AI-agent absorption. They're not blocked waiting for peers.
- The success criterion explicitly names "real federation events" alongside retention, so we'll see this risk materializing in metrics rather than discovering it after launch.
- Onboarding documentation emphasizes the bilateral nature: a peer who already has FFS can invite a friend with substrate-sovereignty as a personal benefit before federation is set up.
- Folodex's roadmap can include incentives for inviting peers (the intersection feature only works with mutually-mounted peers), making peer onboarding part of the product loop.

## Architecture Decision Records

This PRD's design decisions are documented in ADRs. The complete list:

- [ADR-001: Records-shaped substrate, not file-shaped](adrs/adr-001.md) — The substrate is in the iCloud Notes lineage, not the OneDrive lineage.
- [ADR-002: Both audiences first-class](adrs/adr-002.md) — Developer and end-user audiences are equally primary.
- [ADR-003: Substrate-First MVP](adrs/adr-003.md) — Working surfaces over polish, breadth over depth, real use over demonstration.
- [ADR-004: Three motivating scenarios all in MVP](adrs/adr-004.md) — Contact-graph sovereignty, home-claw absorption, day-one-clone-and-collaborate all ship together.
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Real files on disk for any editor, with fast-path edits for trivial cases and slow-path corrections for ambiguous ones.
- [ADR-006: `ffs://` URL scheme as public stable contract](adrs/adr-006.md) — Universal addressing modeled on `s3://`.
- [ADR-007: Personal federation in MVP, organizational deferred](adrs/adr-007.md) — Same primitive at two scales; MVP ships personal scale.
- [ADR-008: Speak MCP and A2A at boundaries](adrs/adr-008.md) — Standards integration over invention.
- [ADR-009: Claw integration via OpenClaw or Hermes pattern](adrs/adr-009.md) — FFS contributes agent definitions; doesn't reimplement the host.
- [ADR-010: MCP server deferred to Phase 2](adrs/adr-010.md) — *Superseded by ADR-013.*
- [ADR-011: Path library starts at three (contacts/people/notes)](adrs/adr-011.md) — Decisions, projects, questions, action-items, policies are Phase 2.
- [ADR-012: Bilateral federation in MVP, multi-peer aggregation in Phase 2](adrs/adr-012.md) — MVP proves the primitive; aggregation is UX over the working primitive.
- [ADR-013: MCP server in MVP](adrs/adr-013.md) — Six tools, capability-checked, thin wrapper over substrate API. Supersedes ADR-010 because the home-claw absorption scenario requires bidirectional MCP for full validation.
- [ADR-014: Minimum-viable fast-path for trivial projection edits in MVP](adrs/adr-014.md) — Reverse-map annotations on the three MVP predicate types make fast-path inference reliable; ambiguous edits route to ingest as corrections.

## Open Questions

These items remain unresolved at draft time and need further input or implementation experience to settle.

- **Predicate spec authoring tooling.** The MVP ships text-based authoring with the CLI. Visual editors, template libraries, scribe-assisted authoring are open for Phase 2.
- **Working set materialization heuristics.** What gets kept materialized on disk for editor access — most-recently-touched, user-pinned, scribe-suggested? MVP ships a simple heuristic; refinement is open.
- **Daemon implementation choice (OpenClaw skills vs. minimal FFS daemon).** Both are architecturally supported. The decision is operational and will be made at implementation time based on the relative maturity of OpenClaw's skill system and the cost of a parallel implementation.
- **Indefinite atom accumulation.** v1 deployments accumulate atoms forever. Truncation policies for long-running deployments are open; not needed for MVP small-community validation.
- **Federation handshake details.** What is exchanged when two graphs first establish a bridge; what is the negotiation when contract terms drift. MVP ships a simple version; richer handshake is open.
- **Onboarding workflow details.** The technical-friend-onboarding model is committed; the specific checklist, documentation format, and tooling support are open.
- **Daily health summary specification.** Five items maximum; what specifically surfaces, in what priority order, is open.
- **Path library extensibility for users.** How does a deployment author a custom path joining the starter library? The mechanism is committed (path-definition atoms); the UX is open.
