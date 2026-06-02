---
status: pending
title: Substrate-is-vault — $FFS_DATA_DIR is the Obsidian vault root
type: infra
complexity: low
dependencies:
  - task_22
  - task_25
---

# Task 30: Substrate-is-vault — $FFS_DATA_DIR is the Obsidian vault root

## Overview
The post-task_26 rehearsal exposed a structural mismatch: the installer's `--vault` argument accepted any path, but the materializer writes everything under `$FFS_DATA_DIR` (default `~/.ffs/`). So a user who picked an arbitrary vault path saw an empty vault — the substrate's contacts/, notes/, etc. were materializing somewhere Obsidian couldn't see. ARCHITECTURE.md and `first-use-guide.md` always intended `$FFS_DATA_DIR` to *be* the vault. This task makes that explicit: the installer defaults the vault to the data dir, seeds `.obsidian/plugins/ffs/` there, retires the `--vault <other-path>` flag for MVP (kept behind a `--external-vault` flag for the rare case where someone wants symlink-style separation, which we'll document but not implement).

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST default the installer's vault destination to `$FFS_DATA_DIR` (no `--vault` arg needed); the installer creates `$FFS_DATA_DIR/.obsidian/plugins/ffs/` and copies the plugin there.
- MUST remove the `--vault <path>` flag's previous behavior (writing the plugin into an arbitrary path). The flag stays accepted for backwards compatibility but now warns when the path differs from `$FFS_DATA_DIR`.
- MUST update `docs/onboarding/first-use-guide.md` and `docs/onboarding/technical-friend-checklist.md` to direct users to open `~/.ffs/` as their Obsidian vault — not a separate location.
- MUST update the installer's installer-layout regression test to assert `.obsidian/plugins/ffs/` lands under the data dir.
- SHOULD add an installer warning when `~/.ffs/.obsidian/` already exists with a non-FFS plugin tree, so users with a pre-existing config don't get clobbered silently.
- SHOULD update the `screenshots/README.md` to clarify that the depicted file explorer is rooted at `~/.ffs/`.
</requirements>

## Subtasks
- [ ] 30.1 Update `installer/install.sh` to default the vault to `$FFS_DATA_DIR` and seed `.obsidian/plugins/ffs/` there.
- [ ] 30.2 Same change for `installer/install.ps1` (Windows).
- [ ] 30.3 Update the `installer/uninstall.sh` and `uninstall.ps1` so the `--purge` paths drop `.obsidian/plugins/ffs/` cleanly.
- [ ] 30.4 Update `docs/onboarding/first-use-guide.md` and `docs/onboarding/technical-friend-checklist.md` with the new flow.
- [ ] 30.5 Update `crates/ffs-daemon/tests/installer_layout.rs` to assert the new layout.
- [ ] 30.6 Document the substrate-as-vault decision as a new ADR (post-MVP-plan: ADR-022 or later).

## Implementation Details
The installer's `install_obsidian_plugin` function (in `installer/install.sh`) currently respects an optional `--vault <path>` flag. Change it to default `VAULT_PATH="$DATA_DIR"` when the flag is unset, and add an early `mkdir -p "$DATA_DIR/.obsidian"` so the existing `[ ! -d "$VAULT_PATH/.obsidian" ]` check passes without the user pre-opening the vault in Obsidian.

The user's first interaction with Obsidian becomes: "Open another vault → Open folder as vault → `~/.ffs/`". From there the plugin auto-enables, the daily-summary panel opens, and materialized files appear in the file explorer at the canonical path-library layout.

For users who installed before this change: a one-time migration is straightforward — symlink or copy the plugin from the old vault path into `~/.ffs/.obsidian/plugins/ffs/`. The technical-friend-checklist gets a brief "migrating from a pre-task_30 install" section.

### Relevant Files
- `installer/install.sh` — `install_obsidian_plugin()` function.
- `installer/install.ps1` — Windows equivalent.
- `installer/uninstall.sh` / `installer/uninstall.ps1` — handle `.obsidian/plugins/ffs/` under the data dir.
- `docs/onboarding/first-use-guide.md` — open `~/.ffs/` as vault.
- `docs/onboarding/technical-friend-checklist.md` — installer flow + migration note.
- `crates/ffs-daemon/tests/installer_layout.rs` — assertions.

### Dependent Files
- `docs/onboarding/screenshots/projection-navigation.svg` — caption already says `contacts/by-name/...`; no change needed but confirm it makes sense in context.

### Related ADRs
- [ADR-005: Editor-agnostic working set materialization](adrs/adr-005.md) — Real files on disk for any editor — clarifies which directory holds them.
- [ADR-011: Path library starts at three (contacts/people/notes)](adrs/adr-011.md) — The path-library layout is rooted at the substrate root.
- New ADR: substrate-is-vault as the canonical install shape.

## Deliverables
- Installer scripts that default to substrate-is-vault and seed `.obsidian/plugins/ffs/` automatically.
- Updated onboarding docs explaining the single-root model.
- A new ADR documenting the decision.
- Updated `installer_layout.rs` regression test.
- Unit tests with 80%+ coverage **(REQUIRED)** — for the helper functions in installer-test land.
- Integration tests covering the install path **(REQUIRED)** — already covered by `installer_layout.rs`, extend rather than add new.

## Tests
- Unit/integration tests:
  - [ ] `installer/install.sh` invoked with no `--vault` seeds `$FFS_DATA_DIR/.obsidian/plugins/ffs/main.js` and `manifest.json`.
  - [ ] `installer/install.sh` invoked with `--vault $FFS_DATA_DIR` produces an identical layout (no double-nesting).
  - [ ] `installer/install.sh` invoked with `--vault /some/other/path` writes the plugin under that path AND warns to stderr about the non-canonical install location.
  - [ ] `installer/uninstall.sh --purge` removes `$FFS_DATA_DIR/.obsidian/plugins/ffs/` cleanly.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing
- Test coverage >=80%
- After running the installer, a user can open Obsidian, add `~/.ffs/` as a vault, enable the FFS plugin, and immediately see materialized contact files in the file explorer at the canonical path-library locations.
- The first-use-guide step "find your new contact under contacts/by-name/" works without the user having to know or care about the substrate-vs-vault distinction.
