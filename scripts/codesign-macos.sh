#!/usr/bin/env bash
# scripts/codesign-macos.sh
#
# Sign one or more macOS binaries with the FFS entitlements file
# (keychain-access-groups = 3S9R9K2L38.com.ffs.shared) and a
# hardened-runtime opt-in. Verifies the result, including the
# presence of an embedded provisioning profile (mandatory for the
# entitlement to actually take effect at runtime — see ADR-023).
#
# Usage (full release flow, with provisioning profile + notarization):
#   # 1) Build with the profile embedded at link time
#   FFS_PROVISIONING_PROFILE="$(pwd)/secrets/embedded.provisionprofile" \
#     cargo build --release
#   # 2) Sign with this script
#   FFS_SIGNING_IDENTITY="Developer ID Application: <Name> (<TeamID>)" \
#     ./scripts/codesign-macos.sh \
#     target/release/ffs \
#     target/release/ffs-daemon \
#     target/release/ffs-mcp
#   # 3) Notarize each via `xcrun notarytool submit … --wait` against a
#   #    `xcrun notarytool store-credentials`-saved profile.
#
# Why `--options runtime`:
#   Hardened runtime is required for Apple notarization. The entire
#   notarize+staple flow refuses any binary that wasn't built with
#   the hardened runtime flag.
#
# Why `--timestamp`:
#   Apple requires a secure timestamp on any binary that hits
#   `xcrun notarytool` or runs from a downloaded `.dmg`. Without it,
#   Gatekeeper refuses the binary on Catalina+ when the user
#   double-clicks it from a quarantined download.
#
# Why explicit `--force`:
#   Re-running the script after an in-place rebuild updates the
#   signature without complaining. Without `--force`, codesign
#   refuses to overwrite an existing signature.
#
# Why the provisioning profile check:
#   `keychain-access-groups` is a *restricted* macOS entitlement.
#   AMFI (kernel-level Apple Mobile File Integrity) silently
#   SIGKILLs a binary that claims a restricted entitlement without
#   an Apple-signed profile authorizing it — exit 137, zero stderr,
#   no log entries the unprivileged side can see. The profile is
#   embedded at LINK time via `-Wl,-sectcreate,__TEXT,__provisioning`
#   (handled by each crate's `build.rs` when
#   `FFS_PROVISIONING_PROFILE` is set). This script's check refuses
#   to sign a binary that's missing the section so the failure mode
#   stays at sign-time, not at run-time. See ADR-023 + Eskimo on
#   Apple Dev Forums thread/782084.
#
# Sister of ADR-024 (Windows ACL hardening at install time):
# signing is conceptually an install-time concern, not a runtime
# one. The daemon detects the signed state at boot per task_33's
# `is_signed_with_keychain_entitlement` but doesn't sign itself.

set -euo pipefail

if [[ -z "${FFS_SIGNING_IDENTITY:-}" ]]; then
  echo "error: FFS_SIGNING_IDENTITY is not set" >&2
  echo "  expected: \"Developer ID Application: <Name> (<TeamID>)\"" >&2
  echo "  find yours: security find-identity -p codesigning -v" >&2
  exit 1
fi

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <binary> [<binary>...]" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENTITLEMENTS="${SCRIPT_DIR}/../entitlements/ffs.entitlements.plist"
if [[ ! -f "$ENTITLEMENTS" ]]; then
  echo "error: entitlements file not found at $ENTITLEMENTS" >&2
  exit 1
fi

# Refuse to proceed if any binary lacks the embedded provisioning
# profile section. Running without it produces a SIGKILL at
# launch-time which is the worst possible failure mode (no
# stderr, no log). See ADR-023.
check_provisioning_section() {
  local bin="$1"
  if ! otool -l "$bin" 2>/dev/null | grep -q "sectname __provisioning"; then
    cat >&2 <<EOF
error: $bin has no __TEXT,__provisioning section embedded.

The restricted \`keychain-access-groups\` entitlement requires an
Apple-signed provisioning profile embedded at link time. Without
it, AMFI SIGKILLs the binary at exec — exit 137, no stderr.

Build with the profile embedded:
  FFS_PROVISIONING_PROFILE="\$(pwd)/secrets/embedded.provisionprofile" \\
    cargo build --release
then re-run this script. See ADR-023 + technical-friend-checklist.md
Step 2 Path B for the developer.apple.com setup.
EOF
    exit 1
  fi
}

for bin in "$@"; do
  if [[ ! -f "$bin" ]]; then
    echo "error: binary not found: $bin" >&2
    exit 1
  fi
  check_provisioning_section "$bin"

  echo "==> signing $bin"
  codesign \
    --sign "$FFS_SIGNING_IDENTITY" \
    --options runtime \
    --timestamp \
    --force \
    --entitlements "$ENTITLEMENTS" \
    "$bin"

  echo "==> verifying $bin"
  codesign --verify --deep --strict --verbose=2 "$bin"

  echo "==> embedded entitlements for $bin:"
  codesign -d --entitlements - --xml "$bin" 2>/dev/null || \
    codesign -d --entitlements - "$bin"
done

echo "==> done"
