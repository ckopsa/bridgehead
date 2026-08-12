# RTS Commander Briefing (LLM seat)

You command one faction of a Warcraft-3-style RTS through a file channel.
Your seat directory is given in your instructions as `<SEAT>` (e.g. `bridge/red`).

**If your seat is `bridge/copilot` you are a CO-COMMANDER, not the faction** —
a human is playing this side with you. Read all of this first, then the
co-commander section near the end, which is the only part that differs.

## The loop (event-driven — do not blind-sleep)
0. **`python3 tools/bridge_send.py --seat <SEAT> '[{"type":"ready"}]'`** — send this
   when you have read the map and set your opening. **The match clock does not
   start until every bridged seat has.** Until then your snapshot carries
   `waiting_for: ["red","blue"]` and `match_started: false`, `t` stays at 0, and
   nothing in the world moves — no mining, no building, no spawns.
1. `python3 tools/bridge_wait.py --seat <SEAT> --max 15` — blocks up to 15s but WAKES EARLY
   (~1-2s) the moment an event fires (attacks, losses, bounty spawns, your command errors)
   or the game ends. It prints why it woke.
2. `python3 tools/bridge_view.py <SEAT>/state.json` — compact readout (combine 1+2 in one
   bash call: `wait ... && view ...`). Add `--digest` for the ~15-line version:
   resources, your army by squad, your production queues, the enemy production
   buildings you have SEEN (the win-condition line), the last five events, any
   active alarms, and one line naming what happens if you send nothing this
   cycle. Add `--doc` instead for the whole affordance document: that digest,
   plus every command you could send right now — one line each, command
   included — with its precondition truth already computed. It is ~75 lines and
   affordable every cycle; `--doc --all` unfolds every field and domain (~650
   lines) and is the one to read once, at the start. See **The affordance
   document** below. All read the same `state.json` and none of them writes
   anything; the full readout is there whenever you want the ids the digest
   drops.
3. Decide. 4. Write commands (see below).
Repeat until `game_over` is non-null, then stop and report the result.

**How a match can end**, so you know what you are polling for. `game_over` is a
team name — `"Human"` or `"Claude"` — or the literal `"draw"`, and `game_over_reason`
beside it says which ending it was:

| `game_over_reason` | What happened |
| --- | --- |
| `razed` | the loser has no production buildings left |
| `surrender` | the loser conceded |
| `score` | **the match hit its time cap** and the referee counted assets — bank plus the gold-and-lumber worth of everything still standing. Ahead on that total wins; exactly level is `game_over: "draw"`. |

A capped match is not a stalemate you can wait out: it is scored, so being ahead
on assets when the clock runs out is a real way to win and being behind is a
real way to lose. Every ladder round runs under a cap.

