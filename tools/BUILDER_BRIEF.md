# Builder's Brief (implementation agent)

You have been dropped into a git worktree with one bead. This document is the
half of your instructions that does **not** change from bead to bead: the
isolation contract, the build rules, the code invariants this codebase enforces,
the merge traps that have actually bitten, and how to prove your change works.

Everything here was learned the expensive way. Thirty-odd agents across two
campaigns needed every item below at least once, and each one learned it either
by being told or by failing. The rules carry their reasons because a rule with a
reason survives contact with a smart agent and a bare rule gets "improved" away.

Your spawn prompt should therefore only need to say three things: **read this
brief, here is your bead, here is what is different this time.**

---

## 0. The shape of the job

One bead → one branch → one merge. The division of labour:

| You own | The orchestrator owns |
| --- | --- |
| your worktree, your branch, your commits | `master`, the merge, the tracker (`bd`), the remote |
| the code your bead names | which beads exist, in what order, and when they land |
| proving your change works | deciding it landed |

**DONE** means: your work is committed on `bead/<id>`, `cargo test` is green,
you have run the verification tier your change deserves (§8), and your final
report says what changed, what you verified, and what you left undone. Nothing
is pushed. Nothing in `.beads/` moved. You do not merge yourself unless asked.

---

## 1. The worktree is not the repository

Your cwd is a git worktree. It is yours. The main checkout — the one everything
else in this project points at — is **not**, and the rule is absolute:

> **Never read-modify-write, build in, check out in, or launch the game from the
> main repository directory.**

Two reasons, both of which have already cost a session:

1. **Live matches run there.** The orchestrator runs arena rounds and bridge
   matches out of the main checkout. `cargo build` there swaps the binary under
   a running match; a checkout there changes the `.ron` files and the seat
   directories that match is reading. `tools/arena_run.py` refuses to start when
   an engine process is alive precisely because an agent once destroyed a real
   match by running a verification against a seat somebody was playing.
2. **Siblings.** You are one of several agents working at once. The main
   checkout is the only shared surface, so a write there is a write into
   everyone's blast radius.

Reading a file from main is fine (`git log`, `cat`). Writing, building, or
running is not.

### The process table is NOT isolated

This is the part that surprises people. A worktree isolates *files*. It does not
isolate PIDs, and every agent's `bridgehead` binary has a command line that looks
like every other agent's.

An agent once ran `pkill -f bridgehead` to tidy up after itself and killed a
sibling's live arena round. The rules that followed:

- **Kill by exact owned PID only.** Capture it when you launch
  (`cmd & PID=$!`), kill `$PID`. Never `pkill`, never `killall`, never
  `kill $(pgrep ...)` over a pattern you did not narrow to your own process.
- **Bracket your pgrep patterns so they cannot match themselves.**
  `pgrep -f '[t]arget/debug/bridgehead'` — the bracket is a character class that
  matches `t` but is not literally the string `target`, so your own shell's argv
  (which contains the pattern) never matches. Without it, `until pgrep -f
  "cargo build"; do sleep 5; done` matches *itself*, waits forever on a
  condition it is causing, and yields the turn. Two orphaned waiters from that
  exact bug had to be found and killed by hand.
- **Reap what you background before you finish.** Anything you launched with `&`
  or `run_in_background` is yours. Before you write your final report, kill your
  own background jobs and confirm `pgrep -f '[b]ridgehead'` shows nothing of
  yours. An agent that ends its turn with a live game process leaves a landmine
  for the next `arena_run.py`, which will then refuse to start.

---

## 2. Setup, in order

```bash
cp -a --reflink=always /path/to/main/target ./target
git checkout -b bead/<id>
```

**`-a`, not `-r`. This flag is worth thirty minutes.** Cargo's fingerprints are
mtimes. `cp -r` stamps every copied file with *now*, so every source and every
dependency looks newer than the artifact built from it, and cargo rebuilds the
entire graph. For Bevy that is a 30+ minute cold build, and five agents doing it
simultaneously on eight cores is a build convoy that has already OOM-killed
itself once. `--reflink=always` makes the copy free on a CoW filesystem; `-a`
preserves the mtimes that make the copy *useful*. The reflink is the cheap part;
`-a` is the correct part.

Verify it worked: your first `cargo build` should compile your crate only, in
seconds to a minute. If it starts compiling `bevy_render`, the copy was wrong —
stop and redo it rather than waiting out a rebuild you do not need.

**Branch name is `bead/<id>` with the bare id** — `bead/xxr`, not
`bead/wc3clone-xxr`. The first-parent log is the project's changelog and it
reads that shape.

---

## 3. What you must not touch

| Do not | Why |
| --- | --- |
| `git push`, or anything touching a remote | The orchestrator owns publication. There is nothing you need from the remote that `master` in this repo does not already have. |
| `.beads/` — any file, ever | The tracker is a Dolt DB with its own sync protocol (`refs/dolt/data`). A JSONL diff from an agent branch is not an update, it is a corruption the orchestrator has to unpick by hand. |
| `bd create` / `bd update` / `bd close` | Same reason. **Report** follow-up work in your final message and let the orchestrator file it. You may `bd show` / `bd ready` to read. |
| `Cargo.toml` | Bevy 0.16 is pinned deliberately (DESIGN.md line 3). A version bump is its own bead with its own migration. |
| `master`, and any branch that is not yours | Never commit to it, never force anything, never rebase it. |
| `--release` | See §4. |

