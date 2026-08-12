# The Arena Ledger

*Dogfooding as data. A round is a hypothesis test, and this is the file it goes in.*

## Why this exists

For ten rounds, the series lived in prose: a memory file, sixteen after-action
reports, a paragraph in `THESIS.md`, and eight beads. That prose is doing real
work — THESIS.md's fifth principle is that *the players are the playtest lab*,
and the reports are the lab notebook. But prose is a terrible place to keep an
experiment, because narrative reliably drops the one thing an experiment is
made of: **the question that was asked before the match started.**

"Round 9 ended at 5:24" survives retelling. "Round 9 was run to find out whether
3500-gold mines still leave room for tier 2 to exist" does not — and without it,
round 10 is just another match instead of *the answer to round 9*. The change
between them (`MINE_GOLD` 3500 → 5000, commit `c8be188`) was a hypothesis being
tested, and the only reason anyone can still say so is that someone happened to
write it in a commit message.

So the ledger's unit is not a match. It is a hypothesis test.

## The grammar of a round

Every record answers six questions, in this order:

| Field | The question it answers |
|---|---|
| `ruleset` | **Under what rules?** Map, environment, the balance constants at play, the scaffold version in force, the commit. |
| `seats` | **Who was playing?** Which side, which team, scripted or commander, which creed, which model, and which seat read the affordance document. |
| `hypothesis` | **What did we want to find out?** Written before the match. |
| `result` | **What happened?** Winner, length, and which of the engine's endings it was. |
| `evidence` | **How do we know?** AARs, logs, final snapshots, screenshots, metrics, sources. |
| `verdicts` | **What are we now entitled to believe?** Claims, each confirmed, refuted, or unresolved. |
| `lessons` | **What did it teach?** The sentences worth carrying into the next round. |

`hypothesis` and `verdicts` are the two fields that make a round evidence rather
than an anecdote. They are also the two a match cannot produce on its own: the
engine knows who won, and only a person knows what the win meant.

The distinction between `result` and `verdicts` is deliberate and load-bearing.
Round 6 was **won** by the rusher and **proved** nothing, because the loser
conceded on the owner's instruction while its position was winning. A schema
where the result implies the verdict cannot record that round honestly — and a
series that cannot record its own asterisks will eventually quote itself as
evidence for something it never showed.

## The honesty rule

Rounds 1–10 were played before this file existed. They are backfilled from
AARs, commit messages, source comments and the project memory file, and some of
their fields are simply **not recoverable**:

- Round 2 never produced a verdict at all — the simulator stopped feeding the
  bridge at t=3656 with `game_over` still null. Its length is genuinely unknown;
  three different clocks in the two reports disagree.
- `game_over_reason` did not exist in the engine until *after* round 10 (it was
  added because round 9's winner could not tell which victory it had got — see
  `shared.rs`, `GameOverReason`). Round 9's ending is therefore recorded as
  `null`, which is the honest answer and also the finding.
- No round has a recorded RNG seed, because there is no seed to record: the
  world seed is the compile-time constant `MAP_SEED` in `terrain.rs`.

A backfill that guessed at those would be worse than no backfill, because
nobody downstream could tell reconstructed numbers from recorded ones. So:

> **A missing value is `null`, never an invented one, and every `null` in a
> record must be named in that record's `unknown` list.**

This is checked, in both directions. `tools/arena.py validate` fails a record
with an undeclared `null`, *and* fails one that declares something which isn't
actually missing. The `unknown` list is a claim about the limits of our
knowledge, and it is the only claim in the file the tooling can verify on its
own.

`provenance` says which kind of record you are reading:

- `recorded` — a tool watched this match end and wrote down what it saw.
- `backfilled` — reconstructed from prose. `evidence.sources` says from what.

Rounds 1–10 are all `backfilled`. Round 11 onward, run through
`tools/arena_run.py`, are `recorded`.

## The schema

One JSON object per line in `arena/ledger.jsonl`, keys in schema order so a git
diff shows what changed about a round rather than that a dict reordered itself.

