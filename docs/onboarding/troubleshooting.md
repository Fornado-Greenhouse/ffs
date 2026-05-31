# Troubleshooting

When something goes wrong with FFS, the answer is almost
certainly one of the issues below. Each entry covers symptoms,
what's actually going on, and how to fix it.

For the **why** behind each known risk, see TechSpec § Known
Risks in [`_techspec.md`](../../.compozy/tasks/ffs-mvp/_techspec.md).

> Reading order: this file is meant for skim-and-find. Jump to
> the headline that matches your symptom.

---

## Daemon doesn't start

### Symptom

`ffs health` prints `Error: connection refused` or similar;
`systemctl --user status ffs-daemon` shows the unit as
`activating` or `failed`; on Windows the Scheduled Task shows
"Last Run Result" as something other than 0x0.

### What's happening

The daemon process either crashed during startup (missing
templates, malformed predicate spec, port-of-call permission
issue) or never ran at all (service unit not enabled, PATH
missing the binary).

### Fix

1. **Check the daemon's stderr.** On Linux:
   ```sh
   journalctl --user -u ffs-daemon -n 50
   ```
   On macOS: `~/.ffs/log/ffs-daemon.err.log`. On Windows: open
   Event Viewer → Task Scheduler → look for the FFS Daemon task
   history.
2. **Common boot errors** and what they mean:
   - `templates dir <path>: not found` — the installer didn't
     seed `~/.ffs/config/templates/`. Re-run the installer (it's
     idempotent).
   - `predicate dir <path>: malformed TOML` — a user-edited
     predicate spec broke. Restore from `installer/starter/
     predicates/` or revert the edit.
   - `bind <path>: permission denied` — `~/.ffs/run/` exists but
     not owned by you. `chown -R "$USER" ~/.ffs` and re-run.
3. **Confirm the binary is on PATH.**
   ```sh
   which ffs                       # Linux/macOS
   Get-Command ffs                 # Windows PowerShell
   ```
   If missing, add `$HOME/.local/bin` to PATH (Linux/macOS) or
   reopen the shell (Windows — install.ps1 only updates PATH for
   *new* sessions).

---

## SQLCipher cross-platform issues

### Symptom

Daemon refuses to open the atom store with errors mentioning
`file is not a database`, `sqlite_init`, or `cipher_v4`. The
working set silently doesn't update because writes are failing.

### What's happening

SQLCipher is a build-time-feature crate; each target needs its
own build. If the daemon binary you installed was built against
a different SQLCipher major version than the store on disk, it
can't open the file. TechSpec § Known Risks calls this out as
the **SQLCipher cross-compilation friction** risk.

### Fix

1. **Confirm the version mismatch:**
   ```sh
   ffs --version              # daemon version
   sqlite3 ~/.ffs/atoms.db ".dbinfo"   # may or may not work depending on tools installed
   ```
2. **If you installed from a release binary**, re-download the
   archive *for your platform* from the FFS releases page —
   don't reuse a binary built for another OS or arch.
3. **If you built locally**, ensure the `bundled-sqlcipher`
   feature was active:
   ```sh
   cargo build --release -p ffs-daemon --features bundled-sqlcipher
   ```
4. **If the store is irrecoverably bad** but you have peer
   federations: rebuild from a federation pull. *(Phase 2.)*

> **MVP scope note.** SQLite blessing-carrying source files
> currently use `MemAtomStore` for the daemon binary; a SQLite-
> backed `AtomStore` is wired into `ffs-core` but is not the
> daemon's default. This troubleshooting section becomes
> load-bearing when the SQLite backend is flipped on by default
> in a future release.

---

## Windows named-pipe quirks

### Symptom

On Windows, the Obsidian plugin shows `daemon unreachable` or
`pipe busy`. The CLI works but the plugin doesn't (or vice
versa). Intermittent disconnects after Obsidian sleeps and wakes.

### What's happening

