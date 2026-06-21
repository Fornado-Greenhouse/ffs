#!/usr/bin/env bash
# scripts/codesign-macos.sh
#
# Build the macOS signing layout for FFS:
#
#   FFS.app/                          <- daemon bundle (gets the
#                                        keychain-access-groups
#                                        entitlement)
#     Contents/
#       Info.plist                    (CFBundleExecutable=ffs-daemon)
#       embedded.provisionprofile
#       MacOS/
#         ffs-daemon
#       _CodeSignature/
#   ffs                               <- standalone signed Mach-O
#                                        (no entitlements; CLI
#                                        talks to the daemon over
#                                        the UDS socket)
#   ffs-mcp                           <- standalone signed Mach-O
#                                        (no entitlements; MCP
#                                        server is a stdio bridge
#                                        to the daemon)
#
# Per ADR-025: AMFI requires `Contents/embedded.provisionprofile`
# as a file inside a .app bundle for restricted entitlements like
# `keychain-access-groups`. Mach-O sections aren't honored.
#
# Empirical finding during task_35: when a bundle holds multiple
# Mach-Os in Contents/MacOS/, only the CFBundleExecutable gets its
# Info.plist bound to its signature; auxiliary Mach-Os report
# `Info.plist=not bound` and AMFI refuses to authorize their
# entitlement claims. So the daemon (the only binary that
# actually accesses the keychain) lives alone inside FFS.app;
# the CLI + MCP ship as standalone signed+notarized Mach-Os.
#
# Usage (full release flow, with provisioning profile + notarization):
#   # 1) Build all three release binaries
#   cargo build --release --workspace --bins
#   # 2) Construct + sign FFS.app + the two standalone binaries
#   FFS_SIGNING_IDENTITY="Developer ID Application: <Name> (<TeamID>)" \
#     ./scripts/codesign-macos.sh \
#     target/release/ffs \
#     target/release/ffs-daemon \
#     target/release/ffs-mcp
#   # 3) Notarize the bundle (single submission) + the two binaries
#   /usr/bin/ditto -c -k --keepParent target/release/FFS.app /tmp/FFS.app.zip
#   xcrun notarytool submit /tmp/FFS.app.zip --keychain-profile ffs-notary --wait
#   xcrun stapler staple target/release/FFS.app
#   # ffs + ffs-mcp: ditto + notarytool, no stapling (raw Mach-O can't be stapled)
#
# Required positional arguments (in this order):
#   $1 — path to the `ffs` CLI binary
#   $2 — path to the `ffs-daemon` binary (goes inside FFS.app)
#   $3 — path to the `ffs-mcp` binary
#
# Why `--options runtime`:
#   Hardened runtime is required for Apple notarization. The entire
#   notarize+staple flow refuses any binary not signed with it.
#
# Why `--timestamp`:
#   Apple requires a secure timestamp on any binary that hits
#   `xcrun notarytool` or runs from a downloaded `.dmg`. Without it,
#   Gatekeeper refuses the binary on Catalina+ when the user
#   double-clicks it from a quarantined download.
#
# Why `--force`:
#   Re-running the script after an in-place rebuild updates the
#   signatures without complaining about existing ones.
#
# Why the script doesn't notarize:
#   Notarization requires user-side credentials (Apple ID,
#   app-specific password) that aren't appropriate to embed in a
#   build-time script. Notarization is the next step the caller
#   runs (see usage block above + ADR-025).

set -euo pipefail

# ---- argument validation ----

if [[ -z "${FFS_SIGNING_IDENTITY:-}" ]]; then
  echo "error: FFS_SIGNING_IDENTITY is not set" >&2
  echo "  expected: \"Developer ID Application: <Name> (<TeamID>)\"" >&2
  echo "  find yours: security find-identity -p codesigning -v" >&2
  exit 1
fi