```jsonc
{
  "id": "r10",                    // r<number>, unique
  "date": "2026-08-10",           // YYYY-MM-DD
  "kind": "commander",            // commander | scripted | mixed
  "provenance": "backfilled",     // recorded | backfilled
  "ruleset": {
    "map": "crossings",           // open | crossings
    "env": {"BH_BRIDGE": "both", "BH_MAP": "crossings"},
    "constants": {
      "mine_gold": 5000,                 // a balance value not visible in env
      "alarms_ron": "a773dd4c4f9a",      // content digest of assets/data/alarms.ron
      "stances_ron": "22ef85561d44",     // content digest of assets/data/stances.ron
      "playbooks_ron": "6f0b1c9d2a44",   // content digest of assets/data/playbooks.ron
      "affordance_doc": "affordance-doc/1"  // only when a seat read the document
    },
    "commit": "c8be188",
    "notes": "One lever changed from r9: MINE_GOLD 3500->5000."
  },
  "seats": [
    {"seat": "bridge/red",  "team": "Claude", "kind": "commander",
     "persona": "rusher",
     "model": "opus",                    // optional; absent on a scripted seat
     "scaffold": "affordance-doc/1",     // optional; absent on a seat that played bare
     "autopilot_secs": 261.4,            // game seconds this seat spent handed to ai.rs
     "autopilot_spans": [{"from": 189.3, "to": 450.7}]}  // optional; when it was
  ],
  "hypothesis": "Does the rusher line still win with 40% more gold in the ground?",
  "result": {
    "winner": "Claude",           // Claude | Human | null (a draw is an absent winner)
    "winner_persona": "rusher",
    "duration_s": 561,            // game seconds, not wall seconds
    "game_over_reason": "surrender",  // razed | surrender | score | none | null
    "decisive": true,
    "duration_approx": false,     // optional; true when the number came from prose
    "duration_note": "..."        // optional; when the sources disagree, say so
  },
  "evidence": {
    "aars":    [{"seat": "bridge/red", "path": "arena/r10/red-aar.md"}],
    "logs":    ["arena/r10/engine.log"],
    "shots":   ["arena/r10/shots/bh-1754870400-t0324-01.png"],
    "sources": ["commit c8be188", "bead wc3clone-2hs"],
    "metrics": {"red_gold_final": 1539}
  },
  "verdicts": [
    {"claim": "More gold gets commanders to T3", "status": "refuted",
     "note": "Lumber, not gold, was the cap all game."}
  ],
  "lessons": ["Lumber, not gold, is what actually gates tier 3."],
  "unknown": []                   // every null path in this record, and nothing else
}
```

### Notes on the vocabulary

- **`razed` and `surrender` are the engine's own two wins** (`shared.rs`,
  `GameOverReason`). **`score`** is the time-cap verdict — a referee's
  opinion, not a win the game recognises, named differently so it can never be
  quoted as one. **`none`** is a round that stopped without ending.
