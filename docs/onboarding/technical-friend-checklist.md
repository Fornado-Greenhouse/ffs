# Technical-friend onboarding checklist

You are setting up FFS for someone — yourself, a friend, a family
member — who will use it through Obsidian without touching the
terminal again. This document is the punch list. Plan on **45–60
minutes** for the first install; subsequent peer installs go
faster.

> Audience: a technical friend doing the install. The end user
> reads [`first-use-guide.md`](first-use-guide.md), not this file.

For the architectural context behind every step, see
[`ARCHITECTURE.md`](../../ARCHITECTURE.md), the PRD
[`_prd.md`](../../.compozy/tasks/ffs-mvp/_prd.md), and the 22
[ADRs](../../.compozy/tasks/ffs-mvp/adrs/). For when things go
wrong, see [`troubleshooting.md`](troubleshooting.md).

## Before you start

You will need:

- Admin rights on the user's machine (to install binaries and the
  per-user service unit).
- ~200 MB of disk space.
- An Obsidian install — if the user doesn't have one, point them
  at [obsidian.md](https://obsidian.md) and have them open a vault
  at least once before you start. The plugin registration step
  needs the `.obsidian/` directory to exist.
- The FFS release archive for the user's platform (`ffs-<version>-
  <platform>.tar.gz` on macOS/Linux, `.zip` on Windows). For
  developer installs, a workspace `cargo build --release` produces
  the same three binaries in `target/release/`.

## Step 1 — Install (5 min)

### macOS or Linux

```sh
# Unpack the release somewhere (anywhere — installer copies what
# it needs into $HOME/.local).
tar xzf ffs-<version>-<platform>.tar.gz
cd ffs-<version>

# Inspect what install.sh will do without changing anything.
bash installer/install.sh --dry-run

# Apply.
bash installer/install.sh
```

No `--vault` argument needed: the installer defaults to
**substrate-is-vault** (see [ADR-022](../../.compozy/tasks/ffs-mvp/adrs/adr-022.md)).
`~/.ffs/` is both the substrate root *and* the Obsidian vault,
and the installer seeds `~/.ffs/.obsidian/plugins/ffs/`
automatically. Pass `--prefix /custom/prefix` to install
binaries somewhere other than `$HOME/.local/bin`. Pass
`--vault /external/path` only if the user has a strong
existing-vault reason (the materializer always writes to
`~/.ffs/`, so an external vault appears empty and the installer
warns).

### Windows

```powershell
# Unpack ffs-<version>-windows.zip and open a PowerShell window
# in the unpacked folder.
.\installer\install.ps1
```

Same substrate-is-vault default: the plugin lands at
`%USERPROFILE%\.ffs\.obsidian\plugins\ffs\`. The installer adds
`%LOCALAPPDATA%\FFS\bin` to the user PATH (opens a fresh shell
to pick it up). It also registers a Scheduled Task named
**FFS Daemon** that launches `ffs-daemon.exe` at logon.

### What landed where

| Path | Contents |
|---|---|
| `~/.local/bin/` (Linux/mac) or `%LOCALAPPDATA%\FFS\bin\` (Windows) | `ffs`, `ffs-daemon`, `ffs-mcp` binaries |
| `~/.ffs/config/predicates/` | Starter predicate specs (contact.person, person.generic, note) |
| `~/.ffs/config/templates/` | Starter Tera templates |
| `~/.ffs/skills/` | Python skill bundles (scribe, librarian, auditor) |
| `~/.ffs/run/` | Daemon UDS socket (mode 700) |
| `~/.ffs/log/` | Daemon stderr captures |
| `~/.ffs/.obsidian/plugins/ffs/` | Obsidian plugin (substrate-is-vault) |
| `~/.config/systemd/user/ffs-daemon.service` (Linux) | Per-user systemd unit |
| `~/Library/LaunchAgents/com.ffs.daemon.plist` (macOS) | launchd agent |
| Scheduled Task **FFS Daemon** (Windows) | At-logon launcher |

### Migrating from a pre-task_30 install

If you installed FFS before substrate-is-vault landed and your
plugin lives somewhere other than `~/.ffs/.obsidian/plugins/ffs/`
(e.g., inside an existing user vault you passed to `--vault`),
do this once:

```sh
# Stop the daemon so the watcher doesn't race the move.
launchctl unload ~/Library/LaunchAgents/com.ffs.daemon.plist  # macOS
# OR
systemctl --user stop ffs-daemon                              # Linux

# Re-run the installer with no --vault — picks up the new default.
bash installer/install.sh

# Restart.
launchctl load ~/Library/LaunchAgents/com.ffs.daemon.plist    # macOS
# OR
systemctl --user start ffs-daemon                             # Linux
```

Then in Obsidian: **Open folder as vault** → `~/.ffs/`. The old
external vault can be retired (or left as an empty Obsidian
workspace the user ignores).

## Step 2 — Identity setup (5–10 min)

The substrate is encrypted at rest. The **DEK** (database
encryption key) protects the SQLite atom store; the **owner
signing key** stamps every atom they author. Both should live
durably across daemon restarts so the substrate's identity is
stable.

### Path A: env-var-pinned (current MVP default)

Generate 32 random bytes for each key, hex-encode, and pin them
in the service unit's environment. Stash a fallback copy under
`~/.ffs/secrets/` with mode 0600.

```sh
KEY_HEX=$(head -c 32 /dev/urandom | xxd -p -c 64)
DEK_HEX=$(head -c 32 /dev/urandom | xxd -p -c 64)

mkdir -p ~/.ffs/secrets && chmod 700 ~/.ffs/secrets
printf '%s\n' "$KEY_HEX" > ~/.ffs/secrets/owner_key_hex
printf '%s\n' "$DEK_HEX" > ~/.ffs/secrets/sqlcipher_key_hex
chmod 600 ~/.ffs/secrets/*
```

Then edit `~/Library/LaunchAgents/com.ffs.daemon.plist` (macOS)
or `~/.config/systemd/user/ffs-daemon.service` (Linux) to
inject the two values via `FFS_OWNER_KEY_HEX` and
`FFS_SQLCIPHER_KEY_HEX` in the `EnvironmentVariables` block.
Also set `FFS_KEYRING_DISABLE=1` until task_33 lands, so the
daemon doesn't attempt the (currently broken under launchd)
keychain path.

After reload, `ffs identity show` will print:
```
owner pubkey: z…
source:       env_var
```

### Path B: OS-keychain-pinned

> **Status (task_27 + task_33):** the keychain helpers in
> `ffs-core` and the `ffs identity show` subcommand are
> implemented. They work correctly from interactive CLI
> contexts. They do NOT work yet from a launchd-spawned daemon
> on macOS — Keychain Services partitions entries by binary
> code-signing identity, and the FFS binaries aren't signed
> with a matching `keychain-access-groups` entitlement yet.
> Use Path A above until task_33 lands the codesigning
> infrastructure. The troubleshooting guide has the long-form
> explanation.

When task_33 ships, this section will become: "on first boot
the daemon generates and stores both keys in the OS keychain;
verify with `ffs identity show`."

## Step 3 — First run (5 min)

The installer wires the daemon to start at logon. To verify it
runs *right now* without rebooting:

```sh
# Linux / macOS — start the user service immediately:
systemctl --user start ffs-daemon.service   # Linux
launchctl start com.ffs.daemon              # macOS

# Check it's running:
~/.local/bin/ffs health
# Expected output (atom_count is 0 for a fresh install):
#   proposals: 0
#   questions: 0
#   drift_flags: 0
#   atom_count: 0
```

On Windows:

```powershell
Start-ScheduledTask -TaskName "FFS Daemon"
& "$env:LOCALAPPDATA\FFS\bin\ffs.exe" health
```

If `ffs health` errors out, check
[`troubleshooting.md`](troubleshooting.md) under **Daemon
doesn't start**.

## Step 4 — Predicate inspection (2 min)

The substrate ships three starter predicates. Make sure they
loaded:

```sh
ffs predicate inspect contact.person | head -20
ffs predicate inspect note | head -10
ffs predicate inspect person.generic | head -10
```

Each should print a JSON object with a `claim_schema`, a
`rendering` block referencing a `.tera` template, and a
`reverse_map` array. If any predicate fails to load, the daemon
will have logged a startup error — check `~/.ffs/log/` (macOS) or
`journalctl --user -u ffs-daemon` (Linux).

## Step 5 — Open the vault in Obsidian (3 min)

The installer already seeded `~/.ffs/.obsidian/plugins/ffs/`.
Walk the user through opening the substrate as a vault:

1. **Open Obsidian.**
2. Top-left vault switcher → **Open another vault** → **Open
   folder as vault** → navigate to `~/.ffs/` → Open.
3. If a "trust author" dialog appears for community plugins,
   click **Trust** (the plugin is local-only; it just talks to
   the daemon over your `~/.ffs/run/ffs.sock`).
4. Settings → **Community plugins** → toggle the **FFS** plugin
   on. (If the plugin isn't listed, Obsidian needs a vault
   reload — quit and reopen.)
5. Gear icon next to the FFS row → confirm the **Daemon socket**
   field points at `~/.ffs/run/ffs.sock` on Linux/macOS or
   `\\.\pipe\ffs-daemon` on Windows. The default should already
   match.

After enabling the plugin, you should see two new things:

- A **Daily summary** panel in the right sidebar.
- A new command **FFS: Search FFS entities by name…**, bound to
  whatever hotkey the user prefers.

See [`screenshots/`](screenshots/) for what each surface looks
like. The file explorer shows the substrate's path-library
layout directly: `ingest/`, `contacts/by-name/...`,
`notes/by-name/...`, `audit/`, etc.

## Step 6 — Federation handshake (15 min)

Federation is optional for MVP — if the user is going solo for
now, skip this section and come back when they want to share
contacts with someone else who runs FFS.

For each peer they want to federate with:

1. **Exchange fingerprints.** Both peers run `ffs federation
   peer self-fingerprint` and read each other their result over
   a trusted channel (in person, on the phone, signal — any
   channel they trust). The fingerprint is a 64-character hex
   string.
2. **Add each other as peers.** Each runs:
   ```sh
   ffs federation peer add \
     https://<their-peer>.example:14400 \
     <fingerprint-they-read-you>
   ```
3. **Author the bridge atom.** This atom names the federation
   relationship — the peer, the capability scope they grant, the
   tiers in play. The Obsidian plugin's federation panel is the
   Phase 2 polish; for MVP, this is a `ffs federation bridge
   create` invocation through the CLI. *(Phase 2: GUI walkthrough.)*
4. **Watch the daily summary.** The first federation pull
   appears in the next daily-summary atom (run `ffs audit
   query | head -1` to inspect). If the handshake failed, the
   daily summary surfaces the error.

The federation handshake is the most failure-prone part of
onboarding. If anything looks off, check
[`troubleshooting.md`](troubleshooting.md) under **Federation
handshake fails**.

## Step 7 — Hand-off (3 min)

Walk the end user through [`first-use-guide.md`](first-use-guide.md)
together. They should:

- Open Obsidian, see the daily-summary panel populated (it will
  be empty on day one — that's correct).
- Capture one contact in `~/.ffs/ingest/`, watch it appear as a
  proposal in the daily summary, accept it, and find it under
  `contacts/by-name/<letter>/`.
- Edit a frontmatter field on a projection and watch the
  supersession atom land (`ffs cat ffs://_root_/by-entity/<id>` —
  but they shouldn't need to run that; the visible behavior is
  that the file re-renders).

Leave them with:

- [`first-use-guide.md`](first-use-guide.md) bookmarked.
- [`troubleshooting.md`](troubleshooting.md) bookmarked.
- Your contact info for the inevitable "something looks weird"
  question.

## You're done

The substrate is theirs now. They own it, they hold the keys,
and the daemon does its work without prompting them. If they hit
something this checklist didn't cover, check
[`troubleshooting.md`](troubleshooting.md) first; if the answer
isn't there, file an issue on the FFS repo so the next install
goes smoother.
