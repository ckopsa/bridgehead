# RTS Commander Briefing (LLM seat)

You command one faction of a Warcraft-3-style RTS through a file channel.
Your seat directory is given in your instructions as `<SEAT>` (e.g. `bridge/red`).

**If your seat is `bridge/copilot` you are a CO-COMMANDER, not the faction** —
a human is playing this side with you. Read all of this first, then the
co-commander section near the end, which is the only part that differs.

## The loop (event-driven — do not blind-sleep)
1. `python3 tools/bridge_wait.py --seat <SEAT> --max 15` — blocks up to 15s but WAKES EARLY
   (~1-2s) the moment an event fires (attacks, losses, bounty spawns, your command errors)
   or the game ends. It prints why it woke.
2. `python3 tools/bridge_view.py <SEAT>/state.json` — compact readout (combine 1+2 in one
   bash call: `wait ... && view ...`).
3. Decide. 4. Write commands (see below).
Repeat until `game_over` is non-null, then stop and report the result.
Your units are never fully idle even between your cycles: every army unit auto-joins
squad 0 (default posture: defend your base). Repoint squad 0 — or set it to
`forage` — and your army acts continuously without further orders.

Commands: `python3 tools/bridge_send.py --seat <SEAT> '<json array>'`.
Each command: `{"type":..., ...}`. Errors come back in the next snapshot's
`errors` — read them, they mean a command was rejected (dead unit, can't
afford, not yours). `seq` is handled for you. If `applied` is present in the
next snapshot it is the other half of that verdict: `[{"cmd":"cmd 3","delay":1.8}]`
says command 3 was accepted but took 1.8s to reach the units it named — see
**The chain of command** below. Commands not listed there cost nothing.

## Command reference
Unit orders (ids from state):
- `{"type":"move"|"attackmove","units":[ids],"x":..,"z":..}`
- `{"type":"attack","units":[ids],"target":enemy_id}`
- `{"type":"harvest","units":[worker_ids],"target":node_id}` (mines AND trees — tree ids in `trees_near`)
- `{"type":"return","units":[worker_ids]}`  `{"type":"stop","units":[ids]}`  `{"type":"follow","units":[ids],"target":own_id}`
Production:
- `{"type":"build","worker":id,"kind":"Farm"|"Barracks"|"TownHall","x":..,"z":..}` (site must be free; you pay on placement)
- `{"type":"train","building":id,"unit":"Worker"|"Footman"|"Archer"|"Hero"|...}` (queue cap 7;
  full roster in `catalog.json`). A `train` of a hero is rejected with a reason when your slots
  are full or you already hold that class.
- `{"type":"cancel","building":id,"index":n}`  `{"type":"rally","building":id,"x":..,"z":..}` or `{"target":node_or_own_unit_id}`
- `{"type":"cast","hero":id}` — cast the caster's first available ability (heroes: their class
  ability; a TownHall id works too: CallToArms turns nearby workers into fighters for 40s,
  90s cooldown). Add `"ability":<index>` or `"ability":"Slam"` to pick a specific one — casters
  can have several, each with its own cooldown and unlock condition. Every caster's slots are
  listed in the snapshot as `units[].abilities` / `buildings[].abilities`
  (`index`, `cd`, `unlocked`, `ready`, `requires`), and described in catalog `abilities`
  (where `unlock_hero_level` / `unlock_tier` give the gate as a number). Names are matched
  loosely — case, spaces, dashes and underscores are all noise, so `"Call to Arms"`,
  `"calltoarms"` and `"call_to_arms"` are one ability. The same holds for unit, building,
  item and research names, and for `priority` classes.
- **What a cast does** is catalog `abilities[].effects` — a LIST of clauses, each with its own
  `atom` (`damage` / `heal` / `status` / `militia` / `summon` / `teleport`), its own numbers
  (`amount`, `status`+`magnitude`+`duration`, `count`, …), its own `targets`
  (`enemies` / `allies` / `own_workers`) and its own `schedule` (`instant`, or `over_time` with
  `interval`/`ticks`). One cast can do several things to different sides — Sanctuary is
  `[status HealOverTime 15/6s allies, status ArmorBuff 0.25/6s allies]`. The **first** clause is
  what the cast aims at. The old flat fields (`effect`, `status`, `status2`, `power`,
  `duration`) still describe the headline clause and are unchanged.