- **A draw is an absent winner**, not a sentinel team. Round 2 is the reason.
- **The engine now *records* the score verdict rather than only printing it**
  (`wc3clone-j84`). A capped round ends the match the same way a raze does —
  `GameOver::decide`, both seats' final `state.json`, `game_over_reason:
  "score"` — because a commander's poll loop terminates on `game_over` and a
  round that merely stopped left every bridged seat waiting on a file nobody
  would write again. Two things did **not** change: `decisive` is still
  `false` for a `score` round, and the winner is still whoever `asset_score`
  had ahead when the cap expired. The engine and the ledger reach that verdict
  from the same numbers, so `arena_run.py` records the round it always did.
  On the wire only, a dead-even cap is spelled `game_over: "draw"` — the poll
  loop needs a value there; the ledger keeps the absent winner.
- **`duration_s` is game seconds.** Wall time is a property of the hardware and
  `BH_SPEED`; every AAR in the series is written in game seconds.
- **`persona` on a scripted seat is the literal string `scripted`.** "This seat
  had no creed" and "we don't know what creed this seat had" are different
  facts, and only the second one is a `null`.
- **`constants` is for balance values that were in force but are not in `env`.**
  `MINE_GOLD` is a compile-time constant, so the only way a round can say which
  value it was played under is to write it down.
- **`autopilot_secs` is the one key where zero is a value and absence is the
  gap.** Everywhere else in this schema an absent key is a fact ("this seat
  played bare") and a `null` is a gap. Delegation inverts it: `0.0` means *we
  read the intent log and nobody handed the faction over*, and an absent key
  means *nobody looked* — every round before r28, and any round whose intent
  log was not kept. The two are different claims and the flag is worthless
  unless they are told apart, so the runner stamps every commander seat of a
  measured round, zero included. A scripted seat never carries it: `ai.rs` is
  already playing that faction and there is no handover to record.
  `autopilot_spans` carries the edges (`from`/`to` in game seconds, plus
  `to_end: true` when the match ended with the seat still delegating), and the
  validator checks the spans add up to the total, because a summary line and a
  stamp that can disagree give the ledger two answers for one round.
- **A `score` round is never `decisive`, and the validator now says so.** Both
  readers of a verdict derive it that way already (`read_log` from the engine's
  log, `wait_for_seat_game_over` from the snapshot); the check exists because a
  hand-written or backfilled record has no such habit, and the claim belongs to
  the ledger rather than to whoever filled it in.

### What the ruleset records about the model and the scaffold

`docs/AFFORDANCES.md` constraint 3: *once the scaffold encodes any judgment, an
arena result measures model+scaffold. That is fine — it is the experiment we
want — but the scaffold version must appear in the round's `ruleset` so ledger
comparisons stay honest.* Five keys carry that, and they are written by
`tools/arena_run.py` rather than typed:

| Key | Where | When | What it is |
|---|---|---|---|
| `seats[].model` | seat | only on commander seats somebody named a model for | the model id that sat in that chair — `--model red=opus,blue=haiku`. |
| `ruleset.constants.affordance_doc` | round | only when a seat read the document | `tools/affordances.py`'s `DOC_VERSION` — the media type of the affordance document (`bridge_view.py --doc`). |
| `seats[].scaffold` | seat | only on the seats that read it | the same version, on the chair it sat in. |
| `ruleset.constants.alarms_ron`, `.stances_ron`, `.playbooks_ron` | round | **always** | the first 12 hex of the sha256 of `assets/data/alarms.ron`, `stances.ron` and `playbooks.ron`. `playbooks_ron` is the sharpest of the three: it is the digest of authored STRATEGY, which constraint 3 permits in the scaffold only on condition that it is versioned here — and rewriting a build order changes no line of `tools/affordances.py`, so `affordance_doc` cannot see it. |
| `ruleset.commit` | round | **always**, in a git checkout | `git rev-parse --short HEAD` at launch. Defaulted rather than typed: it was null on every round the runner ever recorded, and it is the only record of which stat tables the binary was compiled with. |

**`model` is free-form and per seat.** Free-form because model ids are somebody
else's vocabulary and they change faster than this repo does — a closed set
here would refuse a valid round every time a model shipped, which is a worse
failure than a typo you can grep for. Per seat because the interesting rounds
are the asymmetric ones, and the sentence the ladder is built to say is *this
model, with this scaffold, in this chair, beat that one*. A scripted seat
cannot be given a model and the runner refuses to stamp one: `ai.rs` is not a
model, and a round with no model in it must not enter a model comparison.

Three decisions in there are worth their reasons:

- **The document version is per seat, because the interesting rounds are A/B
  rounds** — the same model in both chairs, the document in one of them. A
  round-level flag cannot describe that experiment, so the seat carries which
  chair had it and `constants` carries which version was in force. A seat that
  played bare **omits** the key; it is a fact, not a gap, and nothing lands in
  `unknown`.
- **The stamp is conditional.** An unconditional `affordance_doc` on every
  round would make the scaffolded and unscaffolded rounds indistinguishable,
  which is the one comparison the field exists to enable.
- **The tuning digests are unconditional.** `alarms.ron` decides when a
  commander is *forced* to re-decide and `stances.ron` decides what each stance
  word does; a retune of either moves every round after it, scaffolded or not,
  so recording them only for scaffolded rounds would hide a change in exactly
  the comparison it invalidates. They hash the bytes, so a comment reflow reads
  as a retune — the safe direction to be wrong in, since a false "something
  moved" costs one glance at a diff and a missed one costs a comparison between
  two rounds that were not playing the same game. `BH_DATA_DIR` is honoured
  when it is set, because that flag is what decides which copy of the tables
  the engine actually reads; without it the engine runs the copy compiled into
  the binary and `ruleset.commit` is the record of that.

## Using it

```bash
tools/arena.py series                     # the standings, the round table, pacing
tools/arena.py show r9                     # one round in full
tools/arena.py rounds --hypothesis mine    # every round that tested the economy clock
tools/arena.py rounds --persona boomer --winner Human
tools/arena.py lessons --grep tower        # what the players actually learned
tools/arena.py validate                    # schema + honesty check
tools/arena.py add-aar r11 --seat bridge/red --path arena/r11/red-aar.md
tools/arena.py autopilot r33 --write       # stamp delegation from the intent log
```

`series` is the query the AARs used to end on by hand:

```
10 rounds — rusher 6, boomer 3, 1 draw(s)
length: median 13:00, shortest 5:24, longest 64:43 (9/10 rounds timed)
provenance: 0 recorded, 10 backfilled
autopilot: 2 of 8 measured rounds (35 on file) — r33 red 261s; r35 blue 156s
  * marks a win the winner spent on autopilot: r33, r35 — those verdicts
  measure when the seat delegated, not how it played
