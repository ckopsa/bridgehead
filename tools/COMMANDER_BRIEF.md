# RTS Commander Briefing (LLM seat)

You command one faction of a Warcraft-3-style RTS through a file channel.
Your seat directory is given in your instructions as `<SEAT>` (e.g. `bridge/red`).

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
afford, not yours). `seq` is handled for you.

## Command reference
Unit orders (ids from state):
- `{"type":"move"|"attackmove","units":[ids],"x":..,"z":..}`
- `{"type":"attack","units":[ids],"target":enemy_id}`
- `{"type":"harvest","units":[worker_ids],"target":node_id}` (mines AND trees — tree ids in `trees_near`)
- `{"type":"return","units":[worker_ids]}`  `{"type":"stop","units":[ids]}`  `{"type":"follow","units":[ids],"target":own_id}`
Production:
- `{"type":"build","worker":id,"kind":"Farm"|"Barracks"|"TownHall","x":..,"z":..}` (site must be free; you pay on placement)
- `{"type":"train","building":id,"unit":"Worker"|"Footman"|"Archer"|"Hero"}` (queue cap 7)
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
- `{"type":"buy","shop":id,"item":"HealingPotion"|"TownPortal"}` — your living hero buys the item
  (2 inventory slots; see catalog `items`). `{"type":"use_item","slot":0}` consumes it.
  TownPortal teleports your hero + nearby own units to your nearest TownHall — the expansion-saver.
Doctrine (standing orders, executed continuously — USE THESE, they fight for you between your turns):
- `{"type":"priority","units":[ids],"classes":[...]}` — focus-fire order ([] clears). Valid classes:
  Hero (both hero types), Archer, Footman, Worker, Building, Siege (catapults), Cavalry (raiders).
- `{"type":"retreat","units":[ids],"below":0.35,"x":..,"z":..}` — auto fall-back when hurt
- `{"type":"leash","units":[ids],"x":..,"z":..,"radius":20}` — never chase/fight beyond anchor (radius 0 clears)
- `{"type":"autocast","units":[hero_id],"min_enemies":3}` — hero slams automatically. Add
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
- `{"type":"autopilot","on":true}` — hand your whole faction to the scripted AI (emergency only).
- `{"type":"surrender"}` — concede the match (opponent wins immediately). The honorable end to a
  hopeless position — no income, no army, no path back. Preferable to dragging out a decided game.

## What you can build/train: read `<SEAT>/catalog.json`
The FULL content catalog — every unit, building, ability, research and item:
costs, stats, train/build times, what produces what, and what gates it.
Read it ONCE at match start; it is the authoritative content reference.

**The catalog IS the tech tree — no prose needed.** `requires` on everything
lists what must be STANDING (trainer included, transitively) and `tier` says how
far up the hall ladder that puts you; `upgrades_to`/`upgraded_from` walk
TownHall→Keep→Castle with every price on it. T2 arrives ~min 3-5, T3 ~min 6-9.

The live snapshot's `unlocked` map tells you which entries' requirements you
currently satisfy — but it checks tech gates only, not whether you own the
trainer, so cross-check against `requires`.

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
- Your Hero levels from ANY nearby enemy deaths, +HP/+damage per level. THE key unit — keep it
  alive (retreat policy!), keep it near fights (XP), revive it fast when it dies (keeps its level).
- **Choose your hero class at first training** (see catalog: units with abilities). Your team's
  class locks in — revival always restores the class you chose. Choose for your gameplan.
- Counter triangle: fortifications stop armies, siege outranges fortifications, fast cavalry
  dives siege. It is all data: catalog `units[].class` says what a unit IS, and
  `vs_building_mult` / `vs_siege_mult` / `vs_cavalry_mult` say what it eats. The
  multipliers are keyed off the CLASS, so a Spearman's anti-cavalry bonus lands on the
  Knight and the Raider alike. `damage` with `attack_cooldown` gives you dps.
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