---

## 4. Building

**Dev profile only.** `cargo build`, `cargo test`, `cargo check`. Never
`cargo build --release`.

The reason is not dogma, it is that the dev profile here is *already* the fast
one: `Cargo.toml` sets `opt-level = 1` for this crate and `opt-level = 3` for
every dependency, so dependencies are fully optimized and only your own code is
compiled quickly. There is no custom `[profile.release]` at all. A release build
recompiles all of Bevy from scratch into a second artifact directory, costs
upward of half an hour, and buys you nothing the sim can tell you — the
simulation is deterministic and headless runs are already time-compressed with
`BH_SPEED`. One agent spent forty minutes on a release build to "check
performance" and reported the same numbers.

There is a second, sharper reason. `tools/determinism_check.sh` picks its binary
by auto-detection: **`target/release/bridgehead` first**, `target/debug/bridgehead`
only if that is absent. Leave a release binary lying around and every later
determinism check silently verifies *it* — including after you have rebuilt
debug twenty times. A stale artifact you forgot about is the worst kind.

**Never run `cargo fmt`.** The repo is not rustfmt-clean and has no intention
of becoming so mid-bead: a bare `cargo fmt` rewrites thousands of lines in
files you never touched, drowning your actual diff and setting up merge
conflicts for every sibling branch. One agent's fmt run produced 5,946
insertions of pure churn across 10 untouched files and cost a three-way
un-merge to undo. Match the surrounding style by hand, as the code-style
section says.

**`cargo build` before any live check.** This one has a body count.

> `cargo test` and `cargo test --no-run` build the **test harness**. They do not
> rebuild `target/debug/bridgehead`.

An agent ran `cargo test`, launched a headless match, watched the fix "work",
and reported it — against a binary two commits old that did not contain the fix.
Nothing errored. Stale binaries never error; that is what makes them expensive.

So: if you are about to run the game, build it first, in the *same* shell call
so there is no window between them:

```bash
cargo build && BH_HEADLESS=1 ... ./target/debug/bridgehead
```

**Expect to wait, and wait correctly.** Bevy links slowly. Background long
builds, and if you write a wait loop, bracket the pattern (§1) and reap the
loop. Do not chain waits without checking the result in between — the first
wake is the news and the second wait throws it away unread.

---

## 5. Siblings, master, and the re-merge

You are not alone and `master` moves under you.

**To learn what landed, read the log — do not ask.**

```bash
git log --first-parent --oneline $(git merge-base HEAD master)..master
```

Every merge subject is `merge bead/<id>: <what it did>`, one line, written to be
read in exactly this listing. **The first-parent log IS the changelog.** If your
bead's area was touched by a sibling, that line is how you find out, and reading
the merge commit's body tells you the rest.

**Expect to be asked to re-merge.** The normal life of a bead branch is: you
finish, a sibling lands first, and the orchestrator asks you to merge `master`
into your branch and re-verify. Keep the branch ready for that:

- **Merge, do not rebase.** Your branch may already have been read by the
  orchestrator; rewriting its history makes the re-merge harder, not easier.
- **Commit in topical chunks with real messages.** When a merge conflicts, the
  orchestrator resolves it by reading your commits. One giant "implement the
  bead" commit makes that impossible; twenty "wip" commits make it worthless.
  The house style is a subject line of `<area>: <what changed>` and a body that
  explains *why*, in prose. Read `git log` for a dozen examples — the bodies in
  this repo are unusually good and they are the reason the design docs could be
  written at all.
- **Re-verify after the merge, do not assume.** A clean textual merge of two
  correct branches is routinely a broken program (§7).

---

## 6. The code contract

`DESIGN.md` is the module contract and you should read it. This section is the
subset that agents needed *regardless of which bead they had*.

### 6.1 `shared.rs` is the integrator contract; one file per module

`src/shared.rs` holds every cross-module type: `Team`, the kind enums and stat
accessors, `Health`, `Order`, `MoveTo`, `NavGrid`, `Economies`, the spawn
events, `GameOver`, `Intent` and the whole intent vocabulary, the status-effect
and ability frameworks, `SimSet`/`SIM_ORDER`, `SimRng`, `GameEvents`. Everything
module-private lives in your own module's file.

**A note on the "do not edit `shared.rs`" rule.** Its file header and DESIGN.md's
"Ground rules for module agents" both say module agents must not touch it. That
was the *bootstrap* rule, written when a dozen agents were filling in stubs in
parallel from nothing, and it is no longer literally true: adding a verb to
`Intent` is the sanctioned way to add a player capability, and that is an edit
to `shared.rs`. Read the rule as what it has become — **`shared.rs` is not
scratch space.** A type there is a promise to every other module and to
whichever agent merges next. Add to it when your bead is about the contract;
solve it in your own file when it is not.

