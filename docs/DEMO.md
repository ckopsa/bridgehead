# The 5–10 minute demo

A presenter's runbook: what to run, what to show, what to say. The spine of the
demo is one live match in which **you** play a faction through the same file
bridge an LLM uses — because the fastest way to prove the thesis is to *be* the
AI for three minutes. Every command below was rehearsed against the engine; the
refusal text is quoted verbatim.

Timings assume the 8-minute version. For a tight 5, cut beat 5 (the live LLM
handoff) and point at the arena ledger instead. With 10, let the match breathe
and take a question mid-demo — the game keeps playing.

## Pre-flight (15 minutes before)

```bash
cargo build                      # binary at target/debug/bridgehead — do not skip
python3 --version                # the bridge tools are stdlib-only python3
```

- **Layout**: game window on one half of the screen, two terminals on the
  other. T1 is your seat driver; T2 is for the LLM handoff (beat 5).
- If doing beat 5 live: `claude` (or your agent CLI) authenticated and warm.
- **Fallbacks on standby**: `docs/fog-of-war.png`, `docs/ARENA.md`, and
  `arena/ledger.jsonl` open in an editor tab — they carry beats 5–6 if
  anything live misbehaves.
- No other engine may be running: `pgrep -af 'target/debug/bridgehead'`.

## Beat 1 — The hook (0:00–0:45, nothing on screen yet)

Say the thesis, not the feature list:

> Every game ever shipped was built for one kind of player — a creature with
> eyes, hands, and millisecond reflexes. When an AI plays, it plays through a
> disguise: parsed pixels, synthesized clicks, or a privileged API bolted on
> the side. This is an RTS built the other way: the human and the AI get the
> same game, the same vocabulary, the same fog — and the winner is decided by
> judgment, not reaction speed.

## Beat 2 — A world holding its breath (0:45–2:00)

In T1:

```bash
BH_BRIDGE=red BH_AI_BOTH=1 BH_READY_TIMEOUT=600 ./target/debug/bridgehead
```

A window opens: terrain, two bases, workers standing still. **Nothing moves.**

```bash
python3 -m json.tool bridge/red/state.json | head -30
```

Point at `"match_started": false`, `"waiting_for": ["red"]`.

> The red faction isn't played by the mouse — it's played by whoever writes
> JSON into this directory. Right now that's nobody, and the game refuses to
> start until every seat says ready — no player loses the opening to a slow
> connection. Today, the commander of the red faction is me, in a terminal.
> Everything I'm about to do is exactly what the LLM does.

## Beat 3 — Play the game through a file (2:00–4:30)

The compact readout the LLM actually uses:

```bash
python3 tools/bridge_view.py bridge/red/state.json
```

It lists your idle worker ids and the mines, with `(near me)` on the close one.
Copy a few ids, then — still frozen at t=0 — queue your opening and say ready:

```bash
python3 tools/bridge_send.py --seat bridge/red \
  '[{"type":"harvest","units":[<worker ids>],"target":<mine id>},
    {"type":"train","building":<townhall id>,"unit":"Hero"},
    {"type":"ready"}]'
```

The world springs to life: workers mine, the hall trains. Two talking points
while it does:

- **the catalog**: `head -40 bridge/red/catalog.json` — *"every unit, cost and
  tech gate as data. The human's build menus and the LLM's knowledge are
  generated from the same tables — new content needs no patch notes for
  either."* (And yes — the first hero is free. It said so in the catalog.)
- **refusals teach.** Send a typo on purpose:

```bash
python3 tools/bridge_send.py --seat bridge/red \
  '[{"type":"move","units":[<worker id>],"region":"the-perimiter"}]'
python3 tools/bridge_view.py bridge/red/state.json   # read the errors line
```

> `cmd 0: no region named 'the-perimiter' — known places: our base, their
> base, mid, southwest mine, northeast mine, northwest mine, southeast mine`
>
> "A refusal that names no alternative is a refusal to help. Every error in
> this game is written for a reader who will try again in five seconds."