if [[ $# -ne 3 ]]; then
  cat >&2 <<EOF
usage: $0 <ffs> <ffs-daemon> <ffs-mcp>

The three binaries must be passed in this exact order so the
script knows which one becomes the bundled daemon.
EOF
  exit 1
fi

FFS_BIN="$1"
DAEMON_BIN="$2"
MCP_BIN="$3"

for bin in "$FFS_BIN" "$DAEMON_BIN" "$MCP_BIN"; do
  if [[ ! -f "$bin" ]]; then
    echo "error: binary not found: $bin" >&2
    exit 1
  fi
done

# ---- locate companion files ----

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

INFO_PLIST="$REPO_ROOT/bundle/Info.plist"
ENTITLEMENTS="$REPO_ROOT/entitlements/ffs.entitlements.plist"

# Provisioning profile path: env var override wins (CI runners use
# this to point at a runner-temp file); fall back to the local
# secrets/ stash.
PROFILE="${FFS_PROVISIONING_PROFILE:-$REPO_ROOT/secrets/embedded.provisionprofile}"

for f in "$INFO_PLIST" "$ENTITLEMENTS" "$PROFILE"; do
  if [[ ! -f "$f" ]]; then
    echo "error: required file missing: $f" >&2
    if [[ "$f" == "$PROFILE" ]]; then
      cat >&2 <<EOF

Download a Developer ID Distribution provisioning profile from
developer.apple.com → Profiles → "+" → Distribution → Developer ID,
save it as secrets/embedded.provisionprofile.

Or set FFS_PROVISIONING_PROFILE to the absolute path of an existing
profile file.

See docs/onboarding/technical-friend-checklist.md Step 2 Path B
for the full Apple-portal setup.
EOF
    fi
    exit 1
  fi
done

# Read the bundle ID from Info.plist so the codesign --identifier
# flag matches what the profile authorizes.
BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST")"

# ---- bundle construction (daemon only) ----

# The bundle lives alongside the binaries (target/release/ for a
# standard cargo build). The two standalone binaries get re-signed
# in place.
BIN_DIR="$(cd "$(dirname "$FFS_BIN")" && pwd)"
APP="$BIN_DIR/FFS.app"

echo "==> constructing $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
install -m 0755 "$DAEMON_BIN"  "$APP/Contents/MacOS/ffs-daemon"
install -m 0644 "$INFO_PLIST"  "$APP/Contents/Info.plist"
install -m 0644 "$PROFILE"     "$APP/Contents/embedded.provisionprofile"

# ---- sign the bundle ----

echo "==> signing $APP"
# --deep walks Contents/MacOS/ and signs every Mach-O inside.
# --identifier forces the codesign identifier to match the
# bundle's CFBundleIdentifier (also matches the profile's
# application-identifier minus team prefix).
codesign \
  --sign "$FFS_SIGNING_IDENTITY" \
  --force --deep \
  --identifier "$BUNDLE_ID" \
  --options runtime \
  --timestamp \
  --entitlements "$ENTITLEMENTS" \
  "$APP"

echo "==> verifying $APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# ---- sign the two standalone binaries (no entitlements) ----

# `ffs` and `ffs-mcp` talk to the daemon over the UDS socket and
# don't touch the OS keychain directly, so they don't need the
# `keychain-access-groups` entitlement. Signing them as plain
# Developer ID Mach-Os with hardened runtime is what Gatekeeper +
# notarization expects.
for standalone in "$FFS_BIN" "$MCP_BIN"; do
  echo "==> signing standalone $standalone"
  codesign \
    --sign "$FFS_SIGNING_IDENTITY" \
    --force \
    --options runtime \
    --timestamp \
    "$standalone"
  codesign --verify --strict --verbose=2 "$standalone"
done

# ---- verification summary ----

echo "==> Gatekeeper assessment (pre-notarization; expect rejected/Unnotarized):"
spctl --assess --type install --verbose=4 "$APP" 2>&1 || true

cat <<EOF

==> done.

Next step: notarize the bundle + the two standalone binaries.

  # bundle (single submission for all Mach-Os inside)
  /usr/bin/ditto -c -k --keepParent "$APP" /tmp/FFS.app.zip
  xcrun notarytool submit /tmp/FFS.app.zip --keychain-profile ffs-notary --wait
  xcrun stapler staple "$APP"

  # standalone Mach-Os (raw Mach-Os can't be stapled; Gatekeeper
  # does an online lookup against Apple's CDN at first launch)
  for bin in "$FFS_BIN" "$MCP_BIN"; do
    /usr/bin/ditto -c -k --keepParent "\$bin" "/tmp/\$(basename \$bin).zip"
    xcrun notarytool submit "/tmp/\$(basename \$bin).zip" \\
      --keychain-profile ffs-notary --wait
  done

  # final assessments
  spctl --assess --type install --verbose=4 "$APP"   # expect: accepted, Notarized
  spctl --assess --type install --verbose=4 "$FFS_BIN"
  spctl --assess --type install --verbose=4 "$MCP_BIN"

See docs/onboarding/technical-friend-checklist.md Step 2 Path B
for the notarytool credentials setup.
EOF