These files are large — `shared.rs` is ~13.8k lines, `ui.rs` ~12.5k,
`intent.rs` ~6.5k. Read the region you need, not the file. Grep for the type,
then read around it; the doc comments in this codebase are unusually load-
bearing and generally explain the *why* on the spot.

### 6.2 One choke point per rule. Extend it; never bypass it.

This is the single most load-bearing idea in the codebase, and every one of
these was, at some point, two places:

| The rule | Its one place |
| --- | --- |
| a player mutates the world | `Intent` → `SubmitIntent` → `intent::apply_intents`. `ui.rs` and `bridge.rs` build intents and mutate **nothing**. |
| what a unit's stats actually are | `effective_stats` / `effective_stats_with` — base row + statuses + research, one function. |
| what a place name means — and a role selector | `intent::resolve_places`, run once at the top of `compile_intent`, before any verb arm sees the intent. Since 0uu.1 it also resolves `select` / `target_select` / `site` phrases (`my hero`, `all army`, `nearest tree`, …) at fire time. |
| what an ability effect *does* | `combat::apply_atom` — used by the instant path and by the `ScheduledEffect` entities that pay out `OverTime` clauses. |
| spending money | `Economies::get_mut(team).pay(..)` |
| moving a unit's `Transform` | `units.rs`. Everyone else inserts `MoveTo`. |
| subtracting `Health` | `combat.rs`. Despawn is central in `shared.rs`. |
| writing `Selected` | `ui.rs` |

**Adding a capability means extending the choke point, not adding a second
path.** A new player-facing verb is a variant on `Intent`, which gives it to
both seats at once — that symmetry is the project's whole thesis (THESIS.md,
docs/INTENT.md §The fairness invariant), and a feature that reaches only one
seat is a bug even when it works.

The counter-case is instructive: `ai.rs` still writes `Order` directly, and this
is written down as a **known asymmetry** in DESIGN.md and docs/INTENT.md rather
than quietly tolerated. That is the standard. If you must bypass a choke point,
the bypass is a documented, named exception, not a shortcut.

### 6.3 The compiler validates for error messages; the paying system makes the rule true

`intent.rs` checks affordability, tech gates, ownership and busy-ness so it can
*explain a refusal in the frame the player made it*. It is **not** where the
rule is enforced.

The reason is concrete: the compiler busy-checks via components inserted through
`Commands`, which have not been flushed yet, so two commands in the same batch
both pass. The system that spends the resource is the one that makes the rule
true — `economy.rs` enforces one research job per forge, pays at enqueue, checks
supply at spawn.

So when you add a gate: put the *enforcement* in the paying system, and put a
matching *check* in the compiler only to produce a good message. If you put it
only in the compiler, it is advisory. If you put it only in the payer, the
player learns about it as a silent no-op.

And make the message teach. This codebase spends real effort on refusals that
name the fix — `Raider trains at the Barracks once a Workshop stands (you have
none)`, `no region named 'the-perimiter' - known places: …`. A refusal that
names no alternative is a refusal to help (docs/INTENT.md §Legibility).

### 6.4 Every gameplay system lands in a named `SimSet`

`shared::SIM_ORDER` names every phase of a frame and `CorePlugin` chains them
pairwise straight out of that constant, so the constant *is* the schedule:

```
Deaths → Fog → Input → CoCommand → AiThink → Think → Intent
       → Movement → Combat → Bounty → Economy → Upkeep → Feed → Cosmetic
```

A gameplay system with no `.in_set(..)` is scheduled by Bevy's multi-threaded
executor against whatever else is running, which means two runs of the same
binary can step the same units in different orders. That is not a theoretical
worry: before `SimSet` existed, movement, combat and separation all took
`&mut Transform` and the frame order genuinely varied run to run.

The older named sets (`FogSet`, `IntentApply`, `BridgePoll`,
`CommandNodeRefresh`, `CopilotSet`) are **nested** inside `SimSet`, so a system
carrying only `.in_set(IntentApply)` still inherits the frame order. Use them.
`Cosmetic` is the escape hatch for anything genuinely outside the contract
(health bars, rings, camera) — put it there deliberately, not by omission.

`the_frame_order_names_every_phase_exactly_once` fails if DESIGN.md and the
constant drift apart.

**Iteration order is randomness too.** `SquadOrders`, the fog `ghosts` map and
the `GameEvents` memo are `BTreeMap`s because std's `HashMap` reseeds per
process. If you add a collection on a gameplay path, it is a `BTreeMap`.

### 6.5 Seed `GlobalTransform` on any root spawn or teleport in `Update`

Bevy propagates `GlobalTransform` in `PostUpdate`. So any **root** entity you
spawn *or* teleport during `Update` must write its own
`GlobalTransform::from(transform)` **in the same statement** that writes the
`Transform`.

