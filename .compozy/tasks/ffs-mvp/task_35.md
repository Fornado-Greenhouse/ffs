---
status: pending
title: macOS .app bundle wrapping so the keychain entitlement actually works
type: infra
complexity: high
dependencies:
  - task_22
  - task_27
  - task_33
---

# Task 35: macOS .app bundle wrapping so the keychain entitlement actually works

## Overview
Task_33 landed the codesigning + entitlement + notarization infrastructure with the understanding (per ADR-023) that AMFI would read the embedded provisioning profile from a Mach-O `__TEXT,__provisioning` section. Empirical investigation on 2026-06-19 against macOS 26.2 proved that recipe is folklore — AMFI doesn't look at that section. The canonical Apple path is to wrap the binaries inside a `.app` bundle with `Contents/embedded.provisionprofile` as a file. ADR-025 captures the finding + decision. This task carries the implementation work to switch from raw Mach-O CLIs to a single FFS.app bundle wrapping all three binaries, then validates end-to-end that the keychain path finally works (task_27's empirical close).

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST emit a `FFS.app` bundle as the macOS release artifact, structured per ADR-025: `Contents/Info.plist` + `Contents/embedded.provisionprofile` + `Contents/MacOS/{ffs,ffs-daemon,ffs-mcp}` + `Contents/_CodeSignature/`. CFBundleIdentifier matches the bundle ID the provisioning profile authorizes; `LSUIElement=true` so the bundle stays headless.
- MUST delete the per-crate `build.rs` files added in commit `2be3b5a`. Their `-Wl,-sectcreate` linker args are no-ops AMFI never reads; keeping them around is misleading and they break the FFS_PROVISIONING_PROFILE env-var contract (the env var becomes irrelevant because the profile is a file in the bundle, not a section in the binary).
- MUST rewrite `scripts/codesign-macos.sh` to: (1) construct the bundle from the just-built release binaries, (2) place `embedded.provisionprofile` + `Info.plist` at `Contents/`, (3) `codesign --force --deep --options runtime --timestamp --entitlements … --sign … FFS.app`. The current per-Mach-O sign loop goes away. The script's existing argument shape can stay (binaries listed positionally) so the CI workflow doesn't need to know about the bundle structure.
- MUST update `installer/install.sh` to: (1) place `FFS.app` under `~/.local/Applications/FFS.app` (or `/Applications/FFS.app` for system installs), (2) create symlinks at `~/.local/bin/{ffs,ffs-daemon,ffs-mcp}` pointing into `Contents/MacOS/`, (3) clean up old install paths if upgrading from a pre-task_35 install (`~/.local/bin/ffs-daemon` may previously have been a regular file, not a symlink).
- MUST update `installer/launchd/com.ffs.daemon.plist` so `ProgramArguments` points at the bundle's internal daemon path. Symlinks would break under launchd's plist resolution; the absolute bundle path is the correct shape.
- MUST update `.github/workflows/release.yml` to produce a single `FFS.app.zip` release artifact (one zip per macOS arch — x86_64 + aarch64), submit to `notarytool` as the bundle, and `xcrun stapler staple FFS.app` so the ticket is attached to the bundle directly. Stapling works on bundles where it didn't work for raw Mach-O.
- MUST update the integration test override (`FFS_SIGNED_DAEMON_BIN` in `tests/sqlite_persistence.rs`) so it accepts either a bundle path (`…/FFS.app/Contents/MacOS/ffs-daemon`) or a raw binary path. Today the test code is already shape-agnostic since it just passes the path to `Command::new`, but the docs around `FFS_SIGNED_DAEMON_BIN` need to clarify the bundle case.
- MUST update ADR-023 to add a "Superseded for implementation by ADR-025" pointer in its references section. The intent (use access groups + codesigning + notarization) is upheld; only the section-embed mechanism is corrected.
- MUST update `docs/onboarding/technical-friend-checklist.md` Step 2 Path B with the corrected build → bundle → sign → notarize → staple recipe. Drop the `FFS_PROVISIONING_PROFILE` env var references (the env var no longer exists because build.rs is gone).
- MUST update `docs/onboarding/troubleshooting.md` keychain section with the three-gate diagnostic shape adapted to bundle paths: `codesign -d --entitlements - FFS.app`, `xcrun stapler validate FFS.app`, and a path check that the daemon binary symlink resolves into a bundle.
- SHOULD verify the bundle works under launchd by loading the daemon plist on the project lead's Mac and confirming `ffs identity show` reports `source: keychain` AND the pubkey stays stable across two consecutive boots. This is the empirical close of task_27 we've been chasing.
- SHOULD test the same on a second Apple architecture if available (x86_64 in Rosetta or aarch64 native), to make sure the bundle layout works on both. The CI release matrix already produces both, so the CI green is partial proof.
</requirements>

