---
status: pending
title: macOS code signing + keychain-access-groups entitlement so task_27 works under launchd
type: infra
complexity: high
dependencies:
  - task_01
  - task_22
  - task_27
---

# Task 33: macOS code signing + keychain-access-groups entitlement so task_27 works under launchd

## Overview
Task_27 added the keychain wiring (read/write helpers in `ffs-core`, daemon precedence chain, `ffs identity show` CLI). The wiring is functionally correct — unit tests + the `FFS_KEYRING_DISABLE` integration test pass. But the live deploy proved it doesn't work end-to-end on macOS under launchd: the `keyring` crate's `Entry::get_password()` returns `NoEntry` from a launchd-spawned daemon even when an interactive CLI process wrote the entry moments earlier. Each daemon boot then generates a fresh key, breaks the SQLCipher DEK match, and crashes against the existing `atoms.db`.

The root cause is macOS Keychain Services partitioning entries by **binary code-signing identity** plus **launch context**. The fix is to (a) code-sign all three FFS binaries (`ffs`, `ffs-daemon`, `ffs-mcp`) with a Developer ID Application certificate, (b) attach a `keychain-access-groups` entitlement so all three share one keychain partition, and (c) make the daemon refuse the keychain path with a clear log message when it detects it's running unsigned.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST add `entitlements/ffs.entitlements.plist` declaring `keychain-access-groups` with one shared group covering all FFS binaries (proposed: `$(AppIdentifierPrefix)com.ffs.shared`). Also include `com.apple.security.app-sandbox = false` since FFS reads `~/.ffs/` outside the sandbox.
- MUST add `scripts/codesign-macos.sh` that signs each FFS binary in a release-builds directory using a configurable `FFS_SIGNING_IDENTITY` env var (a "Developer ID Application: <name> (<team-id>)" string from the user's keychain). Includes `--options runtime` so notarization is a future no-op extension. Verifies with `codesign --verify --deep --strict --verbose=2` and prints the embedded entitlements.
- MUST keep `cargo build --release` working unmodified for unsigned dev builds. Codesigning is a separate step invoked after the build (per the user's decision in plan review): `cargo build --release && ./scripts/codesign-macos.sh`.
- MUST detect at daemon startup whether the running binary is code-signed AND has the `keychain-access-groups` entitlement. When unsigned (or missing the entitlement), refuse the keychain path with a one-time-per-boot warning and fall back to env-var-or-generate. The chaos mode where multiple keychain entries got created across processes must not be reachable.
- MUST verify in the macOS keyring path that we either (a) confirm the `keyring` v3 crate honors `kSecAttrAccessGroup` automatically from the entitlement, or (b) drop down to the `security-framework` crate and explicitly set `kSecAttrAccessGroup`. Whichever the answer, the daemon and CLI must write/read entries that the other can find.
- MUST augment `.github/workflows/release.yml`'s macOS jobs to codesign the produced binaries using a CI-side identity secret. When the signing identity secret is absent (forks, untrusted contributors), the workflow still produces unsigned artifacts but labels them as such in the release notes.
- MUST extend the technical-friend-checklist's Step 2 and `troubleshooting.md`'s keychain section with the new diagnostics (`codesign -d --entitlements -:- /path/to/ffs-daemon`) and the dev-vs-release distinction.
- SHOULD add an ADR documenting the decision: code-signed binaries + shared keychain access group as the macOS keychain pattern, with the alternatives (file-based secrets, manual ACL config) considered and rejected.
- SHOULD verify the same binary works on both x86_64-apple-darwin and aarch64-apple-darwin since the release matrix covers both.
</requirements>

## Subtasks
- [ ] 33.1 Add `entitlements/ffs.entitlements.plist` with the access-group declaration.
- [ ] 33.2 Add `scripts/codesign-macos.sh` honoring `FFS_SIGNING_IDENTITY`. Verify with `codesign --verify` + `codesign -d --entitlements -:-`.
- [ ] 33.3 Add a `is_signed_with_keychain_entitlement()` runtime check to the daemon binary; gate the keychain path on it and refuse with a clear log message when unsigned.
- [ ] 33.4 Confirm or implement `kSecAttrAccessGroup` handling in the macOS keyring path (read keyring v3 source; drop to `security-framework` if needed).
- [ ] 33.5 Augment `.github/workflows/release.yml` macOS jobs with codesigning. Use the `MACOS_SIGNING_IDENTITY` + `MACOS_SIGNING_CERT_P12` + `MACOS_SIGNING_CERT_PASSWORD` repository secrets pattern. Skip cleanly when the secrets aren't set.
- [ ] 33.6 Update docs (technical-friend-checklist Step 2, troubleshooting keychain section) + add ADR-023.
- [ ] 33.7 Add an integration test that signs the test binary on demand and confirms two consecutive daemon spawns produce the same identity. Skip cleanly when `FFS_SIGNING_IDENTITY` isn't set so the test suite still passes on contributor machines without signing setup.

## Implementation Details

### Entitlements file
`entitlements/ffs.entitlements.plist`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>keychain-access-groups</key>
    <array>
        <string>3S9R9K2L38.com.ffs.shared</string>
    </array>
    <key>com.apple.security.app-sandbox</key>
    <false/>
</dict>
</plist>
```
Team ID `3S9R9K2L38` is the project lead's Apple Developer Team ID; the access group is namespaced under it.

### Codesigning script
`scripts/codesign-macos.sh` accepts the binaries as positional arguments, signs each with the entitlements file, then verifies. Run from CI or locally:
```sh
export FFS_SIGNING_IDENTITY="Developer ID Application: Alex Foley (3S9R9K2L38)"
cargo build --release
./scripts/codesign-macos.sh \
  target/release/ffs \
  target/release/ffs-daemon \
  target/release/ffs-mcp
```

### Signed-binary detection
At daemon startup, before the keychain path is consulted, run `codesign --display --entitlements - /proc/self/exe` (or its `_NSGetExecutablePath`-equivalent on macOS) and parse the output for the access group. When missing, set `FFS_KEYRING_DISABLE=1` internally and log:
```
WARN ffs-daemon: binary is not signed with com.ffs.shared keychain-access-group;
     falling back to env-var keys (see task_27 docs). Run scripts/codesign-macos.sh.
```

### Keyring crate access-group plumbing
keyring v3's `MacCredentialBuilder` has a `with_target` / `with_access_group` configurable option — verify this in the source. If it doesn't accept an access group, switch the `dek_from_keyring` / `owner_key_from_keyring` helpers in `ffs-core::store::keyring` to use `security-framework` directly. The `(service, account)` API surface stays the same.

### CI augmentation
The macOS jobs in `release.yml` get:
```yaml
- name: Import signing certificate
  if: env.MACOS_SIGNING_CERT_P12 != ''
  env:
    MACOS_SIGNING_CERT_P12: ${{ secrets.MACOS_SIGNING_CERT_P12 }}
    MACOS_SIGNING_CERT_PASSWORD: ${{ secrets.MACOS_SIGNING_CERT_PASSWORD }}
  run: |
    echo "$MACOS_SIGNING_CERT_P12" | base64 --decode > /tmp/cert.p12
    security create-keychain -p ci ci.keychain
    security default-keychain -s ci.keychain
    security unlock-keychain -p ci ci.keychain
    security import /tmp/cert.p12 -k ci.keychain -P "$MACOS_SIGNING_CERT_PASSWORD" -T /usr/bin/codesign
    security set-key-partition-list -S apple-tool:,apple: -s -k ci ci.keychain

- name: Codesign macOS binary
  if: env.MACOS_SIGNING_IDENTITY != ''
  env:
    MACOS_SIGNING_IDENTITY: ${{ secrets.MACOS_SIGNING_IDENTITY }}
  run: |
    FFS_SIGNING_IDENTITY="$MACOS_SIGNING_IDENTITY" \
      ./scripts/codesign-macos.sh target/${{ matrix.target }}/release/${{ matrix.artifact }}
```

### Relevant Files
- `entitlements/ffs.entitlements.plist` (new).
- `scripts/codesign-macos.sh` (new).
- `.github/workflows/release.yml` — macOS job augmentation.
- `crates/ffs-daemon/src/main.rs` — signed-binary detection at startup.
- `crates/ffs-core/src/store/keyring.rs` — possibly switch to `security-framework` if keyring v3 doesn't expose access groups.

### Dependent Files
- `docs/onboarding/technical-friend-checklist.md` — Step 2 wording.
- `docs/onboarding/troubleshooting.md` — keychain section diagnostics.

### Related ADRs
- [ADR-007: Personal federation in MVP](adrs/adr-007.md) — Stable identity is the precondition for federation.
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Daemon-as-keychain-consumer.
- New ADR-023: code-signed binaries with shared keychain-access-group as the macOS keychain pattern.

## Deliverables
- `entitlements/ffs.entitlements.plist` and `scripts/codesign-macos.sh`.
- Daemon-side signed-binary detection that disables the keychain path safely when unsigned.
- `.github/workflows/release.yml` macOS jobs producing signed artifacts when secrets are present.
- Updated onboarding + troubleshooting docs.
- ADR-023 documenting the decision.
- Unit tests with 80%+ coverage **(REQUIRED)** — signed-binary detection helper, codesign-script smoke (against the actually-signed binary in CI).
- Integration test that proves cross-process keychain access works after signing **(REQUIRED)** — skip when `FFS_SIGNING_IDENTITY` is unset.

## Tests
- Unit tests:
  - [ ] `is_signed_with_keychain_entitlement()` returns `true` for a binary whose `codesign --display --entitlements` output contains the access group string.
  - [ ] `is_signed_with_keychain_entitlement()` returns `false` for an unsigned binary AND for a signed binary missing the access group.
  - [ ] `is_signed_with_keychain_entitlement()` on non-macOS targets returns `true` (the entitlement concept doesn't apply; the keyring backend has its own permission model).
- Integration tests:
  - [ ] (Skip-when-unsigned) Spawn the signed daemon binary, wait for first boot to write the keychain entries, SIGTERM, spawn again, confirm `owner_source=keychain` on the second boot AND that the pubkey matches.
  - [ ] (Skip-when-unsigned) Spawn the signed daemon, then invoke the signed CLI's `ffs identity show` — confirm the pubkey matches the daemon's owner.
  - [ ] On every CI run: the unit-test suite passes regardless of whether signing infrastructure is configured.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- After running `./scripts/codesign-macos.sh` against fresh release binaries and re-installing the daemon, two consecutive daemon boots produce the same identity from the keychain.
- `ffs identity show` (run interactively) and the daemon's `owner=…` log line (run under launchd) match.
- An unsigned dev build still works for the test suite and `cargo nextest run --workspace`; it just refuses the keychain path with a clear log message instead of producing the cross-process chaos task_27's first live-deploy did.
- task_27's success criterion ("daemon comes back with the same identity after restart") is finally met when task_33 lands.
