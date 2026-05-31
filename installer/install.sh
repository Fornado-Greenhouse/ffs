#!/usr/bin/env bash
# installer/install.sh — POSIX install for FFS on Linux + macOS.
#
# Lays down:
#   - Binaries (ffs, ffs-daemon, ffs-mcp) into a per-user prefix.
#   - ~/.ffs/{config/predicates, config/templates, skills, run, log}.
#   - Starter predicate-spec library and Tera templates (idempotent
#     copy — existing user-edited files are preserved by default).
#   - Python skill bundles (auditor/librarian/scribe), copied under
#     ~/.ffs/skills/.
#   - Per-OS daemon-on-login wiring:
#         Linux  -> ~/.config/systemd/user/ffs-daemon.service
#         macOS  -> ~/Library/LaunchAgents/com.ffs.daemon.plist
#   - Optional Obsidian plugin registration into a vault when
#     --vault <path> is passed (or $FFS_VAULT is set in the env).
#
# This script is intentionally a single file with no external tool
# requirements beyond POSIX shell + `install`/`cp`/`mkdir`. Cargo
# release builds are expected to live alongside this script under
# `bin/<target>/release/` (the CI release archive layout from
# task_01); when missing, the script falls back to the workspace's
# `target/release/` so a local `cargo build --release && bash
# installer/install.sh` works for developers.
#
# Re-running the installer is safe — every step is idempotent.

set -euo pipefail

# -------- argument parsing --------

VAULT_PATH="${FFS_VAULT:-}"
PREFIX="${FFS_PREFIX:-$HOME/.local}"
SKIP_SERVICE=0
SKIP_PLUGIN=0
DRY_RUN=0
ARCH_OVERRIDE=""

usage() {
    cat <<'USAGE'
Usage: install.sh [options]

Options:
  --vault <path>       Register the Obsidian plugin into the given vault.
                       Also honored via the $FFS_VAULT env var.
  --prefix <path>      Binary install prefix (default: $HOME/.local).
  --skip-service       Skip per-OS daemon-on-login wiring.
  --skip-plugin        Skip Obsidian plugin registration.
  --dry-run            Print what would happen without changing anything.
  -h, --help           Show this help.

The installer writes to:
  $PREFIX/bin              # ffs, ffs-daemon, ffs-mcp
  $HOME/.ffs/config/       # predicates + templates (seeded if missing)
  $HOME/.ffs/skills/       # auditor, librarian, scribe Python bundles
  $HOME/.ffs/run/          # daemon socket (mode 700)
  $HOME/.ffs/log/          # daemon stderr captures

Environment knobs:
  FFS_VAULT      Obsidian vault path to register the plugin into.
  FFS_PREFIX     Binary install prefix.
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --vault) VAULT_PATH="$2"; shift 2 ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --skip-service) SKIP_SERVICE=1; shift ;;
        --skip-plugin) SKIP_PLUGIN=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        --arch) ARCH_OVERRIDE="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "install.sh: unknown argument: $1" >&2; usage; exit 64 ;;
    esac
done

# -------- platform detection --------

OS="$(uname -s)"
case "$OS" in
    Darwin) PLATFORM=macos ;;
    Linux)  PLATFORM=linux ;;
    *) echo "install.sh: unsupported OS: $OS (use install.ps1 on Windows)" >&2; exit 64 ;;
esac

ARCH="${ARCH_OVERRIDE:-$(uname -m)}"
case "$ARCH" in
    arm64|aarch64) ARCH_TAG=aarch64 ;;
    x86_64|amd64)  ARCH_TAG=x86_64 ;;
    *) echo "install.sh: unsupported arch: $ARCH" >&2; exit 64 ;;
esac

DATA_DIR="$HOME/.ffs"

# -------- helpers --------

# `say`: log a step.
say() { printf '[install] %s\n' "$*"; }
# `run`: execute (or print, when dry-run).
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '+ %s\n' "$*"
    else
        eval "$@"
    fi
}

# Resolve the directory holding the installer scripts (script home).
script_dir() {
    local src="${BASH_SOURCE[0]:-$0}"
    if [ -L "$src" ]; then
        src="$(readlink "$src")"
    fi
    cd "$(dirname "$src")" >/dev/null 2>&1 && pwd -P
}