```

`autopilot` reads a round's intent log (`arena/<id>/bridge-logs/intent_log.jsonl`
by default, `--log` for anywhere else) and stamps the spans onto the record. The
runner does this for every round it runs; the subcommand is how a round recorded
before the stamp existed gets its numbers, and it refuses a round whose log is
not on disk rather than filling in a zero.

## Running a round

`tools/arena_run.py` owns the mechanical half of a round — the half that
rounds 9 and 10 did by hand, and that is therefore the half most likely to be
recorded wrong.

```bash
# scripted vs scripted, headless, appended to the ledger when it ends
tools/arena_run.py --hypothesis "does the tier ladder change scripted pacing?" \
    --seat red=scripted --seat blue=scripted \
    --map crossings --speed 16 --cap 1800

# a commander round: prepare the seats, then the orchestrator spawns the agents
tools/arena_run.py --hypothesis "does the rush still win at 5000g?" \
    --seat red=commander:rusher --seat blue=commander:boomer \
    --map crossings --windowed --cap 3000

# an A/B round: red plays with the affordance document, blue plays bare
tools/arena_run.py --hypothesis "does the scaffold carry a smaller model?" \
    --seat red=commander:haiku --seat blue=commander:haiku \
    --model both=haiku --scaffold red

# a ladder round: two models, same persona pair, same everything else
tools/arena_run.py --hypothesis "does opus still out-macro haiku at 16x?" \
    --seat red=commander:rusher --seat blue=commander:rusher \
    --model red=opus,blue=haiku