**Read the map before you say `ready` — that is what the time is for, and your
opponent gets exactly the same amount of it.** You may look at everything and
send your whole opening batch *before* readying; those orders compile at t=0 and
your units act on the very first live frame. This is legal for both sides
equally, which is the entire point: the engine used to start the clock at
process start, so whichever commander connected second simply forfeited the
opening (arena round 9: Red's first order landed at t=41s). Now nobody does.
Dawdling is not an advantage either — you are only holding up a match that
starts for both of you at once, and `BH_READY_TIMEOUT` (default 120s wall)
starts it without you if you go quiet. Say `ready` once you have a plan.
`ready` is idempotent, never refused, and costs no chain-of-command latency.

**Never chain `bridge_wait` calls without a view between them.** Two waits back
to back is a blind sleep wearing the costume of an event loop: the first one's
wake is the news, and the second one throws it away unread. One wait, one view,
one decision — every cycle, even the boring ones. Arena round 17 was lost in a
~100 game-second gap made of chained waits, with 2280 gold banked and nothing
built. Waiting is not playing.

**A blocked plan is a decision prompt, not noise to sleep through.** `blocked:
cannot afford X` in `plans[].status` means your economy cannot pay for the
sequence you wrote. You are told **once**, when it starts and again if the
reason changes or when it clears — the retries in between are silent on
purpose, so the condition lives in `plans[].status` where you can read it
whenever you like. The two answers are *fix the economy* (workers on gold, a
Farm, a cheaper step) or `plan_clear` and write a plan you can afford. Sleeping
until the grace window expires is neither, and it ends with the plan `halted`
on the step you needed most.

**Check `alarms` on every view.** It is the short list of things the engine
thinks are worth a fresh decision — and each entry already tells you what is
being done about it if you do nothing. Everything not in that list defaults to
continue. See **Alarms** below.

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
- `{"type":"move"|"attackmove","units":[ids],"x":..,"z":..}` — or
  `{"type":"move","units":[ids],"region":"north-pass"}`. **Every verb below that
  takes `x`/`z` also takes `"region":"<name>"` instead**; see *Territory*. And
  **every verb below that takes `units` also takes `"select":"my hero"` /
  `"all army"` / `"squad 2"` instead**, resolved when the command runs rather
  than when you wrote it; see *Selectors*. Both channels are optional and both
  outrank the frozen form beside them.

- `{"type":"attack","units":[ids],"target":enemy_id}`
- `{"type":"harvest","units":[worker_ids],"target":node_id}` (mines AND trees — tree ids in `trees_near`;
  a mine with `"remaining": 0` is refused — say `"target_select":"nearest mine"` instead)
- `{"type":"return","units":[worker_ids]}`  `{"type":"stop","units":[ids]}`  `{"type":"follow","units":[ids],"target":own_id}`
Production:
- `{"type":"build","worker":id,"kind":"Farm"|"Barracks"|"TownHall","x":..,"z":..}` (site must be free)

  **You pay when the worker breaks ground, not when the command is accepted.**
  That is one sentence with three consequences worth knowing before you plan
  around it. (a) Bank the price for the whole walk — an expansion hall is a long
  trip and a Barracks queued in the meantime can spend the money out from under
  it. (b) An accepted build that dies before the foundation is laid — the worker
  re-tasked, killed, turned away at the site, or the site no longer clear —
  costs you **nothing**; there is no refund because there was no charge. (c) It
  is no longer silent: every one of those cases now writes a
  `build abandoned: <Kind> at (x, z) — <why>; nothing was spent` line into
  `events`, which is the edge you should be waking on. While the worker is still
  walking, the build is visible as `units[].build_site` (`{"kind","pos"}`) on
  the worker carrying it, and the digest prints it as `walking: Barracks@(12,-4)`.
  Once ground is broken it joins `buildings[]` with `"done": false` and
  `"build_progress"` (0.0–1.0, present only while it is going up — `progress`
  beside it is the *training queue's* and is `0.0` on any site).
- `{"type":"train","building":id,"unit":"Worker"|"Footman"|"Archer"|"Hero"|...}` (queue cap 7;
  full roster in `catalog.json`). A `train` of a hero is rejected with a reason when your slots
  are full, when you already hold that class, or when you cannot afford it — and the refusal
  says which: `only your FIRST hero is free` vs `reviving a class you have lost`.
- `{"type":"cancel","building":id,"index":n}` — **refunds in full** whatever had already been
  charged for that item, and nothing when nothing was charged (cancelling your free first
  hero returns 0; cancelling a 400g/100l revival returns 400g/100l).
- `{"type":"rally","building":id,"x":..,"z":..}` or `{"target":node_or_own_unit_id}` —
  read back as `buildings[].rally` on your own buildings (`{"pos":[x,z]}` or
  `{"target":id}`), so you never have to re-send one to find out what it is.
  Absent means no rally point is set. Never present on an enemy building.
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
  re-task themselves every second and ADVANCE COHESIVELY: a strung-out Push/Forage squad gathers before pressing on, so slow units set the pace and you arrive as one force.
  **Watch `squads[].status` for which of those two it is doing** — `"gathering"` means the
  squad is walking BACKWARDS to a regroup point behind its front, `"pressing on"` means it is
  walking at the objective. Same posture, opposite directions, and the key is the only thing in
  the snapshot that tells them apart. A gather that stops making progress gives up on its
  stragglers after about twelve seconds and presses on by itself, so a squad stuck on
  `"gathering"` is one whose tail keeps being refilled faster than it closes: stop training into
  it, or split the front off into its own squad. Absent on postures that are not walking
  anywhere (a defend ring, an escort, a forager with no treasure in sight). Defend postures are REACTIVE: enemies entering the radius
  pull the whole squad onto them. FORAGE squads continuously hunt the nearest bounty cache
  (attack-moving, so they fight what they meet) and hold at the muster point when none are up —
  the set-and-forget way to own the midfield. Squad 0 exists automatically (all army units
  enroll unless you assign them elsewhere; default posture defends your base).
- `{"type":"stance","squad":1,"stance":"push","target":"north-pass"}` — **the five doctrine
  verbs above, in one word.** A stance is a fixed engine-defined bundle of posture + anchor +
  leash + retreat threshold + focus priority, set atomically. Instead of `squad` + `posture` +
  `leash` + `retreat` + `priority`, send one line. See "Stances" below for the five and what
  each one is made of.
- `{"type":"template","building":id,"squad":1,"retreat":{"below":0.35,"x":..,"z":..},"priority":["Hero",...],"autocast":3}`
  — stamp standing doctrine on a production building: every unit it trains spawns WITH these
  policies (null/omitted pieces skipped; all null clears). Set once, stop re-issuing per spawn.
Triggers (CONTINGENT standing orders — see the section below; this is the shape):
- `{"type":"trigger_set","name":"home-guard","when":{...},"then":{<any intent>},"repeat":30}`
  — the engine watches `when` at 4 Hz and submits `then` itself. `repeat` omitted = fires once.
- `{"type":"trigger_clear","name":"home-guard"}` — disarm one. Omit `name` to clear all of them.
Plans (SEQUENCED standing orders — see the section below; this is the shape):
- `{"type":"plan_set","name":"opening","steps":[{"intent":{<any intent>},"advance":{...}},...]}`
  — the engine walks the sequence for you, submitting each step when its turn comes. `advance`
  omitted = "as soon as this one is accepted"; `{"type":"when","when":{<any trigger predicate>}}`
  = wait for that condition; `{"type":"after","secs":30}` = wait that long. Max 8 steps, max 2
  plans running, once through (no loops — that is what a trigger's `repeat` is for).
- `{"type":"plan_clear","name":"opening"}` — drop one. Omit `name` to drop all of them.
- `{"type":"autopilot","on":true}` — hand your whole faction to the scripted AI (emergency only).
- `{"type":"surrender"}` — concede the match (opponent wins immediately). The honorable end to a
  hopeless position — no income, no army, no path back. Preferable to dragging out a decided game.
- `{"type":"ready"}` — start the clock (see step 0 of the loop). The one verb that is legal
  *before* the match exists. Idempotent and never refused, so re-sending it after a reconnect
  costs you nothing.

## Stances: one word for a whole doctrine

Five fixed words. Each sets a squad's posture, its anchor, its leash, its
retreat threshold and its focus-fire list in one sentence, using the same
machinery the individual verbs use — the engine cannot tell a stanced squad from
a hand-tuned one, and neither can your opponent.

```json
{"type":"stance","squad":1,"stance":"push","target":"north-pass"}
{"type":"stance","squad":2,"stance":"secure","x":-60.0,"z":60.0}
{"type":"stance","squad":0,"stance":"turtle"}
```

`target` is a place name — anything from `map.places` or your own `regions`, and
`"region"` is accepted as a synonym. Give `x`/`z` instead if you prefer numbers.
Omit the anchor entirely and it is **your own base**, which is what `turtle`
means anyway.

| stance | what it does | ring | leash | falls back at | focus |
|---|---|---|---|---|---|
| `turtle` | hold home tight | defend r14 | 20 | 45%, to the anchor | Siege, Cavalry |
| `stage` | gather forward and wait | defend r10 | 16 | 40%, to the anchor | — |
| `push` | commit to the objective | push | none | 25%, to your base | Siege, Archer, Hero |
| `secure` | hold ground away from home | defend r30 | 38 | 35%, to the anchor | Siege, Cavalry |
| `harass` | hit soft targets, leave before the trade | push | 44 | 55%, to your base | Worker, Siege |

Three things worth knowing:

1. **Switching replaces the whole bundle**, never merges. `turtle` then `push`
   leaves no leash behind — which matters, because a leash you did not know was
   there would recall your push twenty metres from home.
2. **The ring is the stance's, not the region's.** Unlike `posture defend`, a
   named region's own radius does NOT become the ring: `secure north-pass` gets
   `secure`'s 30 whatever size you drew the circle. That is what makes a preset a
   preset. Use `posture` when you want to pick the number yourself — the full
   vocabulary stays open and nothing here can be expressed only as a stance.
3. **Reinforcements inherit it.** A stance belongs to the *squad*, not to the
   bodies that were standing in it when you sent the word: enrol a unit into a
   stanced squad and it arrives wearing the posture, the leash, the threshold and
   the focus list. That holds however it got there — a `squad` command, a
   `template` at the barracks, or the engine's own enrolment into squad 0 — and
   moving a unit between two stanced squads swaps the whole bundle, exactly as
   switching a squad's word does. So you can stance an empty squad and train into
   it, and sending `squad` and `stance` **in the same batch works** too — the
   stance sees the enrolment the line above it made.

   The limit: a squad you tasked with `posture` rather than a stance has no word
   to hand down, and `leash`/`retreat`/`priority` name units rather than squads,
   so joiners there are left as they are. If you want doctrine to follow the
   squad, use a stance.

**Silence continues a stance.** Nothing in the engine ever clears one — not a
poll you skipped, not a fight, not a wipe. Your next snapshot echoes it back as
`squads[].stance`, so "what is my army doing?" is answerable without remembering
what you said four minutes ago, and a cycle in which you send nothing is a cycle
in which your doctrine keeps running. The one thing that ends a stance is you:
send a different one, or hand-task the squad with `posture` (which clears the
word, because it is no longer true).

An unknown word is refused with all five listed, and installs nothing.

## Territory: name the ground once, then speak in names

Your snapshot hands you two vocabularies of named circles. Anywhere a verb takes
`x`/`z`, send `"region":"<name>"` instead and the engine resolves it when the
command arrives.

**`map.places`** — the map's own names. Read-only, public (your opponent reads
the same list), and live from second zero with nothing armed:

| name | what |
|---|---|
| `our base` / `their base` | the two starting halls. Seat-relative: same words in both snapshots, different coordinates |
| `mid` | the map centre, where bounties spawn |
| `northwest mine`, `southeast mine`, … | one per gold mine, named for its compass corner |
| `northwest ford`, `center ford`, … | one per chokepoint. Empty on `open`; three on `crossings` |

**`regions`** — circles YOU named, up to **8**. These are doctrine, not
information: they appear in your snapshot only, and naming ground tells your
opponent nothing.

```json
{"type":"region_set","name":"north-pass","x":-60.0,"z":60.0,"radius":20.0}
{"type":"region_clear","name":"north-pass"}      // or {} for all of them
```

Re-using a name **moves** that circle rather than spending a slot. Radius must
be 4..60. You may not take a name `map.places` already owns.

**What each verb does with a region:**

| verb | region means |
|---|---|
| `move` / `attackmove` / `build` / `rally` | go to / build at the centre |
| `posture` `defend` | centre is the anchor **and the region's own radius is the ring** — omit `radius` and stop carrying the number around |
| `posture` `push` | push to the centre (a push has no radius) |
| `posture` `forage` | the centre is the muster point |
| `stance` | the centre is the anchor. The region's radius is **ignored** — a stance's ring is its own (see "Stances") |
| `leash` | anchor at the centre; omit `radius` and the region's own is the leash |
| `retreat` | fall back to the centre |

**Why bother.** Two reasons, and the second is the one that wins matches:

1. Your log becomes readable. `squad 2 defends north-pass` instead of
   `squad 2 defends (-60.0, 60.0) within 20`.
2. **The engine resolves the name, not you.** A standing order or an armed rule
   that says `north-pass` keeps meaning *the pass* — so one `region_set` moving
   the circle re-aims every posture and every trigger that mentions it. That is
   one command instead of five, at zero polls.

A name you have not defined is refused with the list of names you do have, so a
typo costs you one error line rather than a silent order to the map's centre.

## Selectors: name a ROLE once, and stop plumbing ids

Regions are late-bound *places*. Selectors are late-bound *roles*, and they work
the same way: the phrase travels in the command, and the engine resolves it when
the command is **compiled** — which for a trigger's `then` and a plan's step is
when it **fires**, not when you wrote it.

Anywhere a verb takes `units`, send `"select":"<phrase>"` instead:

| phrase | means |
|---|---|
| `"my hero"` | every living hero of yours (`"hero"`, `"heroes"` are the same phrase) |
| `"all army"` | every living non-worker of yours, heroes included (`"army"`) |
| `"all units"` | every living unit of yours, workers included |
| `"workers"` | every living worker of yours |
| `"idle workers"` | the same, narrowed to the ones whose `order` is `"Idle"` — the number the digest prints as `workers 12 (idle 3)` |
| `"squad 2"` | the members squad 2 has **right now** |

Three more phrases answer "which one", not "which units":

| phrase | where | means |
|---|---|---|
| `"nearest worker"` | `build`'s `"select"` | the worker with the shortest walk to the site. Only `build` names ground for it to measure from; any other verb refuses it and tells you to say `"workers"` or `"idle workers"` |
| `"nearest tree"` / `"nearest mine"` | `harvest`'s `"target_select"` | the nearest live node of that kind to the workers you are sending |
| `"nearest legal site"` | `build`'s `"site"` | move the footprint to the nearest legal one within 15 of the point you named — preferring ground clear of the haul lanes between your halls and the mines they work, so your own base cannot wall in its miners — instead of refusing |

And a fourth channel names a **building**, for the six verbs that act on one —
`train`, `template`, `rally`, `cancel`, `upgrade`, `research`. Send `"select"`
instead of `"building"`:

| phrase | means |
|---|---|
| `"my barracks"` | your own FINISHED buildings of that kind. Any `catalog.buildings[].id`, singular or plural: `"barracks"`, `"my farms"`, `"war mill"` |
| `"idle barracks"` | the same, narrowed to the ones with **nothing in the training queue** |
| `"my hall"` | whichever rung of the hall ladder you have standing — TownHall, Keep or Castle. Use this rather than `"my town hall"`, which stops matching the moment you upgrade |

All six act on exactly one building, so they take the **lowest-id
match**. `"idle"` is the one to reach
for in a repeating rule: it walks past a producer that is already working, and
when they are all working it refuses in words rather than queueing six deep.

Case, spaces, dashes and underscores are noise, exactly as in every other name
on this wire. A phrase that is not one of these is refused with the list. A
phrase from the *wrong* channel is refused with the right one —
`{"type":"train","select":"my hero"}` earns `'my hero' names units, not a
building`.

Where each channel lives:

```json
{"type":"move","select":"all army","region":"north-pass"}
{"type":"attackmove","select":"squad 2","x":40,"z":-12}
{"type":"retreat","select":"all army","below":0.35,"region":"home"}
{"type":"priority","select":"all army","classes":["Archer","Hero"]}
{"type":"squad","select":"all army","id":1}
{"type":"harvest","select":"workers","target_select":"nearest tree"}
{"type":"follow","select":"all army","target_select":"my hero"}
{"type":"build","select":"workers","kind":"Farm","region":"home","site":"nearest legal site"}
{"type":"cast","select":"my hero","ability":"Slam"}
{"type":"train","select":"idle barracks","unit":"Footman"}
{"type":"train","select":"my hall","unit":"Worker"}
{"type":"rally","select":"my barracks","region":"north-pass"}
{"type":"template","select":"my barracks","squad":2}
{"type":"cancel","select":"my barracks","index":0}
{"type":"upgrade","select":"my hall"}
{"type":"research","select":"idle blacksmith","upgrade":"attack"}
```

`"upgrade":"my hall"` is the one worth learning by heart. A hall's entity id
changes when it is razed and rebuilt, and a plan written before the match has no
id to write down at all — so the phrase is what makes a tier-up something a
chain can contain.

`cast` and `follow`'s `target_select` need exactly one unit, so they take the
**lowest-id match** — the same documented tie-break `buy` and `use_item`
already use for an omitted `hero`.

**`build` is the exception, and you want it to be.** Its `"select"` picks a body
that has to *walk*, so lowest-id was the wrong answer and it cost two arena
rounds: with fifteen workers, `{"select":"workers"}` handed every build in a
batch the same peasant, and each new order abandoned the previous foundation
before ground was broken — three expansions and a Blacksmith gone, no error
line, no building. The rule now, in order:

1. an explicit `"worker": <id>` always wins — naming a body means "that one,
   even though it is busy";
2. workers **already walking to a build site** are excluded, including one
   claimed by an earlier `build` in the same batch. If the phrase reaches
   nobody else, the command is **refused in words** rather than stealing a
   builder;
3. an **idle** worker beats a working one (`"nearest worker"` is how you say
   "distance, never mind idleness");
4. then the shortest walk to the site;
5. then the lowest id, so a seeded match replays identically.

So a batch of three `build`s now uses three different workers, and a repeating
build trigger no longer cannibalises its own previous order.

**Three rules worth knowing before you rely on it:**

1. **A selector outranks the `units` list beside it.** Send both and the phrase
   wins, and the stale ids are not even reported. Send neither and nothing has
   changed: `"units":[…]` still means exactly what it always did.
2. **An empty match is a refusal, not a silent nothing.** `"select":"all army"`
   with no army alive orders *nobody* and tells you so — `'all army' matches
   none of your units right now — nothing was ordered`. You cannot express
   "move 0 units", which is the exact command that got a hero killed in r21.
   In a plan, that refusal **blocks the step** rather than skipping it.
3. **Nothing is written back.** The armed rule still says `"my hero"` on its
   hundredth firing. Your hero can die, be revived with a brand-new id, and the
   same rule keeps working.

**One caveat.** A `squad` command and a `"select":"squad 1"` in the **same
batch** do not see each other — enrolment lands after the batch compiles. Send
the enrolment, then use the selector next cycle; or address the squad by number
with `posture`, which never needed a roster. You get the ordinary
"matches none of your units right now" refusal, never a stale roster.

**What this replaces.** Everything you used to do with id plumbing: re-reading
`units[]` every poll to refresh a `priority` list; arming a hero-save rule
against an id that becomes a corpse; memorising a tree id that gets chopped
down; a `build` trigger on fixed coordinates that reports `site blocked` for the
rest of the match. Write the role once and stop.
## Alarms: the four things worth re-deciding

Everything else defaults to **continue**. Silence is a legal, and usually
correct, move — your squads keep their postures, your triggers keep watching,
your plans keep stepping. The engine forces a fresh choice on exactly four
conditions, and it puts them in your snapshot's `alarms` array:

| `id` | fires when |
| --- | --- |
| `enemy_army_sighted` | a body of enemy troops at or above the threshold is in **your `intel` ledger**, seen recently |
| `squad_below_half` | one of your own squads is under half its pooled health |
| `income_collapse` | your gold has stopped — every mine your halls work is dry, or nobody is on gold |
| `places_under_attack` | your buildings are being hit in **two or more places at once** |

```json
"alarms": [
  {
    "id": "places_under_attack",
    "fact": "2 places under attack at once: near our base (Barracks, TownHall); at (60, 60) (Farm)",
    "running_default": "your trigger home-guard fired at t=203 — squad 1 is closing on near our base (ETA 22s)",
    "since_t": 209.0,
    "severity": "critical",
    "eta_s": 22.0
  }
]
```

**Read `running_default` first, every time.** An alarm never acts and the sim
never waits for your answer. It fires only *after* the fast tier — your
doctrine, your armed triggers — has already responded, and `running_default` is
that response, named. If you say nothing, that is what happens. A commander
that only ever confirms its defaults is playing acceptably; the alarm exists so
that confirming is a decision instead of an accident.

When nothing is standing, `running_default` says so in those words — *"nothing
recovers this automatically"*, *"nothing pulls them out (no retreat threshold
set)"*, *"nothing is moving to either place"*. That sentence is the most useful
one in the array: it is the difference between a raid being handled and a raid
being watched.

`eta_s` is present only when the running default is something walking. It is
**absent**, not zero, when nothing is in transit — "nothing is moving" and "it
arrives now" are opposite answers.

**Levels and edges.** The array is a *status*: an entry is there for exactly as
long as its condition holds, refreshed every sweep, so a slow poller reads what
is true now rather than what was true when it fired. The transitions are in
`events` — one `alarm: <fact> — default: <what is happening>` line on the way
in, one `alarm clear: <name>` on the way out, and **nothing in between**. An
alarm that is still true is not new news. `bridge_wait.py` wakes you on the
edges; the array answers "and is it still going on?".

**What an alarm is not.** It is not a first responder. You answer at tens of
seconds; anything with a shorter deadline has to live in doctrine or a trigger,
which is what the section below is for. The right reading of an alarm is
policy-shaped and stays valid over a thirty-second window — *"full recall, or
sacrifice the expansion?"* is still the right question a minute later; *"dodge
the ambush"* never was. Arm the fast answers ahead of time and spend your polls
on the slow ones.
## Triggers: make the engine react for you

**This is the single biggest thing you can do about your own latency.** You poll
every ~15 seconds. Between polls the engine runs the game. Doctrine already
fights for you continuously; a trigger makes it *react* for you — a condition it
checks four times a second and an intent it submits the instant the condition
holds. The order lands in the frame the rule fired, and it **pays no command
link** (see the chain-of-command section) because you reached the unit when you
armed the rule, not when it fired.

Arm your standing plans at the top of the match and stop spending polls on
situations you already know how to answer. (An *alarm* in this brief means the
`alarms` array above — a prompt to re-decide. A trigger is how you answer one
before it is raised.)

Rules: max **8** armed at once. Re-using a `name` replaces that rule in place
(free — the cap counts names, so tune all you like). A trigger cannot arm or
clear another trigger. Your armed rules come back in the snapshot's `triggers`
array with `status` (`armed`/`cooling`/`spent`), `last_fired`, and the English
`sentence` — and every fire writes a line into `events`:

    trigger home-guard fired: squad 1 defends (-70.0, -70.0) within 26

### The fourteen predicates

| `when` | means |
|---|---|
| `{"type":"base_under_attack"}` | any of YOUR buildings damaged in the last 8s |
| `{"type":"hero_below","frac":0.35}` | any of your living heroes under that fraction |
| `{"type":"hero_above","frac":0.8}` | **every** living hero of yours at or above that fraction, and you have at least one. "My hero is healed" — the wait-condition half of the pair, and **not** the negation of `hero_below`: with no hero alive this is **false**, so a chain waiting on it never advances over a corpse |
| `{"type":"squad_below","id":1,"frac":0.5}` | squad 1's POOLED health under that (false if the squad is empty) |
| `{"type":"enemy_sighted","class":"Siege","count":3}` | you can SEE that many enemies now (`class` optional; fog-honest) |
| `{"type":"enemy_in","region":"north-pass","count":5}` | you can see that many enemies **inside a named place** (`class` optional; fog-honest both ways — see *Territory*) |
| `{"type":"enemy_army_seen","size":6}` | your **intel ledger** holds 6+ enemy troops seen as one force. Optional `"within_s":30` demands a fresh sighting rather than a remembered one. Workers never count |
| `{"type":"enemy_hero_down"}` | an enemy hero you **watched die** and have not seen alive since. Optional `"class":"Hero"` / `"Priestess"` for just one |
| `{"type":"bounty_spawned"}` | a cache you can see is on the map |
| `{"type":"mine_dry"}` | a dry gold mine within 40 of one of your finished halls |
| `{"type":"supply_capped"}` | no free supply left — `supply_used` + everything already in your training queues has reached `supply_cap`. False while `supply_cap` is 0 (that is "no base yet", not "blocked") |
| `{"type":"tier_reached","tier":2}` | your tech tier |
| `{"type":"unit_count","kind":"Footman","count":8}` | your living count of one unit kind |
| `{"type":"game_time","at":360}` | the match clock, in seconds |

`enemy_army_seen` vs `enemy_sighted` is the difference between memory and
sight. `enemy_sighted` is true only while you have eyes on them, so it goes
false the moment your scout dies — which is what the scout was killed for.
`enemy_army_seen` reads the ledger and stays true.

`enemy_hero_down` is a **level** predicate, not an edge: it means "as far as I
know, their hero is down". Arm it `once` (no `repeat`) and it fires on the
first sweep after you witness the death and then disarms, which is what "when
their hero falls" means. Give it a `repeat` and it re-fires for as long as the
belief stands — "keep pressing while they have no hero" — which is a different
and equally legitimate order. If you see the hero alive again after a revive
the belief flips back and a re-armed rule can fire on the next death you watch.

There is still nothing here about the enemy's gold, their tech, or their hero's
**health**. Those are facts no human can obtain — you cannot select an enemy
hero, so no number about one has ever been on anybody's screen — so no
predicate can read them. That their hero *died in front of you* is a different
matter, and that one you may have.

### Seven recipes worth arming in your first batch

**1. Home guard** — the army comes home when the base burns. Repeating, because
a base is raided more than once.

```json
{"type":"trigger_set","name":"home-guard","repeat":30,
 "when":{"type":"base_under_attack"},
 "then":{"type":"posture","id":1,
         "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}}}
```

**2. Hero save** — the hero walks out before it dies. Your hero is a command
node, a hero slot, and — if it was your free first one — the only thing you own
whose whole price is charged at the moment you lose it (400g/100l to bring it
back). It is the most expensive single event in a match, and it happens inside
one poll cycle. Arm this before your first fight.

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

**4. The counter-punch** — the sentence this whole layer was named after. Their
hero is the most expensive thing on their side of the map; the window after it
dies is the one moment their army is worth less than yours. Once, because you
only get to spend that window once.

```json
{"type":"trigger_set","name":"their-hero-down",
 "when":{"type":"enemy_hero_down"},
 "then":{"type":"posture","id":1,
         "posture":{"type":"push","x":-70.0,"z":-70.0}}}
```

`tools/intent_compile.py` writes exactly that from `strike when their hero
falls` — arm the squad first (`squad 1 is the army`) or let the tool mint one.

**5. The doorbell** — react to a force you scouted, not to one you can still
see. Repeating, because an army that is answered comes back.

```json
{"type":"trigger_set","name":"army-6","repeat":45,
 "when":{"type":"enemy_army_seen","size":6,"within_s":30},
 "then":{"type":"posture","id":1,
         "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}}}
```

**6. Hold the pass** — the territorial rule, and the one that most repays a
region. It answers a question you would otherwise have to ask every poll ("is
anything at my ford yet?") and it answers it four times a second.

```json
{"type":"region_set","name":"north-pass","x":-60.0,"z":60.0,"radius":20.0}
{"type":"trigger_set","name":"pass-watch","repeat":30,
 "when":{"type":"enemy_in","region":"north-pass","count":5},
 "then":{"type":"posture","id":2,
         "posture":{"type":"defend","region":"north-pass"}}}
```

Read that action: no coordinates and no radius. The squad defends the circle at
the circle's own size, and if you later decide the pass is somewhere else, ONE
`region_set` moves both the rule and the posture it fires.

`enemy_in` is fog-honest in both directions: it counts only enemies you can see
AND that are inside the shape. A region is ground you are *watching*, not a
sensor — so keep something with vision near a pass you are counting on. If you
`region_clear` a region a rule names, that rule goes quiet rather than firing on
the whole map.

**7. The supply valve** — the rule that plays for you while you are reading.
Repeating, because you will hit the cap again at 40, at 60, at 80. Arena round
17 was lost at 28/28 supply with 2280 gold in the bank: not a fight, not a
tech choice, just a commander who did not notice a number. This is the number
noticing itself.

```json
{"type":"trigger_set","name":"supply-capped","repeat":45,
 "when":{"type":"supply_capped"},
 "then":{"type":"build","worker":<worker id>,"kind":"Farm","x":-58.0,"z":-64.0}}
```

`tools/intent_compile.py` writes it from `whenever we are supply blocked, build
a farm`. It counts your **queues**, so it fires as production stalls rather
than a mining trip later — and it stays false while `supply_cap` is 0, so it
cannot go off before you have a hall. Pick a site with room around it; a build
that is refused reports into `errors` tagged `trigger:supply-capped`, and the
repeat means it tries again rather than giving up. `supply_capped` is also a
legal plan `advance`, so `{"advance":{"type":"when","when":{"type":"supply_capped"}}}`
means "…then, once we are capped, the next thing".

A caution that applies to all seven: `then` is frozen when you arm it, so ids init are ids that may die. Prefer `posture` on a **squad** over a list of unit ids
where you can — a squad survives its members. And a trigger whose action is
refused when it fires reports that in your `errors` array tagged
`trigger:<name>`, so check there if a rule seems to do nothing.

## Plans: make the engine run your build order

**This is the second biggest thing you can do about your own latency, and it is
the one that wins the first six minutes.** A trigger deletes the cost of
*reacting*. A plan deletes the cost of *transcribing* — the build order you had
already decided before the match started and were going to feed the engine one
command per poll, at ten to fifteen seconds a command, while your opponent's
economy compounded.

A plan is named ordered steps. The engine submits step 1, waits for that step's
`advance` condition, submits step 2, and so on. Once through, then it is done.

Rules: max **2** plans running, max **8** steps each. Re-using a `name` replaces
the plan and restarts it from step 1 (free — the cap counts live plans). A plan
step may arm a `trigger_set`, but a plan may not set another plan. Your plans
come back in the snapshot's `plans` array with `step`/`of`, `status`, the
current step's `sentence`, and the whole `steps` list as the JSON you sent — so
you can read one out, change a number, and send it back under the same name.

Every step writes a line into `events` as it goes out:

    plan opening step 2/5: worker 4294968100 builds Sanctum at (58.0, 66.0)

### The three advance conditions

| `advance` | the plan moves on |
|---|---|
| omitted (or `{"type":"on_applied"}`) | the moment this step is ACCEPTED — the plain meaning of "then" |
| `{"type":"when","when":{...}}` | when that condition holds — **any of the fourteen trigger predicates above**, including the intel ones |
| `{"type":"after","secs":30}` | 30 seconds after this step was accepted |

"Accepted" means the order was legal and taken, NOT that the building finished.
To wait for something to finish, use `when` with `tier_reached` or `unit_count`.

### If a step is refused

The plan **blocks on that step and never skips it**. Its `status` becomes
`blocked: <the exact compiler error>`, it retries the same step every 5s, and
if it is still refused a minute later the status becomes `halted: <error>` and
the plan stops for good — on the step that failed. Both are also announced in
`events`. So a plan is never quietly wrong: it is either running, done, or
telling you which step it is stuck on and why.

The minute is deliberate: the commonest refusal a plan meets is "cannot afford
it yet", and money arrives on a scale of tens of seconds, so the engine keeps
trying long enough for your economy to answer. You are told at the *first*
bounce either way — `blocked:` shows up within a tick — so if the reason is
permanent ("you have no Sanctum") you can `plan_clear` it immediately rather
than waiting for the halt.

**You are told on the EDGES, not on every retry.** One line in `events` and one
entry in `errors` when the step first bounces; one more if the reason *changes*
to different words; one `plan <name> step k/n unblocked` when it recovers; one
on `halted`. The twelve retries in between say nothing at all — they are the
engine trying, not new news, and a channel that repeats itself is a channel you
learn to skip. The condition itself is never hidden: `plans[].status` reads
`blocked: <why>` in every snapshot for as long as it is true. Read the status
to know; the `errors` array is only there to *interrupt* you.

(This is also why `bridge_wait.py` no longer wakes on an error it has already
shown you. A stuck plan used to wake it every couple of seconds — which is what
made chaining waits look attractive in round 17, and lost that match.)

A step that reaches SOME of its units is a partial success, not a refusal: if
one member of a listed squad has died, the survivors are still ordered, the dead
id is still reported in `errors`, and the plan carries on. Only a step that
reaches nothing blocks.

### If a step's `when` names a place you have not named

`{"type":"when","when":{"type":"enemy_in","region":"staging",...}}` is legal in a
plan step **before** `staging` is a place — you may `region_set` it later by
hand, or in an earlier step of the same plan. `plan_set` arms it and tells you
which step is holding, in the same words a late-bound target gets:

    cmd 1: chain holds at step 2: no region named 'staging' - known places: … —
    plan commit is armed anyway; the step waits there until the place is named

If the step's turn comes and the name is still not a place, the plan's `status`
becomes `held: no region named 'staging' - known places: …` and one line goes
into `events`. **`held` is not `blocked`**: the step's order already ran, so
nothing is retried and nothing halts — the plan simply waits, for as long as you
leave it, and says `plan <name> step k/n no longer held` on the sweep after you
name the ground.

(A **trigger** is stricter: `trigger_set` refuses an unknown `enemy_in` region
outright, because a trigger has no earlier step to name ground in and re-arming
it costs you one line.)

### THE CANONICAL EXAMPLE: the boomer opening as one plan

Economy first, workers second, tech third, army buildings last — sequenced so
each step waits for the thing it needs. **This is one command instead of six
polls.** Every id in it exists on your very first snapshot (five workers, a
TownHall, the mines and trees arrays), which is the point: send it before you
have done anything else.

This exact plan has been run against a live seat and completes in about a
minute of game time. It is not a sketch.

```json
{"type":"plan_set","name":"boomer","steps":[
  {"intent":{"type":"harvest","units":[<worker A>,<worker B>,<worker C>],
             "target":<your nearest mine>}},

  {"intent":{"type":"harvest","units":[<worker D>,<worker E>],
             "target":<a tree from trees_near>}},

  {"intent":{"type":"train","building":<your TownHall>,"unit":"Worker"}},

  {"intent":{"type":"train","building":<your TownHall>,"unit":"Worker"},
   "advance":{"type":"when","when":{"type":"unit_count","kind":"Worker","count":7}}},

  {"intent":{"type":"upgrade","building":<your TownHall>},
   "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}},

  {"intent":{"type":"build","worker":<worker A>,"kind":"Barracks","x":-58.0,"z":-58.0}}]}
```

Read it as English — it is exactly what the replay log will write:

> plan boomer (6 steps): 3 units harvest node …, then 2 units harvest node …,
> then building … trains Worker, then building … trains Worker, then when we
> field 7 or more Worker: building … upgrades to its next tier, then when we
> reach tier 2: worker … builds Barracks at (-58.0, -58.0)

and here is what the engine actually wrote while running it:

```
[    3.3] plan boomer step 1/6: 3 units harvest node 4294967365
[    3.5] plan boomer step 2/6: 2 units harvest node 4294967775
[    3.8] plan boomer step 3/6: building 4294968193 trains Worker
[    4.0] plan boomer step 4/6: building 4294968193 trains Worker
[   19.8] plan boomer step 5/6: building 4294968193 upgrades to its next tier
[   59.8] plan boomer step 6/6: worker 4294968174 builds Barracks at (58.0, 58.0)
[   60.0] plan boomer complete (6 steps)
```

Four things to copy from this, because all four are easy to get wrong:

- **The advance-condition of a step governs the move to the NEXT step.** The
  `when tier_reached 2` sits on the `upgrade` step, and what it does is make the
  *Barracks* wait for the Keep to finish.
- **`train` queues ONE unit.** Two workers is two steps. A single `train` step
  under `unit_count >= 7` is a plan that waits forever — and it will sit there
  reporting `running`, perfectly honestly, because nothing checks a wait against
  a world that will never satisfy it.
- **Harvest FIRST, and harvest lumber too.** The first version of this example
  skipped the wood and halted on `cannot afford Keep (320g 160l)` — with plenty
  of gold and 150 of the 160 lumber. A plan spends real resources on a real
  clock; if nothing is mining, the tech steps cannot pay.
- **Put the cheap, always-legal steps early.** Steps that just re-task units
  (`harvest`, `posture`, `stance`, `squad`, `template`) cannot be refused for money, so a
  plan that opens with them starts earning while the expensive steps wait.

### The idiom for units you do not have yet

**A step's unit ids are frozen when you set the plan.** You cannot write "the 8
footmen I will have by then" — those units do not exist and have no ids.

Two answers, and you will usually want both:

**Send a selector.** `"select":"all army"` or `"select":"squad 2"` in place of
`units` resolves when the step runs, so a step written before the army exists
still finds it:

```json
{"intent":{"type":"attackmove","select":"squad 2","region":"their-base"},
 "advance":{"type":"when","when":{"type":"unit_count","kind":"Footman","count":6}}}
```

**And name a SQUAD in the doctrine.** `template` stamps every
unit a building trains into squad 2; `posture` addresses squad 2 by number and
resolves its membership when the step runs:

```json
{"type":"plan_set","name":"army","steps":[
  {"intent":{"type":"template","building":<your Barracks>,"squad":2}},
  {"intent":{"type":"train","building":<your Barracks>,"unit":"Footman"}},
  {"intent":{"type":"train","building":<your Barracks>,"unit":"Footman"}},
  {"intent":{"type":"train","building":<your Barracks>,"unit":"Footman"},
   "advance":{"type":"when","when":{"type":"unit_count","kind":"Footman","count":3}}},
  {"intent":{"type":"posture","id":2,
             "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}}}]}
```

Prefer squad-addressed steps over id lists everywhere in a plan — a squad
survives its members, and a plan runs long enough for members to die. (A step
that names a list and loses one member to a corpse is a *partial* success: the
survivors are ordered, the dead id is reported in `errors`, and the plan carries
on. Only a step that reaches nothing blocks.)

**Why this is a SECOND plan and not steps 7–11 of the first one.** The Barracks
does not exist yet when you set `boomer` — step 1 is what builds it — so it has
no id for a `train` step to name. Squads close this gap for units, because
membership is late-bound by number; there is no equivalent handle for buildings.
So: send `boomer` on your first poll, and send `army` on the poll after the
Barracks shows up in your snapshot. Two plans is exactly the cap, and this is
what the cap is for.

### Chains: a plan whose steps are stances

**There is no `stance_plan` verb, because there does not need to be.** A *chain*
is an ordinary plan whose steps are `stance` sentences and whose `advance`
conditions are the waits you would otherwise have spent a poll discovering:

> turtle until the hero is healed, then secure the northwest mine

```json
{"type":"plan_set","name":"hold","steps":[
  {"intent":{"type":"stance","squad":1,"stance":"turtle"},
   "advance":{"type":"when","when":{"type":"hero_above","frac":0.8}}},

  {"intent":{"type":"stance","squad":1,"stance":"secure",
             "target":"northwest mine"}}]}
```

Two steps, **zero entity ids, zero coordinates**. `squad 1` is late-bound by
number, so it means whoever is in the squad when the step runs; `northwest mine`
is late-bound by name against `map.places`, so it is armable on your opening
poll with nothing named first. Both stances are re-decided by the engine at
sim-tick speed for as long as they stand — a chain is not a script that runs and
finishes, it is a sequence of *doctrines* with a condition between them.

This is the **pre-armed policy** tier, and it is where you should be spending
your thinking. An alarm asks you to re-decide at LLM latency; a chain is the
answer you gave earlier, when there was no clock on it. "If my push meets a
dozen defenders, break contact to the staging anchor" is a decision you can make
in your first minute and have executed in your twentieth:

```json
{"type":"region_set","name":"staging","x":-30.0,"z":-30.0,"radius":20.0}

{"type":"plan_set","name":"probe","steps":[
  {"intent":{"type":"stance","squad":1,"stance":"push","target":"their base"},
   "advance":{"type":"when","when":{"type":"enemy_in","region":"their base","count":12}}},

  {"intent":{"type":"stance","squad":1,"stance":"stage","target":"staging"}}]}
```

**A target you have not scouted yet still arms.** That is the point of deciding
at leisure: the ground you will want may not have a name yet. The engine checks
each step's target when you set the plan and *tells* you, without refusing
anything:

    cmd 2: chain holds at step 2: no region named 'their-expansion' - known
    places: our base, their base, mid, northwest mine, … — plan hold is armed
    anyway; the step resolves when its turn comes, and blocks there if it
    still cannot

Read that as **armed, holding** — not rejected. Your plan is in the `plans`
array and step 1 is already running. If you name the ground (`region_set`)
before that step's turn, it resolves and nothing was ever wrong. If you do not,
the step blocks exactly like any other refused step — `status` reads
`blocked: no region named 'their-expansion' - known places: …`, it retries every
5s, it recovers the moment you name the region (with a `plan … unblocked` line),
and it halts after a minute if you never do. Nothing is ever skipped.

Here is a real seat doing exactly that — the chain armed in the same batch as
`ready`, held for the 25 seconds its hero took to train, then run:

```
[    0.0] cmd 3: chain holds at step 2: no region named 'their-expansion' - known
          places: our base, their base, mid, … — plan hold is armed anyway
[    3.0] plan hold step 1/2: squad 1 takes the turtle stance: defends … within 14,
          leashed to 20, retreat below 45% to its anchor, focus Siege > Cavalry
          ... plans[0].status = "running" for 24 game seconds; the hero is training
[   27.0] plan hold step 2/2: squad 1 takes the secure stance: defends
          their-expansion within 30, leashed to 38, retreat below 35% …
[   27.0] plan hold step 2/2 blocked: no region named 'their-expansion' - known places: …
          >> the commander sends region_set their-expansion
[   31.0] plan hold step 2/2 unblocked
[   31.0] plan hold complete (2 steps)
          squads[1] = {"posture":"defend@(40.0,-40.0)r=30","members":1,"stance":"secure"}
```

Note what step 1's wait was really doing: this seat had **no hero at all** when
the chain was armed, and `hero_above` is false for an empty roster. "Turtle
until the hero is healed" and "turtle until the hero exists" are the same
sentence, which is what you want — the chain will not commit over a corpse
either.

Three things a chain does NOT do, all of them on purpose:

- **It does not loop.** Once through, then `done`. Repetition is a trigger's
  `repeat`.
- **It does not outrank a trigger.** If a chain step and a trigger re-task the
  same squad on the same tick, the trigger wins — a rule written for the
  situation in front of you beats a sequence written before the match.
- **It does not hold the squad still while it waits.** The stance from the
  previous step is *running* the whole time. Silence between steps is the
  doctrine you already set, not a gap.

### Plans vs triggers — use both

They are two halves of one sentence and they do not compete for slots.

- A **plan** is what you are going to do. Sequence, once through.
- A **trigger** is what to do if something happens. Condition, optionally
  repeating.

A plan step can arm a trigger, which is how you say "once the barracks is up,
start guarding the base". If a plan step and a trigger both re-task the same
squad on the same tick, **the trigger wins** — a rule written for the situation
in front of you beats a sequence written before the match.

## The affordance document: everything above, as one page you can read

```bash
python3 tools/bridge_view.py --doc <SEAT>/state.json          # ~75 lines: read every cycle
python3 tools/bridge_view.py --doc --all <SEAT>/state.json    # ~650 lines: read once
python3 tools/bridge_view.py --doc --json <SEAT>/state.json   # to parse; never folded
python3 tools/bridge_view.py --doc-version                    # affordance-doc/2.1
```

The digest tells you **what is going on**. The document tells you **what you can
say about it**, on one page, with the command already written. It is rendered
from your `state.json` and your `catalog.json` and nothing else — it sends
nothing, writes nothing, and knows nothing you do not.

### How to read it

Five sections, in this order.

| section | what it is |
|---|---|
| `properties` | the `--digest` view, embedded verbatim |
| `default` | **what happens if you send nothing this cycle**, with `command: null` |
| `alarms` | each ringing alarm, leading with the reflex that already fired |
| `playbook` | the plan library, and — if you declared one — the fork you are on |
| `actions` | every link and every form (below) |
| `preference` | your own declared doctrine, focus and playbook, if you declared them |

Plus `doc_version` — the version of this scaffold, which an arena round records
in its `ruleset` so a result can be read as model+scaffold rather than as model.

### The page is folded (`affordance-doc/2.0`)

`--doc` used to print every action with all of its fields and all of their
domains: ~650 lines. Every commander in the model ladder — four models, ten
seats — read that page once and then never re-opened it, which meant the
readiness facts on it were served every cycle and read none of them
(`arena/LADDER.md`, Findings 2 and 5). So the default render **folds each action
onto one line**:

```
  READY (39) — the command as printed is complete
    army (24)
      stance:squad-1:secure · squad 1 → secure · Hold ground away from home. · {"type":"stance","squad":1,"stance":"secure","target":"mid"}
      train:Barracks · train at your Barracks — no building id … · {"type":"train","select":"my Barracks","unit":null} · you fill: unit
    …
  NOT READY (4) — listed and still sendable; NOT READY is a fact, never a refusal
    stance:squad-1:push · squad 1 → push · {"type":"stance","squad":1,"stance":"push","target":"mid"} · BLOCKED: push gates: not consolidated: 3 of your 9 army units are outside squad 1 · intel: last seen: 7 troops … — not since
```

Read a folded line left to right: the **name** (`rel`), what it does, **the
complete command**, then whichever of these apply — `you fill:` the fields that
are yours, the collection's slot pressure, `BLOCKED:` and the facts that stop it,
and your `intel` ledger. A folded link is still rung 2: copy the JSON and send
it. A folded form is still rung 3: fill the `null`s and send the template.

**Nothing is hidden — only folded.** Every action is on the page, with its
count. What the fold drops is authoring detail: the per-field notes and the
served `domain` lists. Three ways to get them back:

- `--doc --all` — every action in full. This is the old page, unchanged, and it
  is the one to read at t=0 when you have time and no clock;
- **declare a focus** (next section) — one section rendered in full, every cycle;
- `--doc --json` — **never folded**. A parser has no page to run out of, so the
  JSON always carries every field, every domain and every reason. Each action
  also carries `sections` (which of `economy` / `tech` / `army` / `harass` /
  `standing` it belongs to) and `collapsed` (whether the text render folded it).

### The ladder: default, link, form, raw

Four rungs, in increasing order of how much you have to decide. Take the lowest
one that says what you mean. *The blocks below are the `--all` render; the
default page carries the same facts folded onto one line each.*

1. **`default`** — zero decisions. Say nothing and this is what happens. Your
   stances persist, your triggers keep watching, your plans keep stepping. A
   cycle in which you send nothing is a cycle in which your doctrine runs. It is
   printed every time, so silence is never a guess.

2. **link** — zero fields. `command` is complete; send it back verbatim.

   ```
   [READY    ] stance:squad-1:secure  squad 1 → secure · Hold ground away from home.
               why: squad 1 holds 6 units, pooled strength 729
               send: {"type": "stance", "squad": 1, "stance": "secure", "target": "mid"}
   ```

   Every link is written in **selectors and place names, never entity ids**, so
   a link is still a valid sentence a minute after it was rendered.

3. **form** — the engine filled every fact-shaped field and left the
   judgment-shaped ones `null`. Fill those and send the template.

   ```
   [READY    ] recipe:hero-save  HERO SAVE — the hero walks out before it dies
       template: {"type":"trigger_set","name":"hero-save","repeat":45,
                  "when":{"type":"hero_below","frac":null},
                  "then":{"type":"move","select":"my hero","region":null}}
         when.frac   <number> range 0.05..0.95  (leave null — this one is yours)
         then.region <place>                    (leave null — this one is yours)
           domain: our base · their base · mid · north-pass (YOUR region) · …
   ```

   A field's `domain` is the real list the engine will accept — place names from
   `map.places` plus your own `regions`, the selector phrases, the building kinds
   with their prices and whether your tech allows them. The refusal that would
   have taught you this arrives with the form instead of costing you a cycle.

4. **raw** — the URI bar. **Everything in this brief is legal whether or not it
   appears in the document.** The document is a floor, never a ceiling: it lists
   what is easy to say, and it constrains nothing. If the winning move is a verb
   the page did not offer, send the verb.

### Readiness is a fact. There is no recommendation.

Every link carries `ready` and a `reason`, and the reason prints **both sides of
every comparison**, whether it passed or failed:

```
[NOT READY] stance:squad-1:push  squad 1 → push · Commit to the objective.
            why: push gates: not consolidated: 3 of your 9 army units are outside
                 squad 1 (met: size 6/6; Hero at 92%, gate is 80%)
            intel: last seen: 7 troops (3 Archer, 2 Footman, 1 Catapult, 1 Hero)
                 at (-12, -11), 0s ago — not since
```

Three things about that block:

- **NOT READY never blocks anything.** The command underneath is complete and
  the engine will accept it. `ready` is the engine telling you what it can
  measure, not what it thinks.
- **The push gates are this scaffold's numbers** — one consolidated squad, six
  or more units, heroes at 80% — and they are printed with both halves so you
  can disagree with the threshold rather than with the engine.
- **`intel` is your ledger, not your eyes.** "Last seen … — not since" is the
  sentence a commander lost a match for not having. An empty ledger says so in
  those words: what you cannot see is not what is not there.

Costs, tech gates and staleness are facts too. *Best move* is an opinion and you
will not find one on the page.

### Standing state is a set of resources

`trigger_set` / `region_set` / `plan_set` were always CRUD by name — a fresh name
creates, your snapshot echo reads, the same name updates in place for free,
`*_clear` deletes. The document says so out loud:

- each collection reports its **slot pressure** (`2 of 8 region names in use`);
- each thing you already have gets an **edit form pre-filled with its current
  values** and a **delete link** beside it;
- a full collection is `NOT READY` with the way out in the reason ("re-use one
  of those names to replace a rule in place").

The recipes from *Triggers* above are served the same way — `recipe:home-guard`,
`recipe:hero-save`, `recipe:expand`, `recipe:counter-punch`,
`recipe:steady-production` — with the ids taken out and one or two judgment
fields left open.

### Production is on the page (`affordance-doc/1.1`)

One `train:<kind>` form for every producer kind you have standing, written in
the building selector so there is no id to look up:

```
[READY    ] train:Barracks  train at your Barracks — no building id, the role
                            resolves when it runs
            why: 2 finished Barracks, 1 idle; you hold 964g/145l at 36/50 supply
    template: {"type":"train","select":"idle Barracks","unit":null}
      select <selector> default="idle Barracks"
      unit   <kind>     (leave null — this one is yours)
        domain: Footman — 135g/0l 2supply — available
                Knight  — 245g/60l 3supply — NOT AVAILABLE at your tech
                …
```

The `select` default is `idle <kind>` when one of yours is free and `my <kind>`
when they are all busy — both facts about your own buildings, neither advice.
The `unit` domain prices every row against your own bank, your own tech and
(for heroes) your own slots, so a refusal that would have cost a poll cycle
arrives with the menu instead.

`affordance-doc/2.1` adds the PLAYBOOK section (below) — a declared game-plan
served as the fork you are on. Additive: no action moved, and a seat that
declares nothing gets one line advertising the library.
`affordance-doc/2.0` folds the text render (above) and adds the declared focus
(below); the action set, the JSON and every command are unchanged, which is why
the minor half went back to zero and the major half moved.
`affordance-doc/1.3` fixes the steady-production recipe (a served template now always compiles); `1.2` added `rally:<kind>`, `template:<kind>` and `cancel:<kind>`
beside them, the other three verbs a building selector made sayable without an
id. `rally:` states the current rally point in its reason line (off
`buildings[].rally`), and `cancel:` is listed NOT-READY with your queues spelled
out when there is nothing to cancel — the menu never hides an option, it explains
why it would refuse.

### Declaring a doctrine and a focus (optional)

The engine will **sort** the menu under your own values, and **expand** the part
of it you say you are working on. It will never generate either. Write a file
and point the view at it:

```json
{"doctrine": "aggression: high, risk: low",
 "prefer": ["harass", "push"],
 "avoid":  ["turtle"],
 "focus":  "army"}
```

```bash
python3 tools/bridge_view.py --doc --prefs doctrine.json <SEAT>/state.json
```

`prefer` and `avoid` are matched against each action's name and title.
Preference reorders the page and does nothing else: not one `ready`, `reason` or
`command` changes. With no file, the order is facts only — anything an alarm
points at, then links before forms, then ready before not-ready.

`focus` is one of **`economy`**, **`tech`**, **`army`**, **`harass`**. The
section you name is rendered **in full** — every field, every domain, exactly as
`--all` would — and everything else stays folded. Three things it is not:

- **It is not a filter.** Every other action is still listed, still counted,
  still carries its complete command. Nothing you could send yesterday became
  unsayable.
- **It is not the engine's opinion.** The engine never infers a focus. Leave the
  key out and the page is fact-collapsed, which is the default and costs you
  nothing. You declare a focus because you decided to work on something; the
  page renders your decision back at you.
- **It does not survive an alarm.** Anything a ringing alarm points at is
  expanded whatever your focus says, and the alarm block itself is on top of the
  page as always. A focus can never be the reason you did not see the fork.

Rewrite the file whenever your plan changes — that is the point. A focus you
declared at t=60 and a different one at t=400 is a phase transition you can
name, and an arena round can measure it against what you actually did.

An unrecognised word is ignored and the page says so, rather than quietly
serving you the default while you think you are reading a focused page.

### Playbooks: a build order, in the game (`affordance-doc/2.1`)

A **playbook** is a declarative game-plan that ships with the engine as content
(`assets/data/playbooks.ron`, published in your `catalog.json` as
`playbooks[]`). It is the wiki build order, in-game and symmetric: the human
seat reads the same rows you do, the engine authored none of it, and the engine
executes none of it either. You enact a step by **sending its command**, through
the same wire as everything else.

Declare one in the same prefs file the doctrine and focus travel in:

```json
{"playbook": "standard-kingdom", "focus": "army"}
```

The page then carries a PLAYBOOK section, between the alarms and the actions:

```
PLAYBOOK standard-kingdom · step 3/10 — Stamp the doctrine once, and every new body inherits it
  you are here: this step is OPEN — Footman 3/1
  advance when: NOT YET — Footman 3/5
  why: A squad outlives its own members and an entity id does not, so stamping the Barracks once
    means every Footman it ever trains walks out already enrolled ...
  the fork (3 options — choose one. Off-book is legal and unflagged, and silence runs the DEFAULT above)
    1 CONTINUE · {"type":"template","select":"my Barracks","squad":1} · advance when: Footman 3/5
    2 EXIT · Enrol what is already standing · {"type":"squad","id":1,"select":"all army"} · why: ...
    3 EXIT · Rally them somewhere useful while you mass · {"type":"rally",...} · why: ...
```

**THE RULE, and it is the only one that matters: a step is a FORK, never an
instruction.** Every step ships with two or three authored alternatives beside
its own command, each with its complete JSON and its reason, and the engine
refuses at startup to load a step that has none. You are being asked to
*choose*, not to comply. Going off-book is legal, unflagged, and not reported
anywhere: the document is a floor, never a ceiling, and that applies to a plan
as much as to a menu.

Four things to know before you follow one:

- **"You are here" is a fact, not a bookmark.** The pointer is the first step
  whose `gate` does not hold, recomputed from your snapshot every render. It can
  move BACKWARDS — lose your army and the plan says the army step is where you
  are, because it is. Nothing is remembered between cycles and no wire key
  carries it.
- **`entry` and `gate` are different questions.** `entry` says whether the step
  is open (a `game_time` step you have arrived at early reads `NOT OPEN YET`
  with the clock on the line); `gate` says whether you may move on. Both are
  ordinary predicates from the fourteen, minus `base_under_attack` — a playbook
  may only use the ones your own snapshot can answer.
- **A step can be INVALIDATED, and that is an interrupt.** Every step carries a
  `fail_when`. When it holds, the section re-renders with `INVALIDATED` in the
  heading, the broken assumption named in numbers, and the exits moved to the
  top with the step's own move demoted to last and labelled as being taken on an
  assumption that has gone. Abandon it. A plan ridden past its own validity is
  the most expensive thing on this page.
- **An alarm outranks any plan.** ALARMS stays above PLAYBOOK, always, and a
  ringing alarm adds one more option to the fork pointing back at it.

`catalog.playbooks[]` is the machine-readable version, with every step's `entry`
/ `action` / `gate` / `why` / `fail_when` / `exits[]` in full — read it once at
t=0 if you want the whole plan; the page serves the step you are on.
`ruleset.constants.playbooks_ron` records which version of the library a round
was played under, because authored strategy in the scaffold has to be versioned
for a result to mean anything.

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
  ok       'hold the northwest ford'   -> squad 1 defends (-60.0, 60.0) with all army (7 right now)
  ok       'forage mid with the cavalry'-> squad 2 forages (0.0, 0.0) with 3 unit(s)
  ok       'retreat at 35%'            -> all army (10 right now) fall back to (70.0, 70.0) below 35%
```

Two of those name a **role** and one names a kind, and the difference is on the
wire: a role compiles to `"select":"all army"`, a kind to a list of ids. The
count in the confirmation is a fact about right now; the phrase is what was
sent.

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

- **It speaks selectors, so your standing orders stay live.** Four English
  words are engine roles and go on the wire as roles: `the army` /
  `everything` → `"select":"all army"`, `workers` → `"select":"workers"`,
  `the hero` → `"select":"my hero"`, `squad 2` → `"select":"squad 2"`.
  `harvest gold` says `"target_select":"nearest mine"` rather than memorising a
  node, and a `build` at a landmark (not at typed coordinates) adds
  `"site":"nearest legal site"`. A phrase naming KINDS — `the cavalry`, `the
  siege`, `the champion` — has no role to become and still compiles to ids, so
  say it with a squad when it has to outlive the units in it.
- **Stances have sentences too**: `squad 1 turtles at our base`, `squad 2
  secures north-pass`, `harass their base with squad 3`, and for the two words
  that are also postures, the longer spelling — `put squad 2 on push at the
  northwest ford` or `squad 2 takes the push stance at north-pass`. Bare
  `squad 2 pushes their base` still means `posture push`, so both vocabularies
  stay reachable. An unknown stance word is refused with all five named.
- `--explain` prints the full vocabulary. **Read it once**; it is the list of
  idioms that compile deterministically.
- Anything it does not know, write by hand. It is a convenience over the schema
  above, never a gate in front of it, and it never guesses: an unresolvable
  place or an unknown noun is a reported error, not a silent whole-army move.
- **Conditionals compile to `trigger_set`**: "strike when their hero falls" arms
  a rule the engine watches at 4 Hz. `when`/`if`/`once`/`after` arm a once-rule;
  `whenever`/`every time` arm a repeating one. Only a condition outside the
  thirteen predicates defers, and then the tool says which condition and prints
  the command to run when you see it in `events`. Enemy hero *health* is the
  standing example — that one is genuinely unknowable, and it still defers.
- **Sequences compile too.** Clauses joined by `", then"` become one
  `plan_set`:

  ```bash
  python3 tools/intent_compile.py --seat <SEAT> --send \
    "build a barracks, then when we reach tier 2, build a sanctum, then train 3 sorcerers"
  ```

  A bare `", then"` is "as soon as that lands"; `", then when <cond>,"` waits on
  a trigger predicate; `", then after 30s,"` waits a fixed time. Name it with a
  trailing `as <name>`. **The comma matters**: `focus siege then heroes` is a
  focus-fire chain inside one clause, not two steps.

  For units you will not have yet, say it with a squad — the tool and the engine
  agree on this idiom:

  ```bash
  "the barracks units join squad 2, then when I have 8 footmen, squad 2 pushes their base"
  ```

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
| `trigger:home-guard move by bridge t=41` | a rule YOU armed fired and moved it |
| `plan:opening step 2/5 build by bridge t=41` | step 2 of your 5-step plan ordered it |
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
do not pay: `squad`, `posture`, `stance`, `retreat`, `leash`, `autocast`, `priority`,
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

Everything above still applies — same verbs, same snapshot, same fog, same
`bridge_send.py`. One thing changes, and it is the important one: **you are not
the faction.** A human is playing this faction with a mouse, and you are sitting
next to them. Your snapshot's top-level `copilot` block confirms it:

```json
"copilot": {"trust":"split",
            "direct":["priority","retreat","leash","autocast","squad","posture","stance","template"],
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

   **A `"select"` phrase is expanded for that readout**, so
   `{"type":"move","select":"all army",…}` is weighed as the units it currently
   names, and your partner also sees `the roles named reach 7 unit(s) as of
   now` — the size of what they are agreeing to, which the sentence alone does
   not give. If the phrase currently matches nobody they are told that instead
   (`as of now this would refuse: …`), which is your cue that the proposal is
   worth withdrawing rather than waiting on. Every tag is dated `as of now`
   because the scope is measured when your proposal arrives and resolved again
   when your partner approves it; those are up to 20 seconds apart, and what
   lands is the second answer.
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

`BH_COPILOT_TRUST=full` (everything direct) and `=strict` (everything
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

The catalog also carries the **vocabularies** — `stances`, `selectors` (by
channel) and `predicates`. `predicates` is the table below in machine-readable
form: one row per `when` arm, each with the fields it takes, whether each is
required, and the value the engine fills in when it is not. If you generate
commands, read that instead of parsing this document.

## The rules of the world (not in the catalog)
- **FOG OF WAR — read this before you read `units`.** Your snapshot shows only what your
  team can currently SEE, and it is the same rule the human player's screen obeys. So:
  - **An empty `units` list means "I have no information", not "there are no enemies."**
    Check the top-level `fog` object (`enabled`, `explored`, `visible`) before drawing any
    conclusion from silence. `explored: 0.1` means you have looked at a tenth of the map.
  - Enemy **units** appear in `units` only while you can see them. An army that leaves
    your sight is gone from `units` — it has NOT died. But it is not forgotten: see
    **INTEL** below, which is where the memory of it lives.
  - Enemy **buildings** you have scouted stay in `buildings` as remembered ghosts carrying
    `last_seen` (game time of the sighting). A ghost may be stale: the building may have
    been destroyed, or upgraded to a higher tier, since. `last_seen` present == memory;
    absent == you are looking at it right now.
  - `bounties` lists only caches you can SEE. Treasure you have no eyes on is invisible.
  - Still public and unfiltered: `map`, `mines` (position AND remaining gold), `trees_near`.
    Map geography is not a secret; what the enemy is DOING with it is. A mined-out
    mine **stays** in `mines` reading `"remaining": 0` — a dry mine is still a place,
    and it is the thing `mine_dry` watches for. (Felled trees leave `trees_near`.)
  - You cannot `attack` an id you cannot see or remember — it is rejected as
    `target N is not visible`. Use `attackmove` to advance into the unknown.
  - **Scout deliberately.** Vision radius is per-kind in `catalog.json` (`vision`).
    Raiders see 24 and are the cheapest eyes on the map; Catapults see 14 but shoot 20, so
    unescorted siege is firing blind. Halls see furthest of all and grow with the tier.
- **INTEL — what you REMEMBER of them (`intel`).** Fog decides what you can see; this is
  what you learned from having seen it. Always present. Nothing in it was inferred,
  deduced, or handed to you: every entry is something one of your units was looking at.
  - `intel.sightings[]` — every enemy unit you have seen and not yet forgotten:
    `{id, kind, pos, hp_frac, heading?, t_seen, age}`. **Every record carries its age**,
    which is the whole reason this is a separate array from `units` — a memory reported
    in the same shape as a sighting is a decoy. `heading` is a coarse compass point
    (`"NE"`) and is absent when the unit was standing still or when you only glimpsed it
    once: a heading is a difference between two looks.
  - Entries **expire**. A sighting is dropped `intel.ttl_s` (90s) after its last refresh,
    because a ninety-second-old unit position is not a stale fact but a wrong one. It is
    also dropped the instant you **watch the unit die**. It is NOT dropped merely because
    you walked back and found the spot empty — the record only ever claimed the unit was
    there *at `t_seen`*, and it still was.
  - So: an empty `sightings` means "nothing seen recently", never "nothing exists". Read
    `age` before you act. A 5s-old sighting is a target; a 70s-old one is a rumour that
    tells you where they *were going*, not where they are.
  - `intel.groups[]` — the same sightings clustered into forces:
    `{size, composition, pos, t_seen, age, place}`, e.g. `~8 (5 Footman, 3 Archer)` near
    the center ford. Units cluster only if they were seen close together **in space and
    in time**, so a group is a picture that existed at some instant. Workers are excluded:
    a mining crew is not an army. `place` is the public name of the ground.
  - `intel.heroes` — one entry per enemy hero class, always all of them:
    `status` is `"unknown"` (never met), `"alive"` (seen alive, nothing since says
    otherwise) or `"seen-dying"` (you watched it die), plus where and when.
    **Read `"alive"` as *alive as far as you know*** — a hero that died out of your sight
    goes on reporting `"alive"` for as long as nobody looks. `"unknown"` is not `"they
    have no hero"`.
  - There is **no level, xp, mana, inventory or squad** on any of this, ever. A human
    cannot select an enemy unit, so none of those has been on anybody's screen. What you
    get is what a player watching the same fight would have: existence, kind, place,
    health bar, which way it was walking, and when.
  - The human at the keyboard sees the identical ledger — faded markers where units were
    last seen, on the map and the minimap, and a "Their heroes:" line. Same knowability,
    both renderers.
  - Two predicates read it: `enemy_army_seen` and `enemy_hero_down`. See Triggers above.
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
- **YOUR FIRST HERO IS FREE; THE SECOND CLASS IS 400g/100l ONCE A KEEP STANDS; LOSING
  EITHER IS 400g/100l MORE.** Your very first hero — whichever class you pick — costs
  **0 gold, 0 lumber**: 25 seconds of hall time (the same building that makes your workers)
  and 5 supply, and nothing else. That waiver is **one per team, not one per class**, and it
  is spent the moment you QUEUE a hero, not when it walks out. Everything after it is full
  fare: the second class at a Keep, and any class you have lost, both at **400g/100l**, flat,
  at every level. So there is no such thing as a first hero you cannot afford, and there is
  such a thing as a hero you cannot afford to lose. **Train one in your opening. There is no
  argument against it.** The second one is a purchase like any other — weigh it against the
  three Footmen it costs, and note that a Keep buys you the SLOT, not the hero.
  This is new, and it is new because of you: across arena rounds 17-19 every single winner
  fought hero-less, and in r19 BOTH commanders looked at the old 400g price tag, counted
  three Footmen, and skipped — correctly. The price moved to the place where skipping is not
  an option.
- Your heroes level from ANY nearby enemy deaths, +HP/+damage per level. THE key units — keep
  them alive (retreat policy!) and near fights (XP). Revival preserves the level, and the fee
  does NOT scale with it, so a level-6 hero bought back for 400g is the best gold in the
  game — which is exactly why the enemy wants it dead and why `hero-save` (recipe 2 above) is
  worth arming before your first fight, not after it.
  Two heroes at Keep means two retreat policies and two autocast rules.
- **HERO SLOTS SCALE WITH YOUR HALL TIER: TownHall 1, Keep 2, Castle 3.** A second hero is
  one of the two concrete things teching to a Keep buys you (the other is the Arcane Sanctum).
  Heroes must be of **distinct classes** — Champion *and* Priestess is legal, two Champions
  never is. Read `me.hero_slots` / `me.hero_slots_used` in the snapshot before you train:
  used counts living heroes **plus any hero already sitting in a queue**. `me.hero_records`
  lists every class you have ever fielded (with `alive`), and `me.hero_costs` prices each
  class: `gold`/`lumber` are what queuing it costs you RIGHT NOW (**0/0 only while your team
  has no hero at all** — alive, queued, or dead and awaiting revival), and
  `revive_gold`/`revive_lumber` are what the next death will cost — carried even while the
  hero is alive and well, so you can budget for the funeral before it happens. **Read those
  0s as alternatives, not as a shopping list**: before you field anything, every class prices
  at 0 because any ONE of them could be your free hero, and the instant you queue one the
  rest read 400/100 in the next snapshot. Revival stays per class and is never discounted by
  the other: a dead Champion does not make reviving the Priestess cheaper, and vice versa. Only two classes ship today, so a Castle's third slot currently has nothing to put
  in it.
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