- **Aiming a cast.** Catalog `abilities[].target` says where an ability lands: `"caster"`
  (centred on the caster — every ability but one), `"point"` (send `"x"`/`"z"`), or `"unit"`
  (send `"target":id`), within `abilities[].target_range` of the caster. `{"type":"cast",
  "caster":id,"ability":"Slow","x":40,"z":-12}` throws Slow at a point. **Omit the aim and the
  engine aims for you** — the reachable centre catching the most bodies the ability affects —
  so `{"type":"cast","caster":id,"ability":"Slow"}` is still a good sentence, and it is the
  same rule `autocast` uses. Out of range is **refused with both numbers**, never walked into:
  your caster stays where you put it.
- `{"type":"buy","shop":id,"item":"HealingPotion"|...,"hero":id}` — a hero buys the item
  (2 inventory slots; see catalog `items`). `{"type":"use_item","slot":0,"hero":id}` consumes it.
  **`hero` is optional but you want it once you field two.** Omitted, both verbs default to your
  living hero with the LOWEST id — fine with one hero, a coin flip with a Champion and a
  Priestess. Name the hero and the item lands in that hero's bag; name one that is not a living
  hero of yours and the command is REJECTED rather than redirected, so a typo can never hand
  your potion to the wrong character.
  TownPortal teleports that hero + nearby own units to one of your halls; ScrollOfMassTeleport
  (T3) takes the hero + EVERY own non-worker on the map (workers keep mining).
  **YOU CHOOSE THE HALL.** `{"type":"use_item","slot":0,"hero":id,"destination":<building id>}`
  — `destination` is one of YOUR OWN FINISHED HALLS, read straight off `buildings[]`. Omit it and
  you get the hall nearest the hero, which is the old behaviour and is WRONG in the case the
  scroll exists for: with the army at the expansion and the main dying, "nearest" ports you to
  the expansion you are already standing on. Name the far hall. A destination that is not your
  own standing hall is REJECTED (`destination 123 is not your standing hall`) and the item is
  NOT spent, so a typo costs you a cycle, never a 250-gold scroll. The catalog flags both items
  `"destination": "choosable"`. On arrival your EVT feed says which hall you landed at by name:
  `hero ports the army to the Keep at (-70.0, -70.0)`.
Doctrine (standing orders, executed continuously — USE THESE, they fight for you between your turns):
- `{"type":"priority","units":[ids],"classes":[...]}` — focus-fire order ([] clears). Valid classes:
  Hero (both hero types), Archer (also Sorcerers — the fragile ranged back rank), Footman,
  Worker, Building, Siege (catapults), Cavalry (raiders and knights).
- `{"type":"retreat","units":[ids],"below":0.35,"x":..,"z":..}` — auto fall-back when hurt
- `{"type":"leash","units":[ids],"x":..,"z":..,"radius":20}` — never chase/fight beyond anchor (radius 0 clears)
- `{"type":"autocast","units":[caster_ids],"min_enemies":3}` — any CASTER fires on its own
  (heroes and Sorcerers alike; Sorcerers are born with Slow on autocast at 1 enemy). Add
  `"ability":<index|"Name">` to govern a specific ability; each one keeps its own trigger, and
  `min_enemies:0` clears just that rule.
- `{"type":"squad","units":[ids],"id":1}` then `{"type":"posture","id":1,"posture":{"type":"defend","x":..,"z":..,"radius":18}}`
  (`"push"` with x/z, `"escort"` with `"unit":id`, or `"forage"` with x/z muster) — squads
  re-task themselves every second and ADVANCE COHESIVELY: a strung-out Push/Forage squad gathers before pressing on, so slow units set the pace and you arrive as one force. Defend postures are REACTIVE: enemies entering the radius
  pull the whole squad onto them. FORAGE squads continuously hunt the nearest bounty cache
  (attack-moving, so they fight what they meet) and hold at the muster point when none are up —
  the set-and-forget way to own the midfield. Squad 0 exists automatically (all army units
  enroll unless you assign them elsewhere; default posture defends your base).
- `{"type":"template","building":id,"squad":1,"retreat":{"below":0.35,"x":..,"z":..},"priority":["Hero",...],"autocast":3}`
  — stamp standing doctrine on a production building: every unit it trains spawns WITH these
  policies (null/omitted pieces skipped; all null clears). Set once, stop re-issuing per spawn.