Node's `net.createConnection` to `\\.\pipe\<name>` has a few
quirks: it doesn't always reconnect cleanly after pipe-server
restart, and it's strict about path format (must start with
`\\.\pipe\`, not `\\?\pipe\`). TechSpec calls this the
**Obsidian plugin's Windows named-pipe path** risk.

### Fix

1. **Verify the pipe path** in Obsidian's FFS settings:
   `\\.\pipe\ffs-daemon` (double backslashes if you typed it,
   single backslashes in the displayed value).
2. **Restart the plugin** (Settings → Community plugins → toggle
   FFS off then on again). This re-runs the reconnect logic.
3. **CLI fallback.** If direct IPC keeps misbehaving, the plugin
   can be told to shell out to `ffs.exe` instead. Set **Use CLI
   subprocess** in the plugin settings to *Yes*. This is slower
   but more robust.
4. **Daemon-side check.** Confirm the daemon is actually
   listening:
   ```powershell
   Get-Process ffs-daemon
   # If absent, restart the Scheduled Task:
   Start-ScheduledTask -TaskName "FFS Daemon"
   ```

---

## Federation handshake fails

### Symptom

`ffs federation peer add` exits with `tls_handshake_failed` or
`peer fingerprint mismatch`. Pulls never complete; the daily
summary stays empty of federation events. The peer's contacts
never appear under `contacts/from/<peer>/`.

### What's happening

Federation uses mTLS (mutual TLS) with peer fingerprints
exchanged out-of-band. If the fingerprint you typed doesn't
match what the peer's server presents — typo, stale fingerprint
because the peer regenerated their key, MITM somewhere — the
connection rejects. TechSpec § Known Risks calls this the
**federation handshake UX is unforgiving** risk.

### Fix

1. **Both sides re-print fingerprints**:
   ```sh
   ffs federation peer self-fingerprint
   ```
2. **Compare character by character.** Fingerprints are 64-char
   hex strings; a single transposition kills the handshake. Read
   them slowly over voice; don't paste over a channel that might
   mangle whitespace.
3. **Check the daily summary** on both sides:
   ```sh
   ffs audit query | head -2
   ```
   Recent handshake failures appear as `error:
   tls_handshake_failed` entries with the peer endpoint.
4. **Network reachability.** Make sure the peer's endpoint is
   actually reachable: `curl -kI https://<peer-endpoint>` should
   complete the TLS handshake (it'll fail cert verification —
   that's expected since they're not signed by a public CA, but
   the connection should *establish*). If even that times out,
   it's a firewall or NAT issue, not an FFS issue.
5. **Re-issue the bridge atom.** Capabilities can drift; revoke
   and re-issue if you've been federating for a while and the
   peer's view stopped updating. *(Phase 2: GUI-driven bridge
   management.)*

---

## Skill subprocess hangs

### Symptom

The daily summary stops updating. `ffs audit query` returns
older-than-expected entries. Scribe proposals stop appearing
even though you're still capturing in `~/.ffs/ingest/`. The
auditor flags repeated invocations with timing out.

### What's happening

The Python skill bundles (scribe, librarian, auditor) run as
subprocesses of the daemon. If one of them gets stuck in a
slow extraction (rare), it can wedge until the per-call timeout
fires. The ffs-skills-host supervisor catches this and restarts
the skill, but a wedged skill blocks all calls of that type for
the duration. TechSpec § Known Risks: **skill subprocess hangs**.

### Fix

1. **Wait 60s.** The per-call timeout fires at 30s by default
   (configurable per skill); after that the supervisor kills the
   skill and the next invocation gets a fresh one.
2. **Check skill restart counts** in the daily summary:
   ```sh
   ffs audit query | jq '.entries[].skill_restarts' | sort | uniq -c
   ```
   Repeated restarts of the same skill mean it's failing
   deterministically — there's a bug or a malformed input it
   chokes on.
3. **Manually restart the daemon** if a skill is wedged badly:
   ```sh
   systemctl --user restart ffs-daemon       # Linux
   launchctl kickstart -k gui/$UID/com.ffs.daemon   # macOS
   Restart-ScheduledTask -TaskName "FFS Daemon"     # Windows
   ```
4. **Capture the input that wedged it.** If you can reproduce,
   file an issue with the input markdown attached so the skill
   can be hardened against it.

---

## Reverse-map silently mis-authors atoms

### Symptom

You edit a contact's frontmatter; the projection appears to
update but the wrong field changed. Or: a notes section gets
re-classified as tags. Daily summary shows rapid same-field
supersessions on the same entity.

### What's happening

Each predicate's TOML spec declares **reverse-map rules** that
tell the fast-path classifier "this rendering output corresponds
to that atom field." A bad rule routes edits to the wrong atom
field. TechSpec § Known Risks: **reverse-map rule mistakes
silently mis-author atoms**.

### Fix

1. **Inspect the offending predicate's reverse-map:**
   ```sh
   ffs predicate inspect contact.person | jq '.reverse_map'
   ```
2. **Compare to the rendered output** (the Tera template in
   `~/.ffs/config/templates/<name>.tera`). The `output =
   "frontmatter.<field>"` lines must match the keys the
   template emits.
3. **The auditor surfaces this** as `rapid_supersession`
   warnings in the daily summary — those are usually a
   reverse-map bug, not user behavior.
