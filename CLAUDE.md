# CLAUDE.md — operational discipline for the FFS repo

This file is for Claude Code (and any agentic operator). It captures the operational norms that keep work in this repo reproducible and low-friction. For the project's *substance* — what FFS is, how it's structured, why decisions were made — read [`ARCHITECTURE.md`](ARCHITECTURE.md), [`_prd.md`](.compozy/tasks/ffs-mvp/_prd.md), [`_techspec.md`](.compozy/tasks/ffs-mvp/_techspec.md), and the 21 [ADRs](.compozy/tasks/ffs-mvp/adrs/).

---

## Test, check, clippy

Use **`cargo nextest`** for all test runs, never bare `cargo test`. Nextest prints a single stable summary line so no grep/awk/sed is required to read the result:

```
Summary [   2.341s] 84 tests run: 84 passed, 0 skipped
```

### Canonical commands

| What | Command |
|---|---|
| Full test suite (default — use this) | `cargo nextest run --workspace --all-features` |
| One crate | `cargo nextest run -p ffs-core` |
| One test file | `cargo nextest run -p ffs-core --test store_integration` |
| One test by name | `cargo nextest run -p ffs-core --test capability_evaluator superseded_capability_no_longer_grants` |
| Doc tests (rare in this repo) | `cargo test --doc --workspace` |
| Check | `cargo check --workspace --all-targets --all-features` |
| Clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| Format check | `cargo fmt --all -- --check` |
| Apply format | `cargo fmt --all` |

### Rules

- **Run bare, never pipe.** No `... | grep`, `... 2>&1 | tail`, `... | awk`. The summary is already at the end of the output. Piping breaks the allowlist match and triggers a permission prompt each time.
- **Default scope = `--workspace --all-features`.** Scope down only when iterating on one file's tests, and switch back to the workspace command before declaring a task done.
- **Doc tests run separately.** Nextest skips them by default. Run `cargo test --doc --workspace` once per phase, not per task.
- **Release-mode perf tests.** A few tests assert production performance budgets (e.g., `ten_thousand_evaluations_against_one_thousand_capabilities_under_one_second` in `capability_evaluator`). The assertions use `cfg!(debug_assertions)` to relax in debug; the production-relevant numbers come from `cargo nextest run --release ...`.

### What this replaces

This rule replaces ad-hoc `cargo test ... | grep "test result"` / `... | awk '{sum += $4}'` patterns that triggered per-invocation permission prompts. The allowlist entry `Bash(cargo nextest run *)` covers every scope; no per-pipeline approval needed.

---

## Python tests

Skill bundles (`skills/<name>/`) are claw-shape distributable artifacts (ADR-009). They must NOT carry a Python interpreter or test-tool dependencies — those live at the repo root.

### Canonical layout

- `pyproject.toml` at the repo root declares the Python tooling for every skill bundle (scribe, librarian, auditor, future skills).
- A single virtualenv at the repo root: `.venv/`. Gitignored. The same env runs every skill's tests.
- Skill bundles stay clean: their directories contain only `SKILL.md`, the entry script, `prompts/`, `tests/`, `definition.atom.json`. No `.venv/`, no `__pycache__/`, no `requirements.txt`.

### Setup (one-time)

```sh
python3 -m venv .venv
.venv/bin/pip install -e '.[dev]'
```

### Canonical commands

| What | Command |
|---|---|
| All skill tests | `.venv/bin/python -m pytest skills/` |
| One skill | `.venv/bin/python -m pytest skills/scribe/tests/` |
| One test | `.venv/bin/python -m pytest skills/scribe/tests/test_extraction.py::test_frontmatter_name_yields_contact_person` |

### Rules

- **Never `python3 -m venv` inside a skill bundle.** The venv goes at the repo root; nested copies pollute the bundle and confuse the skills host (which loads the bundle as data, not as a Python package).
- **Never `pip install --break-system-packages` or `--user`.** Use the repo-root venv; if pytest isn't found, set the venv up first.
- **Skill-bundle Python code has zero pip dependencies.** Tests may use pytest; runtime extraction logic uses only stdlib + the `ffs_skill` helper.
- Bare invocations only (no piping); pytest's summary line is already at the end.