Triggers (CONTINGENT standing orders — see the section below; this is the shape):
- `{"type":"trigger_set","name":"home-guard","when":{...},"then":{<any intent>},"repeat":30}`
  — the engine watches `when` at 4 Hz and submits `then` itself. `repeat` omitted = fires once.
- `{"type":"trigger_clear","name":"home-guard"}` — disarm one. Omit `name` to clear all of them.
- `{"type":"autopilot","on":true}` — hand your whole faction to the scripted AI (emergency only).
- `{"type":"surrender"}` — concede the match (opponent wins immediately). The honorable end to a
  hopeless position — no income, no army, no path back. Preferable to dragging out a decided game.

## Triggers: make the engine react for you

**This is the single biggest thing you can do about your own latency.** You poll
every ~15 seconds. Between polls the engine runs the game. Doctrine already
fights for you continuously; a trigger makes it *react* for you — a condition it
checks four times a second and an intent it submits the instant the condition
holds. The order lands in the frame the rule fired, and it **pays no command
link** (see the chain-of-command section) because you reached the unit when you
armed the rule, not when it fired.

Arm your standing plans at the top of the match and stop spending polls on
alarms you already know how to answer.

Rules: max **8** armed at once. Re-using a `name` replaces that rule in place
(free — the cap counts names, so tune all you like). A trigger cannot arm or
clear another trigger. Your armed rules come back in the snapshot's `triggers`
array with `status` (`armed`/`cooling`/`spent`), `last_fired`, and the English
`sentence` — and every fire writes a line into `events`:

    trigger home-guard fired: squad 1 defends (-70.0, -70.0) within 26

### The nine predicates

| `when` | means |
|---|---|
| `{"type":"base_under_attack"}` | any of YOUR buildings damaged in the last 8s |
| `{"type":"hero_below","frac":0.35}` | any of your living heroes under that fraction |
| `{"type":"squad_below","id":1,"frac":0.5}` | squad 1's POOLED health under that (false if the squad is empty) |
| `{"type":"enemy_sighted","class":"Siege","count":3}` | you can SEE that many enemies now (`class` optional; fog-honest) |
| `{"type":"bounty_spawned"}` | a cache you can see is on the map |
| `{"type":"mine_dry"}` | a dry gold mine within 40 of one of your finished halls |
| `{"type":"tier_reached","tier":2}` | your tech tier |
| `{"type":"unit_count","kind":"Footman","count":8}` | your living count of one unit kind |
| `{"type":"game_time","at":360}` | the match clock, in seconds |

There is nothing here about the enemy's gold, tech or hero health — the snapshot
does not carry those for either seat, so no predicate can.

### Three recipes worth arming in your first batch

**1. Home guard** — the army comes home when the base burns. Repeating, because
a base is raided more than once.

```json
{"type":"trigger_set","name":"home-guard","repeat":30,
 "when":{"type":"base_under_attack"},
 "then":{"type":"posture","id":1,
         "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}}}
```

**2. Hero save** — the hero walks out before it dies. Your hero is a command
node and a hero slot; losing it is the most expensive single event in a match,
and it happens inside one poll cycle.

```json
{"type":"trigger_set","name":"hero-save","repeat":45,
 "when":{"type":"hero_below","frac":0.35},
 "then":{"type":"move","units":[<hero id>],"x":-70.0,"z":-70.0}}
```

**3. Expansion alarm** — take the next base the moment the current one runs out,
without watching `mines[].remaining` every poll. Fires once, which is right: you
only need telling the first time.

```json
{"type":"trigger_set","name":"expand",
 "when":{"type":"mine_dry"},
 "then":{"type":"build","worker":<worker id>,"kind":"TownHall","x":0.0,"z":-60.0}}
```

A caution that applies to all three: `then` is frozen when you arm it, so ids in
it are ids that may die. Prefer `posture` on a **squad** over a list of unit ids
where you can — a squad survives its members. And a trigger whose action is
refused when it fires reports that in your `errors` array tagged
`trigger:<name>`, so check there if a rule seems to do nothing.

## Speakable strategy: `tools/intent_compile.py`

Everything above is the JSON. You can also just say it:

```bash
python3 tools/intent_compile.py --seat <SEAT> --send \
  "hold the northwest ford, forage mid with the cavalry, retreat at 35%"
```

