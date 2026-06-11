---
status: pending
title: Windows daemon path correctness — fastpath path normalization + scribe budget + named-pipe e2e
type: backend
complexity: high
dependencies:
  - task_07
  - task_09
  - task_11
  - task_22
---

# Task 34: Windows daemon path correctness — fastpath path normalization + scribe budget + named-pipe e2e

## Overview
The CI cleanup pass on 2026-06-09/11 (commits `c1a9a70`, `eee8b73`, `283c2de`) got the Windows job *compiling* cleanly across the workspace by gating Unix-only call sites with `#[cfg(unix)]` and pinning line endings via `.gitattributes`. With those compile/checkout gates in place, the Windows test run surfaced what was hiding underneath: two genuine cross-platform product bugs and a missing transport story.

This task closes the Windows daemon-path gaps so a future Windows install actually works. It is gating for task_27 (keychain) finally landing across all three OSes — and is the Windows-side sibling of task_33's macOS-signing work.

<critical>
- ALWAYS READ the PRD and TechSpec before starting
- REFERENCE TECHSPEC for implementation details — do not duplicate here
- FOCUS ON "WHAT" — describe what needs to be accomplished, not how
- MINIMIZE CODE — show code only to illustrate current structure or problem areas
- TESTS REQUIRED — every task MUST include tests in deliverables
</critical>

