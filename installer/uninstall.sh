#!/usr/bin/env bash
# installer/uninstall.sh — reverse what install.sh did.
#
# By default this removes:
#   - The three binaries under $PREFIX/bin.
#   - The systemd user unit OR the launchd agent (whichever applies).
#   - The Obsidian plugin under <vault>/.obsidian/plugins/ffs/ (when
#     --vault is provided).
#
# It does NOT touch $HOME/.ffs/ (atom store, config, skills, run
# dir) unless `--purge` is passed — those carry user state.

set -euo pipefail

PREFIX="${FFS_PREFIX:-$HOME/.local}"
VAULT_PATH="${FFS_VAULT:-}"
PURGE=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --vault) VAULT_PATH="$2"; shift 2 ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --purge) PURGE=1; shift ;;
        -h|--help)
            cat <<'USAGE'
Usage: uninstall.sh [options]
  --vault <path>   Remove the Obsidian plugin from this vault.
  --prefix <path>  Where binaries live (default: $HOME/.local).
  --purge          Also delete $HOME/.ffs (DESTRUCTIVE — user data).
USAGE
            exit 0 ;;
        *) echo "uninstall.sh: unknown arg: $1" >&2; exit 64 ;;
    esac
done

OS="$(uname -s)"

say() { printf '[uninstall] %s\n' "$*"; }

# Remove binaries.
for name in ffs ffs-daemon ffs-mcp; do
    if [ -e "$PREFIX/bin/$name" ]; then
        say "removing $PREFIX/bin/$name"
        rm -f "$PREFIX/bin/$name"
    fi
done

# Service unit.
case "$OS" in
    Linux)
        unit="$HOME/.config/systemd/user/ffs-daemon.service"
        if [ -f "$unit" ]; then
            if command -v systemctl >/dev/null 2>&1; then
                systemctl --user disable --now ffs-daemon.service 2>/dev/null || true
            fi
            rm -f "$unit"
            say "removed $unit"
        fi
        ;;
    Darwin)
        plist="$HOME/Library/LaunchAgents/com.ffs.daemon.plist"
        if [ -f "$plist" ]; then
            if command -v launchctl >/dev/null 2>&1; then
                launchctl unload "$plist" 2>/dev/null || true
            fi
            rm -f "$plist"
            say "removed $plist"
        fi
        ;;
esac

# Obsidian plugin.
if [ -n "$VAULT_PATH" ] && [ -d "$VAULT_PATH/.obsidian/plugins/ffs" ]; then
    rm -rf "$VAULT_PATH/.obsidian/plugins/ffs"
    say "removed Obsidian plugin at $VAULT_PATH/.obsidian/plugins/ffs"
fi

# User data (only with explicit --purge).
if [ "$PURGE" -eq 1 ]; then
    if [ -d "$HOME/.ffs" ]; then
        rm -rf "$HOME/.ffs"
        say "PURGED $HOME/.ffs"
    fi
else
    say "preserved $HOME/.ffs — pass --purge to also delete it"
fi
