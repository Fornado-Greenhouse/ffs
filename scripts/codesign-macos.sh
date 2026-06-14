#!/usr/bin/env bash
# scripts/codesign-macos.sh
#
# Sign one or more macOS binaries with the FFS entitlements file
# (keychain-access-groups = 3S9R9K2L38.com.ffs.shared) and a
# hardened-runtime opt-in. Verifies the result.
#
# Usage:
#   FFS_SIGNING_IDENTITY="Developer ID Application: Alex Foley (3S9R9K2L38)" \
#     ./scripts/codesign-macos.sh \
#     target/release/ffs \
#     target/release/ffs-daemon \
#     target/release/ffs-mcp
#
# Why `--options runtime`:
#   Hardened runtime is a precondition for Apple notarization. Even
#   without notarization in MVP, signing with the hardened runtime
#   from day one means task_22's installer-emitted binaries don't
#   need to be re-signed when notarization lands.
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

for bin in "$@"; do
  if [[ ! -f "$bin" ]]; then
    echo "error: binary not found: $bin" >&2
    exit 1
  fi

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