## Subtasks
- [ ] 35.1 Define the `FFS.app/Contents/` layout. Write `bundle/Info.plist` (or wherever the canonical source lives) with the keys ADR-025 specifies (`CFBundleIdentifier`, `LSUIElement`, etc.). Add `bundle/` (or chosen path) to `.gitignore` for the *generated* `.app` output; the source `Info.plist` template gets committed.
- [ ] 35.2 Delete `crates/{ffs-daemon,ffs-cli,ffs-mcp}/build.rs` + the workspace-level Cargo additions tied to them. Confirm `cargo build --release` still works (it should — those build.rs were no-ops without the env var).
- [ ] 35.3 Rewrite `scripts/codesign-macos.sh`: take a list of pre-built binaries, construct `target/release/FFS.app` around them, place `embedded.provisionprofile` + `Info.plist` as files, `codesign --force --deep …` the bundle. Verify with `codesign -d --entitlements - FFS.app` + `spctl --assess --type install FFS.app`.
- [ ] 35.4 Update `installer/install.sh` for the bundle layout: place `FFS.app` under `~/.local/Applications/` (or `/Applications/` with `--system`), symlink into `~/.local/bin/`. Handle upgrades from old non-bundle installs cleanly (don't leave orphan binaries).
- [ ] 35.5 Update `installer/launchd/com.ffs.daemon.plist`: `ProgramArguments[0]` becomes `…/FFS.app/Contents/MacOS/ffs-daemon` (absolute path; symlinks would break under launchd's path resolution).
- [ ] 35.6 Update `.github/workflows/release.yml`: drop the per-binary sign/notarize loop, produce a single `FFS-{arch}.app.zip` per macOS arch, submit + staple as a bundle. Drop `MACOS_PROVISIONING_PROFILE_B64` env-var threading into `cargo build` since the profile is now a file written by the workflow.
- [ ] 35.7 Update `tests/sqlite_persistence.rs` docs around `FFS_SIGNED_DAEMON_BIN` to clarify the bundle case. The test code itself doesn't need changes (path-agnostic), but the comment block + the empty-set assertion need a refresh.
- [ ] 35.8 Update ADR-023 references section with "Superseded for implementation by ADR-025".
- [ ] 35.9 Update `docs/onboarding/technical-friend-checklist.md` Step 2 Path B + `docs/onboarding/troubleshooting.md` keychain section.
- [ ] 35.10 Run `signed_daemon_produces_stable_keychain_identity_across_boots` integration test against the new bundle layout. This is the empirical close of task_27 — two consecutive daemon boots produce the same identity from the keychain via the wildcard `3S9R9K2L38.*` access group authorized by the embedded profile.
- [ ] 35.11 Live launchd validation on the project lead's Mac: reload the daemon plist with the new bundle path, watch `log show --predicate 'process == "ffs-daemon"'` confirm `owner_source=keychain` on first AND second boot.

## Implementation Details

The bundle layout per ADR-025:

```
FFS.app/
  Contents/
    Info.plist
    embedded.provisionprofile
    MacOS/
      ffs
      ffs-daemon
      ffs-mcp
    _CodeSignature/
```

`Info.plist` minimum keys (load-bearing: `CFBundleIdentifier` for the profile match, `LSUIElement=true` for headless behavior, `CFBundleExecutable` to identify the primary binary):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>           <string>com.fornado.ffs</string>
    <key>CFBundleName</key>                 <string>FFS</string>
    <key>CFBundleExecutable</key>           <string>ffs</string>
    <key>CFBundlePackageType</key>          <string>APPL</string>
    <key>CFBundleShortVersionString</key>   <string>0.1.0</string>
    <key>CFBundleVersion</key>              <string>1</string>
    <key>LSUIElement</key>                  <true/>
    <key>LSMinimumSystemVersion</key>       <string>11.0</string>
</dict>
</plist>
```

`scripts/codesign-macos.sh` rewrite skeleton:

```sh
# Construct bundle
APP="$BUILD_DIR/FFS.app"
mkdir -p "$APP/Contents/MacOS"
cp "$1" "$2" "$3" "$APP/Contents/MacOS/"     # pre-built ffs, ffs-daemon, ffs-mcp
cp bundle/Info.plist                   "$APP/Contents/Info.plist"
cp secrets/embedded.provisionprofile   "$APP/Contents/embedded.provisionprofile"

# Sign the bundle (codesign --deep walks Contents/MacOS/ and signs each Mach-O)
codesign \
  --sign "$FFS_SIGNING_IDENTITY" \
  --force --deep \
  --options runtime \
  --timestamp \
  --entitlements entitlements/ffs.entitlements.plist \
  "$APP"

# Verify
codesign --verify --deep --strict --verbose=4 "$APP"
spctl --assess --type install --verbose=4 "$APP"
```

The bundle is then zipped + submitted to `notarytool` as `FFS.app.zip` (single submission per arch, not three). `xcrun stapler staple FFS.app` attaches the ticket so first-launch on offline machines works.

### Relevant Files
- `bundle/Info.plist` (NEW) — Info.plist source committed in the repo, copied into `FFS.app/Contents/Info.plist` during build.
- `scripts/codesign-macos.sh` — rewrite for bundle-shaped signing.
- `installer/install.sh` — bundle install + symlink creation.
- `installer/launchd/com.ffs.daemon.plist` — `ProgramArguments` update.
- `.github/workflows/release.yml` — bundle-zip submission + staple.
- `entitlements/ffs.entitlements.plist` — stays unchanged structurally; the `keychain-access-groups` value still names `3S9R9K2L38.com.ffs.shared`.
- `crates/{ffs-daemon,ffs-cli,ffs-mcp}/build.rs` — DELETE.

### Dependent Files
- `crates/ffs-daemon/tests/sqlite_persistence.rs` — `FFS_SIGNED_DAEMON_BIN` doc block update.
- `docs/onboarding/technical-friend-checklist.md` — Path B rewrite.
- `docs/onboarding/troubleshooting.md` — keychain section + diagnostic commands.
- `.compozy/tasks/ffs-mvp/adrs/adr-023.md` — "Superseded for implementation by ADR-025" pointer.

### Related ADRs
- [ADR-023: Code-signed macOS binaries with shared `keychain-access-group`](adrs/adr-023.md) — Intent upheld; implementation superseded by ADR-025.
- [ADR-025: macOS .app bundle wrapping for restricted-entitlement CLIs](adrs/adr-025.md) — This task's foundation.
- [ADR-024: Windows ACL hardening at install time via `icacls`](adrs/adr-024.md) — Sibling install-time concern.

## Deliverables
- `bundle/Info.plist` source file committed.
- Rewritten `scripts/codesign-macos.sh` producing a signed `FFS.app`.
- `installer/install.sh` install + symlink updates.
- Updated `com.ffs.daemon.plist`.
- Updated `.github/workflows/release.yml` producing notarized + stapled `FFS.app.zip` artifacts.
- Deleted `build.rs` files (three crates).
- ADR-023 superseded-by note + ADR-025 reference.
- Updated docs (checklist Path B + troubleshooting keychain section).
- Integration test (`signed_daemon_produces_stable_keychain_identity_across_boots`) passes against the bundle layout — empirical close of task_27.

## Tests
- Unit tests:
  - [ ] None new. The runtime detection helper `is_signed_with_keychain_entitlement` keeps its current behavior — it still shells out to `codesign -d --entitlements -` and looks for the access group string. Bundle vs flat binary makes no observable difference to that helper.
- Integration tests:
  - [ ] (cfg(target_os = "macos"), skip-when-unsigned) `signed_daemon_produces_stable_keychain_identity_across_boots` runs against `FFS_SIGNED_DAEMON_BIN=/path/to/FFS.app/Contents/MacOS/ffs-daemon`. Two boots, identical owner pubkey, second boot reports `owner_source=keychain`.
  - [ ] On every CI run: the unit-test suite passes regardless of whether signing infrastructure is configured.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing.
- Test coverage >=80%.
- `target/release/FFS.app` exists after `cargo build --release && ./scripts/codesign-macos.sh …` and `spctl --assess --type install FFS.app` reports `accepted, Notarized Developer ID`.
- `installer/install.sh` produces a working install with `~/.local/bin/ffs-daemon → …/FFS.app/Contents/MacOS/ffs-daemon` symlink.
- Two consecutive launchd-spawned daemon boots produce the same `owner=…` log line AND the second reports `owner_source=keychain`. Task_27's launchd identity-drift bug is finally closed.
- CI release workflow produces signed + notarized + stapled `FFS-{arch}.app.zip` artifacts that pass `spctl --assess` on a fresh Mac without network access (stapled tickets enable offline verification).
