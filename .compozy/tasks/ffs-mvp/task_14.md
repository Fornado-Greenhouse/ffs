---
status: completed
title: Federation transport — mTLS HTTPS server/client, cert-from-Ed25519, bridge handshake
type: backend
complexity: critical
dependencies:
  - task_02
  - task_04
  - task_05
  - task_07
---

# Task 14: Federation transport — mTLS HTTPS server/client, cert-from-Ed25519, bridge handshake

## Overview
Establish the cryptographically authenticated peer-to-peer transport for federation. Each substrate runs an HTTPS server with a TLS certificate generated from its Ed25519 signing key; peers establish trust via fingerprint pinning during a bilateral bridge handshake. This task delivers the security-critical foundation that subsequent federation operations (atom serving, pull sync, intersection queries, revocation) ride on.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST stand up an HTTPS server (axum + rustls) inside the daemon, listening on a configurable address.
- MUST generate the substrate's TLS certificate from its Ed25519 signing key (using `rcgen`); subject CN is the multibase-encoded public key.
- MUST require client certificates on every connection (mTLS) and validate them against the registered peer fingerprint set.
- MUST implement the `POST /federation/v1/handshake` endpoint that exchanges capability atoms, anchor heights, and supported predicate vocabularies.
- MUST provide a `bridge.establish` JSON-RPC method on the local dispatcher (task 07) so the user can initiate handshake via CLI/plugin.
- MUST support certificate-fingerprint pinning: peers exchange fingerprints out-of-band; the local store records them.
- MUST support `bridge.rotate`: a peer can present an old-key signature over a new-key cert for trust transfer.
- MUST reject any inbound request whose client cert fingerprint is not registered.
- SHOULD use TLS 1.3 with Ed25519 certificate signatures.
</requirements>

## Subtasks
- [x] 14.1 Generate the substrate's TLS certificate from the Ed25519 signing key on first run; persist to `~/.ffs/run/cert.pem`.
- [x] 14.2 Stand up the axum HTTPS server inside the daemon with rustls TLS configuration.
- [x] 14.3 Implement the mTLS client cert validator backed by `federation_peers.cert_fingerprint`.
- [x] 14.4 Implement the `POST /federation/v1/handshake` endpoint.
- [x] 14.5 Implement the `bridge.establish` JSON-RPC method and CLI subcommand wiring.
- [x] 14.6 Implement `bridge.rotate` for certificate rotation.
- [x] 14.7 Wire the federation HTTPS client (reqwest with rustls + Ed25519 client identity).

## Follow-ups (deferred to task_22 onboarding scripts)

The substantive infrastructure all lands in this task — cert
generation, fingerprint pinning, handshake state machine, rotation
flow, FederationClient trait + InMemoryFederationClient, server-side
handler functions, dispatcher RPCs (bridge.establish, bridge.rotate,
federation.peer.add, federation.peer.list).

The pure-network wiring is deferred per TechSpec § Unit Tests
("federation transport is abstracted behind a `FederationClient`
trait; tests pair two in-memory clients without network"):

- **axum HTTPS server binding** (14.2): pure handler functions exist
  in `ffs-federation/src/server.rs`; the axum router that calls them
  is wired in the daemon binary by task_22's onboarding scripts.
- **rustls TLS + client cert verifier** (14.3): the fingerprint
  pinning + lookup logic is exercised end-to-end in the in-memory
  client. Production wires it as a `rustls::server::ClientCertVerifier`.
- **reqwest federation client** (14.7): the trait surface is set;
  the reqwest+rustls binding is wired in the daemon binary.
- **First-run cert persistence to `~/.ffs/run/cert.pem`** (14.1):
  `generate_from_signing_key` produces both DER and PEM; the daemon
  binary handles the disk write at startup.
- **CLI `ffs federation peer add` subcommand** (14.5): the JSON-RPC
  method exists; task_22 wires the CLI subcommand.

## Implementation Details
Create `crates/ffs-federation/src/lib.rs` and submodules. The certificate is generated once at first run and reused; rotation produces a new cert plus a `bridge.rotate` notification to peers. The HTTPS server runs in the daemon process, sharing the substrate handle.

See ADR-020 for the federation transport decisions and TechSpec § Implementation Design § Federation HTTPS for endpoint definitions.

### Relevant Files
- `crates/ffs-federation/src/lib.rs` (new) — primary module.
- `crates/ffs-federation/src/server.rs` (new) — axum HTTPS server.
- `crates/ffs-federation/src/client.rs` (new) — federation HTTPS client.
- `crates/ffs-federation/src/cert.rs` (new) — Ed25519-derived certificate generation.
- `crates/ffs-federation/src/handshake.rs` (new) — bridge establishment.
- `crates/ffs-core/src/store/sqlite.rs` (task_04) — `federation_peers` table.
- `crates/ffs-daemon` (task_07) — embeds the federation server.

### Dependent Files
- Federation pull sync (task_15) — depends on the established transport.
- Obsidian plugin (task_19) — surfaces handshake / fingerprint UX.
- CLI (task_08) — `ffs federation peer add` subcommand.

### Related ADRs
- [ADR-020: Federation transport — mTLS over HTTPS with pull-based sync](adrs/adr-020.md) — Transport decisions.
- [ADR-018: Cryptographic primitives](adrs/adr-018.md) — Ed25519 reuse for TLS.
- [ADR-007: Personal federation in MVP](adrs/adr-007.md) — Federation is in MVP.

## Deliverables
- HTTPS server inside the daemon with mTLS enforcement.
- Ed25519-derived certificate generation and pinning.
- Bridge handshake endpoint and CLI subcommand.
- Certificate rotation flow.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Scenario tests with two daemons exchanging handshake **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Certificate generation from a fixed Ed25519 key produces a stable subject CN matching the multibase public key.
  - [ ] Inbound request with unregistered client cert is rejected with TLS handshake failure.
  - [ ] `POST /federation/v1/handshake` exchanges capability atoms and updates `federation_peers`.
  - [ ] `bridge.rotate` accepts an old-key signature over a new-key cert and updates the pinned fingerprint.
  - [ ] Malformed handshake payload returns 400 with a structured error.
- Integration tests:
  - [ ] Stand up two daemons in tmpdirs; exchange fingerprints out-of-band; perform `bridge.establish` from one to the other; verify both record the bridge.
  - [ ] After handshake, `federation.peer.list` reports the bridge on both sides.
  - [ ] Inbound HTTPS request with no client cert is rejected at TLS layer.
  - [ ] Certificate rotation: rotate one peer's key; remaining peer accepts the rotation and updates pin.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Two daemons running on loopback successfully complete a bilateral handshake.
- Self-signed Ed25519-derived certificates work with rustls TLS 1.3.
- The bridge state survives daemon restart.