---

## Obsidian-plugin tests

The Obsidian plugin (`obsidian-plugin/`) is a TypeScript project with
its own `package.json` per the Obsidian plugin convention. Its tests
run via vitest from inside that directory.

### Setup (one-time)

```sh
cd obsidian-plugin
npm install
```

`node_modules/` is gitignored at every level; never commit it.

### Canonical commands

| What | Command |
|---|---|
| All plugin tests | `npm test` (from `obsidian-plugin/`) |
| Bundle the plugin to `main.js` | `npm run build` |
| Watch-build during development | `node esbuild.config.mjs --watch` |

### Rules

- **Never run `npm install -g`** for anything. Per-project deps only;
  no shared global toolchain.
- **Plugin source has no FFS-specific Node runtime deps.** The runtime
  side talks to the daemon over UDS (`node:net`) or via the `ffs` CLI
  subprocess (`node:child_process`); both are Node built-ins.
- **vitest mocks Obsidian at test time** via the alias in
  `vitest.config.mts` → `tests/obsidian-shim.ts`. Production gets the
  real `obsidian` runtime; tests get a minimal stub of just the API
  surface we touch. Never `import obsidian` in `client.ts`,
  `events.ts`, `backoff.ts`, or the testable half of `settings.ts` —
  only the `main.ts` entrypoint and the Obsidian-runtime wrapper in
  `settings.ts` are allowed to depend on it.

---

## Shell tool discipline

Default to dedicated tools, not shell text-munging:

- **Reading files** → `Read` tool, never `cat`/`head`/`tail`/`sed -n`.
- **Editing files** → `Edit` tool, never `sed -i`/`awk` rewrites.
- **Searching code** → `Grep` tool (or `rg` bare), never piped `grep | grep | awk`.
- **Test / check / clippy output** → run bare per the test-runner rule above; never pipe to `grep`/`awk`/`tail`/`sed` to filter the result.
- **One-off transforms** (`tr`, `cut`, `sort | uniq`) → fine as bare commands; never chain 3+ pipes just to extract one field.

If you find yourself writing a 2+ pipe shell incantation, stop and ask: is there a dedicated tool that does this? Almost always yes. Piped one-offs cost a permission prompt every time the exact pipeline shape changes.

**This rule applies to subagent prompts too.** When briefing a subagent (Agent tool), do not embed `grep | grep | head` or similar piped pipelines as instructions. Tell the subagent the *question* and let it use Read/Grep tools. A subagent that runs `grep ... | head` triggers the same permission prompts as if you ran it.

---

## Working directory

The session cwd is the repo root at `/Users/midnightblue/Git/ffs`. It's sticky across tool calls.

- **Do not** prepend `cd /path && ...` to commands — every distinct `cd /path` is a new allowlist match. The `Bash` tool already runs in the session cwd; `cd` is redundant.
- **Do not** prepend `git -C /path ...` — git uses cwd by default.
- **Use relative paths** for files inside the repo (`crates/ffs-core/src/atom.rs`, not absolute paths). Subagents inherit cwd too — brief them with relative paths.
- If you need to verify cwd, run `pwd` bare — not `cd /path && pwd`.

This repo does not currently use git worktrees. If that changes, this section gets a worktree-specific rule update.

---

## Repo conventions

These are the norms that have shaped commits to date. Make them explicit so future work doesn't drift.

### Commits

