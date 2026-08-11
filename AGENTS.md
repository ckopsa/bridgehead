# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `bd prime` for full workflow context.

> **Architecture in one line:** Issues live in a local Dolt database
> (`.beads/dolt/`); cross-machine sync uses `bd dolt push/pull` (a
> git-compatible protocol), stored under `refs/dolt/data` on your git
> remote — separate from `refs/heads/*` where your code lives.
> `.beads/issues.jsonl` is a passive export, not the wire protocol.
>
> See [SYNC_CONCEPTS.md](https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md)
> for the one-screen overview and anti-patterns (don't treat JSONL as the
> source of truth; don't `bd import` during normal operation; don't
> reach for third-party Dolt hosting before trying the default).

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work atomically
bd close <id>         # Complete work
bd dolt push          # Push beads data to remote
```

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

## Build & Test

**Dev profile only — never `--release`.** Deps are already built at `opt-level = 3`
in dev, so release buys nothing and costs half an hour. `cargo test` builds the
test harness, **not** the binary: run `cargo build` before any live check or you
will verify a stale binary.

```bash
cargo build          # the binary — required before running the game
cargo test           # the Rust test suite
python3 -m pytest tools/   # the tooling tests (tools/test_*.py)
```

Verification is a named tier (`tools/verify.sh`, composing the raw commands
documented in tools/BUILDER_BRIEF.md Appendix A):

```bash
tools/verify.sh smoke      # compiles, tests pass, one short headless match reaches game over
tools/verify.sh standard   # + both maps + the four bridge protocol verifiers
tools/verify.sh full       # + longer caps, the arena runner path, screenshots
tools/verify.sh identity   # two seeded fixed-dt runs, fingerprints diffed byte-for-byte
```

Say which tier you ran. `identity` is the cheap proof for any
"no behaviour change" claim — reach for it on every refactor.

A headless match by hand:

```bash
cargo build && WC3_HEADLESS=1 WC3_AI_BOTH=1 WC3_SPEED=16 WC3_MAX_GAME_SECS=900 \
  WC3_MAP=crossings ./target/debug/wc3clone
```

## Architecture Overview

A Warcraft-3-style 3D RTS in Rust + **Bevy 0.16** (pinned), built so that a human
at a mouse and an LLM commander on a file bridge play the *same game* — one
vocabulary, one rule of knowability, one set of refusals. `src/shared.rs` is the
integrator contract every module speaks through; each other file in `src/` owns
one concern (`intent.rs` the compiler, `units.rs` movement, `combat.rs` damage,
`economy.rs` money and construction, `ui.rs` the human seat, `bridge.rs` the LLM
seat, `ai.rs` the scripted one, `data.rs` the RON stat tables). Every player
mutation — from either seat — becomes a `shared::Intent` and is applied by
`intent::apply_intents`; every gameplay system runs inside a phase of
`shared::SIM_ORDER`, which makes a match reproducible from a seed.

- **DESIGN.md** — the module contract, the frame order, the data-file rules, and
  the Bevy 0.16 idioms that keep you off stale APIs. Read this first.
- **docs/INTENT.md** — the intent vocabulary, the fairness invariant, triggers,
  plans, territory, co-command, and the rules for error messages that teach.
- **docs/FOG.md** — one rule of knowability, computed once, rendered three times.
- **docs/TEMPO.md** — why command latency exists and what it taxes.
- **docs/ARENA.md** — the dogfooding ledger and how a round is run safely.
- **docs/ITERATION.md** — the loop that produces all of this: roles, decision rules,
  and the artifacts of iterating on the game.
- **tools/COMMANDER_BRIEF.md** — the wire protocol as its users read it.
- **THESIS.md** — why the project exists.

## Conventions & Patterns

**Implementation agents: read `tools/BUILDER_BRIEF.md` first.** It is the standing
half of your instructions — worktree and process isolation, the `cp -a
--reflink=always` target copy, the build rules, every code invariant, the merge
traps that have actually broken this build, and the verification litany. Your
bead only needs to tell you what is different this time.

The four rules that must survive even a skim:

1. **Your worktree is yours; the main checkout is not, and the process table is
   shared.** Never build, check out, or launch the game in the main repo — live
   matches run there. Kill only by exact owned PID (never `pkill -f`), and
   bracket pgrep patterns (`'[t]arget/debug/wc3clone'`) so a waiter cannot match
   itself and livelock.
2. **One choke point per rule — extend it, never bypass it.** Player mutation is
   `Intent` → `apply_intents` and nothing else; `effective_stats` is the one stat
   law; `resolve_places` the one name resolver; `apply_atom` the one effect
   applier; `Economies::pay` the one spender. A second path is a divergence with
   a schedule. And the compiler validates only to produce good *messages* — the
   system that spends the resource is what makes a rule true.
3. **Determinism is structural.** Every gameplay system goes in a named `SimSet`;
   collections on gameplay paths are `BTreeMap`s (`HashMap` reseeds per process);
   any root entity spawned *or* teleported during `Update` must seed its own
   `GlobalTransform::from(transform)` in the same statement.
4. **Data and wire are append-only.** Stat tables are rows in `assets/data/*.ron`
   (tables move, scalars stay) because interleaved `match` arms merge silently;
   snapshot and command keys are additive only, with `skip_serializing_if`, and
   the historical key sets are pinned by tests.

Do not commit `.beads/`, do not push, and do not mutate the tracker — the
orchestrator owns all three. Every commit carries
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

> **This file mirrors CLAUDE.md.** Substantive edits to the sections above must be
> applied to both.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:6cd5cc61 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep persistent project memory in Beads via `bd remember`; do not create ad hoc memory files.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->