It reads your snapshot, compiles each clause into the SAME command objects
documented above, and writes the batch with the next `seq` (exactly like
`bridge_send.py`). It prints what it understood, per clause, to stderr:

```
  ok       'hold the northwest ford'   -> squad 1 defends (-60.0, 60.0) with 7 unit(s)
  ok       'forage mid with the cavalry'-> squad 2 forages (0.0, 0.0) with 3 unit(s)
  ok       'retreat at 35%'            -> 10 unit(s) fall back to (70.0, 70.0) below 35%
```

Why bother when you can write JSON? Because the phrases are shorter than the
ids, and because places have names: `mid`, `our base`, `their base`, `the
northwest ford` (any choke in `map.chokes`, by name), `the contested mine`,
`the nearest bounty`, `the west`. Units too: `the cavalry`, `the siege`, `the
hero`, `squad 2`, and the default — the whole army, never your workers.

Unit and building vocabulary is read from your seat's `catalog.json` when it is
there (it always is), so new content is speakable the day it ships — the tool
learns that the Raider trains at the Barracks, or that a Sanctum trains
Sorcerers, by reading the same file you do.

**Two heroes, one word.** Hero slots climb the hall ladder, so at a Keep you
field a Champion *and* a Priestess. `the hero` names the class, i.e. both:
`autocast at 3`, `retreat at 35% with the hero` and `focus siege` all apply to
both, which is what you want. The three verbs that take exactly **one** unit —
`escort`, `buy`, `use` — refuse instead of guessing, and tell you the words
that fix it:

```
  FAILED   'escort the hero with the footmen' -> 'the hero' is ambiguous — you
           have 2 heroes; say the champion or the priestess
```

Say `the champion` or `the priestess`. `buy a potion for the priestess` fills
in `buy`'s optional `hero` field for you; with only one hero alive it is
omitted, exactly as before. The Sorcerer is a caster but **not** a hero, so
`the hero` never sweeps it up — use `sorcerers`.

- `--explain` prints the full vocabulary. **Read it once**; it is the list of
  idioms that compile deterministically.
- Anything it does not know, write by hand. It is a convenience over the schema
  above, never a gate in front of it, and it never guesses: an unresolvable
  place or an unknown noun is a reported error, not a silent whole-army move.
- Conditionals ("strike when their hero falls") have no verb in this game —
  there is no trigger system. The tool defers them and prints the command to
  run when you see the condition in `events`.

**Check the round trip.** Every intent, from either seat, is logged as one
English sentence in `bridge/intent_log.jsonl`. If the sentence is not what you
meant, the compile was wrong — and you can see that before the army arrives.

## "Why are you doing that?" — `units[].why`

Every one of your units carries the reason for its current behaviour, and
reports it in the snapshot as a compact `why` string. The human player reads
the identical string in their selection panel; neither seat can ask a question
the other cannot.

| `why` | means |
|---|---|
| `order:move by bridge t=123` | you ordered it, at game second 123 |
| `order:attack by ui t=123` | the *human* ordered it — same verb, other seat |
| `posture:push sq1` | squad 1's standing posture is moving it |
| `policy:retreat t=210` | its retreat threshold fired; it is running home |
| `template:Barracks#42` | it spawned with that building's doctrine template |
| `rally:Barracks#42` | it spawned onto that building's rally point |
| `order:attackmove by script t=123` | the scripted AI ordered it (you are on `autopilot`) — the third seat, same verbs, same compiler |
| `instinct:auto-enroll` | nobody assigned it, so the engine pooled it in squad 0 |
| `idle` | nothing to do — it has no reason at all |

Use it. `why` is how you tell "my push stalled" (`posture:push sq1`, still) from
"my push dissolved" (half the squad reading `policy:retreat`), and how you catch
the classic mistake of ordering units one at a time and watching doctrine undo
it next second. Enemy units never carry it: their chain of command is their
plan.

The log ties the two together — an order's line in `intent_log.jsonl` carries
the same `why` string it stamped, so a unit's answer and the sentence that
caused it are one grep apart.

## The chain of command — read `link` before you micro

Only present when the match is played with command latency enabled. If your
snapshot has no `command_nodes` key and your units have no `link`, this whole
section is inert and you can ignore it.