## Beat 4 — One log, every hand (4:30–5:30)

```bash
tail -f bridge/intent_log.jsonl
```

Orders stream past as English sentences — yours *and* the scripted opponent's,
same schema, `"source"` and `"why"` on each line:

> `attack-move unit 8589935607 to (70.0, 70.0)` — `order:attackmove by script`.
> Every mutation from every seat — mouse, LLM, script — compiles to the same
> intent and lands in the same log. Fairness here isn't a policy, it's
> structural: there is no second API to cheat through. And any unit can answer
> "why are you doing that?" with its chain of command.

Meanwhile the opponent's first push usually arrives around game-minute two.
**Let it.** A skirmish over your mine is the best possible backdrop — this is
a real game with real stakes, and if you idle, you lose.

## Beat 5 — Hand your seat to an LLM (5:30–7:30) *(cut for the 5-min version)*

> I've been the AI long enough. Same seat, same files — I'm handing my job to
> Claude, mid-match.

In T2:

```bash
claude -p "Read tools/COMMANDER_BRIEF.md and follow it exactly. You are the
red commander, seat bridge/red, mid-match — the opening is already played.
Persona: calm and decisive. Loop with bridge_wait/bridge_view/bridge_send
until game_over is non-null, then report what you did in three sentences."
```

Stop typing. Watch the intent log: the source flips from your commands to the
agent's, mid-game, with no seam — because there is no seam to cross. Narrate
whatever it does ("it just set a squad posture — that standing order now
executes at engine speed, which is the whole tempo thesis").

**Fallback** if no agent CLI is at hand: open `arena/ledger.jsonl` and
`docs/ARENA.md` — twenty recorded LLM-vs-LLM rounds with hypotheses, verdicts
and after-action reports written by the commanders themselves.

## Beat 6 — The receipts, and the close (7:30–8:30)

Three fast proof points, then out:

- **Dogfooded by LLMs.** The arena ledger: rush-vs-boom series across twenty
  rounds; the balance patches (siege, upkeep, bounty caches, anti-idle
  autonomy) came from *LLM playtest after-action reports*. Quote one: after a
  UX bug cost a commander a match, its report opened with *"that punished me
  for using the feature well"* — and that sentence changed the event protocol.
- **Co-command exists.** `BH_BRIDGE=copilot` seats an AI *alongside* a human
  on one faction: postures apply directly, anything spending the shared
  treasury arrives as a proposal to approve or veto. Neither partner can act
  invisibly on the other.
- **Deterministic to the byte.** Seeded fixed-dt runs fingerprint the whole
  world; two runs diff byte-identical. Every match is replayable from its log.

> One vocabulary, one rule of knowability, one set of refusals — and the only
> thing left to compete on is judgment. github.com/ckopsa/bridgehead.

## When it goes sideways

| It happened | Do this |
| --- | --- |
| you talked past the ready hold and forgot the timeout | with `BH_READY_TIMEOUT=600` you have ten minutes; if it fired anyway, the match simply started — say "no player waits forever for a silent one" and keep going |
| a command is rejected mid-demo | best thing that can happen — read the error aloud; they're written for exactly this |
| blue's push overruns you in beat 4 | narrate it: "and that's the scripted baseline punishing an idle commander"; a lost match still shows every system |
| the agent in beat 5 is slow to its first order | expected — deliberation is wall-time; show the intent log's last line and talk co-command until it moves |
| the window won't open (no display) | rerun everything with `BH_HEADLESS=1`; every beat except the pretty window works in two terminals |
| total loss of the live demo | `docs/*.png` + `docs/ARENA.md` + this file's quotes carry beats 2–6 as a told story |

After the demo: close the window (or `kill` the engine **by the PID you
launched**), and note `bridge/` now holds your match's seat files — the next
match overwrites them.