4. **Fix the rule** in `~/.ffs/config/predicates/<name>.toml`
   and restart the daemon. Predicate specs are themselves
   bitemporal atoms in the substrate's auditing log; a bad spec
   is correctable without data loss.

---

## Capability evaluator denies what should be allowed

### Symptom

`ffs cat ffs://...` returns `capability denied: no read scope
covers this atom`. The Obsidian plugin shows blank where a
contact should be. A federation peer sees nothing of an entity
you've explicitly granted them.

### What's happening

The capability evaluator does action × scope × bitemporal-window
intersection. A misalignment in any of those three (the action
isn't `Read`, the scope doesn't cover the predicate, the
capability's `valid_to` is in the past) silently denies. TechSpec
§ Known Risks: **capability evaluator subtle bugs**.

### Fix

1. **Inspect the capability atoms** in play:
   ```sh
   ffs cat ffs://_root_/by-agent/<your-pubkey>/capabilities
   ```
2. **Check the times.** If `valid_to` is set and in the past,
   the capability has expired — issue a new one.
3. **Check the scope shape.** The scope must include the entity
   id (or be unscoped — `CapabilityScope::default()`) and an
   action set containing the action you're attempting.
4. **For federation denials**, check the bridge atom on the
   *peer*'s side; capability flows both ways and one side may
   have revoked.

---

## Working-set policy chose the wrong contacts to materialize

### Symptom

Contacts you accessed yesterday aren't in `~/.ffs/contacts/`.
The Obsidian plugin's folder browser shows fewer entries than
you expect. Re-opening a previously-edited contact takes a
noticeable second because the projection isn't on disk.

### What's happening

The working set is a subset of your full atom store, materialized
to disk for editor access. MVP's policy is
"most-recently-touched + user-pinned"; if that policy is wrong
for your usage, you'll feel it. TechSpec § Known Risks:
**working-set policy is wrong**.

### Fix

1. **Pin contacts you want always-materialized.** Right-click in
   Obsidian → **FFS: Pin this contact**. (Phase 2 polish: this
   is a CLI invocation in MVP — `ffs workingset pin <entity>`.)
2. **Manually request a render** before opening a file Obsidian
   has cached as missing:
   ```sh
   ffs cat ffs://_root_/contacts/by-name/S/Sara_Chen.md
   ```
   That triggers a render-on-demand and writes the file to disk.
3. **Working-set tuning is a Phase 2 deliverable**. Provide
   feedback on what got materialized vs. what you wanted — the
   policy is intentionally easy to revise.

---

## MCP agent can't see what it should

### Symptom

Claude Code or another MCP-aware agent reports
`capability_denied` from `ffs_query` even when you think you
granted it access. `tools/list` works but `tools/call` rejects.

### What's happening

The MCP server enforces capability checks at the boundary
(ADR-013). The agent's identity (its FFS author key, configured
via `FFS_AGENT_IDENTITY`) needs an explicit capability atom
granting it the scope it's trying to use. TechSpec § Known
Risks: **MCP capability-check correctness**.

### Fix

1. **Confirm the agent's identity** is what you think it is:
   ```sh
   echo $FFS_AGENT_IDENTITY  # in the agent's spawn env
   ```
2. **Issue a capability atom** scoped to that identity:
   ```sh
   ffs capability grant \
     --agent <agent-identity-uri> \
     --actions read,write \
     --scope all
   ```
   *(For MVP development this is a manual `ffs cat` of a
   pre-signed capability atom; Phase 2 ships the helper
   subcommand.)*
3. **Restart the MCP subprocess.** Capabilities are checked at
   call time, but some MCP clients cache `tools/list` output —
   a fresh subprocess picks up the new capability.

---

## When all else fails

The substrate is git-cloneable. Worst case, you can:

1. Stop the daemon: `systemctl --user stop ffs-daemon` (or
   `launchctl unload`, or `Stop-ScheduledTask`).
2. Back up your atom store: `cp -R ~/.ffs ~/.ffs.bak-$(date +%F)`.
3. Re-run the installer: `bash installer/install.sh` (idempotent;
   preserves user-edited predicates).
4. Restart the daemon and verify with `ffs health`.

If the issue persists, file an issue on the FFS repo with:

- Your OS + arch (`uname -a`).
- The exact command that failed and its output.
- The last 50 lines of `~/.ffs/log/ffs-daemon.err.log` (or
  `journalctl --user -u ffs-daemon -n 50`).

> Issues are tracked at the FFS repo; see the [project
> README](../../README.md) for the link.