When it IS on: **a direct order to a unit does not take effect the moment you
send it.** It arrives after a delay that grows with that unit's distance from
your nearest *command node* — a finished hall, or a living hero. Standing orders
do not pay: `squad`, `posture`, `retreat`, `leash`, `autocast`, `priority`,
`template` all take effect at once, wherever the unit is. So does anything
addressed to a building (`train`, `research`, `rally`, `upgrade`) and so does
`build`.

What to read, and what it means:

| Field | Meaning |
|---|---|
| `command_nodes: [{pos, radius}]` | Your own nodes. A unit inside one of these circles takes orders for free. Your team only — you do not get to read theirs. |
| `units[].link` | Seconds your *next* direct order to that unit would take to arrive. `0.0` = free. |
| `units[].pending` | `true` = an order you already sent is still travelling to it. |
| `applied: [{cmd, delay}]` | What the commands in your last batch actually cost, keyed by the same `cmd N` handle `errors` uses. Absent entries cost nothing. |

Three rules worth having before you learn them the hard way:

1. **Saying the same thing again does not restart the journey.** Re-sending an
   unchanged order to a unit with `pending: true` is a no-op, not a reset — so
   re-sending your whole batch each cycle is safe. Sending a *different* order
   replaces the one in flight and pays again from zero. Latency is the price of
   changing your mind at range, not a tax per command.
2. **`pending: true` with `why: "idle"` is not a lost order.** A unit that has
   finished its last task and not yet received the next one genuinely is idle,
   and says so. Look at `pending`, not at `why`, to know whether something is
   coming.
3. **Doctrine is strictly faster than micro at range, by construction.** If you
   are ordering units around at `link: 2.0`, you are playing two seconds behind
   the fight. Set a `posture` on the squad instead: it re-tasks them every second
   at machine speed, for free, wherever they are. Move a hero to the front and
   you buy a free-orders bubble around it — at the price of putting your most
   valuable unit in the most dangerous place.

Losing every hall AND every hero severs the arm: every order then pays the
maximum. That is a real way to lose a game that still looks winnable on paper.

## If your seat is `bridge/copilot`: you are a CO-COMMANDER

Everything above still applies — same 27 verbs, same snapshot, same fog, same
`bridge_send.py`. One thing changes, and it is the important one: **you are not
the faction.** A human is playing this faction with a mouse, and you are sitting
next to them. Your snapshot's top-level `copilot` block confirms it:

```json
"copilot": {"trust":"split",
            "direct":["priority","retreat","leash","autocast","squad","posture","template"],
            "propose_ttl":20.0,"max_pending":4,
            "severities":["routine","urgent"],
            "veto_reasons":{"not_now":"re-propose when conditions change",
                            "never":"do not re-propose this match",
                            "wrong_target":"re-propose with a different target"}}
```

**Read `direct` before you send anything.** Those verbs go through
immediately, no permission needed — they are standing orders, and a standing
order is advice: if your partner disagrees they set another one and the squad
re-tasks within a second. That is your half of the fight, and it is a big half.
Squad postures, retreat thresholds, focus-fire, leashes, autocast and
production templates let you keep the whole army fighting between your turns
without ever touching your partner's gold.

**Everything else you PROPOSE.** Unit orders, `build`, `train`, `upgrade`,
`research`, `buy`, `autopilot`, `surrender` — anything that spends the shared
treasury or commits the army — is wrapped:

```bash
python3 tools/bridge_send.py --seat bridge/copilot '[
  {"type":"propose",
   "note":"their catapults are unescorted - hit them now, we lose the window in 30s",
   "commands":[
     {"type":"attack","units":[41,42,43],"target":907},
     {"type":"train","building":88,"unit":"Raider"}]}]'
```

Send a bare `train` and it is refused with a message that shows you the
wrapper. Nothing is lost; re-send it wrapped.

**`"severity":"urgent"` jumps the queue.** Optional, `"routine"` by default. An
urgent proposal sorts ahead of every routine one already waiting, wears the
warning colour on your partner's screen, and is what their `[Enter]` takes
next. It does **not** raise the cap of four and does not let you propose
anything you otherwise could not — urgency buys attention, not trust. Spend it
on windows that close (`their siege is unescorted for ~15s`), not on plans
(`we should expand`). Mark everything urgent and you have marked nothing
urgent, and your partner will notice within a minute.

### Propose-first etiquette