```

`--model SIDE=ID` (comma-separated, `both=` accepted) records which model sat
in each commander chair. It is a flag rather than a fourth colon-field on
`--seat` because a model id has no reliable separator and
`red=commander:rusher:brief:opus` is a line nobody can read. It refuses a
scripted seat: `ai.rs` is not a model, and naming one there would put a round
with no model in it into a model comparison. **Every ladder round needs it** —
without it the ledger records the scaffold and leaves the model to a commit
message, which is half an experiment.

`--scaffold red|blue|both` says which commander seats read
`tools/bridge_view.py --doc`, and it does two things: it stamps the round (the
three keys above) and it puts the document in the briefing line the orchestrator
spawns that seat from — the ledger entry is a claim about what the seat was
given, and the briefing is where the claim is made true. It refuses a scripted
seat, which reads no snapshot and therefore no document.

`--no-autopilot` sets `BH_NO_AUTOPILOT=1`, which makes `intent.rs` refuse a
mid-match handover to the scripted AI for the whole match. **It is off by
default**, because banning a documented verb is a round rule and the owner has
not made it one — see "Autopilot in a ladder round" below. It is round-level
rather than per-seat because the engine reads one process-wide variable. The
round records what it was played under either way: the flag lands in
`ruleset.env`, and `seats[].autopilot_secs` records what actually happened.

It derives `BH_BRIDGE` and `BH_AI_BOTH` from the seats rather than trusting a
hand-typed value — they are two spellings of one fact (who is playing which
side), and a launch line where they disagree produces a match that isn't the
one anybody meant. It waits for a real game over, reads the duration and ending
out of the engine's own log, and appends a validated record.

**It does not spawn commanders.** An LLM seat is an agent with a persona, a
budget and a transcript; deciding one exists is the orchestrator's job. For
commander seats the runner prepares the seat directory, prints the briefing each
seat needs, and waits while the orchestrator spawns agents against those
directories. After-action reports do not exist when the match ends either, so
they are attached afterwards with `arena.py add-aar`.

### Safety

The bridge is a live singleton: one directory per seat, overwritten in place. An
agent once destroyed a real match by running a verification against a seat
somebody was playing. Two rules follow, enforced rather than documented:

- **If any engine process is already running, the runner refuses to start.** It
  does not ask and it does not kill anything. The check matches the *executable*,
  not a command line mentioning it — a `pgrep -f target/debug/bridgehead` would
  match the runner's own argv and refuse every run.
- **It never deletes a bridge directory.** Directories it creates are its own;
  an existing one is reused only with `--reuse-seat`, and a pre-existing
  snapshot is *renamed aside*, never removed — a stale `game_over` from the last
  match would otherwise read as this match ending instantly.

### When a windowed round freezes

A windowed round runs the simulation on the same loop that draws the window, so
**anything that stops the presenter stops the match.** Round 32 is the worked
example: on Hyprland/XWayland, with the window parked on an inactive workspace,
the engine stopped stepping at t=1495.7 of an 1800s cap — every thread parked,
~zero CPU, both seats' snapshots five minutes stale — and it did *not* recover
when the workspace came back. It had to be killed by PID and the round was
abandoned (evidence: `arena/r32-frozen/`). Five headless rounds and two watched
windowed rounds on the same binary were clean, so it is rare and it is
windowed-only.

Three things follow, and all three are on by default:

- **An unattended windowed run does not present in step with the display.**
  `BH_MAX_GAME_SECS` is the engine's mark of a run nobody is watching, and it
  now also selects `AutoNoVsync` (an `Immediate`/`Mailbox` present, which is
  fire-and-forget rather than a slot in a queue the compositor has to drain)
  plus a 60 Hz timer-driven winit update mode instead of `Continuous`, whose
  only wakeup is a redraw round-trip through the window system. Force either
  with `BH_PRESENT=vsync|novsync`. The runner sets it for **every** windowed
  round regardless of the cap, so the ledger records which pacing a round was
  played on — r32 is the round nobody can answer that about. A human's game is
  unchanged — vsync, because a human is present to notice a freeze and tearing
  is a real cost.
- **The engine watches its own frame counter.** `BH_WATCHDOG=<wall seconds>`
  (default 45 on an unattended windowed run, off otherwise, `0` disables) logs
  loudly when no frame has been stepped for that long, naming the game second
  the match stopped at, and logs again when frames resume.
  `BH_WATCHDOG_ABORT=<wall seconds>` (off by default) aborts the process at a
  longer threshold — `abort()` rather than `exit()` because it leaves a **core
  file**, which is a backtrace that needs no debugger permissions.
- **The runner stops waiting.** `arena_run.py --stall SECS` (default 120, `0`
  to disable) ends a windowed round whose game clock has stopped moving, so a
  wedge costs two minutes instead of the round's whole wall timeout. It only
  starts counting once the clock has moved at all — `t` is legitimately 0 for
  the whole ready handshake. A wedged round records **no** verdict: a wedge is
  not a match, and `game_over_reason` stays null with the story in `engine.log`.

**What that changes about a hidden window.** Before this, a windowed round
whose window was minimised, occluded or on another workspace slowed to
whatever rate the compositor chose to take frames at — including zero — and
said nothing about it, so a round could quietly stop being played while the
runner waited. An unblocked present has no such back-pressure: the match keeps
simulating at 60 Hz with nobody looking at it, which is what an arena round
was always supposed to do.

This is mitigation, not a cure. The root cause was never confirmed — the
backtrace note below is how to change that — and the simulation is still not
decoupled from the presenter, so a hard GPU-side wedge would still stop the
game. What changed is that it now announces itself and costs two minutes.

**Getting a backtrace from the next one.** `arena/r32-frozen/freeze-backtrace.txt`
contains one line — `ptrace: Operation not permitted` — because this machine
runs with `kernel.yama.ptrace_scope=1`, which forbids attaching to a process
that is not your child. Options, in order of preference:

```bash
# 1. Let the engine dump its own core (no ptrace involved at all):
BH_WATCHDOG=45 BH_WATCHDOG_ABORT=180 ...        # then: coredumpctl gdb bridgehead
# 2. Relax the restriction for the session (root, resets on reboot):
sudo sysctl kernel.yama.ptrace_scope=0          # then: gdb -p <pid> / eu-stack -p <pid>
# 3. Start the engine under the debugger, so it IS your child:
gdb --args ./target/debug/bridgehead
```

`thread apply all bt` is the wanted artifact: whether the wedged thread is in
`vkQueuePresentKHR`/`eglSwapBuffers` (a presenter whose consumer stopped) or in
winit's event wait (a redraw that never arrived) decides which half of the
mitigation above is the real one.

## Autopilot in a ladder round

> **Status: owner decision pending.** Nothing below is a policy. The machinery
> for all three options exists and the recording happens regardless; which
> option the ladder adopts is the owner's call, and this section is the brief
> it should be made from.

### What happened

`autopilot` hands a faction to `ai.rs` for as long as the commander leaves it
there. It is a documented verb — `tools/COMMANDER_BRIEF.md` calls it *emergency
only* — so engaging it is **legal**, and both seats that did so disclosed it in
their AARs. The problem is not conduct. It is that **a round's verdict stops
measuring the model the moment it engages**, and until this bead nothing in the
round's record said it had.

Two of the four Haiku seats across the second ladder delegated, and both ended
on the winning side:

| round | seat | span | share of the match | ending |
|---|---|---|---|---|
| r33 | `bridge/red` (Claude) | t=189.3 → t=450.7, released 9.8s before the end | 261.4s of 460.5s — **57%** | red wins, surrender |
| r35 | `bridge/blue` (Human) | t=316.0 → the end, **never released** | 156.8s of 472.8s — **33%** | blue wins, surrender |

In r33 the delegated stretch spans the whole recovery from blue's worker raid
and the winning army buildup. In r35 the entire winning late game — tier 2, the
Spearman counter, the expansion, the pressure from t=316 onward — was scripted
play, and the seat that *lost* had played the floor tier's best own-hands game
on record (arena/LADDER2.md, Addendum 1). The six other rounds with a kept
intent log (r28, r29, r30, r31, r34, r36) measure zero.

That is the finding the options are about: **at the floor tier, delegation
beat playing, twice.** Either it is a legitimate skill the ladder is measuring
on purpose, or the ladder is not measuring what it says it is.

### What the machinery does now, whichever way it goes

Recording is unconditional, because the bead that added it said so and because
r33 and r35 are the proof: the spans were in `bridge/intent_log.jsonl` all
along, and nobody could ask the ledger a question nobody had thought to ask
before the round.

- **The runner stamps every round.** `arena_run.py` notes the intent log's size
  before launch, keeps this round's slice at
  `arena/<id>/bridge-logs/intent_log.jsonl`, and writes `seats[].autopilot_secs`
  (and `autopilot_spans`) onto every commander seat — `0.0` included, so a
  measured zero is distinguishable from an unmeasured round.
- **The ledger shows it.** `arena.py series` has an `autopilot` column, marks
  the `won` cell with `*` when the winner delegated, and ends with a series
  line naming the rounds.
- **Old rounds can be stamped.** `arena.py autopilot <id> --write` reads a kept
  log and backfills. r28–r36 have been stamped from the logs in `arena/`.
- **The ban is one flag.** `arena_run.py --no-autopilot` sets
  `BH_NO_AUTOPILOT=1`; `intent.rs` then refuses the verb with a message that
  says whose rule it is and points at the doctrine tier instead. r36 was run
  that way (through `--env`, before the flag existed).

### The three options

**(a) Ban mid-match autopilot in ladder rounds.** Round rules say so, the
briefing says so, and `--no-autopilot` makes it true — a prompt cannot bind a
model, which r33 and r35 demonstrated, so the enforcement has to be at the
compiler. *Cost:* the seat loses a real escape hatch, and a round where a
commander would have delegated instead plays on with whatever it has, which is
its own kind of unrepresentative. *Already available:* one flag, plus a line in
the round rules.

**(b) Allow it and record it.** Rounds stay as they are; the ledger carries the
seconds and the summary flags the qualified wins, so a ladder table can weight
or filter them. *Cost:* the ladder's headline numbers keep mixing two skills
unless whoever reads them applies the filter, and "r33 was a Haiku win" stays
technically true and misleading. *Already available:* everything — this is what
the machinery does with no further decision.

**(c) Make delegation the measured skill at small tiers, and say so in the
hypothesis.** The claim becomes *knowing when to hand off is the competence a
floor-tier commander has*, and a round's hypothesis states it, so the result
means what it says. *Cost:* it is a different experiment from the one the
ladder has been running, and it cannot be compared against r25–r32 without
saying so. *Already available:* the hypothesis field, plus the seconds to
report against.

A fourth shape is available cheaply if the owner wants it and is **not**
recommended here: a per-seat ban. `intent.rs` reads one process-wide
`BH_NO_AUTOPILOT`, so a round where red may delegate and blue may not would
need the refusal to consult the seat rather than the environment. That is an
engine change and a bead of its own.

## Screenshots

`F10` writes a PNG of the window to `shots/`, or to `$BH_SHOT_DIR` — which the
runner points at `arena/<round>/shots/`, so a screenshot files itself with the
round it belongs to. Names carry both clocks: `bh-<unix>-t<game seconds>-<n>.png`.

This exists because external capture does not work here. Three agents in a row
photographed the game with an outside tool and filed a **stale pixmap** as
evidence: under XWayland the X11 window contents are not what is on the screen,
so the shot showed a frame from minutes earlier — and nobody could tell, because
a stale frame of an RTS looks exactly like a fresh one. The only process that
reliably knows what a frame looks like is the one that drew it, so the engine
takes its own pictures.

Headless runs have no key to press and no renderer to ask, and simply never do
this. That is the graceful no-op: not a branch, an absence — the hotkey is
registered by `UiPlugin`, which `main.rs` adds only when there is a window.

![A frame captured by F10 during a live match](arena-f10-shot.png)

*Taken by pressing F10 during a windowed AI-vs-AI run, 45 game-seconds in:
`bh-1786388320-t0045-02.png`. The point of the picture is that it is the frame
that was actually on the screen — HUD, fog boundary, workers on the mine, all
consistent with t=45 — which is exactly what the external tools failed to
deliver.*

## What the backfill could not recover

Recorded here so the gaps are a known quantity rather than a surprise:

- **Round 2's length and verdict.** No `game_over` was ever set; the two reports
  cite three different clocks (t≈2600, t≈2915, t≈3656).
- **Round 9's `game_over_reason`.** The field did not exist. The winner's own
  report says it could not tell whether it had razed the enemy or been conceded
  to — which is precisely why the field exists now.
- **Round 8's exact length.** The loser reports surrendering at t≈3700; the
  referee and the winner record a raze at t≈3883. The raze is the engine's own
  ending, so it is the one recorded, with the disagreement in `duration_note`.
- **Rounds 3, 5, 8 lengths are prose-rounded** ("game-minute 13", "t≈1167") and
  carry `duration_approx: true`.
- **Launch environments for rounds 1–8.** Only `BH_BRIDGE=both` is attested.
  The map is inferable — `crossings` did not exist until commit `62d81b0` — so
  rounds 1–8 are recorded as `open`.
- **AAR files for rounds 1–8.** They exist only inside subagent transcripts, not
  as files. Rounds 9 and 10's reports are in the repo at `arena/r9/` and
  `arena/r10/`; earlier rounds cite their transcripts in `evidence.sources` and
  carry their substance in `lessons` and `verdicts`.
- **Per-round seeds.** There are none to recover: `MAP_SEED` is a compile-time
  constant and the world is identical every run.
- **`seats[].model` and `ruleset.commit` for rounds r1–r23.** Both keys start
  at the next round and the earlier ones **stay empty** rather than being
  filled in. This is a decision, not an omission.

  `ruleset.commit` is null on every round the runner has recorded so far,
  because it was a flag nobody typed; it now defaults to
  `git rev-parse --short HEAD`. Reconstructing the old values from the ledger's
  dates would mean picking the commit nearest each round's `date`, which is a
  guess dressed as provenance — and provenance that might be wrong is worse
  than provenance that is absent, because the second kind announces itself.
  A round whose commit genuinely is recoverable can be corrected by hand with
  the source cited in `evidence.sources`, which is what that field is for.

  `seats[].model` is worse to guess at, because the ledger contains no trace
  of it at all. The rounds were played by whatever model the orchestrator was
  running that day, and "whatever we were probably using in mid-August" is not
  a variable anybody may compare against. So r1–r23 carry no `model` key, the
  ladder starts from the first round that does, and the honest reading of the
  earlier series is *these rounds tested personas and rules, not models*.

  The two absences read differently in `unknown[]`, and correctly so.
  `ruleset.commit` is a `null` on r1–r23, so every one of those rounds already
  carries `"ruleset.commit"` in its `unknown` list — the ledger says out loud
  that it does not know, which is the whole point of that list. `model` is a
  key those records simply do not have, and an absent key is not a null: "this
  round was not a model experiment" is a fact, not a gap, the same rule
  `scaffold` and `ready_wait_s` already follow.