- **One commit per task.** Each `cy-execute-task` invocation produces exactly one commit. Subject line ends with `Implements task_NN of the ffs-mvp plan.` (or the appropriate plan name).
- **Conventional-ish subject prefixes**: `feat(<area>):`, `chore(<area>):`, `docs:`, `fix(<area>):`. Areas in active use: `core`, `daemon`, `cli`, `mcp`, `federation`, `fastpath`, `skills`, `workspace`.
- **Co-author trailer**: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>` on any commit Claude authored.
- **Never `git add -A` or `git add .`** — list paths explicitly. The `.agents/` and `.claude/` directories are intentionally untracked; so is `Cargo.lock` (for now).
- **Never push without explicit user confirmation.** Auto-commit means commit, not push.

### Task tracking

For tasks under `.compozy/tasks/<plan>/`, the completion sequence is:

1. Flip `status: pending` → `status: completed` in the task's frontmatter.
2. Check off all subtasks (`- [ ]` → `- [x]`).
3. Update the matching row in `_tasks.md` (`| NN | ... | pending |` → `| NN | ... | completed |`).
4. Run `compozy tasks validate --name <plan>` — must report `all tasks valid`.
5. Stage + commit per the rules above.

### ADRs

ADRs live at `.compozy/tasks/ffs-mvp/adrs/adr-NNN.md` with zero-padded 3-digit numbers. The current split:

- **001–014**: PRD-level decisions (product shape).
- **015–021**: TechSpec-level decisions (technical implementation).
- **022+**: new decisions — continue the sequential numbering regardless of which layer they originate from.

A new ADR is required for any change to a public type, the atom envelope, the capability evaluator, the federation transport, or any other surface listed under [`ARCHITECTURE.md` § Stability commitments](ARCHITECTURE.md#stability-commitments).

### SQLite blessing carry-forward

When a new source file integrates with SQLite (currently only `crates/ffs-core/src/store/sqlite.rs`), carry the SQLite blessing as the top-of-file comment block:

```rust
// May you do good and not evil.
// May you find forgiveness for yourself and forgive others.
// May you share freely, never taking more than you give.
//   — the SQLite blessing, carried with gratitude
```

This is a per-file convention, not a rule about the project as a whole. The public-facing acknowledgment lives in [`README.md`](README.md).

### Pre-1.0 stability

The project is pre-1.0. Breaking changes are possible everywhere with notice. Post-1.0, the stable surfaces listed in [`ARCHITECTURE.md` § Stability commitments](ARCHITECTURE.md#stability-commitments) lock down (`ffs://` URL scheme, atom envelope shape, JSON-RPC method set, MCP tool signatures, predicate-spec format).

Until 1.0, prefer breaking change + clear migration over backward-compatibility hacks. After 1.0, prefer additive change over breaking change.

---

## Verification before completion claims

Every "task complete", "tests pass", "ready to commit" claim requires fresh verification evidence. The full template lives in the `cy-final-verify` skill; the short form is:

```
VERIFICATION REPORT
-------------------
Claim:   [what is being claimed]
Command: [exact command run — e.g., `cargo nextest run --workspace --all-features`]
Executed: just now, after all changes
Exit code: 0
Output summary: [the nextest Summary line, plus fmt + clippy results]
Verdict: PASS
```

For a "task complete" claim, the verification must include:

- `cargo nextest run --workspace --all-features` — all green.
- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — 0 warnings.
- Line-by-line check of the task spec's requirements and tests (not just "tests pass").

If verification fails, the task status stays unchanged until the failure is resolved. Never use "should pass" / "probably works" / "tests will pass once I push" language.

---

## Subagent briefings

When dispatching subagents (the `Agent` tool, `Explore`, `Plan`, etc.):

- **Brief them on the task or question, not the commands.** They inherit this `CLAUDE.md` and will use the right tools.
- **Use relative paths** in the prompt. They share the session cwd.
- **Don't embed piped pipelines** in agent prompts. A subagent that runs `grep ... | head ... | awk ...` triggers the same permission prompts you'd hit running it yourself.
- **Self-contained briefs**: subagents don't see prior turns. Give them what they need.

---

## When this file conflicts with anything else

User instructions (this turn's message) override CLAUDE.md. CLAUDE.md overrides skill defaults. Skill defaults override agent defaults.

If CLAUDE.md contradicts the substance of `ARCHITECTURE.md`, the PRD, or an ADR, that's a bug — fix CLAUDE.md or fix the doc, but don't let the contradiction persist.