Otherwise every `GlobalTransform` reader that frame — `combat.rs` reads
positions that way — sees the origin for a fresh spawn or the pre-teleport
position for a mover. The symptom is famous here: towers plinking at the map
origin for one frame after a spawn, and teleport scrolls that shot from where
the hero used to be. It cost two beads to find and it is one line to prevent.

### 6.6 Bevy's 16-system-param ceiling, and the SystemParam bundle convention

Bevy caps a system at 16 parameters. Several systems here — `apply_intents`,
`write_snapshot` — sit *exactly* on it. The moment three features land at once,
somebody's build breaks with a trait-bound error that does not mention the
number sixteen anywhere.

The convention is a `#[derive(SystemParam)]` bundle, **split by access**: a
read-only bundle and a write bundle, so the borrow checker and the scheduler can
still see what conflicts. The precedents to copy are
`intent::IntentTables` / `intent::DeferredPolicy` / `intent::IntentWorld` /
`intent::IntentEvents`, `bridge::TeamTech` / `bridge::SeatVerdicts` /
`bridge::StandingOrders`, and `ui::CastLookup`.

Two things to know before you write one:

- If you are adding a param to a system near the ceiling, **bundle rather than
  hope.** You are not the last agent to touch that system.
- **Check whether the bundle already exists before you name a new one.** Two
  agents once invented the same bundle name in the same file in the same week
  (see §7).

### 6.7 Tables move, scalars stay

Every stat table is a RON file in `assets/data/` (`units.ron`, `buildings.ron`,
`abilities.ron`, `items.ron`, `research.ron`), loaded and validated by
`src/data.rs`, which panics at startup naming the offending row.

This is a **merge decision** before it is a data decision. Row literals inside
one big `match` interleave silently — git merges two agents' hunks cleanly
because they touch different lines — and the damage surfaces as a missing-field
compile error some commits later, if at all. One record per row either conflicts
loudly or not at all.

The line is: *every number and every flag is data; identity and rules are code.*
Kinds stay enums (they need a mesh arm anyway), derived facts stay derived
(`building_tier`, `is_hall`, `unit_tier`), formulas stay code (`research_bonus`,
`upkeep_rate`, `bounty_value`), and one-line singleton constants stay code
because a file per scalar is ceremony without a payoff.

**Adding a whole new kind** is a checklist, and the loader refuses to start if
you miss the last step: the enum variant, the entry in `ALL_*_KINDS`, the
mesh/colour arm in `units.rs` (or the `parts` block in `economy.rs` for a
building), the hotkey row, and the data record. DESIGN.md §"How to add a row"
has it in full. Put the balance rationale in a `//` comment next to the number
it explains — that is where this project keeps its design commentary now.

While tuning, use `BH_DATA_DIR=assets/data` so `.ron` edits take effect on the
next launch. Without it the built-in `include_str!` copy wins and editing a
`.ron` triggers a crate recompile — correct, but a rebuild you did not need.

### 6.8 Hotkeys go through the registry

`src/hotkeys.rs` holds `REGISTRY`, a table of `(Action, KeyCode, CardContext)`
rows, and `ui.rs` writes **no** key literals — captions are derived from the
`KeyCode` at draw time, so a caption cannot drift from its key.

Every letter A–Z is already used somewhere. "Free" is per-card, and the only
thing keeping two cards' `[I]` apart is a selection-disjointness argument. So:
add your row to `REGISTRY`, name the *semantic* action and never the key, and
run `the_registry_has_no_collision_in_any_card_context`. If it names a clash,
**pick another letter — do not widen the context to make the check pass.**
`every_tag_is_reachable_from_some_context` exists because a tag no context
includes is never collision-checked at all, and
`the_protected_bindings_are_where_they_have_always_been` is a deliberate
tripwire pinning A/S/Q/W/E/R/T/B/F/H/I/U — the keys players have muscle memory
for. If you find yourself editing that test, you are moving somebody's hands.

A card over twelve tiles is fine: `ui::paginate` gives it a `[Tab]` overflow
page and the hotkey stays live on every page. (It was not always fine. The card
used to truncate silently, and the worst failure mode was a building becoming
unbuildable *invisibly*.)

### 6.9 Wire compatibility is sacred

`state.json` and the command schema are read by `tools/*.py`, by the arena
ledger, and by LLM commanders following `tools/COMMANDER_BRIEF.md`. The
compatibility rules:

- **Additive keys only**, and mark them `#[serde(skip_serializing_if = ...)]`
  when they are optional, so a snapshot from a match that does not use your
  feature is byte-identical to the one that shipped before it.
- **Never rename or remove a key**, and never change a key's type. Keep the
  historical spelling as an alias if you must have a new one — `cast` accepts
  both `hero` and `caster` for exactly this reason.