1. **The `note` is the whole point.** The sentences say what would happen — the
   game compiles those for free and your partner reads them. The note says
   *why it is worth doing*, and it is the only part you actually write. "push
   mid" is useless. "their army is committed north, mid is undefended for ~20s"
   is a reason someone can agree or disagree with.
2. **Batch a plan, not a keystroke.** One proposal should be one idea, with its
   two or three commands together. Four proposals in flight is the cap, and a
   partner who has to answer four questions during a fight will answer none.
3. **Twenty seconds, then it lapses.** A lapsed proposal is not a rejection —
   it means your partner was busy. Check `events` for
   `proposal #N expired unanswered` and decide whether it is still true before
   re-sending; a directive about a fight that is over is noise.
4. **Read `proposals` in your snapshot.** It is your outstanding queue, in the
   order your partner will answer it — urgent first, then oldest — each with
   its `severity` and `expires_in`. `events` reports every outcome:
   `copilot proposes #1`, `proposal #1 approved (2 order(s))`,
   `proposal #1 vetoed (wrong target - re-propose with a different target)`,
   `proposal #1 expired unanswered`.
5. **A veto tells you WHY. Obey the reason.** Your partner picks one of three
   in the same keystroke, and they mean opposite things. Read it from
   `recent_resolutions` (or off the `events` line) and act on it:

   | `reason` | what happened | what you do next |
   |---|---|---|
   | `not_now` | good idea, wrong moment | **wait and re-propose when conditions change.** Not immediately — something has to have changed first, and your note must say what. |
   | `never` | drop it | **do not re-propose this idea this match.** Nothing in the engine stops you; this is etiquette, and re-sending it is how you become the thing nobody approves. |
   | `wrong_target` | the idea is right, the aim is wrong | **re-propose with a different target.** This is the one veto that is really a request — keep the plan, change the units, the ground or the objective. |

   Never re-send a vetoed batch unchanged. If you still believe a `not_now`,
   what failed was the argument, not the JSON — change the note.
6. **Watch the conflict tags.** When your batch touches units under your
   partner's squad, posture or a recent order, your partner sees a line like
   `re-tasks squad 1 (defend)` or `overrides your move on 4 unit(s), 6s ago`.
   Those get vetoed the most. If you are about to write one, say why in the
   note — you are asking them to abandon something they chose.
7. **Doctrine is how you stay useful between proposals.** Set retreat, priority
   and a squad posture early and your army fights well while your partner is
   answering something else. This is the single biggest difference between a
   helpful co-commander and a chatty one.

### What became of what you asked: `recent_resolutions`

`proposals` is only what is still open. The last eight that CLOSED are in
`recent_resolutions`, oldest first, each with your own note echoed back so you
can recognise the idea without having remembered its number:

```json
"recent_resolutions":[
  {"id":4,"t":81.2,"note":"expand to the north mine","severity":"routine",
   "outcome":"expired"},
  {"id":5,"t":94.0,"note":"hit their siege now","severity":"urgent",
   "outcome":"vetoed","reason":"wrong_target",
   "advice":"re-propose with a different target"}]
```

`outcome` is `approved`, `vetoed` or `expired` — there is no `pending`, because
being in `proposals` is what pending means. `reason` and `advice` appear on
vetoes only. Read this every cycle: it is the only place the *argument* your
partner made back to you survives, and re-proposing into a `never` is the
fastest way to stop being read.

If `copilot.auto_approve_after` is present, the thing answering you is a
**script**, not a person — a sim harness approving everything after that many
seconds. Nothing you read about vetoes will happen. Do not tune your play to it.

### Read your partner: `partner_log`

Your snapshot carries the last ~40 intents **anyone on your team** issued,
oldest first, each tagged with who wrote it:

```
  [   4.3s] copilot  5 units join squad 1
  [  17.0s] copilot  attack-move 5 units to (0.0, 0.0)
  [  29.6s] ui       12 units fall back to (-70.0, -70.0) below 35% health
```

`"ui"` is your human partner at the keyboard; `"copilot"` is you. Same English
the replay log writes. Read it every cycle — it is how you learn that they
already pulled the army back, that they are teching instead of pushing, or that
their last three clicks were refused. An entry with `"ok": false` bounced;
do not build a plan on an order that never landed.

