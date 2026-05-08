---
status: pending
title: Cross-platform installer scripts for Linux, macOS, Windows
type: infra
complexity: medium
dependencies:
  - task_08
  - task_17
---

# Task 22: Cross-platform installer scripts for Linux, macOS, Windows

## Overview
Package the FFS MVP for the three OS targets: ship the daemon, CLI, MCP server binaries, the Obsidian plugin, the Python skill bundle, the starter predicate-spec library, and the starter Tera templates as a coherent installer per platform. The installer is the friction floor for the technical-friend onboarding flow — it must be runnable with light support and no terminal use during normal operation.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST install the four Rust binaries (`ffs-daemon`, `ffs`, `ffs-mcp`, plus any helper) into a platform-appropriate location.
- MUST install the Python skill bundle (scribe, librarian, auditor) under `~/.ffs/skills/`.
- MUST seed `~/.ffs/config/predicates/` with the starter spec files (task 20) and `~/.ffs/config/templates/` with the starter Tera templates (task 21).
- MUST set up the daemon to start on user login (systemd-user / launchd / Windows scheduled task).
- MUST register the Obsidian plugin (copy to `<vault>/.obsidian/plugins/ffs/`) when an Obsidian vault is configured.
- MUST initialize the OS keychain entries for DEK and author signing key on first run (or guide the user through setup).
- MUST be runnable as a single command per platform: `bash install.sh` (Linux/macOS) or `install.ps1` (Windows).
- MUST produce a working substrate after install: daemon runs, CLI connects, plugin loads.
- SHOULD include an uninstall script that reverses the install cleanly.
</requirements>

## Subtasks
- [ ] 22.1 Author `install.sh` for Linux + macOS (POSIX shell, single file).
- [ ] 22.2 Author `install.ps1` for Windows (PowerShell, single file).
- [ ] 22.3 Implement binary placement, PATH integration, and per-OS daemon-on-login wiring.
- [ ] 22.4 Implement Python skill bundle deployment under `~/.ffs/skills/`.
- [ ] 22.5 Implement starter library deployment under `~/.ffs/config/`.
- [ ] 22.6 Implement OS keychain bootstrap (or interactive guide).
- [ ] 22.7 Implement Obsidian plugin registration into a configured vault.
- [ ] 22.8 Author the matching uninstaller scripts.

## Implementation Details
Create `installer/install.sh`, `installer/install.ps1`, `installer/uninstall.sh`, `installer/uninstall.ps1`. The release artifacts produced by task 01's CI (Rust static binaries) are bundled into a tarball / zip per platform. Installers fetch the appropriate archive or work from a local copy.

See PRD § User Experience § Onboarding by a technical friend for the user-experience constraints.

### Relevant Files
- `installer/install.sh` (new) — Linux + macOS.
- `installer/install.ps1` (new) — Windows.
- `installer/uninstall.sh` (new).
- `installer/uninstall.ps1` (new).
- `installer/systemd/ffs-daemon.service` (new) — Linux user service.
- `installer/launchd/com.ffs.daemon.plist` (new) — macOS launchd plist.
- `crates/ffs-cli/`, `crates/ffs-daemon/`, `crates/ffs-mcp/` (tasks 07, 08, 16) — produce binaries.
- `obsidian-plugin/` (task_17) — produces the plugin bundle.
- `skills/` (tasks 11, 12, 13) — Python skill bundle.
- `starter/predicates/`, `starter/templates/` (tasks 20, 21) — config seed.

### Dependent Files
- Onboarding documentation (task_23) — references these installers.

### Related ADRs
- [ADR-015: Minimal FFS-specific daemon implemented in Rust](adrs/adr-015.md) — Static binaries make installation simple.
- [ADR-009: Claw integration](adrs/adr-009.md) — Skills install as `SKILL.md`-shaped directories.

## Deliverables
- Three working installers (`install.sh` for Linux + macOS, `install.ps1` for Windows).
- Daemon-on-login wiring for each OS.
- Obsidian plugin registration when a vault is provided.
- Starter library and skill bundle deployment.
- Uninstaller scripts.
- Unit tests with 80%+ coverage **(REQUIRED)** — for the installer's helper functions.
- Integration tests on each OS in clean VMs/containers **(REQUIRED)**.

## Tests
- Unit tests:
  - [ ] Installer's path-resolver helper produces correct destinations on each OS.
  - [ ] Keychain bootstrap fails gracefully when the user denies permission and emits a clear error.
  - [ ] Skill-bundle copier handles existing skill directories without overwriting unrelated files.
- Integration tests (on clean OS images):
  - [ ] On a clean Ubuntu VM, `bash install.sh` produces a running daemon, working `ffs --version`, and seeded `~/.ffs/`.
  - [ ] On a clean macOS VM, `bash install.sh` configures launchd and starts the daemon on next login.
  - [ ] On a clean Windows VM, `install.ps1` registers a scheduled task and the daemon runs at login.
  - [ ] Obsidian plugin registration: with a configured vault path, the plugin appears in Obsidian's plugin list.
  - [ ] Uninstaller cleanly removes binaries, skills, plugin, and login wiring; user data under `~/.ffs/` is preserved unless `--purge` is passed.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- Total install time on each OS under 5 minutes from script invocation to "daemon running and ready".
- Technical friend can onboard a non-technical peer using only the installer + checklist (task 23) in under one hour total.