- **The historical key sets are pinned by tests, on both sides of the wire.**
  In Rust: `intent::tests::legacy_wire_commands_parse` (every verb and its
  optional-field forms), plus the shape guards beside it —
  `the_item_verbs_carry_an_optional_hero_on_the_wire`,
  `the_destination_rides_the_wire_without_disturbing_the_old_shape`,
  `the_ready_verb_travels_from_the_wire_to_the_gate` — and in `bridge.rs`,
  `a_plan_round_trips_through_the_snapshot_json` and its siblings. In Python:
  `EXPECTED_TOP_KEYS` / `OPTIONAL_TOP_KEYS` in `tools/verify_intent_bridge.py`,
  an exact-set assertion on `state.json`'s top level. If your change makes one
  fail, the test is right and you are wrong — unless the bead is explicitly a
  protocol change, in which case update `tools/COMMANDER_BRIEF.md` in the same
  commit, because that document is the protocol's user manual and a commander
  reading a stale one is the bug.
- If you add a verb, you have added it to **both seats**. Check that the UI
  gesture and the bridge command produce the same `Intent`, not merely similar
  behaviour. docs/INTENT.md §"The residual asymmetry" is the worked example of
  what "similar" costs.

### 6.10 Fog-honesty is inherited, not re-derived

Fog is "one rule of knowability, computed once, rendered twice". Anything that
needs to know what a team knows asks the existing structure:

- live sight → `FogGrid::sees` / `FogGrid::knows_entity` (visible **or**
  remembered structure);
- memory → the intel ledger. The two predicates that read memory
  (`enemy_army_seen`, `enemy_hero_down`) **touch `world.units` not at all**,
  which is the structural version of the claim that they are fog-honest.

**The pattern to copy is the one insert guard.** Don't sprinkle visibility
checks through a feature; put one guard at the point where the fact *enters* the
feature, and let everything downstream be honest by construction. `ui.rs`'s
right-click picker was made to pick against `FogGrid::ghosts()` — the same
iterator that draws the translucent boxes — so *what is clickable is what is
drawn*, by construction rather than by two pieces of code agreeing.

The failure mode this prevents is not "an agent cheats". It is two renderers of
one fact silently disagreeing, which is the single thing docs/FOG.md promises
cannot happen — and which happened anyway when the fog quad's material was not
republished on repaint, so the ground wore frame one's fog while the minimap
tracked the match perfectly. Nothing errored. Nothing was wrong. They just
disagreed.

Note also that under `BH_FOG=0` the disabled path is a *fully lit grid*, not a
flag checked at every call site, so the off path cannot drift from the on path.
If your feature needs a disable switch, build it that way.

### 6.11 Levels are status; transitions are events

The sharpest edge any vocabulary here has cut its own user on, and it lost an
arena match.

- **`events` and `errors` are edge-triggered.** They *interrupt*. A thing that is
  still true is not a new interruption. Emit on the transition into a state,
  once more if the reason changes, once when it clears, once on the terminal
  transition.
- **Status fields are level-triggered.** `plans[].status` reads
  `blocked: <why>` in every snapshot for exactly as long as it is true, so
  nothing is hidden by the silence and a reader who wants to know can look.

The cautionary tale: a plan blocked on `cannot afford Footman` re-appended that
string to the seat's `errors` on every 5-second retry, `bridge_wait.py` woke on
every one, and the commander's event loop became a fire hose. They escaped it by
chaining waits, went ~100 game seconds without an order with 2280 gold banked,
and lost. The AAR's first line: *"that punished me for using the feature well."*

When you add a recurring condition, ask which of the two it is. If it repeats,
it is a status. If it happens, it is an event. And if you emit an edge on entry,
**you owe the reader the exit edge too** — told once that something is stuck and
never told it recovered, a reader has to poll, which is the polling the whole
layer exists to delete.

---

## 7. Merge traps

These are not hypothetical. Each one has broken a build here.

**1. The stat-table / enum-tail interleave.** Two agents each append a row —
to an enum, to `ALL_*_KINDS`, to a match, to a RON list. Git merges both hunks
cleanly because they are adjacent-but-different lines, and the result is a
struct literal missing a field or a kind with no row. It surfaces as `E0063`
(missing field in initializer) somewhere unrelated. **Chase every `E0063` after
a merge to its actual cause; do not paper over it by filling in a plausible
value.** The RON migration (§6.7) killed most of this, which is why the
remaining cases are in the enums and their tails.

**2. The shared closing brace in a both-added-at-anchor union.** This bit
**four** agents. Two branches both add a block at the same anchor point — two
new functions at the end of a file, two new arms, two new test cases. Git's
union-ish resolution keeps both bodies but the branches were sharing the
*closing delimiter*, so the merged file has one `}` too few or too many. The
commit `fix: restore mod tests closing brace lost in m5p union merge` is one of
these in the log. **After every union-style conflict resolve, check the
delimiters** — the cheapest check is that `cargo check` compiles and the second
cheapest is that the file's brace depth returns to zero.

**3. Test modules concatenating over one brace.** The special case of trap 2
that hurts most, because `#[cfg(test)] mod tests { ... }` is the last thing in
every file and therefore where every branch appends. The failure is subtle: the
tests still *compile* if the braces happen to balance, and a chunk of one
branch's tests quietly ends up nested inside the other's. `cargo test` passing
does not prove this did not happen. Count the tests you expect.