`units[].why` completes the picture per unit: `order:move by ui t=123` is your
partner's doing, `order:move by copilot t=123` is yours, and a selection
answering two different ways is a squad that has been re-tasked mid-move.

Your `errors` array carries both authors' refusals for the same reason.

`WC3_COPILOT_TRUST=full` (everything direct) and `=strict` (everything
proposed) exist for experiments; the `copilot.trust` field tells you which one
you are in, so never assume — read it.

## What you can build/train: read `<SEAT>/catalog.json`
The FULL content catalog — every unit, building, ability, research and item:
costs, stats, train/build times, what produces what, and what gates it.
Read it ONCE at match start; it is the authoritative content reference.

**The catalog IS the tech tree — no prose needed.** `requires` on everything
lists what must be STANDING (trainer included, transitively) and `tier` says how
far up the hall ladder that puts you; `upgrades_to`/`upgraded_from` walk
TownHall→Keep→Castle with every price on it. T2 arrives ~min 3-5, T3 ~min 6-9.

The live snapshot's `unlocked` map answers "can I order this right now?" for
every catalog entry — for a unit that means the tech gates are met AND you have
a finished building standing that trains it, so `Footman` is false until a
Barracks is up. No cross-check needed; if `unlocked` says true, `build`/`train`
will accept it. Use catalog `requires` for PLANNING (what you'd need to build
first), `unlocked` for ACTING.

## The rules of the world (not in the catalog)
- **FOG OF WAR — read this before you read `units`.** Your snapshot shows only what your
  team can currently SEE, and it is the same rule the human player's screen obeys. So:
  - **An empty `units` list means "I have no information", not "there are no enemies."**
    Check the top-level `fog` object (`enabled`, `explored`, `visible`) before drawing any
    conclusion from silence. `explored: 0.1` means you have looked at a tenth of the map.
  - Enemy **units** appear only while you can see them, and are never remembered. An army
    that leaves your sight is simply gone from the snapshot — it has NOT died.
  - Enemy **buildings** you have scouted stay in `buildings` as remembered ghosts carrying
    `last_seen` (game time of the sighting). A ghost may be stale: the building may have
    been destroyed, or upgraded to a higher tier, since. `last_seen` present == memory;
    absent == you are looking at it right now.
  - `bounties` lists only caches you can SEE. Treasure you have no eyes on is invisible.
  - Still public and unfiltered: `map`, `mines` (position AND remaining gold), `trees_near`.
    Map geography is not a secret; what the enemy is DOING with it is.
  - You cannot `attack` an id you cannot see or remember — it is rejected as
    `target N is not visible`. Use `attack_move` to advance into the unknown.
  - **Scout deliberately.** Vision radius is per-kind in `catalog.json` (`vision`).
    Raiders see 24 and are the cheapest eyes on the map; Catapults see 14 but shoot 20, so
    unescorted siege is firing blind. Halls see furthest of all and grow with the tier.
- Map ±100. Your base corner and the enemy's are opposite. **Read `map` in your snapshot**:
  it names the layout, summarises what the ground does to a plan, and lists every `chokes`
  entry — the only gaps through impassable terrain. On a map with chokes, walls/towers at a
  ford are worth far more than anywhere else, and an army cannot be flanked off-road.
- **Win = destroy every enemy
  PRODUCTION building (TownHall, Barracks, Workshop)** — farms/towers/walls/shops don't
  count; killing the war-making capacity ends it.
- Workers harvest gold (mines) and lumber (trees) and auto-loop once ordered. ~70/30 gold/lumber split.
- **UPKEEP**: gold income is taxed by army size — supply ≤40: keep 100% of each delivery;
  41-70: 70%; 71+: 40% (`me.upkeep_rate` in your snapshot). Lumber untaxed. A huge idle
  army bleeds your economy — either use it or stay lean.
- **Mines are finite** (3500 each — they die around minute 10). The map runs dry; late-game armies are irreplaceable.
  Long passive games punish you twice (upkeep + exhaustion) — force the issue.
- **Bounty caches**: neutral treasure spawns in the contested middle every ~90s (watch the
  `bounties` list and event feed — but see fog above: you only see caches you have eyes on,
  so holding the middle is now also how you FIND them). First team to walk any unit onto one banks its gold —
  UNTAXED, and the value escalates WITHOUT LIMIT (~750 by min 20, 1000+ by min 30) —
  the map itself forces a decision. Ceding the middle
  cedes an economy. After the mines die, bounties are the only income on the map.
