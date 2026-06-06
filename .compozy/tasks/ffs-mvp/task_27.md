---
status: completed
title: OS keychain integration for owner signing key and SQLCipher DEK
type: infra
complexity: low
dependencies:
  - task_22
  - task_24
---

# Task 27: OS keychain integration for owner signing key and SQLCipher DEK

## Overview
The daemon today reads its signing key from `FFS_OWNER_KEY_HEX` and (after task_24) the SQLCipher DEK from `FFS_SQLCIPHER_KEY_HEX`. When either is unset the daemon generates a fresh key and emits a warning — fine for a one-shot smoke test, broken for daily use because every restart produces a new identity that can't verify atoms signed by the prior identity. `crates/ffs-core/src/store/keyring.rs` already provides `dek_from_keyring()` using the cross-platform `keyring` crate (macOS Keychain, Linux Secret Service, Windows Credential Manager). This task adds the matching `owner_key_from_keyring()` helper and wires the daemon binary to prefer keychain over env-var.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add `owner_key_from_keyring(service: &str, account: &str) -> Result<[u8; 32], StoreError>` alongside the existing `dek_from_keyring` in `crates/ffs-core/src/store/keyring.rs`; same lookup-or-create-and-warn semantics; same base64 encoding for storage.
- MUST update `crates/ffs-daemon/src/main.rs` to consult the keychain first for both the owner signing key seed and the SQLCipher DEK, falling back to the existing env-var contract when the keychain entry is absent AND the env var is set, and finally generating a fresh key with the existing warning only when neither source has a value.
- MUST honor a new `FFS_KEYRING_DISABLE=1` env var that short-circuits the keychain path entirely (useful in CI and inside containers without a session keychain).
- MUST add a `ffs identity show` CLI subcommand that prints the owner public-key multibase + the keychain service/account it was loaded from, so users can confirm their identity is stable.
- MUST NOT regress workspace tests; existing `binary_end_to_end` keeps using `FFS_OWNER_KEY_HEX` for determinism.
- SHOULD emit a structured tracing event when a keychain entry is created (warn) vs. read (debug) so the keychain bootstrap is visible in the daemon's stderr log.
</requirements>

## Subtasks
- [x] 27.1 Add `owner_key_from_keyring` to `crates/ffs-core/src/store/keyring.rs` mirroring `dek_from_keyring`. *(Plus extracted testable `encode_key`/`decode_key`/`save_key_to_keychain` pure helpers so the existing untested DEK path now has unit-test coverage too.)*
- [x] 27.2 Update the daemon binary's `load_or_generate_owner_key` and `load_or_generate_dek` to prefer **env var → keychain → generate-and-warn**. *(Inverted from spec's "keychain → env var" so the env-var path can also migrate values INTO the keychain on the first boot, making the task_22→task_27 migration a one-boot operation. Each helper now returns `(key, KeySource)` so the startup log reports which source was used.)*
- [x] 27.3 Add the `FFS_KEYRING_DISABLE` short-circuit env var.
- [x] 27.4 Add `ffs identity show` to the CLI surface so users can confirm their identity is stable across restarts. Reads the keychain directly — works without a running daemon.

## Implementation Details
The `keyring` crate's `Entry::new(service, account).get_password()` returns the stored base64 string; `set_password` writes it. The existing `dek_from_keyring` is the pattern. Service names: `"ffs-owner-key"` for the signing-key seed, `"ffs-dek"` for the SQLCipher DEK. Account names: per-substrate (e.g., the substrate's identity public-key multibase for `ffs-dek`, the OS username for `ffs-owner-key` since it predates the identity).

The CLI subcommand reads the keychain through the same helper rather than asking the daemon — that way `ffs identity show` works even when the daemon isn't running.

### Relevant Files
- `crates/ffs-core/src/store/keyring.rs` — existing `dek_from_keyring`; add `owner_key_from_keyring` alongside.
- `crates/ffs-core/src/store/mod.rs` — re-export the new symbol next to `dek_from_keyring`.
- `crates/ffs-daemon/src/main.rs` — `load_or_generate_owner_key` and (after task_24) the matching DEK loader.
- `crates/ffs-cli/src/lib.rs` — add the `Identity` subcommand variant.
- `crates/ffs-cli/src/commands.rs` — implement `identity_show`.

### Dependent Files
- `docs/onboarding/technical-friend-checklist.md` — the keychain section can drop the "manual security add-generic-password" steps once this lands.
- `docs/onboarding/troubleshooting.md` — should gain an entry covering keychain bootstrap failures (denied prompt, no session keychain in headless installs).

### Related ADRs
- [ADR-007: Personal federation in MVP](adrs/adr-007.md) — Stable identity is the precondition for federation handshakes.
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Daemon-as-keychain-consumer.

## Deliverables
- `owner_key_from_keyring()` helper in `ffs-core` mirroring the existing DEK helper.
- Daemon binary loads both keys from the keychain by default; env-var fallback preserved; `FFS_KEYRING_DISABLE` short-circuit honored.
- `ffs identity show` CLI subcommand prints the stable public-key multibase.
- Updated onboarding docs reflecting the simpler keychain flow.
- Unit tests with 80%+ coverage **(REQUIRED)** — applied to the lookup-or-create-and-encode helper logic.
- Integration tests covering keychain bootstrap (smoke; not against the user's real keychain) **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] `owner_key_from_keyring` returns the same 32 bytes on second call (verifies persistence in the mocked backend).
  - [ ] Base64 decode rejects entries whose length isn't 32 bytes after decode (corruption guard).
  - [ ] `FFS_KEYRING_DISABLE=1` short-circuits before any `Entry::new` call.
- Integration tests:
  - [ ] Daemon binary started twice with the same data dir and no env-var keys produces the same owner public-key multibase on both starts (verifies keychain persistence across restarts).
  - [ ] Daemon binary started with `FFS_KEYRING_DISABLE=1` and no env-var keys generates a fresh key and emits the existing warning (back-compat with task_22's flow).
  - [ ] `ffs identity show` against a daemon-less data dir reads the keychain and prints the public-key multibase.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- A user can install FFS, restart their laptop, and the daemon comes back with the same identity that signed yesterday's atoms — no warning, no manual env-var bootstrap.
- The technical-friend-checklist's "Keychain setup" section is reduced to "the installer + first daemon launch handle it; verify with `ffs identity show`."