**4. Two agents inventing the same `SystemParam` bundle name.** Both branches
hit the 16-param ceiling in the same system in the same week, both bundle the
same resources, both call it the obvious name. The merge produces a duplicate
type definition, or worse, one definition and two subtly different sets of
fields. Before you name a bundle, grep for it; when you resolve this conflict,
**keep one and check its field set against both call sites** rather than
assuming the survivor is complete.

The general lesson: **a clean textual merge of two correct branches is routinely
a broken program.** After any merge — including the re-merge in §5 —
`cargo build && cargo test` is not optional, and neither is a smoke run.

---

## 8. Verification

### The interface: `tools/verify.sh` (tiers)

Verification is a named tier, not a remembered command line:

```bash
tools/verify.sh smoke      # it compiles, the tests pass, one short headless match runs to a game over
tools/verify.sh standard   # smoke + crossings sim + every python suite
tools/verify.sh full       # standard + 2-seed both-map matrix + determinism pair + the four bridge verifiers
tools/verify.sh identity   # two seeded fixed-dt runs, fingerprints diffed for byte-identity
```

**Say which tier you ran in your report.** "I ran `standard` and it was green"
is a claim the next agent can reproduce; "I tested it" is not.

Rules of thumb:

- Any code change: `smoke` at minimum, and never skip it because the change
  "can't affect the sim".
- Anything touching intents, the bridge, the snapshot, or a `tools/*.py`
  contract: `standard`.
- **A no-behaviour-change claim: `identity`.** This is the cheap proof and it is
  the one to reach for. If you are refactoring, extracting a bundle, moving a
  system into a set, or converting a table to data, two seeded fixed-dt runs
  producing byte-identical fingerprints *proves* the claim in a way no amount of
  reading proves it. It is also the fastest way to discover that your "pure
  refactor" changed the frame order.

`identity` builds two source trees through one cargo target directory, and both
of them are the same cargo unit — cargo hashes a workspace-root package's path
*relative to the workspace root*, which is empty for every checkout. That is a
fight with cargo's freshness rules, and the script now wins it by force: it
drops this crate's fingerprint before each build and again on the way out, and
refuses to compare unless cargo actually printed `Compiling bridgehead` for
each tree. Two costs to expect, both deliberate — the tier always pays two crate
compiles, and your next `cargo build` in the worktree recompiles once. The
alternative was a tier that reported IDENTICAL in sixty seconds having compiled
nothing, and left the worktree saying `Finished` over a syntax error. The tier's
own negative — a ref that must diverge and a ref that must not — is
`tools/test_verify_identity.sh`, which is manual and takes about ten minutes.

`tools/verify.sh` exists and is the interface — the raw litany in Appendix A
is what its tiers compose, and is there for when you need one piece of it in
isolation.

### Screenshots

`F10` writes a PNG of the window to `shots/` (or `$BH_SHOT_DIR`), named
`bh-<unix>-t<game secs>-<n>.png` so it carries both clocks.
`BH_SHOT_AT=20,90,240` — comma-separated **game** seconds — takes them
automatically, through the same `take_shot` function the key press uses, so a
scheduled shot and a pressed one cannot differ. Headless runs have no key to
press and no renderer to ask: the hotkey is registered by `UiPlugin`, which
`main.rs` adds only when there is a window, so this is an absence rather than a
branch.

> **Never photograph the game with an external tool.** Under XWayland, `import
> -window` returns a **stale pixmap** for an unfocused window. Three agents in a
> row filed a frame from minutes earlier as evidence, and nobody could tell,
> because a stale frame of an RTS looks exactly like a fresh one. The only
> process that reliably knows what a frame looks like is the one that drew it.

### `BH_BRIDGE` belongs to your own worktree only

The bridge is a live singleton: one directory per seat, overwritten in place.
Set `BH_BRIDGE` only for a game **you** launched from **your** worktree,
writing into **your** seat directories. Never point a verification at a seat
directory in the main checkout — that is the incident that produced the
`arena_run.py` safety rules in the first place.

---

## 9. Committing and handing off

- Commit on `bead/<id>`. Subject `<area>: <what changed>`, body explaining
  *why*, in prose someone will read in six months.
- Every commit carries the trailer:

  ```
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  ```

- No `.beads/` in the diff. Check `git status` before you commit; if beads
  artifacts appear, leave them uncommitted and say so.
- Do not push. Do not merge to master. Do not close the bead.

**Your final report should contain**, in this order:

1. What changed, by file, with absolute paths.
2. What you verified — the tier name, or the exact commands, and the result.
3. What you deliberately did not do, and why.
4. Follow-up work you found and did not do, phrased so the orchestrator can file
   it as a bead without asking you a question.
5. Anything you found that contradicts a doc. **Report it; do not fix it** — a
   drive-by doc correction inside a code bead is a merge conflict for whoever
   owns that doc, and the contradiction is often the more interesting finding.

---

## Appendix A — the raw verification litany

What the tiers compose. Use these directly if `tools/verify.sh` is not present
yet.

### A.1 Tests and build