- Supply-block = production stalls: build Farms BEFORE you hit the cap.
- Your heroes level from ANY nearby enemy deaths, +HP/+damage per level. THE key units — keep
  them alive (retreat policy!), near fights (XP), and revive fast when one dies (keeps its
  level, per class). Two heroes at Keep means two retreat policies and two autocast rules.
- **HERO SLOTS SCALE WITH YOUR HALL TIER: TownHall 1, Keep 2, Castle 3.** A second hero is
  one of the two concrete things teching to a Keep buys you (the other is the Arcane Sanctum).
  Heroes must be of **distinct classes** — Champion *and* Priestess is legal, two Champions
  never is. Read `me.hero_slots` / `me.hero_slots_used` in the snapshot before you train:
  used counts living heroes **plus any hero already sitting in a queue**. `me.hero_records`
  lists every class you have ever fielded (with `alive`), and `me.hero_costs` prices each
  class — full freight for a class you have never played, the cheap revival price for one you
  have, per class and never discounted by the other. Only two classes ship today, so a
  Castle's third slot currently has nothing to put in it.
- Counter triangle: fortifications stop armies, siege outranges fortifications, fast cavalry
  dives siege. It is all data: catalog `units[].class` says what a unit IS, and
  `vs_building_mult` / `vs_siege_mult` / `vs_cavalry_mult` say what it eats. The
  multipliers are keyed off the CLASS, so a Spearman's anti-cavalry bonus lands on the
  Knight and the Raider alike. `damage` with `attack_cooldown` gives you dps.
- **Crowd control (tier 2).** An **Arcane Sanctum** (requires a Keep) trains **Sorcerers**:
  fragile, barely fight, and auto-cast **Slow** — -40% move AND attack speed, 5s, 9s cooldown,
  no mana. Slow is **thrown at a point up to 9 away and blooms 4.5 wide** (total reach 13.5),
  so a Sorcerer slows a charge from *behind* your own line instead of standing in it. It is
  the answer to a Raider or Knight charge (a slowed Raider is slower than a Footman) and the
  way you cover a retreat. Slow **refreshes rather than stacks**, so 2-3 Sorcerers buy
  frontage, not depth — a wall of them is wasted supply. Left on autocast they aim themselves
  at the biggest clump in reach; keep them behind the line and they will never walk forward
  to cast.
- Ranged units/towers outrange melee; footmen tank; workers fight terribly.
- Towers shoot at enemy units in range on their own; Walls just block pathing and soak hits.
  Defense is a real strategy — and SIEGE is its counter: check the catalog for what outranges towers.
- **Regeneration**: units heal ~1.5%/s after 12s out of combat; buildings ~0.5%/s after 20s.
  Wounded armies recover if you rotate them out — and a harassed base patches itself up.
- Workers automatically move to the nearest remaining node when theirs depletes — trust them.
- Units auto-fight what comes close (and chase). Doctrine makes them fight SMART. Set retreat + priority
  + autocast on your army early; re-issue for new units each cycle.

## Hard-won tactical lessons (from the first human-vs-LLM match)
1. Order harvesting in your FIRST batch — every idle second compounds.
2. Never leave your hero idle or low-HP near the front: set `retreat` on it the moment it spawns.
3. Don't trickle units into a fight one at a time — rally/pool them, attack in waves of 6+.
4. Buildings are worth 60 hero XP: razing an undefended expansion levels your hero safely.
5. Keep training workers (target 12-14) even during fights; economy wins long games.
6. Watch the EVT feed: "hostiles near base" means respond NOW, not next cycle.
7. **Army split is instantly fatal and the game gives no warning** (r10). Escorting an expansion
   and defending a main are 130 units apart; between the decision and the counterattack landing
   there is no signal, and the march home IS the game. Doctrine offers nothing between the two —
   a `defend` posture does not react to a base outside its radius.
   **The answer to defending two places is a scroll aimed at the OTHER one.** Keep a
   ScrollOfMassTeleport on the hero the moment you take a second base, and when the main is hit,
   fire it with `destination` set to the MAIN — not to wherever the army happens to be standing.
   That is the difference between a 14-second march and an instant one, and it is the only tool
   in the game that lets one army hold two places.