# Locate a binary either next to the installer (release archive
# layout) or in the workspace target/release directory.
locate_binary() {
    local name="$1"
    local SCRIPT_HOME
    SCRIPT_HOME="$(script_dir)"
    local candidates=(
        "$SCRIPT_HOME/bin/$PLATFORM-$ARCH_TAG/$name"
        "$SCRIPT_HOME/bin/$name"
        "$SCRIPT_HOME/../target/release/$name"
        "$SCRIPT_HOME/../../target/release/$name"
    )
    for c in "${candidates[@]}"; do
        if [ -x "$c" ]; then
            printf '%s' "$c"
            return 0
        fi
    done
    echo "install.sh: cannot locate binary '$name' — looked in: ${candidates[*]}" >&2
    return 1
}

ensure_dir() {
    local d="$1" mode="${2:-0o755}"
    run "mkdir -p '$d'"
    if [ "$mode" != "skip" ]; then
        run "chmod ${mode//0o/} '$d'"
    fi
}

# Idempotent install of a single file; preserves existing content
# unless --force-overwrite was set (currently always preserve).
install_seed_file() {
    local src="$1" dst="$2"
    if [ -e "$dst" ]; then
        say "preserving existing $dst"
        return 0
    fi
    run "install -m 0644 '$src' '$dst'"
}

# -------- bin placement --------

install_binaries() {
    say "installing binaries to $PREFIX/bin"
    ensure_dir "$PREFIX/bin"
    for name in ffs ffs-daemon ffs-mcp; do
        local src
        src="$(locate_binary "$name")"
        run "install -m 0755 '$src' '$PREFIX/bin/$name'"
    done
    if ! printf ':%s:' "$PATH" | grep -q ":$PREFIX/bin:"; then
        say "NOTE: $PREFIX/bin is not on \$PATH — add 'export PATH=\"$PREFIX/bin:\$PATH\"' to your shell rc."
    fi
}

# -------- starter library --------

install_starter_library() {
    say "seeding starter library under $DATA_DIR/config/"
    ensure_dir "$DATA_DIR/config/predicates"
    ensure_dir "$DATA_DIR/config/templates"
    local SCRIPT_HOME
    SCRIPT_HOME="$(script_dir)"
    local starter_root="$SCRIPT_HOME/starter"
    if [ ! -d "$starter_root" ]; then
        starter_root="$SCRIPT_HOME/../starter"
    fi
    for f in "$starter_root/predicates/"*.toml; do
        [ -e "$f" ] || continue
        install_seed_file "$f" "$DATA_DIR/config/predicates/$(basename "$f")"
    done
    for f in "$starter_root/templates/"*.tera; do
        [ -e "$f" ] || continue
        install_seed_file "$f" "$DATA_DIR/config/templates/$(basename "$f")"
    done
}

# -------- skill bundles --------

install_skills() {
    say "installing Python skill bundles under $DATA_DIR/skills/"
    ensure_dir "$DATA_DIR/skills"
    local SCRIPT_HOME
    SCRIPT_HOME="$(script_dir)"
    local skills_root="$SCRIPT_HOME/skills"
    if [ ! -d "$skills_root" ]; then
        skills_root="$SCRIPT_HOME/../skills"
    fi
    for skill in auditor librarian scribe _lib; do
        if [ -d "$skills_root/$skill" ]; then
            ensure_dir "$DATA_DIR/skills/$skill"
            # Copy the directory contents rather than the directory
            # itself so re-runs don't nest skills/auditor/auditor/.
            run "cp -R '$skills_root/$skill/'* '$DATA_DIR/skills/$skill/' 2>/dev/null || true"
        fi
    done
}

# -------- runtime dirs --------

prepare_runtime_dirs() {
    ensure_dir "$DATA_DIR/run" 700
    ensure_dir "$DATA_DIR/log"
}

# -------- service wiring --------

install_systemd_unit() {
    local unit_dir="$HOME/.config/systemd/user"
    ensure_dir "$unit_dir"
    local unit_file="$unit_dir/ffs-daemon.service"
    local SCRIPT_HOME
    SCRIPT_HOME="$(script_dir)"
    local template="$SCRIPT_HOME/systemd/ffs-daemon.service"
    if [ ! -f "$template" ]; then
        template="$SCRIPT_HOME/../installer/systemd/ffs-daemon.service"
    fi
    if [ ! -f "$template" ]; then
        echo "install.sh: missing systemd template" >&2
        return 1
    fi
    # Expand $HOME and $PREFIX into the unit file via sed (the
    # template uses placeholders so it is checked in verbatim).
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '+ render systemd unit -> %s\n' "$unit_file"
    else
        sed \
            -e "s|@PREFIX@|$PREFIX|g" \
            -e "s|@HOME@|$HOME|g" \
            "$template" > "$unit_file"
    fi
    say "installed systemd unit at $unit_file"
    if command -v systemctl >/dev/null 2>&1; then
        run "systemctl --user daemon-reload || true"
        run "systemctl --user enable --now ffs-daemon.service || true"
    fi
}