```bash
cargo build            # the binary. Required before ANY live check (§4).
cargo test             # the harness. Does NOT rebuild the binary.
```

### A.2 The headless sim

The engine plays itself, with no window, as fast as the CPU allows, and exits on
game over:

```bash
cargo build && BH_HEADLESS=1 BH_AI_BOTH=1 BH_SPEED=16 BH_MAX_GAME_SECS=900 \
  ./target/debug/bridgehead
```

| Env | What it does |
| --- | --- |
| `BH_HEADLESS=1` | `MinimalPlugins` only — no window, no renderer, no GPU. Registers `headless_exit` in `SimSet::Feed`. |
| `BH_AI_BOTH=1` | the scripted AI plays the Human side too, so the match needs no commander (also toggleable live with F9) |
| `BH_SPEED=16` | `Time<Virtual>` multiplier, **clamped to `0.1..=16.0`**. 16× is the workhorse. Ignored entirely when `BH_FIXED_DT` is set. |
| `BH_MAX_GAME_SECS=<n>` | cap in *game* seconds, after which the run force-exits with a score-based verdict. **There is no default** — without it, a match only ends when a base falls, which for two scripted AIs can be a long time. Always set it in a verification run. |
| `BH_MAP=open\|crossings` | map layout (default `open`; an unrecognized non-empty value warns and falls back) |

**Run both maps.** `crossings` blocks a canyon with three fords in the
`NavGrid`, so it exercises pathing, chokepoint holds and the AI's ford logic
that `open` does not touch. A change that works on `open` and deadlocks on
`crossings` is a normal Tuesday.

**Confirm it reached a real game over**, not just the cap. A run that hit
`BH_MAX_GAME_SECS` proves the sim did not crash; a run that ended in a raze
proves the game still works.

### A.3 Determinism fingerprints — the cheap proof

```bash
tools/determinism_check.sh                    # seed 42, dt 0.05, map open, cap 600
BH_MAP=crossings tools/determinism_check.sh
SEED=7 CAP=300 tools/determinism_check.sh
```

It runs the binary twice with identical `BH_HEADLESS=1 BH_AI_BOTH=1
BH_SEED=$SEED BH_FIXED_DT=$DT BH_FINGERPRINT=$INTERVAL
BH_MAX_GAME_SECS=$CAP`, extracts the `FINGERPRINT` lines from both logs and
diffs them. **The exit code is the result**: `0` identical, `1` diverged (it
prints the first differing sample). Knobs: `SEED` (42), `DT` (0.05), `INTERVAL`
(10), `CAP` (600), `BH_MAP` (open), `OUT`, `BIN`.

Under the hood:

| Env | What it does |
| --- | --- |
| `BH_SEED=<u64>` | seeds `shared::SimRng`, the only source of gameplay randomness. Default is a fresh random seed **logged at startup**, so any match can be replayed from its own log. Terrain is separate and always fixed (`MAP_SEED`). |
| `BH_FIXED_DT=0.05` | headless only (windowed runs warn and ignore it); range `0.001..=0.25`. Installs `TimeUpdateStrategy::ManualDuration` so each frame advances the clock by a constant instead of by however long the frame took. Without it every accumulator in the sim integrates a wall-clock delta and no two runs agree. `BH_SPEED` is ignored while it is set. |
| `BH_FINGERPRINT=<seconds>` | logs a hash of the whole world (raw IEEE bits of every position and health, entity ids, both economies) at fixed game-time intervals. |

All three are opt-in; with none of them set, behaviour is exactly what it was.

**Use this for no-behaviour-change claims.** Fingerprint a run on `master`,
fingerprint the same seed on your branch, diff. Byte-identity is proof; reading
the diff is an opinion. Two guard tests keep the mechanism honest —
`the_fingerprint_describes_the_world_not_the_visit_order` (the hash must not
depend on iteration order) and the seed-reproducibility test beside it.

### A.4 The bridge verifiers

Four scripts in `tools/` that drive a **live** engine through the file protocol
and check the answers. Run them whenever you touch `intent.rs`, `bridge.rs`,
`shared::Intent`, the snapshot structs, or an error string a commander reads.

They do **not** all work the same way, and the difference matters:

| Script | Who launches the game | Notes |
| --- | --- | --- |
| `verify_intent_bridge.py` | **you do** | Expects a live `BH_BRIDGE=1` seat (`bridge/red`) already running, and drives it with `bridge_send.py`. Asserts `state.json`'s exact historical top-level key set (`EXPECTED_TOP_KEYS` + `OPTIONAL_TOP_KEYS`), that refusals keep the `cmd <i>:` prefix, and that every intent reaches `bridge/intent_log.jsonl` as a sentence. Fast, if the seat is up. |
| `verify_research_bridge.py` | itself, via `cargo run` | Headless, `BH_SPEED=16`, cap 4000. Drives five workers to a completed research level over the wire. |
| `verify_r9_legibility.py` | itself, via `cargo run` | Headless, `BH_SPEED=16`, cap 4000, drives four rejections over the wire and then surrenders to read a real `game_over` and its `game_over_reason`. |
| `verify_territory_bridge.py` | itself, from the **pre-built binary** | `python3 tools/verify_territory_bridge.py [--bin target/debug/bridgehead]`. Exits immediately if the binary is missing — it will not build one for you. Runs `crossings`, `BH_FOG=0`, `BH_SEED=7`. |

