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
| 12 | Librarian skill (Python): working-set manager and drift watcher | pending | low | task_04, task_06, task_07, task_10 |
| 13 | Auditor skill (Python): daily health summary atom authoring | pending | medium | task_04, task_05, task_07, task_10 |
| 14 | Federation transport: mTLS HTTPS server/client, cert-from-Ed25519, bridge handshake | pending | critical | task_02, task_04, task_05, task_07 |
| 15 | Federation pull sync: watermarks, capability-filtered serving, intersection, revocation | pending | critical | task_14 |
| 16 | ffs-mcp: six MVP MCP tools wrapping the daemon's JSON-RPC | pending | medium | task_07 |
| 17 | Obsidian plugin: scaffolding + UDS / named pipe client + event subscription | pending | medium | task_07 |
| 18 | Obsidian plugin: paginated folder enumeration + projection rendering on open + edit routing | pending | medium | task_17 |
| 19 | Obsidian plugin: daily health summary panel + entity-name search hook | pending | medium | task_17 |
| 20 | Starter predicate-spec library (contact.person, person.generic, note) | pending | low | task_03 |
| 21 | Starter Tera template library for the three MVP predicate types | pending | low | task_06, task_20 |
| 22 | Cross-platform installer scripts for Linux, macOS, Windows | pending | medium | task_08, task_17 |
| 23 | Onboarding documentation: technical-friend checklist and first-use guide | pending | low | task_22 |