<requirements>
- MUST normalize OS path separators to `/` at every fast-path boundary that produces an `ffs://path` string or a projection-path key. Specifically: `crates/ffs-fastpath/src/watcher.rs:187` produces `rel_str` via `rel.to_string_lossy()` which yields backslashes on Windows. Every consumer of that string (the classifier's reverse-map matcher, `event.projection.invalidated.params.path`, `event.fastpath.applied.params.projection_path`, the route-to-ingest filename derivation) expects forward slashes. The fix is a single normalization helper applied once at the boundary, with backslash-only on Windows behavior covered by a unit test that exercises the helper with both separator shapes.
- MUST raise the scribe extraction budget when running on Windows. The Python subprocess cold-start on `windows-latest` GitHub runners is empirically ~2-3× slower than on macOS/Ubuntu, and the existing `ingest_submit_with_scribe_lands_contact_person_proposal_with_provenance` integration test times out at `scribe extraction completed within budget` (panic at `crates/ffs-daemon/tests/scribe_integration.rs:383:19`) on Windows. Either bump the budget unconditionally to accommodate the slowest platform, or apply a platform-conditional multiplier — but document the choice in the test source.
- MUST add a Windows-side counterpart to the UDS integration tests now gated with `#![cfg(unix)]`. At minimum: a `binary_end_to_end_windows.rs` (or `cfg(windows)`-gated module) that spawns the daemon, connects via `tokio::net::windows::named_pipe::ClientOptions`, sends one `health.summary` JSON-RPC frame, and asserts on the response shape. This proves the Windows transport path actually round-trips — today it has no integration coverage at all.
- MUST add a Windows-side `0o700`-equivalent for the `$FFS_DATA_DIR/run/` directory ACL hardening. The daemon's `main.rs` currently `#[cfg(unix)]`s the `chmod(0o700)` call and does nothing on Windows. Apply an ACL via `windows-sys`/`windows-acl`/PowerShell-from-installer (preferred site TBD during task work) that restricts the run dir to the current user. Document the chosen approach in an ADR.
- MUST extend `installer/install.ps1` to apply the equivalent ACL hardening to `$FFS_DATA_DIR` itself on install, matching what `installer/install.sh` does with `chmod 0o700`.
- SHOULD also normalize path separators in the materializer's projection-path emission (`crates/ffs-daemon/src/materializer.rs`) and the `ffs://path/` URL formatter (`crates/ffs-cli/src/url.rs`). These two surfaces are reachable from Windows but were not exercised by the CI failure; the fix is the same boundary normalization.
- SHOULD verify that `notify::PollWatcher`'s event paths on Windows are absolute (not UNC-prefixed `\\?\C:\…`). If they are UNC-prefixed, the `strip_prefix(working_set_dir)` call at `watcher.rs:183` silently fails and the watcher goes quiet. Add a normalization step (`dunce::canonicalize` or equivalent) and a regression test.
- SHOULD gate the Windows-specific tests AND the Windows-specific ACL code with `#[cfg(windows)]` symmetrically to the existing `#![cfg(unix)]` modules so the cross-platform-CI guardrails stay obvious.
</requirements>

## Subtasks
- [ ] 34.1 Add a `normalize_separators(path: &str) -> Cow<'_, str>` helper in `ffs-fastpath` (or `ffs-core::projection::path`) that returns the input unchanged on Unix and replaces `\\` with `/` on Windows. Apply at every boundary that emits a projection-path string. Add unit tests exercising both shapes.
- [ ] 34.2 Investigate the scribe Python-subprocess cold-start cost on Windows; raise the integration-test budget accordingly. Document the chosen budget shape (unconditional vs. platform-conditional) at the call site.
- [ ] 34.3 Add `crates/ffs-daemon/tests/binary_end_to_end_windows.rs` (or equivalent) that spawns the daemon and round-trips one JSON-RPC call via named pipe. Gate with `#![cfg(windows)]`. Reuse the Unix test's seed-data-dir helpers via a shared `tests/common/` module.
- [ ] 34.4 Implement Windows ACL hardening for `$FFS_DATA_DIR/run/`. Two options to evaluate during task work: (a) daemon-side via `windows-sys` SetNamedSecurityInfo, or (b) installer-side via PowerShell. Pick one; document why in an ADR.
- [ ] 34.5 Extend `installer/install.ps1` to apply the same ACL hardening to `$FFS_DATA_DIR` on install.
- [ ] 34.6 Audit and normalize remaining projection-path emission sites: `crates/ffs-daemon/src/materializer.rs`, `crates/ffs-cli/src/url.rs`. Regression tests for each.
- [ ] 34.7 Verify the `notify::PollWatcher` path shape on Windows; add `dunce`-style normalization if UNC-prefixed paths are observed. Regression test.

## Implementation Details

### Path normalization boundary
The fast-path classifier is built around the assumption that the rendered projection's path is `/`-separated — that's how `path::parse` decomposes `contacts/by-name/S/Sarah_Chen.md` into `(family, letter, entity)`. On Windows, `rel.to_string_lossy()` produces `contacts\by-name\S\Sarah_Chen.md`, which `path::parse` doesn't recognize. The classifier falls through to "no head atom for this projection" → `RoutedToIngest { reason: PathOrHeadUnavailable }` → emits `event.projection.invalidated` instead of `event.fastpath.applied`. The exact failure shape we saw on Windows CI:

```
expected fastpath.applied; got {"jsonrpc":"2.0","method":"event.projection.invalidated",
  "params":{"path":"contacts\\by-name\\S\\Sarah_Chen.md"}}
```

The fix is a single boundary normalization at `watcher.rs:187` and any other site that constructs a projection-path string from a `Path`. The atom envelope and reverse-map rules already use `/`; only the OS-facing read side needs the conversion.

### Scribe budget on Windows
The failing test (`scribe_integration.rs:383`) asserts that scribe extraction completes within a fixed wall-clock budget. On Windows CI the assertion fires because Python subprocess cold-start (interpreter + import-stdlib + import-`ffs_skill` + import-extraction-module) takes long enough to push the test past the budget. Two options:

```rust
const SCRIBE_BUDGET: Duration = if cfg!(target_os = "windows") {
    Duration::from_secs(12)  // Windows Python cold-start ~2-3x slower
} else {
    Duration::from_secs(4)
};
```

vs. a single generous unconditional budget. The platform-conditional approach is more honest about why the gap exists and won't silently mask a regression on the fast platforms.

### Windows named-pipe e2e
`crates/ffs-daemon/src/transport/windows.rs` exists but is exercised by zero integration tests today. The skeleton mirrors the Unix one:

```rust
#![cfg(windows)]

#[tokio::test]
async fn daemon_binary_round_trips_health_summary_via_named_pipe() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();
    seed_data_dir(&data_dir);
    // … spawn daemon with FFS_DATA_DIR + FFS_OWNER_KEY_HEX + FFS_SQLCIPHER_KEY_HEX
    let pipe_name = format!(r"\\.\pipe\ffs-{}", std::process::id());
    let mut client = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe_name)
        .unwrap();
    // … send `{"jsonrpc":"2.0","id":1,"method":"health.summary","params":{}}\n`
    // … assert id=1, result.atoms_total >= 0
}
```

### Windows ACL hardening
The Unix daemon does `chmod 0o700` on `$FFS_DATA_DIR/run/` so a malicious local process can't swap the socket. The Windows equivalent is an ACL that grants Full Control to the current user only and denies everyone else. Concretely (option a — daemon-side):

```rust
#[cfg(windows)]
{
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, SetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    // Build an ACL granting GENERIC_ALL to the current user SID, no inheritance.
    // Apply via SetNamedSecurityInfoW with DACL_SECURITY_INFORMATION.
}
```

Option b (installer-side PowerShell) avoids the `windows-sys` dependency in the daemon at the cost of weaker runtime enforcement. The ADR should weigh the trade-off and pick.

### Relevant Files
- `crates/ffs-fastpath/src/watcher.rs:187` — `rel_str` construction site.
- `crates/ffs-fastpath/src/classifier.rs` — reverse-map matching; consumes `rel_str`.
- `crates/ffs-fastpath/src/dispatch.rs` — emits `projection_path` in event params.
- `crates/ffs-daemon/src/main.rs` — `#[cfg(unix)]`-gated `chmod 0o700` site; Windows counterpart goes here.
- `crates/ffs-daemon/src/materializer.rs` — secondary projection-path emission.
- `crates/ffs-cli/src/url.rs` — `ffs://path/` URL builder.
- `crates/ffs-daemon/src/transport/windows.rs` — currently uncovered.
- `crates/ffs-daemon/tests/scribe_integration.rs:383` — scribe budget assertion.
- `installer/install.ps1` — Windows ACL hardening on `$FFS_DATA_DIR`.

### Dependent Files
- `crates/ffs-daemon/tests/binary_end_to_end_windows.rs` (new).
- `crates/ffs-fastpath/tests/fastpath_integration.rs` — fastpath_applies_frontmatter_value_edit_within_budget will start passing on Windows once 34.1 + 34.7 land.

### Related ADRs
- [ADR-014: Editor-agnostic fast-path with reverse-map annotations](adrs/adr-014.md) — Fast-path's path-key contract.
- [ADR-019: JSON-RPC 2.0 framing over UDS / named pipe](adrs/adr-019.md) — Transport equivalence claim that this task actually proves on Windows.
- New ADR-024: Windows ACL hardening approach (daemon-side `SetNamedSecurityInfo` vs. installer-side PowerShell — pick one).

## Deliverables
- `normalize_separators` helper + boundary applications at every projection-path emission site.
- Bumped scribe budget on Windows (either unconditional or platform-conditional) with rationale in the source.
- `binary_end_to_end_windows.rs` (or equivalent) exercising the named-pipe transport end-to-end.
- Windows ACL hardening for `$FFS_DATA_DIR/run/` (daemon-side or installer-side per ADR-024).
- `installer/install.ps1` ACL extension for `$FFS_DATA_DIR`.
- ADR-024 documenting the Windows hardening approach.
- Unit tests with 80%+ coverage **(REQUIRED)** — the normalizer, the Windows ACL helper if daemon-side, the projection-path consumers' `\`-vs-`/` tolerance.
- Integration test **(REQUIRED)** — Windows named-pipe end-to-end round-trip.

## Tests
- Unit tests:
  - [ ] `normalize_separators("contacts\\by-name\\S\\Sarah_Chen.md")` returns `"contacts/by-name/S/Sarah_Chen.md"` on Windows, unchanged on Unix.
  - [ ] `projection_path::parse` of a normalized `rel_str` produces the same `SingleEntity` whether the input was `\\`- or `/`-shaped pre-normalization.
  - [ ] (cfg(windows) only) Windows ACL helper actually denies a second user — or the installer test exercises the PowerShell path with a fake user and asserts.
- Integration tests:
  - [ ] (cfg(windows) only) Daemon binary spawned with `FFS_DATA_DIR`, `FFS_OWNER_KEY_HEX`, `FFS_SQLCIPHER_KEY_HEX`, `FFS_KEYRING_DISABLE=1` round-trips one `health.summary` JSON-RPC call via the named pipe.
  - [ ] (cfg(windows) only) `fastpath_applies_frontmatter_value_edit_within_budget` passes on Windows after the normalization change — the existing test body should suffice once the underlying bug is fixed.
  - [ ] (any OS) `ingest_submit_with_scribe_lands_contact_person_proposal_with_provenance` passes within the new (possibly-platform-conditional) scribe budget on all three CI runners.
- Test coverage target: >=80%
- All tests must pass

## Success Criteria
- All tests passing on all three CI matrix OSes (ubuntu-latest, macos-latest, windows-latest).
- Test coverage >=80%
- The `actions/CI` job badge on the `main` branch reads green for the first time since task_22 landed.
- A Windows user who follows `installer/install.ps1` ends up with a hardened `$FFS_DATA_DIR` and a working daemon that bind/listens on a named pipe.
- Editing a projection file in Obsidian on Windows produces an `event.fastpath.applied` (not the slow-path `event.projection.invalidated`) when the diff matches a reverse-map rule.
- task_27 (keychain) becomes shippable on Windows once this lands — the daemon's bind, transport, and path-handling are all proven on Windows, which is the precondition for trusting the `keyring` v3 Windows Credential Manager backend in production.