So: `cargo build` first (§4) — the territory verifier demands it and the others
merely benefit — and make sure nothing else of yours is running (§1), because
the bridge is a live singleton and these write into seat directories.

### A.5 Python tooling tests

```bash
for f in tools/test_*.py; do python3 "$f"; done   # what verify.sh's python stage runs
```

Every `tools/test_*.py` file MUST run standalone under bare `python3` — a
`_run()` main, not a hard pytest dependency. pytest is not installed on every
machine this repo builds on (`python3 -m pytest` fails here), and `verify.sh`
invokes the files directly, so a suite that imports pytest at module scope or
in `__main__` fails the python stage everywhere it matters. The files stay
pytest-*compatible* (plain `test_*` functions, zero-arg after decorators) for
anyone who has it. Touch a `tools/*.py` and you own these.

### A.5b Capturing bridge fixtures

A bridged seat LOSES its scripted AI (`bridge.rs` flips that faction's
`AiControlled` off), so `BH_AI_BOTH=1 BH_BRIDGE=red` is *not* an AI-vs-AI
match with an observer — the bridged side sits still and gets razed. To
capture realistic seat snapshots, bridge the seat and hand it straight back:

```bash
BH_HEADLESS=1 BH_AI_BOTH=1 BH_BRIDGE=red BH_SPEED=16 BH_SEED=42 ./target/debug/bridgehead &
python3 tools/bridge_send.py --seat bridge/red '[{"type":"autopilot","on":true},{"type":"ready"}]'
```

### A.6 Other useful envs

| Env | What it does |
| --- | --- |
| `BH_BRIDGE=<seats>` | which seats are played through the file bridge. **Your worktree only.** |
| `BH_FOG=0` | restores the pre-fog omniscient baseline. A comparison baseline, not a gameplay option. |
| `BH_DATA_DIR=<dir>` | prefer `<dir>/<file>.ron` over the compiled-in copy — edit stats without a rebuild. |
| `BH_RACE_BLUE` / `BH_RACE_RED` | `kingdom\|horde`, both defaulting to `kingdom`. With neither set the game is byte-for-byte the pre-races one. |
| `BH_SHOT_DIR` | where `F10` writes PNGs (default `shots/`). |
| `BH_SHOT_AT=<s,s,s>` | screenshot automatically at those game times (windowed only). |
| `BH_READY_TIMEOUT` | wall seconds before a match starts without a silent bridged seat (default 120). `BH_READY=0` disables the handshake entirely. |
| `BH_COMMAND_LATENCY` | Chain of Command travel delay — **off by default**. The `BH_LINK_*` vars tune it. If your bead touches ordering or doctrine, run once with it on. |
| `BH_COPILOT_TRUST` | `split` (default) / `full` / `strict` — what a co-commander may do directly vs must propose. |
| `BH_WINDOW=WxH` | window size, min 320x240. |
| `BH_PRESENT=vsync\|novsync` | windowed pacing. `novsync` (`AutoNoVsync` + a 60Hz timer-driven winit update mode) never blocks the update loop on a present; it is the **default when `BH_MAX_GAME_SECS` is set**, because a windowed run nobody is watching is the one arena r32 froze. A human's game defaults to `vsync`. docs/ARENA.md §"When a windowed round freezes". |
| `BH_WATCHDOG=<wall secs>` | log loudly when the engine has not stepped a frame in that long (and again when it recovers). Default 45 on an unattended windowed run, off otherwise; `0` disables. `BH_WATCHDOG_ABORT=<wall secs>` additionally aborts — for the core file, since `ptrace_scope` blocks live debuggers here. |
| `BH_INTENT_LOG` | path to the intent log (default `bridge/intent_log.jsonl`). |

---

## Appendix B — the map of the documentation

Read the one that matches your bead; do not read all of them.

| Document | What it settles |
| --- | --- |
| `CLAUDE.md` / `AGENTS.md` | the short version of this brief, plus the beads workflow |
| `DESIGN.md` | the module contract: who owns which file, the cross-module conventions, the RON data contract, the `SimSet` frame order, and the Bevy 0.16 API notes that keep you off stale idioms |
| `docs/INTENT.md` | the intent vocabulary and the fairness invariant; triggers, plans, territory, co-command; the legibility rules for error messages |
| `docs/FOG.md` | one rule of knowability, computed once, rendered three times; the intel ledger; what stays omniscient and why |
| `docs/TEMPO.md` | why command latency exists and what it does and does not tax |
| `docs/ARENA.md` | the dogfooding ledger, `arena_run.py`, the safety rules, and the screenshot story |
| `tools/COMMANDER_BRIEF.md` | the protocol as its users read it. **If you change the wire, you change this file.** |
| `THESIS.md` | why the project exists: both seats, same language, same knowability |
