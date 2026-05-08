---
status: completed
title: Atom envelope — JCS canonicalization, Ed25519 sign/verify, BLAKE3 multihash
type: backend
complexity: high
dependencies:
  - task_01
---

# Task 02: Atom envelope — JCS canonicalization, Ed25519 sign/verify, BLAKE3 multihash

## Overview
Implement the substrate's foundational interoperability contract: the canonical-JSON atom envelope, its Ed25519 signing and verification, and its BLAKE3-based multihash content addressing. Every other component reads or writes atoms through this module, so the byte-for-byte stability of the canonical form is load-bearing for federation, signing, and `ffs://atom/<hash>` URL resolution.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST define `AtomEnvelope` matching the shape in TechSpec § Implementation Design § Core Interfaces.
- MUST canonicalize envelopes per RFC 8785 (JSON Canonicalization Scheme) such that any two encoders produce byte-identical output for the same logical atom.
- MUST sign canonical bytes with Ed25519 per RFC 8032; signature covers the envelope with the `signature` field elided.
- MUST compute the content address as `multihash(blake3(jcs_bytes))` with codec `0x1e`.
- MUST encode public keys, signatures, and content hashes as multibase (base58btc, prefix `z`).
- MUST expose `AtomEnvelope::verify()` returning a typed `VerifyError` that distinguishes signature failure, hash mismatch, and malformed envelope.
- MUST validate ISO 8601 timestamps; reject envelopes with non-conforming or non-UTC timestamps.
- SHOULD provide an `AtomTemplate` builder to construct an unsigned envelope and produce a signed `AtomEnvelope` via `sign(template, key)`.
</requirements>

## Subtasks
- [x] 2.1 Define the `AtomEnvelope`, `EntityId`, `PredicateName`, `Tier`, `Provenance`, `Signature`, `PublicKey`, `Multihash`, `Iso8601` types.
- [x] 2.2 Implement JCS canonicalization (use `serde_jcs`) and assert byte stability with property tests.
- [x] 2.3 Implement Ed25519 signing and verification (use `ed25519-dalek`) with the elided-signature signing protocol.
- [x] 2.4 Implement BLAKE3 multihash content addressing (use `blake3` and `multihash` crates).
- [x] 2.5 Implement multibase encoding/decoding for keys, signatures, and hashes.
- [x] 2.6 Provide a `verify()` method returning a typed `VerifyError`.
- [x] 2.7 Add property tests for canonicalization stability, signing roundtrip, multihash roundtrip.

## Implementation Details
Create `crates/ffs-core/src/atom.rs` and supporting submodules. Follow the signing protocol step-by-step from ADR-017 (omit `signature`, JCS-canonicalize, sign, re-insert, JCS-canonicalize, hash). Numbers in atoms use ISO 8601 strings rather than Unix epochs to avoid integer/float ambiguity in JCS. RFC 8785 test vectors should be checked into the repo and exercised in CI.

See TechSpec § Implementation Design § Core Interfaces for the `AtomEnvelope` shape.

### Relevant Files
- `crates/ffs-core/src/atom.rs` (new) — primary module.
- `crates/ffs-core/src/multihash.rs` (new) — multihash codec wrapper.
- `crates/ffs-core/src/multibase.rs` (new) — multibase encoder helpers.
- `crates/ffs-core/tests/jcs_vectors.rs` (new) — RFC 8785 conformance vectors.

### Dependent Files
- `crates/ffs-core/src/store.rs` (task_04) — stores `AtomEnvelope` blobs.
- `crates/ffs-core/src/capability.rs` (task_05) — capability atoms are `AtomEnvelope` instances.
- `crates/ffs-core/src/projection.rs` (task_06) — renders from atoms.
- `crates/ffs-federation` (task_14, task_15) — ships envelopes over the wire.

### Related ADRs
- [ADR-017: Canonical JSON (RFC 8785 JCS) atom envelope](adrs/adr-017.md) — Format and signing protocol.
- [ADR-018: Cryptographic primitives — Ed25519, ChaCha20-Poly1305, BLAKE3](adrs/adr-018.md) — Primitive choices and wrapping.
- [ADR-001: Records-shaped substrate](adrs/adr-001.md) — Why atoms are the substrate.

## Deliverables
- `AtomEnvelope` type with JCS-stable serialization, signing, verification, and content addressing.
- RFC 8785 test vectors checked into the repo and exercised in CI.
- Property tests for envelope canonicalization stability and signing roundtrip.
- Unit tests with 80%+ coverage **(REQUIRED)**.
- Integration tests for cross-module envelope roundtrip **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Permuting field-order in input produces identical canonical bytes.
  - [ ] Signed envelope verifies with the matching public key; mismatched key returns `VerifyError::Signature`.
  - [ ] Tampering with any field after signing produces `VerifyError::Signature`.
  - [ ] Content hash recomputed from envelope matches the stored multihash.
  - [ ] Tampering with the envelope after hash storage produces `VerifyError::HashMismatch`.
  - [ ] Non-UTC or malformed ISO 8601 timestamps are rejected at construction.
  - [ ] Multibase round-trip: `encode(decode(x)) == x` for keys, signatures, hashes.
- Integration tests:
  - [ ] RFC 8785 test vectors yield the expected canonical byte sequences.
  - [ ] Envelope produced in one process and verified in another produces identical hashes.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- An envelope canonicalized in Rust matches a reference output canonicalized by an independent JCS implementation (e.g., the JS `canonicalize` library) byte-for-byte for at least three sample inputs.
- Signing and verification round-trip across 1000 random envelopes without false positives or negatives.