install_launchd_plist() {
    local plist_dir="$HOME/Library/LaunchAgents"
    ensure_dir "$plist_dir"
    local plist_file="$plist_dir/com.ffs.daemon.plist"
    local SCRIPT_HOME
    SCRIPT_HOME="$(script_dir)"
    local template="$SCRIPT_HOME/launchd/com.ffs.daemon.plist"
    if [ ! -f "$template" ]; then
        template="$SCRIPT_HOME/../installer/launchd/com.ffs.daemon.plist"
    fi
    if [ ! -f "$template" ]; then
        echo "install.sh: missing launchd template" >&2
        return 1
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '+ render launchd plist -> %s\n' "$plist_file"
    else
        sed \
            -e "s|@PREFIX@|$PREFIX|g" \
            -e "s|@HOME@|$HOME|g" \
            "$template" > "$plist_file"
    fi
    say "installed launchd plist at $plist_file"
    if command -v launchctl >/dev/null 2>&1; then
        run "launchctl unload '$plist_file' 2>/dev/null || true"
        run "launchctl load '$plist_file'"
    fi
}

wire_service() {
    if [ "$SKIP_SERVICE" -eq 1 ]; then
        say "skipping service wiring (--skip-service)"
        return 0
    fi
    case "$PLATFORM" in
        linux) install_systemd_unit ;;
        macos) install_launchd_plist ;;
    esac
}

# -------- obsidian plugin --------

install_obsidian_plugin() {
    if [ "$SKIP_PLUGIN" -eq 1 ]; then
        say "skipping Obsidian plugin registration (--skip-plugin)"
        return 0
    fi
    if [ -z "$VAULT_PATH" ]; then
        say "no vault path provided (use --vault or set FFS_VAULT); skipping plugin registration"
        return 0
    fi
    if [ ! -d "$VAULT_PATH/.obsidian" ]; then
        say "WARN: $VAULT_PATH/.obsidian does not exist — is the vault open in Obsidian at least once?"
        say "skipping plugin registration"
        return 0
    fi
    local plugin_dst="$VAULT_PATH/.obsidian/plugins/ffs"
    ensure_dir "$plugin_dst"
    local SCRIPT_HOME
    SCRIPT_HOME="$(script_dir)"
    local plugin_src="$SCRIPT_HOME/obsidian-plugin"
    if [ ! -d "$plugin_src" ]; then
        plugin_src="$SCRIPT_HOME/../obsidian-plugin/dist"
    fi
    if [ ! -d "$plugin_src" ]; then
        plugin_src="$SCRIPT_HOME/../obsidian-plugin"
    fi
    if [ -d "$plugin_src" ]; then
        for f in main.js manifest.json styles.css; do
            if [ -f "$plugin_src/$f" ]; then
                run "install -m 0644 '$plugin_src/$f' '$plugin_dst/$f'"
            fi
        done
        say "registered Obsidian plugin at $plugin_dst"
    else
        say "WARN: Obsidian plugin source not found; skipping"
    fi
}

# -------- keychain bootstrap --------

bootstrap_keychain() {
    # MVP: the daemon binary tolerates a missing FFS_OWNER_KEY_HEX
    # by warning and generating a fresh key. The installer-driven
    # path is to seed a stable key into the OS keychain and have
    # the service unit (systemd / launchd) export it before
    # exec'ing the daemon.
    #
    # This task only ensures the data dir exists; on first daemon
    # run the warn-log surfaces the issue. Production key
    # provisioning (via `security add-generic-password` on macOS
    # and `secret-tool store` on Linux) lands as a Phase 2 add.
    say "keychain bootstrap: deferred to first-run interactive (see README)"
}

# -------- main --------

main() {
    say "FFS installer — platform=$PLATFORM arch=$ARCH_TAG prefix=$PREFIX"
    say "dry_run=$DRY_RUN vault=${VAULT_PATH:-<none>}"
    install_binaries
    prepare_runtime_dirs
    install_starter_library
    install_skills
    wire_service
    install_obsidian_plugin
    bootstrap_keychain
    say "done — try: $PREFIX/bin/ffs health"
    say "next steps — see docs/onboarding/technical-friend-checklist.md and docs/onboarding/first-use-guide.md"
    say "trouble?  see docs/onboarding/troubleshooting.md"
}

main
