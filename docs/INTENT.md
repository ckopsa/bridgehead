# INTENT — the shared strategic language

*The v2 layer that makes THESIS.md's fairness claim structural instead of
aspirational. Implements `wc3clone-syf`.*

THESIS.md states the form of fairness this project is after:

> Fairness is structural: the AI *cannot* act in ways the human cannot, because
> there is no other API.

Before this change that sentence was half true. There was one *vocabulary* —
orders, postures, doctrine — but two *implementations* of it: `ui.rs` wrote
`Order` components and pushed training queues from fifteen scattered call
sites, and `bridge.rs` did the same thing again, separately, in a 680-line
`apply_batch`. Two implementations of one language is two languages. Every
divergence between them (and docs/TEMPO.md §2.0 found several: 8 bridge
doctrine commands against 4 coarse UI toggles) was invisible, because nothing
in the code ever compared them.

There is now one list of verbs and one compiler.

- **`shared::Intent`** — the vocabulary, as one serde-serializable type.
- **`intent.rs`** — the only thing that turns an `Intent` into game state.
- **`bridge/intent_log.jsonl`** — every intent anyone issued, as an English
  sentence plus its serialized form. (One exception, added by `wc3clone-vax`
  and argued in *Transitions are announced*: a blocked plan step re-submitting
  itself with the identical verdict is the same sentence still failing, not a
  new one, and the log records the transitions.)

---

## The fairness invariant

> **No commander mutates game state except through intent submission.**

It read "no *player-facing* mutation path exists except intent submission"
until wc3clone-jem, and the hedge was carrying real weight: the scripted
`ai.rs` was the one commander in the game still writing components directly.
It speaks the language now, so the sentence needs no qualifier and names no
exception. Three seats — `ui`, `bridge`/`copilot`, `script` — one verb list,
one compiler.

`ui.rs`, `bridge.rs`, `ai.rs` and `trigger.rs` contain zero writes to `Order`,
`TrainingQueue`, `RallyPoint`, `TargetPriority`, `RetreatPolicy`,
`LeashPolicy`, `AutoCastPolicy`, `SquadId`, `DoctrineTemplate` or
`SquadOrders`, and zero sends of `CastAbility` / `BuyItem` / `UseItem` /
`UpgradeBuilding` / `Surrender`. Each writes `SubmitIntent` events and nothing
else. This is grep-checkable, and checking it is the point: the invariant is
only worth having if a regression is visible. (`ai.rs` is down to *two* write
statements in the whole file, and both are a `SubmitIntent`.)

The two human-facing ones used to carry a *field-for-field identical*
four-writer `SystemParam` bundle (`CardActions` in ui.rs, `CmdEvents` in
bridge.rs) — independent convergence on the same shape, which is exactly the
duplication this layer removes. Both are gone; intent.rs owns the four writers.

Four producers, but **three seats**: `ui`, `bridge`/`copilot`, and `script`.
`trigger.rs` is not a fourth author — a fired trigger carries the source of
whoever *armed* it (`SubmitIntent::fired`), because a rule doing what it was
told is still the commander speaking, just earlier.

What the interfaces still own is the *gesture*: deciding which units a
right-click meant, which worker is nearest the build site, what "guard" implies
as an anchor and a radius. That is the human interface's real job. What comes
out the other side is a value a commander could have typed.

### The third seat

`ai.rs` is a seat. Since **wc3clone-jem** the scripted commander mutates
nothing: every action it takes is a `SubmitIntent` with
`IntentSource::Script`, built out of the same `shared::Intent` values ui.rs
compiles from a right-click and bridge.rs deserializes from `commands.json`,
read by the same compiler in the same frame. There is no longer a footnote on
the invariant, and no longer a rung of `Cause` that only the script could
produce.

What it converted, site by site: `move` (workers fleeing melee), `attackmove`
(defend, the three wave branches, the rally gather), `harvest` (idle-worker
assignment and the multi-mine rebalance), `return` (stranded haulers), `build`
(the whole build order, expansions and ford emplacements included), `train`
(workers, the army mix, casters, siege, heroes and revivals), `upgrade` (hall
tier-ups), `research` (the forge ladder), `buy` and `use_item` (the Shop
rules), `cast` (the Champion's Slam), and `autocast` (the ultimate doctrine a
machine-driven team installs on its heroes).

Four things follow, and they are the reason the bead was worth doing:

- **It is validated.** The compiler refuses the script exactly as it refuses a
  commander — fog, ownership, tech gates, prices, hero slots, queue caps. The
  one place this bites is build placement, where `Intent::Build` snaps a site
  to the nav lattice *before* checking the ground; `ai.rs` therefore snaps its
  candidate first and vets the snapped point. Its site pickers clear
  `size + BUILD_PADDING` — a full cell of slack per side against a half-cell
  snap — so the snapped footprint is provably inside ground already known to
  be free, and the conversion costs the script no placements.
- **It appears in the replay.** `intent_log.jsonl` now records all three
  authors. An AI-vs-AI sim writes a real transcript instead of nothing.
- **It is attributable.** Its units answer `order:attackmove by script t=…`,
  which joins against the log by the same rule everyone else joins by. (The
  free-text `Cause::Script { what }` rung — `script:wave`, `script:flee` — is
  gone with it. The *why* behind a script order lives in ai.rs's `info!` lines
  and the log's sentence now, not on the unit.)
- **It pays the link structurally.** docs/TEMPO.md §3 used to be satisfied
  here by ai.rs reaching for `command::OrderIssuer` by hand — correct, and one
  deleted call away from a cheat. It is satisfied by construction now: the
  script has no way to issue an order except to ask the compiler to.

  This survived meeting triggers (`wc3clone-pec`) without a special case, which
  is the sort of thing the choke point is for. `apply_intents` hands the
  *exempt* issuer to anything with `SubmitIntent::trigger` set, on the sound
  reasoning that a rule's author paid the reach when they armed it. A scripted
  think tick is not a trigger firing — it is a commander deciding, at the
  moment of deciding, which is precisely what the link prices — so
  `SubmitIntent::script` sets `trigger: None` and the script pays. It could
  not accidentally have ridden the exemption either: `SubmitIntent::fired`
  carries the *arming* seat's source, and `ai.rs` arms nothing.

Two consequences worth stating plainly.

**Frame order.** The script's actions used to land in `SimSet::AiThink`,
*before* doctrine ran in `SimSet::Think`. They land in `SimSet::Intent` now —
same frame, four sets later, which is exactly where a human's right-click and a
bridge command have always landed. Nothing is deferred by a tick. What changed
is that a squad posture or a retreat trigger can no longer overwrite a script
order given in the same frame: the script lost a privilege rather than gaining
one, and it only bites on a faction under autopilot that is still carrying
doctrine a player set before handing it over.

**Cost: none measurable.** A think tick that used to write components now
writes events for one system to drain a few sets later. Interleaved A/B on the
release binaries, same map, same seed, 1800 frames, at a population both
binaries reach identically (13 units / 5 buildings a side): **1712ms before,
1667ms after** — the same number, and the same verdict the pre-merge pair gave
(784ms / 780ms). One sim's worth of extra event traffic is free, which is what
you would hope from a handful of small `Vec`s a second going into a system that
was already running.

Two measurement notes, because both cost an hour here. Take the **minimum of
several interleaved runs**: a contended machine reports whatever else it is
doing, and samples on a box at load 18 disagreed by 2.5x in both directions.
And measure **wall time, not CPU time** — user+sys says this branch costs 8–16%
more, which is real and is not a slowdown: making `ai_think`'s building query
read-only (it no longer pushes training queues) lets Bevy run more systems
concurrently, and more parallelism buys lower wall time with *higher*
CPU-seconds. The metric that answers "is the sim slower" is the clock.

**What the sims say.** Headless AI-vs-AI matches across both maps, with
`BH_COMMAND_LATENCY` on and off, **all decisive**; `BH_SEED` determinism
verified on both maps (50 and 55 identical fingerprints across paired runs),
which is the check that the script submits in a deterministic order. Five of
six land in the documented 5–12min band, and on `open` the branch tracks the
baseline closely — 478.0s against 461.1s, 475.0s against 449.1s.

`crossings` is where it diverges, and `crossings` with latency **on** settles
at 280.5s: under the band, and seed-insensitive.

**The cause is worth being precise about, because it is the one behavioural
consequence this refactor could not avoid.** `Intent::Build` snaps a site to
the nav lattice before it checks the ground; the old direct path did not, so
the scripted AI used to place buildings on coordinates no player could have
chosen. It builds where a player builds now. Each footprint moves by at most
half a cell — but a moved footprint occupies *different nav cells*, so the next
site query gets a different answer, and by the time the build order reaches the
ford emplacements the divergence is not small any more: on `crossings`/5 the
defending Tower goes up at (-48,68) here against (-53,54) on master, both of
them "8 back from the crossing" and 14 units apart. On a map whose whole
strategy is who holds the fords, that is a different match.

So it is a *placement* change cascading into a balance change, not a change in
how the script fights — its orders are the ones it always gave. And it is
forced: exempting the script from the snap would hand it back the privilege of
building where nobody else can, which is the exact thing this bead removes.
Flagging it for the next balance pass is the right disposal, and the specific
thing to look at is the emplacement ring in `ai.rs::pick_spot`, which was tuned
against unsnapped candidates.

**Rejection.** The compiler can now say no to the script, which the old direct
path could not. Nothing latches: a think tick states what it wants against the
world it saw, the refusal is recorded, and the brain re-thinks a second later
from the world as it is. Every optimistic flag re-derives itself — `pending_build`
above all, which is released the moment no worker is actually building, and
releases the expansion ring-fence with it. Script errors go to the **debug
log**, not to `IntentErrors`: bridge.rs ships that list to whichever seat is
reading, and a commander handed failures it did not cause would be debugging
the autopilot instead of playing. The verdict is still on the record — the
journal entry says `ok: false` and the intent log carries the string verbatim.

### Who is not a player

One category deliberately keeps writing components directly, and it does not
weaken the invariant:

- **Engine systems.** `economy.rs`'s harvest follow-through, `combat.rs`'s
  chase, `doctrine.rs`'s squad re-tasking and retreat triggers. These are the
  engine executing standing policy at machine speed, for whichever player set
  it. Routing them through intents would be a category error — and it is
  exactly the line THESIS.md principle 3 draws ("the engine does what is fast;
  the player does what is wise").

The distinction that matters is not "human versus machine" — the script is a
machine and it speaks — but **deciding versus executing**. `ai.rs` decides, so
it speaks. `doctrine.rs` carries out a decision already made, so it does not.

One more honest edge: `ui.rs::update_rally_flag` still removes a `RallyPoint`
whose target has died. That is a validator reacting to a world event, not a
player expressing anything, so it stays where it is.

---

## The vocabulary

The verbs, grouped by what they are for. The serde shape **is** the bridge's
historical wire format — tag is `type`, entity ids are `Entity::to_bits`,
positions are flat `x`/`z` — so `commands.json` parses straight into `Intent`
with no translation layer. Backward compatibility is not an adapter here; it is
the schema.

### Unit orders
| Verb | Shape |
|---|---|
| `move` | `{units:[id], x, z}` |
| `attackmove` | `{units:[id], x, z}` |
| `attack` | `{units:[id], target:id}` |
| `harvest` | `{units:[id], target:id}` — mines and trees; workers only |
| `return` | `{units:[id]}` |
| `follow` | `{units:[id], target:id}` |
| `stop` | `{units:[id]}` |

### Production
| Verb | Shape |
|---|---|
| `build` | `{worker:id, kind:"Farm", x, z}` |
| `train` | `{building:id, unit:"Footman"}` |
| `upgrade` | `{building:id}` — tier up in place (TownHall→Keep→Castle) |
| `cancel` | `{building:id, index:n}` |
| `research` | `{building:id, upgrade:"attack"\|"armor"}` — a team-wide ladder, one rung per command |
| `rally` | `{building:id, x, z}` or `{building:id, target:id}` |

### Abilities & items
| Verb | Shape |
|---|---|
| `cast` | `{hero:id, ability?, x?, z?, target?}` (alias `caster`) — any own CASTER: hero, Sorcerer, or ability building |
| `buy` | `{shop:id, item:"HealingPotion", hero?:id}` — `hero` optional, see below |
| `use_item` | `{slot:0, hero?:id, destination?:id}` — `destination` names WHICH hall a teleport item arrives at; see below |

The shop shelf is TIERED. `catalog.items[].tier` gives each item's required
tech tier (1/2/3), and every own finished Shop reports the shelf with this
team's tier already applied as `buildings[].sells[] = {id, cost_gold, tier,
locked}`. Buying a locked rung is refused by the compiler with
`cmd N: BannerOfCommand requires tier T2 (you are T1)`, and economy.rs
re-checks on the frame it pays — so losing the Keep closes the rung again.

Hero ultimates are ordinary second ability slots gated on
`AbilityUnlock::HeroLevel(5)`: `{"type":"cast","hero":<id>,"ability":"Warcry"}`
(Champion) or `"Sanctuary"` (Priestess). `units[].abilities[]` reports each
slot's `unlocked`, `ready`, `cd` and, while locked, `requires: "hero level 5"`.

### Doctrine — standing policy the engine executes at machine speed
| Verb | Shape | Clears when |
|---|---|---|
| `priority` | `{units:[id], classes:["Hero","Siege"]}` | `classes` empty |
| `retreat` | `{units:[id], below:0.35, x, z}` | `below` 0/absent |
| `leash` | `{units:[id], x, z, radius:20}` | `radius` ≤ 0 |
| `autocast` | `{units:[id], min_enemies:3, ability?}` — any own caster | `min_enemies` 0/absent |
| `squad` | `{units:[id], id:1}` | `id` absent |
| `posture` | `{id:1, posture:{type:"defend"\|"push"\|"escort"\|"forage", …}}` | `posture` absent |
| `stance` | `{squad:1, stance:"turtle"\|"stage"\|"push"\|"secure"\|"harass", target?\|x,z?}` | never — see § Stances |
| `template` | `{building:id, squad, retreat, priority, autocast}` | all pieces absent |

#### Stances — five words for the seven verbs above

A stance is a **fixed, engine-defined bundle** of `posture` + anchor + `leash` +
`retreat` + `priority`, set in one sentence. It adds no capability: the arm
writes the same `SquadOrders` entry and the same three components those verbs
write, so `doctrine.rs` cannot tell a stanced squad from a hand-tuned one, and a
stance can never acquire a behaviour the individual verbs lack. Its numbers are
rows in `assets/data/stances.ron`; its five *words* are a `StanceKind` enum,
because a fixed vocabulary both seats and the arena ledger speak is identity.

Four properties are the design, and each answers a failure the arena produced
(docs/AFFORDANCES.md § Stances; arena rounds r21–r23):

1. **The default is persistence.** Nothing in the engine ever clears a stance.
   A commander that says nothing for ninety seconds still has the stance it set
   and the engine is still executing it — silence is a policy, not a gap. r21
   lost a match to the opposite: 98 seconds in which nothing continued the last
   decision because nothing was written down as continuing.
2. **`squads[].stance` echoes it**, which is what makes persistence *usable*
   rather than merely true: "what is my army doing?" is answerable from the
   snapshot instead of from memory. Additive key, `skip_serializing_if`, so a
   match that never sends a `stance` writes a byte-identical `squads[]`.
3. **Switching replaces the bundle atomically.** Absent pieces REMOVE rather
   than survive: `turtle` then `push` leaves no leash behind. A leash nobody set
   and nobody can see, recalling a push twenty metres from home, is the exact
   invisible failure a merge-shaped bundle would produce.
4. **A hand-set `posture` clears the word.** The bundle it installed stays —
   those are honest components a commander could have typed — but the squad is
   no longer *in* a stance, and a readout that still said `"push"` about a squad
   its own commander had just told to defend would be the one lie this feature
   cannot afford.

Two deliberate limits. A named region supplies the stance's **centre and not its
radius** (unlike `posture defend`): a preset whose numbers moved with its target
would not be a preset, and `posture` remains for anyone who wants to pick the
number. And the per-unit half lands on the squad's members *as they stand*,
exactly as `leash`/`retreat`/`priority` do — the posture covers later joiners
because it is per-squad; the rest is what `template` is for.

The vocabulary is fixed rather than commander-assembled, which is the whole
economy of the thing: commander-defined bundles would make the decision surface
as large as the vocabulary they were meant to shrink, and would make two arena
rounds incomparable. The full vocabulary stays open beside it — a stance is a
floor for a small commander, never a ceiling for a capable one.

### Triggers — contingent standing policy (v3)
| Verb | Shape | Clears when |
|---|---|---|
| `trigger_set` | `{name, when:{…}, then:{<any intent>}, repeat?:secs}` | — |
| `trigger_clear` | `{name}` or `{}` for every trigger | — |

Full treatment below (§ Triggers). One line here: doctrine is what the engine
does *continuously*; a trigger is what it does *when something happens*.

### Territory — named places and regions (v3)
| Verb | Shape | Clears when |
|---|---|---|
| `region_set` | `{name, x, z, radius}` | — |
| `region_clear` | `{name}` or `{}` for every region | — |

And one field, on **every verb above that takes `x`/`z`**: an optional
`region:"<name>"` that stands in for the pair. Full treatment below
(§ Territory).
### Plans — sequenced standing policy (v3)
| Verb | Shape | Clears when |
|---|---|---|
| `plan_set` | `{name, steps:[{intent:{<any intent>}, advance?:{…}}]}` | — |
| `plan_clear` | `{name}` or `{}` for every plan | — |

`advance` is one of `{"type":"on_applied"}` (the default — "then"),
`{"type":"when","when":{<any TriggerWhen>}}`, or `{"type":"after","secs":30}`.

Full treatment below (§ Plans). One line here: doctrine is *continuous*, a
trigger is *contingent*, a plan is *sequenced* — and the third one is the word
`then`.

### Match level
| Verb | Shape |
|---|---|
| `autopilot` | `{on:true}` — hand this faction to the scripted AI |
| `surrender` | `{}` |
| `ready` | `{}` — I have read the map; start the clock (see § The ready handshake) |

### The ready handshake

The sim clock used to run from process start. Commander agents connect seconds
to minutes apart — in arena round 9 Red's first order landed at **t=41s**,
against an opponent that had been playing since zero. The opening is part of
the game, and a game whose opening one side simply misses is not a fair one.

So: **when any bridged seat is configured, the engine holds the match at t=0**
until every such seat has sent `{"type":"ready"}`.

- **Which seats gate.** Every seat named by `BH_BRIDGE`, `copilot` included —
  a copilot writes orders that reach units, so starting before it has read the
  map reproduces the same unfairness one rung down. Seats that are *not* in
  `BH_BRIDGE` are not in the list: **scripted AI seats are born ready**, and
  so is any seat whose faction is currently on `autopilot` (checked live, so a
  commander cannot hang a match by autopiloting and walking away).
- **Nothing moves while held.** The hold is a paused `Time<Virtual>`, so every
  accumulator in the sim integrates zero — harvesting, construction, training,
  spawns, doctrine, the headless time cap. The bridge's snapshot and command
  timers run on `Time<Real>` and are unaffected: **snapshots keep being
  written**, which is the point. Both commanders get to read the map and write
  an opening before either of them plays it.
- **Planning before `ready` is legal**, for both sides equally. Orders sent
  during the hold compile at t=0 and their units act on the first live frame.
  The etiquette is symmetric because the hold is.
- **The snapshot says so.** While held it carries `waiting_for: ["red","blue"]`
  (the seats still owed a `ready`) and `match_started: false`. Both keys vanish
  the instant the clock starts, so a snapshot's historical key set is exactly
  what it always was for the whole live match.
- **`ready` is idempotent** and never refused: saying it twice, or after the
  clock has started, is a no-op rather than an error.
- **`BH_READY_TIMEOUT`** (default 120 wall seconds) starts the match anyway if
  a seat never speaks, so a crashed agent costs a round its opening rather than
  its existence. The start is announced as a timeout in the log and in the
  `match start` feed line both sides receive. It is measured on `Time<Real>` —
  the same clock the bridge writes snapshots on — which under `BH_FIXED_DT` is
  the fixed tick rather than the wall clock; a bridged fixed-dt run needs the
  timeout set in ticks' worth of seconds. The determinism harness has no bridge
  seat and never holds, so it is unaffected.
- **`BH_READY=0`** disables the mechanic entirely and restores the old
  behaviour. Runs with no bridged seat are untouched either way — the
  determinism fingerprints for both maps are byte-identical across this change.

### Where a cast lands (v3)

`catalog.abilities[].target` is the geometry, and it is data like everything
else:

| `target` | Payload | Meaning |
|---|---|---|
| `"caster"` | none | Centred on the caster. Every ability but Slow. |
| `"point"` | `x`, `z` | A ground point within `target_range` of the caster. |
| `"unit"` | `target:id` | A unit within `target_range`; the effect follows it. |

`target_range` is caster→centre; the effect's own `radius` blooms from there,
so a spell's total reach is `target_range + radius`.

**Omitting the payload is legal and meaningful.** For a `"caster"` ability it is
the only thing to send, which is why every v2 `cast` command still parses and
still means what it meant. For a targeted one it means *"aim it for me"*, and
the engine answers with `shared::best_cast_focus`: among the bodies the effect
would actually affect, the reachable centre that catches the most of them, ties
to the nearest. A body past `target_range` still proposes a centre — the point
furthest towards it the caster can reach — so the auto-pick is exactly as
long-armed as a player's click, and a cast that would catch nobody does not
happen and spends no cooldown.

That one rule serves three callers deliberately: a commander's bare `cast`, a
player's hotkey before the click, and **`autocast`** — which names no target,
so a Sorcerer on standing orders aims itself.

**Out of range is refused, not walked into.** The alternative — an `Order`
variant that walks the caster forward and then casts, matching `attack`
semantics — was considered and rejected on the merits rather than on cost:
targeted casting exists *because* the arena found Sorcerers dying in the front
rank, and a caster that closes the distance by itself is a caster back in the
front rank. It would also have had to win a continuous argument with doctrine's
squad re-tasking, which re-issues orders every second. So the compiler measures
the same distance the executor will and refuses with both numbers
(`Slow reaches 9 and that point is 40.0 away — …`). The executor re-checks on
arrival and *fizzles* silently if the caster has moved since, which is the same
honest-fizzle rule a link-delayed cast whose mana ran out already obeyed.

**Not yet: the NL compiler.** `tools/intent_compile.py` has no clause for
aiming — *"slow their cavalry"* does not yet compile to a `cast` with an `x`/`z`.
The hook is small and deliberate: the geometry is on the wire and in the
catalog, so a vocabulary bead adds a clause that reads `abilities[].target`,
resolves a noun phrase to a position from the snapshot, and emits the same
object this page documents. Until then a commander aims by writing the JSON, or
omits the aim entirely and lets the engine pick.

### The ability selector

`cast` and `autocast` take an optional `ability`, and it is **untagged** on the
wire: a bare `2` is a slot index, a bare `"Slam"` is an ability id. Omit it and
you get the caster's first unlocked ability — so `{"type":"cast","hero":123}`
means exactly what it always meant.

Ability ids are matched by `shared::normalize_name`, the same function every
other name on the wire goes through: case, spaces, dashes and underscores are
all noise, so `"CallToArms"`, `"calltoarms"` and `"Call to Arms"` are one
ability. (This was the last name in the language matched by
`eq_ignore_ascii_case` instead — which accepted the first two spellings and
rejected the third, the one a person actually types.)

### Which hero an item verb means

`buy` and `use_item` used to name no unit, because a team had at most one hero
and there was nothing to disambiguate. Hero slots scale with the hall ladder
now (`shared::hero_slots`), so a Keep team can field a Champion *and* a
Priestess and "the team's hero" stopped being a well-defined phrase — the
potion went to whichever one the query happened to yield first.

Both verbs therefore take an optional `hero`. The rule is one pure function
(`pick_item_hero`) with two clauses:

* **named** — it must be one of *your* living heroes. A name that is not is
  rejected with an error, never silently redirected: sending the item to a
  different hero is exactly the bug the field exists to prevent.
* **omitted** — the living hero with the **lowest entity id**. Sorted rather
  than left to query order, so it is stable frame to frame and identical for
  both seats; with one hero on the field it picks that hero, which is what
  every call site written before hero slots already got. The field is
  `skip_serializing_if = "Option::is_none"`, so the historical wire shape is
  byte-identical for anyone who does not care.

The two interfaces reach it from opposite ends, as usual. A commander types the
id. The UI never has to: `use_item` names the caster whose bag the button was
drawn from, and the Shop — whose card is a *building* selection, with no hero
selected to read — sells to the last hero the player had selected (`last_hero`),
falling back to the same lowest-id default. So the button that shows you the
Priestess's potion is the button that drinks the Priestess's potion.

### Which hall a teleport item means

Both teleport items used to target **the hall nearest the hero**, which reads
as a sensible default and is wrong in the exact case they exist for. Round 10
turned on it: the army was escorting the expansion, the main was dying 130
units away, and the Scroll of Mass Teleport — pressed to save the main — would
have recalled everyone *to the expansion they were already standing on*. The
default is not a decision; it just happens to be right when you have one base
and silently useless when you have two.

So `use_item` takes an optional `destination`: a building id, validated as
**your own, finished, and a hall**. Both scrolls honour it. `TownPortal` takes
the hero plus allies within `PORTAL_RADIUS`; `ScrollOfMassTeleport` takes the
hero plus every own non-worker on the map — the `army_only` rule is untouched,
workers stay on the gold whichever hall you name.

Three rules, and each one is load-bearing:

* **Omitted is the old behaviour.** No `destination` means the hall nearest the
  hero, exactly as before, and the field is `skip_serializing_if =
  "Option::is_none"` so the historic wire shape is byte-identical. A commander
  written before this existed keeps working and keeps meaning what it meant.
* **A bad destination is refused, not downgraded.** Anything that is not your
  own standing hall earns `cmd N: destination 123 is not your standing hall`
  and the item does **not** fire — the slot is not spent. Falling back to
  "nearest" would be the original bug wearing the new field's clothes: the
  scroll would go somewhere else and look like it worked. One message for all
  four failures (unknown id, enemy building, not a hall, still under
  construction) because "your standing hall" already names every condition, and
  a finer answer would hand a seat a building id it could not otherwise have.
* **A destination that dies between the order and the frame falls back.** The
  item is consumed up front, so refusing at execution time would burn a
  250-gold scroll for nothing. intent.rs is the layer that says no; combat.rs's
  job is to always do something sane, and the something is the rule that has
  always applied when nobody chose.

The sentence carries the choice (`hero 7 uses item in slot 0, bound for hall
34`), and the arrival announces itself into the acting team's event feed by
name: `hero ports the army to the Keep at (-70.0, -70.0)`. Coordinates alone
would not tell a commander whether the scroll did what it was aimed at.

`use_item` stays **exempt** from Chain of Command latency (docs/TEMPO.md §4).
Nothing about naming a hall changes the reason: the item is spent from a hero's
bag and a hero is a command node, so the link is zero however the destination
is spelled. Pinned by
`intent::tests::choosing_a_destination_does_not_start_charging_for_the_item`.

The two interfaces meet in the middle here as usual. A commander types the
building id, which it reads off `buildings[]` in its own snapshot; the catalog
advertises the option as `items[].destination: "choosable"` so it is
discoverable without reading this file. The human never types an id: pressing
the item key **arms a hall-pick** and the next left-click on one of your halls
— in the world, or on the minimap, where the other base actually is — completes
it. With exactly one hall standing the key fires immediately with no
destination at all, because a ceremony that always has one answer is a tax
rather than a decision. Escape cancels it innermost-first, like every other
armed mode.

**A hall under attack is named, not highlighted.** The hint line while the pick
is armed reads `UNDER ATTACK: Keep at (-70,-70)`, driven by the same
`BUILDING_HURT_FRAC` threshold the alert stack and the bridge event feed use to
call a building "under attack" — so the two lines agree by construction rather
than by coincidence. A *highlight* was considered and rejected as not cheap:
the world view's rings are parented on selection and the minimap draws
buildings as flat state-free dots, so either would be new render plumbing for
one transient mode. (`GameEvents`' structured threat state was the other
candidate and is the wrong shape — one hostile count for the whole base, not a
per-hall verdict.)

**A caster is anything with an ability list**, not a hero. `cast` always asked
`abilities_of_unit(kind)` rather than "does it carry a `Hero` component", so the
Sorcerer needed no work there; `autocast` did test for a hero, and now asks the
same question `cast` does. The consequence is that a unit with no mana and no
level is a first-class caster on both seats: the human's `[T]` toggle and a
commander's `autocast` command land on the identical `AutoCastPolicy`.

There is one type for all three jobs: `shared::AbilitySelector` is the intent
field, the `CastAbility` event payload and the wire form. A slot cannot be
named three slightly different ways because there is only one way to name it.
The two interfaces reach it from opposite ends and meet in the middle: the UI
is **index-native** (a hotkey *is* a slot — `[R]`/`[Y]`/`[D]` for heroes,
`[C]`/`[J]`/`[M]` for buildings), while a commander reading the snapshot
naturally writes the id. Both spellings compile to the same value.

Every clear-form is spelled *inside* the verb rather than as a separate verb.
That is what lets a coarse UI toggle and a parameterised bridge command land on
the same object: `[G] Guard` off submits `leash` with `radius: 0`, which is
character-for-character what a commander sends to release a leash.

---

## How a gesture compiles

The `[V] Fall back` key is the clearest case. The command card offers one
keystroke; the language wants parameters. `ui.rs` works out from the selection
what the gesture *meant* — the centroid of the selected group, the nearest own
completed TownHall as the rally point, the card's fixed 35% threshold — and
submits:

```json
{"type":"retreat","units":[41,42,43],"below":0.35,"x":-70.0,"z":-70.0}
```

A bridge commander types that. The human presses `V`. `intent.rs` cannot tell
which happened, and neither can the log. `intent::tests::
a_gesture_and_a_command_are_the_same_intent` asserts exactly this.

Compound gestures become *two sentences* rather than a special case: a
right-click on a gold mine with a mixed selection submits a `harvest` for the
workers and a `move` for everyone else, which is what it always meant.

The doctrine card (`[I]`, added by docs/TEMPO.md's phase 0) is the same trick at
the strategic layer. `Ctrl+1` is a `squad`; `[I][W]` then a ground click is a
`posture`; `[I][F]` steps a retreat *threshold* rather than toggling one. A
posture pressed on a selection that is not already one squad submits `squad`
first and `posture` second — two sentences again — so the log reads:

```
  [ 91.6s] Human/ui: 3 units join squad 1
  [100.2s] Human/ui: squad 1 pushes to (-59.4, -27.1)
  [103.2s] Human/ui: 3 units fall back to (-70.0, -70.0) below 25% health
  [104.1s] Human/ui: 3 units hold within 10 of (-61.0, -40.0)
  [106.9s] Human/ui: squad 1 stands down (posture cleared)
```

Every one of those is a sentence only a bridge commander could produce before.

---

## Knowability: where fog validation lives

Fog of war (docs/FOG.md) is "one rule of knowability, computed once, rendered
twice". The compiler is where that rule stops being about *rendering*.

`Intent::Attack` is refused when the issuing team cannot see or remember the
target — `FogGrid::knows_entity(id, pos)`, meaning visible now **or** a
remembered structure — with the error `cmd N: target X is not visible`. That
check used to live in `bridge.rs::apply_batch`, where it bound exactly one
seat. It now binds whoever is speaking, which is the only version of the rule
worth having: a snapshot that will not show you an enemy must not accept orders
against it either, or the filtering is decoration.

`attack` is the only verb that consults fog, matching master's behaviour
exactly. `harvest`, `follow`, `rally` and `posture escort` name neutral nodes
or the issuer's own units, which need no visibility test.

### The residual asymmetry, and how it was closed

The compiler's rule is `knows_entity` (visible **or** remembered structure).
The human's right-click picker used `sees` (visible only). So a *remembered but
currently unseen* enemy building was a legal target for a bridge commander and
un-clickable for the human: the compiler would have accepted the human's intent
happily, and the UI simply had no gesture that produced it. A real capability
gap in the direction THESIS.md cares about, visible only because there was one
rule to compare the two gestures against.

`ui.rs::right_mouse` now picks against **`FogGrid::ghosts()`** — the same
iterator `sync_building_ghosts` builds the translucent boxes on screen from.
What is clickable is therefore what is drawn, by construction rather than by
two pieces of code agreeing. `ghosts()` never yields a record whose cell is
currently visible, so the ghost set and the live-building set are disjoint and
nothing can be picked twice; enemy units still win ties, exactly as live
buildings lose to them.

The mechanism turns on one field: `RememberedBuilding.id` is the real entity's
`to_bits()` — the same number the bridge names in
`{"type":"attack","target":N}`, and the same key `knows_entity` looks up. So
the gesture produces the **same `Intent::Attack` against the same id**, not an
attack-move to the remembered position. That distinction is the whole point: an
attack-move is a different verb with different behaviour, and "the human has a
gesture that is nearly it" is precisely what the gap already was.

The hover ring follows, and is deliberately driven off the ghost *record*
rather than the live entity: a ring that appeared only for buildings that are
still standing would answer "is it still there?" for free, and walking back
over the rubble is the only thing allowed to answer that. A ghost whose
building has since been razed resolves to a dead entity and the compiler
answers `target N not found` — which is exactly what the bridge already gets
for the same id, and, now that rejections reach the alert stack (below), is how
the player learns their intel was stale.

Covered by `intent::tests::a_remembered_building_is_attackable_by_id`.

### Both seats are told when they are refused

The compiler reaches one verdict, but the two seats read it down different
channels, and for a while only one channel existed. A bridge commander has
always received its errors in the next snapshot's `errors` array. The human at
the keyboard got the identical string written to `IntentErrors`, where only
bridge.rs reads, and then overwritten — same compiler, same verdict, one seat
told and one not. That is the fairness claim failing in the *reverse* direction
from the usual worry, and it made the ghost-attack gesture above nearly
unusable: a stale ghost would simply do nothing.

A `ui`-source rejection is now also pushed onto that team's `GameEvents` feed
as a `Warning`, which the alert stack already renders and `[Space]` already
focuses. Rendering is where the two seats are allowed to differ — a file reader
gets forty lines of history and all the time in the world, a human gets six
rows that fade — but *being told* is not.

The text after the channel tag is byte-identical. The bridge is told
`cmd 3: target 41 not found` because a commander needs to know which command in
its batch bounced; the human is told `order refused: target 41 not found`,
because a gesture is always the one just made. `IntentSource` decides which
renderer hears, and that is the only thing it decides — never whether an intent
was legal.

Because a held mouse button can re-fail at frame rate (where a bridge batch is
a discrete document whose errors arrive once), the channel is rate-limited: the
last dozen distinct messages stay quiet for four game-seconds, and at most two
notices are raised per frame, shared across every gesture in it. One stuck
right-click must never evict "hostiles near base" from a six-row stack.
Covered by `a_refused_gesture_reaches_the_humans_alert_stack`,
`a_held_click_cannot_flood_the_alert_stack` and
`a_bridge_rejection_does_not_touch_the_event_feed`.

### Where `research` is actually enforced

`research` is the one verb whose "is this legal?" answer the compiler cannot
give on its own, and the reason is worth writing down because the next verb
with a per-building lock will hit it too.

The rule is *one job per forge*. The compiler checks it — it refuses a
`research` at a Blacksmith that already carries a `Researching` component, with
`cmd N: building X is already researching attack (24s left)`. But `Researching`
is inserted through `Commands`, so it does not exist until the next flush. Two
`research` commands **in the same batch** therefore both pass that check, and
both reach economy.rs as `StartResearch` events.

So the authority is economy.rs, where the money is: `start_research` keeps a
frame-local set of forges it has already given work to and of `(team, ladder)`
pairs it has already started, and drops the duplicates. Verified live —
`tools/verify_research_bridge.py` sends `attack` and `armor` at one forge in a
single batch and asserts that exactly one rung is bought.

This is not a special case so much as the general shape: **the compiler's
checks are a courtesy that produce a good error message; the system that spends
the resource is what makes the rule true.** `build`, `train` and `upgrade` have
the same division — intent.rs reports "cannot afford", economy.rs is what
actually refuses to pay.

## The replay log

Every submitted intent — applied or rejected — is appended to
`bridge/intent_log.jsonl` (override with `BH_INTENT_LOG`; set it to `0` or
empty to disable). The file is truncated at the first intent of a run, so it is
one file per match. It is opened lazily, so a run in which nobody says anything
leaves no file behind.

Until **wc3clone-jem** an AI-vs-AI headless sim wrote nothing at all, because
`ai.rs` was not a player. It is one now, so a scripted sim produces a full
transcript. Measured across six AI-vs-AI matches on both maps, latency on and
off: **3.7 to 10.4 intents per second across both factions**, i.e. 1.9–5.2 per
team per second against a think tick that runs at 1Hz. A ten-minute match
leaves a 2,000–5,000 line replay. Verb mix of one of them (`open`, seed 42,
478.0s, 1,862 intents):

```
attackmove 1549   harvest 101   train 89   move 65   build 38   cast 8   autocast 7
```

`attackmove` is ~85% of it, and that is a real property of the script rather
than an artifact: it states one order per unit for the army's current job
(see below), and the `defend` branch restates the whole army every tick for as
long as an enemy is standing in the base. The branches that *could* have been
the worst offenders are already self-quieting, because ai.rs only speaks for
units that are idle — a wave in contact, a worker line that is mining and a
base at peace all say nothing. No dedupe layer was added: a few thousand lines
per match is a replay, not a flood, and every suppression rule considered would
have changed behaviour in a bead whose whole claim is that behaviour did not
change.

Volume is also the *reason* for one design decision worth knowing about.
`move`/`attackmove` are the only verbs whose result depends on how many units
one sentence names — `ground_order` spreads a group over `formation_offset`.
Batching the military branches would have cut the log by ~6.6x (3,678 lines to
555, measured on the pre-merge pair — the ratio is the point, not the
absolutes), and it also made the scripted baseline about **40% more lethal**: a
spread line engages with more of itself at once, and `crossings` fell from
~7.6min to ~4.75min. That is a genuine improvement to how the script fights and
a genuine change to every balance number keyed to the baseline, so it did not
belong in a plumbing bead. The script therefore says one `attackmove` per unit,
all naming the same point — the geometry it always had. Nothing is privileged
by that: a human can click units one at a time and a commander can send twenty
one-unit `move`s, and the compiler prices each unit's link identically either
way. Verbs with no geometry (`harvest`, `return`) *are* batched, because there
the two spellings are indistinguishable in the world.

Rejections across all six runs: **zero**. The compiler is stricter than the old
direct path, and the script is written to want only legal things — the one
place the two could have disagreed is build placement, which is why ai.rs snaps
to the compiler's lattice before it vets the ground.

Line 1 is a session header; every line after it is one intent:

```json
{"wall_ms":1786344871503,"session":"wc3clone-intent-log-v1","note":"every player-issued intent, from either interface, in submission order"}
{"wall_ms":1786344893011,"t":21.5,"team":"Claude","source":"bridge","tag":"cmd 0","verb":"move","sentence":"move 2 units to (40.0, 40.0)","ok":true,"intent":{"type":"move","units":[4294968182,4294968185],"x":40.0,"z":40.0}}
```

`sentence` is the half a person reads; `intent` is the half a machine replays.
The sentence deliberately does not say *how* the intent was spelled — that is
in `source`, next to it, so a reader can ask the question but the sentence
never answers it by accident.

A real fragment, from the verification run in this repo:

```
  [  21.5s] Claude/bridge: move 2 units to (40.0, 40.0)
  [  21.5s] Claude/bridge: building 4294968163 rallies to (55.0, 55.0)
  [  21.5s] Claude/bridge: building 4294968163 trains Worker
  [  21.5s] Claude/bridge: 2 units fall back to (70.0, 70.0) below 40% health
  [  21.5s] Claude/bridge: 2 units focus Hero > Siege
  [  21.5s] Claude/bridge: 2 units join squad 2
  [  21.5s] Claude/bridge: squad 2 defends (60.0, 60.0) within 15
! [  21.5s] Claude/bridge: 2 units attack 999999
! [  21.5s] Claude/bridge: worker 4294968182 builds Nonsense at (0.0, 0.0)
! [  21.5s] Claude/bridge: 2 units focus Wizard
```

Rejected intents are kept (`ok:false`, plus an `errors` array). A commander's
mistakes are part of the match record, and the AAR-writing that THESIS.md
principle 5 depends on is better for having them.

**Stamps.** `wall_ms` is Unix epoch milliseconds — real time, the thing that
differs between a human's 200ms and a model's 13 seconds. `t` is game seconds,
matching every other timestamp in the codebase (`Bounty.expires_at`,
`GameEvent.t`). Both are needed: tempo research wants the first, replay wants
the second.

---

## Architecture

```
ui.rs  ──┐                                    ┌── Order / MoveTo        (units.rs)
         ├─→ SubmitIntent ─→ intent.rs ──────→├── TrainingQueue         (economy.rs)
bridge.rs┘   (shared::Intent)  apply_intents   ├── doctrine components   (doctrine.rs)
                                    │          └── Cast/Buy/Use/Surrender events
                                    └─→ intent_log.jsonl
```

- **`shared.rs`** holds `Intent`, `PostureIntent`, `RetreatIntent`,
  `IntentSource`, `SubmitIntent` and `IntentErrors` — the contract, where both
  interfaces can name it, next to the catalog and the doctrine components it
  talks about.
- **`intent.rs`** holds the compiler (`apply_intents`), the validation
  vocabulary (ownership, tech gates, affordability, formation spread, map
  clamping, name parsing) and the log. Registered as `IntentPlugin` in
  `main.rs`.
- **`IntentApply`** is a `SystemSet`, so ordering is explicit rather than
  ambient: `ui.rs`'s whole input chain runs `.before(IntentApply)`, and
  `bridge.rs` brackets it — `poll_commands.before`, `write_snapshot.after`.
  A click or a batch is therefore compiled in the frame it arrived, and the
  bridge's promise that "a batch applied this frame is visible in the snapshot
  written the same frame" still holds.

Downstream consumers see identical components. `units.rs`, `combat.rs`,
`economy.rs` and `doctrine.rs` were not touched by this change at all.

### Validation and the error channel

The compiler validates against the *issuing team*, so no interface can reach
across factions — the check is in one place instead of two, which is the actual
security improvement here. Errors are appended, not returned, so a partially
valid intent (six live units and one corpse) still does what it can, exactly as
the bridge always did.

`IntentErrors` holds per-team validation errors. `bridge.rs` clears its seat's
list when it accepts a new batch and concatenates it onto the seat's own
file-level errors when it writes a snapshot, so the wire format
(`errors: ["cmd 3: …"]`) and the `cmd <i>:` prefix are unchanged. Errors from
`ui`-source intents additionally go to that team's `GameEvents` feed for the
alert stack — one verdict, delivered down whichever channel the seat is
actually reading (see "Both seats are told when they are refused").

**One cosmetic difference:** errors now arrive grouped — commands that failed
to *parse* are listed before commands that failed to *validate* — where they
used to be strictly in command order. The strings, the prefixes and the
contents are identical.

---

## Backward compatibility

Verified end-to-end against a live `BH_BRIDGE=1` seat driven by
`tools/bridge_send.py`:

- `state.json`'s top-level key set is unchanged (15 keys), as are `UnitOut`,
  `BuildingOut`, `MeOut`, `MapOut`, `SquadOut` and every other snapshot struct
  — the diff against master touches none of them.
- Every historical command shape still parses, including the `caster` alias on
  `cast`, the `use_item` rename and the untagged ability selector
  (`intent::tests::legacy_wire_commands_parse` covers every verb and their
  optional-field forms).
- `seq` gating, `last_seq`, the 4 Hz poll and the 1 Hz snapshot are untouched.
- `tools/bridge_send.py`, `tools/bridge_view.py`, `tools/bridge_wait.py` and
  every COMMANDER_BRIEF.md flow work without modification.
- Verified live after the master merge: `upgrade` and both spellings of a
  selector-form `cast` flow through the compiler and land in the log, and the
  16-key snapshot (15 + `fog`) is byte-shape identical.

No new commands were added, and none were removed. **This bead changed no game
behaviour** — it changed how many places can cause it.

---

---

## Legibility: what the round-9 and round-10 AARs cost

*`wc3clone-pbd`, `wc3clone-vjy`, `wc3clone-azo`, `wc3clone-d4y`. Four bugs, no new
verbs and no behaviour change — every one of them was the engine knowing something
and declining to say it.*

The fairness invariant says both seats reach the same verdicts. It says nothing about
whether a verdict is **usable**, and four arena rounds found the gap: a commander who
is refused for a reason they cannot act on has been told no in a language they do not
speak. Each fix below is a string or a field, and each is pinned by a test, because a
teaching error message that nobody asserts on is a comment.

### Where a gate is written is where it must be read

`buildings[].trains` was a bare id list, so the Barracks advertised the Raider with
nothing on that entry to say it waits on a Workshop. The gate did exist — in
`units[].requires`, on the other side of the catalog, behind a join nothing advertised.
A commander reading a *roster* has no reason to suspect a join is needed, and it cost
one their scout timing.

`buildings[].trains_gated` is the same roster with each unit's gate attached:

```json
"trains": ["Footman", "Archer", "Spearman", "Raider", "Knight", "Champion", "Priestess"],
"trains_gated": [
  {"unit": "Footman", "requires": [],          "tier": 1},
  {"unit": "Raider",  "requires": ["Workshop"], "tier": 1},
  {"unit": "Knight",  "requires": ["Castle"],   "tier": 3}
]
```

`trains` is kept verbatim and in the same order — it is the historical shape and tools
read it — and the two are asserted parallel element-for-element, so they can differ in
how much they say and never in what they say. `requires` here is the gate **beyond**
owning the trainer; whatever gates the trainer is the same entry's `requires`, so one
building entry is the whole answer. The Sorcerer is the case that proves the shape:
its own list is legitimately empty, because its gate is on the Arcane Sanctum, and the
Sanctum's `requires: ["Keep"]` is right there.

### A rejection names the building that will accept the order

Round 9's commander met the Raider gate as two true, individually useless sentences:

| where | old | new |
|---|---|---|
| at the Barracks | `Raider requires Workshop` | `Raider trains at the Barracks once a Workshop stands (you have none)` |
| at the Workshop | `Workshop cannot train Raider` | `Workshop cannot train Raider — Raider trains at the Barracks` |

The first, read *at the Barracks*, reads as "wrong building" — so they moved the order
to the Workshop and got the second. Neither ever said **keep training it here**. The new
first string names the Barracks even though the reader is standing at it; that redundancy
is the entire fix.

Two clauses generalise it. On the hall ladder, `you have none` is a lie to somebody
looking straight at their TownHall, so the parenthesis names what they hold and what to
do to it: `Knight trains at the Barracks once a Castle stands (yours is a TownHall —
upgrade it)`. And the wrong-building string carries the *trainer's* own gate, because
the Sorcerer has no building to name until it has one:
`Barracks cannot train Sorcerer — Sorcerer trains at the Sanctum (you have no Sanctum;
it needs a Keep)`.

### A refusal that names no alternative is a refusal to help

`site (56.0, -56.0) is blocked for TownHall` cost both round-9 commanders 20s+ of
probing at 2-unit increments, because it named neither the rule nor a way out. It now
carries both:

```
cmd 0: site (56.0, -56.0) is blocked for TownHall — needs 8x8 clear
       (mines block 6x6, trees 2x2, buildings their own footprint);
       nearest legal: (52.0, -62.0)
```

**The real clearance rule, which turns out to be simpler than it looked.** There is no
"keep away from mines" rule and no clearance ring. A request is clamped to the map
interior, snapped by `snap_footprint` so its edges land on 2.0-unit nav-cell boundaries,
and then every cell the footprint touches must be unblocked (`NavGrid::rect_is_free`).
Cells are blocked by impassable terrain, a gold mine's 6x6 square, a tree's 2x2, and
each standing building's own `size`. Units never block. So the apparent mine-clearance
is just two footprints that cannot overlap: a TownHall (8x8) beside a mine (6x6) needs
its centre 7.0 clear on an axis — which is exactly why the site the eye picks is the
site that fails. `buildings[].size` already exported the footprint; what was missing was
anyone saying it was load-bearing.

The hint is computed on the nav lattice through the *same* `snap_footprint` +
`rect_is_free` pair the rejection just applied, so it is legal by construction rather
than by two functions agreeing — and the test proves it by feeding the hint back through
the compiler and demanding acceptance, rather than by eyeballing the string. Ties break
on `(distance, x, z)` so both seats are given identical advice. Beyond 15 units it says
`no legal site within 15` instead of pointing somewhere useless. This is bridge-seat
parity work: the UI ghost has always previewed and snapped.

### Which win was it

`game_over` has always been the winning team's name or `null`, and round 9's winner
could not tell a razed base from a concession — two endings that call for completely
different AARs.

**The non-breaking shape is a sibling key, and the check that decided it:**
`tools/bridge_view.py` prints `f"{s['game_over']} wins"`, `tools/bridge_wait.py` tests
it for truthiness, and `tools/COMMANDER_BRIEF.md` polls "until `game_over` is non-null".
Turning it into `{winner, reason}` breaks all three at the exact moment a match ends.
So `game_over` keeps its shape forever and `game_over_reason` sits beside it, carrying
`"razed"` or `"surrender"`. It is `skip_serializing_if = "Option::is_none"`, so it is
**absent for the entire live match** and the snapshot's historical key set is untouched
right up to the last tick — `verify_intent_bridge.py`'s exact-key-set assertion runs
mid-match and passes unmodified.

In the engine the pair is one resource with one setter (`GameOver::decide(winner,
reason)`), so a winner with no reason is unrepresentable rather than merely unlikely.
The human's banner sub-line and the headless exit log print the same two words.

**A third reason, and the one value of `game_over` that is not a team.** The time cap
(`BH_MAX_GAME_SECS`) used to stop the process without ending the match, so a capped
round left every bridged seat reading `game_over: null` forever and the documented poll
loop never terminated (`wc3clone-j84`; `wc3clone-0i9` had just fixed the same hang for
the other two endings). The cap now decides through the same setter, with
`GameOverReason::Score` — bank plus the worth of everything still standing, which is the
comparison the timeout has always made and the one `arena_run.py` records. That leaves
the tie, which is the one ending a poller cannot survive as a silence: on the wire only,
a dead-even cap is `game_over: "draw"`. The ledger keeps docs/ARENA.md's spelling (a draw
is an absent winner) and `arena_run.py` translates at the boundary — the wire's job is to
end the loop, the record's job is to name nobody.

Splitting "the match is over" from "somebody won" is the rest of that change:
`GameOver::decided()` is what every gate in the game asks now, and `winner` is asked only
by the things that print a name. They were the same question for as long as a draw was
impossible, and a draw that ended the match in the snapshot while the UI still took
orders is precisely the two-readers-of-one-fact bug docs/FOG.md is written against.

### One sentence standing for four failures

*`wc3clone-d4y`, round 10.* `{"type":"cast","hero":<expansion TownHall id>,
"ability":"CallToArms"}` came back `caster N is not a hero or an own ability building`,
while the identical command worked at the same team's Keep.

**The compiler was right and so was the catalog.** `abilities_of_building` is
`is_hall`-gated, so every rung of the ladder casts; the lookup is a direct
`buildings.get(entity)` with no one-hall-per-team `find`; the snapshot reports
`abilities` on every own hall and ids are plain `to_bits()`. A test with a Keep *and* a
second expansion TownHall passes against the unmodified code.

What was wrong was that **one string stood for four distinct failures** — a dead or
unknown id, an enemy-owned caster, an own building with no ability, an own unit with no
ability — and the wording it chose pointed at the tech tree, the only one of the four
that was never happening. So the commander checked the catalog, found TownHall correctly
listed as a Call to Arms caster, and filed a bug against the roster. Each failure now
answers for itself:

| what the id names | now |
|---|---|
| nothing | `caster N not found — no unit or building has that id (it may have died since the snapshot you read)` |
| the enemy's | `caster N is not yours` |
| own building, no ability | `Farm has no ability` |
| own unit, no ability | `Footman has no ability` |
| own building still going up | `building N is under construction` (unchanged — it was always right) |

The lesson is the one this whole section is about: a diagnostic that cannot distinguish
its cases will be read as whichever case it *sounds* like, and the reader will go and
investigate that one.

---

## Speaking it: English, and "why are you doing that?"

*`wc3clone-ge4`. Two additions, both built on the fact that `Intent` is a value
with a `sentence()` renderer.*

### English is a third spelling, and it lives outside the engine

`tools/intent_compile.py` compiles a natural-language directive plus a snapshot
into a batch of `Intent` objects. It is a **tool, not an engine feature**, and
that placement is the design: the game gains no NLP, no new verb, and no new
mutation path. What it gains is a shorter way to write the same verbs.

```
"hold the northwest ford, forage mid with the cavalry, retreat at 35%"
  -> {"type":"squad","select":"all army","id":1}
     {"type":"posture","id":1,"posture":{"type":"defend","x":-60.0,"z":60.0,"radius":18.0}}
     …
```

**A role goes on the wire as a role.** The clause above says "the army", which
is a *selector*, so the phrase travels and the engine resolves it when the
intent compiles — and the same clause inside a trigger or a plan resolves when
it *fires*. "with the cavalry" names kinds, which no selector spells, so that
one still compiles to ids and goes stale like any photograph. Same rule for the
node a `harvest` gathers (`"target_select":"nearest tree"`) and the footprint a
`build` takes (`"site":"nearest legal site"`, on a landmark but never on
coordinates the commander typed). The tool used to freeze all of it; freezing
was the entire content of red-r23's dead-hero trigger and blue-r23's farm that
reported `site blocked` for a whole match.

Two layers, deterministic first. A pattern table covers the idioms that already
appear in COMMANDER_BRIEF.md and eight rounds of AARs — hold/push/forage/escort,
squad re-tasking, retreat thresholds, focus-fire, leash, autocast, templates,
rally, train/build/harvest, tier-up, research, buy, scout, surrender. Place
names come from the *snapshot*, not a hardcoded table: `map.chokes` gives
"the northwest ford" its position, so the vocabulary changes when the map does.
Whatever the table misses, an LLM fills — `--explain` prints the whole
vocabulary, which is why the file's docstring says it *is* the prompt.

The vocabulary is read from the game, not written down twice. Places come from
`map.chokes`, `mines` and `bounties`; units, their classes and *which building
trains each one* come from `catalog.json` when the seat has one — which, since
bead/polish made the catalog transitively self-sufficient, means the tool's
tech knowledge cannot go stale. It went stale exactly once, in the hardcoded
table this replaced: the Raider moved from the Workshop to the Barracks and
nothing here noticed. The built-in table survives only as an offline fallback.

**The two-hero rule.** Hero slots climb the hall ladder, so `the hero` stopped
naming one unit. It names the *class*, and that is correct for every verb whose
payload is a list — both heroes is a fine answer to `autocast at 3`. The three
verbs that take exactly one unit (`escort`, and `buy`/`use_item` now that both
carry an optional `hero`) **refuse and name the disambiguating words** rather
than taking the first or the nearest. This is the one place a guess is
unrecoverable: an escort posture aimed at the wrong hero sends the army to the
wrong side of the map, silently. `the champion` and `the priestess` resolve it;
the Sorcerer, a caster but not a hero, is deliberately outside the word.

It refuses rather than guesses. An unresolvable place, an unknown noun, a
locked shop rung and a target class the engine does not have are all reported
errors, never a silently different order. **Conditionals** compile to
`trigger_set` — "strike when their hero falls" arms a rule the engine watches
at 4 Hz, which it could not do for as long as nothing in the game had an honest
reading of an enemy hero. The sightings ledger is that reading: whether you
*watched it die* is a fact a human plainly has.

What still defers is a condition outside the predicate vocabulary, and the
neighbouring sentence is the standing example — "strike when their hero is
below 30%". No human can select an enemy hero, so no number about one has ever
been on a screen, and a tool that reached for the nearest predicate would arm a
rule about *our* hero and carry out the opposite order silently. The tool
compiles the action, marks it deferred, and prints the command to run when the
commander sees the condition in `events`. The line between the two sentences is
not "is this about the enemy" but "could a human have seen it".

The confirmation loop is `sentence()`. Compile, send, and the log reads back
what the game understood in English. If the sentence is wrong, the compile was
wrong, and you know before the army arrives.

### Every unit answers "why are you doing that?"

`shared::Provenance` is a `Copy` enum stamped by whoever mints the behaviour,
in the same `Commands` call that mints it — so the answer cannot drift from the
behaviour, because there is no second place that could disagree.

| rung | written by | example |
|---|---|---|
| direct order | `intent.rs`, the eight behaviour verbs | `order:move by bridge t=123` |
| squad posture | `doctrine.rs::run_squad_postures` | `posture:push sq1` |
| standing policy | `doctrine.rs` retreat / leash triggers | `policy:retreat t=210` |
| producing building | `units.rs::spawn_units` via `spawn_provenance` | `template:Barracks#4294968258` |
| engine default | auto-enrolment, idle instinct | `instinct:auto-enroll`, `idle` |

There used to be a sixth rung — `script:wave`, for the scripted baseline, "not
a seat, so its own rung". wc3clone-jem made it a seat and the rung collapsed
into the first row: `order:attackmove by script t=5`, produced by the same arm
of the same compiler as `by ui` and `by bridge`.

Exposed three ways, and it is the *same string* in all three: the snapshot's
`units[].why` (own units only — an opponent's chain of command is their plan),
the human's selection panel, and the intent log, whose order lines carry the
`why` they stamped so a unit's answer and the sentence that caused it are one
grep apart. Introspection is part of the decision surface, so it had to be
equitable too, or one seat could ask a question the other could not.

**Timing convention, once Chain of Command is on** (docs/TEMPO.md): a
`Provenance.at` is always the moment the behaviour *began*, so a delayed order
stamps its **arrival**, not the moment it was spoken — the log keeps the speech
time as its own `t` and the gap as `link`, making the join `why.at == t + link`.

**Why stamping at the mint site keeps paying.** Three branches landed between
this design and its merge — a +1393-line scripted-AI expansion (towers, ford
holds, reactive mixes, Castle, shop use), the ghost right-click attack, and a
second hero class — and none of them needed a new stamp. The AI's new
behaviour is building *placement*, routed through the one `Order::Build` site
that was already stamped; its Castle and shop work emits events, not orders;
its reactive mixes are training-queue pushes. The ghost right-click submits
`Intent::Attack` like every other attack gesture, so it inherits
`mark.order("attack")` from the compiler — which is the fairness invariant
paying for itself, since a UI path that *needed* its own stamp would have been
a UI path that bypassed the compiler. The count of `Order` mint sites is the
count of places provenance must be maintained, and that is a number the
one-compiler layer already keeps small.

Two implementation notes worth keeping:

- **`Order::Idle` is caught once, not eight times.** It is written from eight
  scattered engine systems and always means "the old reason expired", so a
  single `Changed<Order>` system (`doctrine::idle_instinct`) handles all of
  them. Nothing player-facing writes `Order::Idle` — `stop` re-issues a Move to
  the unit's own spot — so it only ever overwrites a reason that has genuinely
  lapsed.
- **A blind forager still says `forage`.** `run_squad_postures` rewrites a
  Forage squad with nothing visible to hunt into a Defend at its muster point,
  but the stamp names the posture the *commander set*, because that is what
  `squads[].posture` reports and a unit contradicting the readout above it is
  worse than a coarse answer.

### Follow-ups this left open

- **The worker panic-flee has no stamp.** It is a `MoveTo` nudge that leaves
  `Order::Harvest` intact, so nothing would ever expire the stamp and the
  worker would blame a five-second sprint for the rest of the match. Giving
  reflex behaviours an expiry is the general fix.
- ~~**`ai.rs` gets `Cause::Script`, not an `IntentSource`.**~~ **Done**
  (wc3clone-jem), and the guess above was right about the shape: the rung
  collapsed into `order:… by script`, an `IntentSource::Script` seat. It was
  wrong about the prerequisite — Chain of Command reached the third seat a bead
  earlier, one layer below the compiler.
- **The tool cannot see money.** It happily compiles a `build` you cannot
  afford; economy.rs refuses it. That is the documented division below, but a
  `me.gold` check would turn a rejected batch into a better error.

---

---

## Co-command: one faction, two authors

*`wc3clone-hre`. The bead THESIS.md was written for — "co-command, where a
human and an AI run one faction and negotiate strategy in a language both speak
natively". Implemented in `copilot.rs`.*

`BH_BRIDGE=copilot` opens **one** seat on `Team::Human` — beside the player at
the keyboard, not opposite them. Its directory is `bridge/copilot/`, its
snapshot is a `Team::Human` snapshot (same fog, same knowability, same
everything), and `tools/bridge_send.py --seat bridge/copilot` drives it with no
changes, because there is nothing new about the transport.

The prediction the previous section made held: two authors on one team really
was "two `SubmitIntent` producers with different `IntentSource`s". `IntentSource`
gained its third variant, `Copilot`, and **every order the second author mints
is attributed with zero new plumbing** — `Cause::Order { source }` was already
stamped at the mint site, `units[].why` already rendered it, the selection panel
already printed the same string, and `ui.rs::why_line` already tallied mixed
answers across a selection. That tally *is* the "did my partner re-task my
push?" readout; it needed no code at all.

The rung is a **seat**, not the old `Cause::Script`. A co-commander pays the
same latency, obeys the same fog and speaks the same verbs; the scripted
`ai.rs` did none of those things at the time, and collapsing the two would have
made "who moved this unit" unanswerable in exactly the case it is asked.
(wc3clone-jem later made `ai.rs` do all three, so it became a seat of its own —
`IntentSource::Script` — rather than borrowing this one. The reasoning is
unchanged: distinct authors get distinct rungs, and there are three of them
now.)

### The real design question was conflict policy

At the engine, conflict policy is unchanged and deliberately so: `Order` is a
component, so **last writer wins**. It has always been overwrite-tolerant —
that is how a human's right-click overrides doctrine's push a second later. A
priority field that made one author's orders outrank the other's would break
the rule that keeps the seats equitable: *source is descriptive, never
authoritative.*

So the deliverable is not arbitration. It is **consent before the fact and
visibility after it**.

#### Consent: proposals, not silent actions

A co-commander's wire carries one shape an ordinary seat's does not:

```json
{"type":"propose","note":"their scout is gone — take the mid mine","commands":[
  {"type":"attackmove","units":[…],"x":0,"z":0},
  {"type":"move","units":[…],"x":10,"z":10}]}
```

It lands in a pending queue (max 4), appears on the human's HUD with the note
and the **compiled sentences** — `Intent::sentence()`, free from this layer, so
the human reads character-for-character what the replay log will write — and
waits 20 game-seconds for a verdict. `[Enter]` approves the oldest, `[Backspace]`
vetoes it, per-card buttons name one exactly, and silence lapses it. All three
outcomes are announced on `GameEvents`, which means the human sees them in the
alert stack *and* the co-commander sees them in its own `events` — one
producer, two renderers, the same rule the event feed has followed since it
existed.

`propose` is **not** an `Intent` variant, and that is a deliberate line: a
proposal is not something a player can *mean*, it is something one author says
to another *about* a batch of meanings. As a verb it would have handed the
human's interface something with nothing to compile and the compiler a case
that changes no game state.

On approval the batch goes through the ordinary compiler, **at approval time,
against the world as it is then** — so a proposal whose units have since died
is refused exactly as any stale command is, with the same strings, into the
same `errors` array. There is no second execution path and therefore no second
set of rules to drift.

#### The direct/propose split, and why it is where it is

| | verbs | why |
|---|---|---|
| **direct** | `squad` `posture` `stance` `template` `priority` `retreat` `leash` `autocast` | doctrine is *advice-shaped already* |
| **propose** | every unit order, all production, all spending, `autopilot`, `surrender` | irreversible, or spends what is not the proposer's |
| **neither** | `ready` | a statement about the clock, not the army — intercepted before the negotiation exists. See § The ready handshake |

The line sits where **the cost of being wrong** is. Vetoing a posture is
trivial — you set another and the squad re-tasks within a second, because a
standing order is a *disposition* the engine re-reads continuously. Vetoing a
spent 400 gold is impossible. So the co-commander may keep the army fighting,
holding fords, falling back at 35% and focusing siege — precisely the
machine-speed work THESIS.md's tempo argument assigns to the engine — while it
may not empty the treasury unasked.

That split is what makes it a partner rather than a nag (propose everything and
it cannot help during a fight) or a stranger with your wallet (do everything and
"co-command" means "handed over"). It is also not a coincidence that the direct
half is exactly the group this document already calls "Doctrine": that grouping
already encoded the property the trust policy needed.

A refused direct command teaches rather than scolds, because the reader is a
model mid-match:

```
cmd 0: 'train' needs the human's approval — it spends or commits what your
partner owns. Wrap it: {"type":"propose","commands":[…],"note":"why"}
```

`BH_COPILOT_TRUST` moves the line for experiments: `full` (everything direct)
or `strict` (everything proposed, doctrine included). The seat reads its own
etiquette out of the snapshot rather than out of the environment — `copilot:
{trust, direct:[…], propose_ttl, max_pending}` — the same principle that makes
the catalog the tech tree.

#### Visibility: what a proposal says it would disturb

Every proposal is tagged with what it would step on, in the human's terms:

```
 ! re-tasks squad 1 (defend)
 ! changes squad 2: push -> defend
 ! overrides your move on 4 unit(s), 6s ago
```

Provenance is what makes this nearly free. Every unit already carries who gave
it its current reason and when (`Cause::Order { source: Ui, at }`), and every
squad already carries its standing posture, so "would this disturb something my
partner set?" is a lookup rather than a new bookkeeping system. The tags are
computed once, when the proposal arrives — they describe the board the
co-commander was looking at when it wrote the note, which is the thing the note
is arguing about.

A partner who re-tasks your push is a partner. A partner who re-tasks your push
*invisibly* is a bug you spend the next minute misdiagnosing.

##### Roles are expanded for the readout, and the readout is dated

*`wc3clone-brq`.* Selectors briefly reintroduced the invisibility they were
meant to remove. A proposal written as `{"type":"move","select":"all army",…}`
carries a *phrase*, and a phrase names no ids, so the scoping the tags are
built from found nobody and the human was shown an empty conflict list under a
batch that was about to take the whole army — the worst possible reading, since
"no tags" is exactly how "this disturbs nothing" looks.

The fix was **not** to teach copilot.rs the selector vocabulary. The preview
runs the batch through `intent::resolve_places` — the same resolver, the same
`LateBind` view of the world, the same empty-match refusal — and tags the
*resolved* copy. A second reader was cheaper than a second vocabulary, and it
cannot drift by construction: there is one statement of what a selector may
see (`intent::LateBindWorld`) and both readers hold it.

Two lines came out of doing it honestly:

```
 ! the roles named reach 7 unit(s) as of now
 ! as of now this would refuse: move: 'all army' matches none of your units
   right now — nothing was ordered
```

The first is the *size* of what is being agreed to, which the sentence alone
never gave — `move all army to north-pass` reads identically at two units and
at twenty. The second is the empty case, which must be said out loud for the
same reason the compiler refuses it out loud rather than moving nobody.

**And one level down** (`wc3clone-e2i`). `resolve_places` deliberately stops at
the top-level intent — a plan's steps and a trigger's `then` are validated when
they *run*, which is what lets an armed rule keep naming *my hero* instead of
freezing an id. That rule is right for the compiler and does not transfer to the
preview, where the question is not "what should this mean later" but "what am I
agreeing to". A proposed five-step opening previewed as scoping **nothing at
all**, while step 3 was about to take the whole army — the same misreading the
paragraph above fixed for direct orders, one rung in. So each step, and each
trigger a step arms, is run through the same resolver and reported beside the
rest:

```
 ! step 1 would reach 5 unit(s) as of now
 ! step 3 does not resolve yet: no region named 'their-expansion' - known places: …
```

A step that names ids says nothing — its sentence is already the whole story.
A step that cannot resolve *yet* is phrased as pending rather than as a refusal,
because that is what it is: the compiler arms such a plan with `chain holds at
step k` (§ *Arm time and late binding*), and warning a reviewer about something
that is going to be fine is how a channel gets ignored. Nothing is written back,
so apply-time behaviour is byte-for-byte what it was.

Both are dated, and so is the whole list by implication. **The preview resolves
at arrival; approval resolves again, up to 20 seconds later, and what lands is
the second answer.** A list of ids cannot change meaning in that window; a
phrase can — the army takes casualties, a worker finishes a barracks, a unit
joins squad 2 — so `as of now` is the difference between advice and a promise
the engine never made. Nothing is written back at preview time, which is what
keeps the two resolutions independent rather than one caching the other.

#### Answering back: a veto has a reason

*`wc3clone-3f7`.* The first cut of this loop was one-sided in a way that is
easy to miss: the co-commander proposed **with an argument**, and got back a
bare "no". Three completely different things hide behind that no — bad timing,
bad idea, bad aim — and they call for opposite next moves. A partner that has
to guess between them guesses wrong, re-proposes into a wall, and becomes the
thing nobody approves.

So a veto carries one of three reasons, and the human picks it **in the same
keystroke that gives it**:

| key | reason | what it asks of the proposer |
|---|---|---|
| `[Bksp]` | `not_now` | the idea is fine, the moment is not — re-propose when conditions change |
| `[Shift]+[Bksp]` | `wrong_target` | the idea is right, the aim is wrong — re-propose elsewhere |
| `[Ctrl]+[Bksp]` | `never` | drop it; do not raise it again this match |

**A held modifier, not a follow-up key**, and that is the whole input decision.
Surrender's "F12 twice within 3 seconds" is the right shape for an irreversible
act: it buys a moment of doubt. A veto is the *safe* answer, the one given
under pressure, and charging two keystrokes for it while approval stays one
builds exactly the wrong incentive into a consent loop. Plain `[Bksp]` is
therefore still one key and means `not_now` — the softest of the three, which
is the right thing to mean when you had no time to modify. Shift and Ctrl were
already this HUD's two modifiers and already meant something close: shift-click
*adds to a selection* (keep the thing, change what it covers → `wrong_target`),
ctrl-digit *binds a control group* (make it standing → `never`). The veto
buttons read the same modifiers, so the mouse can say all three things too.

`never` is **etiquette, not enforcement**. Nothing refuses a re-proposal. That
is the same rule the rest of co-command follows — *source is descriptive, never
authoritative* — and a partner able to silently ban its partner's ideas would
be arbitration by the back door, which this design spent its whole budget
avoiding.

The reason reaches the proposer twice: on the `events` line it reads anyway
(`proposal #5 vetoed (wrong target - re-propose with a different target): hit
their siege`) and in the snapshot's `recent_resolutions` — the last eight
proposals that *left* the queue:

```json
{"id":5,"t":94.0,"note":"hit their siege now","severity":"urgent",
 "outcome":"vetoed","reason":"wrong_target",
 "advice":"re-propose with a different target"}
```

**A tail, not a terminal `status` left on `proposals` for one cycle.** Two
reasons. A one-write status is missed entirely by a seat polling slower than
the snapshot ticks, and "did my partner ever answer #3?" is exactly the
question you ask when you have *not* kept up. And `proposals` keeps meaning one
thing — the queue you can still act on — rather than becoming a mixed list
every reader has to filter. That is also why there is no `pending` outcome:
membership in `proposals` *is* pending, and a status field that restates a list
membership is a second source of truth waiting to disagree with the first.

The `advice` clause is duplicated out of this document deliberately, for the
same reason the refusal above prints the wrapper: a model acting on a veto
mid-match should not need a second file to know whether it may try again.

#### Urgency: the queue is answered in the order that matters

*`wc3clone-3f7`.* `severity: "urgent"` on the wrapper (default `"routine"`)
puts a proposal at the **front** of the queue rather than the back. Oldest-first
was right when four proposals were about the same fight and wrong the moment
they were not: "they are flanking, pull back" and "we should expand" are not
equally answerable at second 40 of a battle.

It changes nothing else. Not what may be proposed — urgency buys attention, not
trust. Not the cap: four is still four, because the cap is about how many
questions a human can hold and marking one urgent does not add attention. The
whole implementation is one insertion index, `copilot::insert_index`, which
places an urgent proposal ahead of every routine one and behind every urgent
one already waiting (urgent-then-oldest: a second urgent proposal did not
become more important by being later).

Keeping `pending` permanently in **answer order** is what made this nearly
free. Index 0 is still "the card `[Enter]` takes" for ui.rs, still the
brightened top card, and still the first entry of the snapshot's `proposals` —
three readers that between them had to learn nothing about severity. `[Enter]`
answering the most-urgent-oldest instead of the plain oldest is not a rule
anywhere; it is what "take index 0" now happens to mean.

The HUD says it twice: the card's spine and headline take the alert stack's
Warning amber, and the header reads `URGENT`. That amber is the one place a
proposal card wears a severity colour rather than the co-commander's violet,
and it earns the exception — urgency is a claim about the *game* ("this window
closes"), not about who is speaking. Because urgent cards sort to the top, the
block of amber at the head of the panel *is* the "answer these first"
instruction, with nothing to read.

A misspelt severity refuses the whole proposal, naming both accepted words.
Silently downgrading to routine would be the worse failure: the proposer
believes it jumped a queue it never jumped, the human never sees it jump, and
nothing anywhere says why.

#### Making the loop measurable: `BH_COPILOT_AUTOAPPROVE`

*`wc3clone-3f7`, closing the "headless has no approver" follow-up below.*
Approval is a human act, so a headless sim has nobody to give it and every
proposal lapses. That is *correct* — and it means the one part of co-command
that is genuinely new was the one part no sim could measure.

Two knobs make it observable without a person:

- **`BH_COPILOT_TRUST=full`** — the control case. No loop at all: every verb
  goes direct, so a sim measures a co-commander *without* the negotiation cost.
  (It already existed as a policy toggle; it works headless because
  `CopilotPlugin` is registered in both branches of `main.rs` and the seat is
  opened by `BH_BRIDGE=copilot` either way.)
- **`BH_COPILOT_AUTOAPPROVE=1`** — a scripted approver that says yes to each
  proposal `BH_COPILOT_APPROVE_DELAY` seconds after it arrives (default 3s).

**The delay is the entire point.** A zero-delay approver would measure a
co-commander with a rubber stamp. What a human's presence reliably costs the
loop is *seconds between the idea and the act*, and whether the board still
rewards the plan after those seconds is the question the proposal loop actually
raises. It approves rather than judges, which is a stated limitation: an
approver that vetoed on some heuristic would be measuring the heuristic.

It is a stand-in for the keystroke, not a new author — the batch goes through
the ordinary compiler and lands stamped `by copilot`, exactly as a human's
`[Enter]` would, so a replay of a scripted sim reads like a replay of a played
one. Queue order is honoured for free, since `pending` is already
urgent-then-oldest. The seat is told: `copilot.auto_approve_after` appears in
the snapshot only when a script is answering, so a co-commander can tell
whether the thing approving it is a person, and the seat's startup line says
`SCRIPTED APPROVER: auto-approving after 3s` — a sim log that does not say
"nobody human answered these" is a result somebody will misread later.

#### Legibility runs both ways: `partner_log`

The human sees the co-commander's directives — they arrive as proposals with a
stated reason. Without something more, the co-commander could not see the
human's, and would be commanding next to someone it cannot hear.

`shared::IntentJournal` keeps the last 40 intents per team in memory — the tail
of `intent_log.jsonl`, not a second record with its own vocabulary — and a
copilot seat serializes its team's as `partner_log`:

```
  [   4.3s] copilot  5 units join squad 1
  [   4.3s] copilot  squad 1 defends (-70.0, -70.0) within 22
  [  17.0s] copilot  attack-move 5 units to (0.0, 0.0)
  [  29.6s] ui       surrender the match
```

Same sentences, same `source` tags `units[].why` carries. Rejected intents are
kept: a partner learning that your last four clicks bounced is a partner that
stops proposing around a plan you never actually issued.

#### What co-command deliberately does *not* change

- **`ExternallyCommanded` stays false for the human's team.** That flag tells
  doctrine.rs "a machine drives this team, so pool its idle units into squad 0
  and seed them a posture" — an autonomy floor that exists to compensate for a
  slow commander. There is no slow commander here; there is a human with a
  mouse who keeps full authority over where their idle units stand. Setting it
  would have the engine quietly start enrolling the player's army the moment a
  partner connected, which is the opposite of asking permission.
- **Autopilot is not touched.** If the player has handed their faction to the
  scripted AI, a co-commander connecting is no reason to take it back for them.
- **`IntentErrors` stays keyed by team**, so a copilot's `errors` array also
  carries the human's refused gestures (`ui: …`). Kept rather than filtered: a
  partner who can see that your click bounced off a stale ghost is a partner
  who can stop proposing around it.
- **Ordinary seats' wire format is byte-shape identical.** `copilot`,
  `proposals`, `recent_resolutions` and `partner_log` are `Option` and skipped
  when absent; `red` and `blue` snapshots keep exactly the keys they had (16,
  verified live). Every addition since has been additive on the copilot side
  only, and the four keys appear together or not at all — including as empty
  lists — so the shape a co-commander parses never changes under it.

### It inherited Chain of Command for free

docs/TEMPO.md §3's order latency (`command.rs`) and co-command were built on
separate branches and met at a merge. Nothing had to be done to make them agree:
an approved proposal is submitted through the ordinary compiler, so under
`BH_COMMAND_LATENCY` a co-commander's orders travel exactly as the human's do,
priced by the same `OrderIssuer` against the same command nodes. There is no
"approved orders arrive instantly" shortcut, because there is no second path
that *could* have one.

That is the same dividend the ghost right-click paid Chain of Command a bead
earlier — a new way of speaking cannot accidentally arrive at a privileged
speed — and it is the strongest argument this document has for the choke point.
Two features that never saw each other's code compose correctly because they
both go through the one place an order becomes real.

### Follow-ups co-command leaves open

- **A proposal's conflict tags can go stale.** They are computed at arrival and
  describe a board up to 20 seconds old. The sentences cannot go stale (they
  are the batch), and approval re-validates everything against the live world,
  so the cost is a tag that over- or under-states — never an order that lands
  differently than it read. Recomputing per frame is the fix if it ever bites.
- ~~**The queue is not prioritised.**~~ **Done** (`wc3clone-3f7`) — see
  "Urgency" above. `severity: "urgent"` was indeed the obvious next rung, and
  it cost one insertion index because `pending` was already the queue every
  reader took index 0 of.
- ~~**A veto tells the partner nothing but "no".**~~ **Done**
  (`wc3clone-3f7`) — see "Answering back" above. The canned reason list won
  over the one-key "not now vs never": three answers, chosen by held modifier
  so the refusal never costs more keystrokes than the approval.
- ~~**Headless has no approver.**~~ **Done** (`wc3clone-3f7`) — see "Making the
  loop measurable" above. Both options the bullet named now exist:
  `BH_COPILOT_TRUST=full` as the no-loop control, and
  `BH_COPILOT_AUTOAPPROVE=1` as a scripted approver with a deliberate delay.
- **The scripted approver never says no.** It measures *delay*, which is the
  part of a human's presence that generalises; it cannot measure how a
  co-commander recovers from a veto, which is now the more interesting
  question given the reasons above. An approver that vetoed on a heuristic
  would mostly measure the heuristic — a replay-driven one, answering the way a
  recorded human did, is the honest version and needs a recording first.
- **`never` is not remembered.** The engine files it in `recent_resolutions`
  and forgets it after eight more answers; nothing refuses a re-proposal of an
  idea already refused forever. That is deliberate (enforcement would be
  arbitration), but a *tag* on the proposal — "you were told never about this
  once already" — would keep it etiquette while making the lapse visible to
  both authors.

---

## Triggers: `when` as a first-class word

*`wc3clone-pec`. Two verbs, nine predicates, one new file (`trigger.rs`), and
no new way to change the game.*

Doctrine relocated **continuous** fast work into the engine: retreat below 35%,
hold this ring, focus the siege, forage that cache. Eight rounds of AARs say it
worked. What it never covered was **reaction**, and the gap had a shape:

> A commander who wanted to answer a base raid had to read `events`, notice the
> line, decide, and speak. For a language model that loop costs ten to fifteen
> seconds *every time*. A human at a keyboard pays 200ms for the same answer.

That difference is not judgment. It is polling latency, and THESIS.md's own
principle 3 says what to do about it: the engine does what is fast. A trigger is
a condition the engine watches at 4 Hz and an intent it submits the instant the
condition holds — **for whichever player armed it**. Doctrine is the engine
doing something continuously; a trigger is the engine doing something *when*.

### The action is any intent, and that is the whole design

`then` is an `Intent`. Not a small private list of "things a trigger may do" —
that would be a second vocabulary, and this document exists because two
implementations of one language is two languages. A fired trigger goes through
`apply_intents` like anything else: same ownership checks, same fog rule, same
tech gates, same `errors` array, same `intent_log.jsonl` line. `trigger.rs` has
exactly one power, which is to write `SubmitIntent`.

The consequences are all free. A trigger that names a dead unit is refused with
the string that verb always produces. A trigger that attacks something the fog
no longer shows is refused by the same `knows_entity` call. A trigger that
spends money it does not have is refused by economy.rs on the frame it tries to
pay. Nothing about triggers needed to learn any of that.

### The predicates, exactly

Fourteen, and the constraint that produced the list is worth stating: **every one is
answerable from state the frame already has, for the arming team, with no new
bookkeeping.** No event subscriptions, no history, no memo. A predicate that
needed its own record-keeping would be a predicate whose truth could drift from
the world, and the whole value of firing at machine speed is that the world is
what fired it.

| `when` | True when |
|---|---|
| `base_under_attack` | Any of **your buildings** has a `LastDamaged` inside `BASE_ATTACK_WINDOW_S` (8s). Buildings only — a skirmish in midfield is not the base being attacked. Half-built ones count: losing an expansion under construction is exactly this raid. |
| `hero_below {frac}` | **Any** of your living heroes is under `frac` of max health. Any, not "the" — hero slots climb the hall ladder, and the useful reading of "save my hero" is "whichever one is dying". |
| `hero_above {frac}` | **Every** living hero of yours is at or above `frac` of max health, and you have at least one. The wait-condition of a chain — "turtle until the hero is healed" — and deliberately **not** `not hero_below`, which this vocabulary could not spell anyway (no `and`, no `or`, no `not`; see *What this leaves open*). Two departures earn it its own name: a **dead** hero is not a healed hero, so an empty roster is false rather than vacuously true, and a chain waiting on it never advances at the instant the hero falls; and it asks about **all** of them rather than any, so a fresh second hero cannot release a wait that the first one is still crawling home from. With at least one hero alive the two predicates are exact complements; with none alive both are false, which is the honest answer to both questions. |
| `squad_below {id, frac}` | Squad `id`'s living members hold, **pooled**, less than `frac` of their combined max HP. Pooled because a squad is a formation: one wounded footman in a healthy line is not a squad in trouble. **False for a squad with no living members** — a squad that is gone cannot be hurt, and firing a rescue at a corpse pile is worse than firing nothing. |
| `enemy_sighted {class?, count}` | You can **see** at least `count` enemy units right now, optionally of one `TargetClass`. Fog-honest: counted against your own `FogGrid::sees`. Remembered buildings do **not** count — remembering where a barracks stood is not the news that an army came out of it. |
| `bounty_spawned` | A neutral cache exists **and you can see it**. The same `fog.sees` filter the snapshot's `bounties` array uses, so the rule sees exactly the caches its owner is shown. |
| `mine_dry` | A gold node with `remaining == 0` lies within `MINE_HOME_RADIUS` (40) of one of your **completed** halls. Mines are neutral and unowned, so "our mine" is defined by geometry: the one your hall was placed to work. |
| `tier_reached {tier}` | `TechTiers::get(you).level() >= tier`. |
| `unit_count {kind, count}` | You field at least `count` living units of `kind`. |
| `game_time {at}` | The match clock has passed `at` seconds. The one predicate about nothing in the world — it is here because "expand at six minutes" is a plan every commander already writes, and as a trigger it stops depending on remembering. |

| `enemy_army_seen {size, within_s?}` | Your **intel ledger** holds at least `size` enemy troops that were observed as one concurrent force (`FogGrid::army_groups`). Reads MEMORY, unlike `enemy_sighted` — which is the point: an army does not stop existing because your scout died, and a rule that disarmed itself at that moment would disarm itself exactly when the enemy wanted. `within_s` bounds how stale the observation may be. Workers never count toward a force. Carries no region: regions are a different vocabulary, and a predicate that grew its own notion of "where" would be the second implementation this project keeps refusing to write. |
| `enemy_hero_down {class?}` | An enemy hero class is **currently believed dead** — you watched one die and have not seen it alive since. A *level* predicate over a belief, not an edge over an event; see below. |

| `supply_capped` | `shared::supply_headroom` is zero for you: `supply_cap - (supply_used + queued)`, where `queued` is the supply cost of everything standing in **your production queues**. Counting the queue is what makes it fire *at* the stall rather than a mining trip after it — economy.rs will not pay for a front item whose supply does not fit, so a team with four Footmen queued into two free supply has already stopped producing while its ledger still reads room. **False while `supply_cap` is 0**, which is "no completed supply building yet" rather than "blocked": the cap is recomputed every frame from standing buildings, so a rule without that guard would fire on frame one of every match, for everyone. ui.rs's supply-blocked badge draws the line in the same place. Arena round 17 asked for this one by name — see `arena/r17/blue-aar.md`, complaint 2. |

**What is deliberately missing** is anything about the *enemy's* internals —
their gold, their tech, their hero's **health**. Not an oversight and not a fog
problem you could scout your way around: those are facts no observation
produces, so a predicate over them would be an information right the human does
not have. A human cannot even select an enemy hero — ui.rs's pickers skip
anything that is not theirs — so no number about one has ever been on a screen.
`tools/intent_compile.py` accordingly still **defers** "strike when their hero
is below 30%".

What it no longer defers is **"strike when their hero falls"**, and the
distinction is the whole of what the intel bead bought. Whether their hero
*died in front of you* is not an internal fact; it is the most public thing that
can happen on a battlefield, and the sightings ledger records it the same way it
records everything else — because one of your units was looking. So the honest
predicate was never "their hero is hurt" but "their hero is believed dead", and
once that was writable the sentence compiled. The line between the two requests
is not *is this about the enemy* but *could a human have seen it*.

`enemy_hero_down` is a **level** predicate, and the wording matters. Armed
`once` — the normal case — it fires on the first sweep after the death is
witnessed and disarms, which is the edge behaviour "when their hero falls"
means, obtained without the engine keeping an edge-detection latch nobody can
inspect. Armed with a `repeat` it re-fires while the belief stands, which reads
as "keep pressing while they have no hero" and is a coherent second order. The
belief is revocable: heroes revive through `HeroRecords`, so seeing the hero
alive again returns the status to `alive` and a re-armed rule fires on the next
death actually witnessed.

### once / repeating / spent

`repeat` absent ⇒ the rule fires **once** and disarms. `repeat: 60` ⇒ it fires,
stamps `last_fired`, and goes quiet for sixty game-seconds.

A spent once-trigger **stays in the list**, marked `spent`. Deleting it would
make "did my rule ever fire?" unanswerable from the snapshot, and that is the
first question anybody asks. Re-sending a `trigger_set` under the same name
re-arms it, which is also how a commander revives one.

### The cap is eight, and it is doctrine rather than programming

`MAX_TRIGGERS_PER_TEAM = 8`. The reasoning is the whole point of the number:
every trigger is a rule that fires while nobody is watching, and a player who
cannot recite their own rules has stopped commanding and started debugging. The
losing AAR would blame the engine. Eight also happens to fit everywhere it has
to — one HUD line, one snapshot array a model re-reads each poll, one 4 Hz sweep
nobody has to think about the cost of.

Two things keep the bound real:

- **The cap counts distinct names.** `trigger_set` replaces by name *in place*,
  so tuning one number never costs a slot — and the tool's auto-derived names
  are stable across phrasings for exactly this reason.
- **A trigger may not arm or clear a trigger.** Refused at compile time. Without
  it, one rule could re-arm seven others forever and eight would be a starting
  balance rather than a limit. It is also the line between doctrine and a
  scripting language, which is the line this whole feature is standing on.

### The frame slot, reasoned out

`SimSet::Think`, `.after(FogSet)`, on a 250ms timer — the same heartbeat as
`doctrine::trigger_retreat`, which is the closest analogue (the other thing in
`Think` watching for a threshold to be crossed).

Against `SIM_ORDER` (`Deaths → Fog → Input → CoCommand → AiThink → Think →
Intent → …`):

- **After `Deaths`** — a predicate must not count a corpse.
- **After `Fog`** — `enemy_sighted` and `bounty_spawned` read the grid the
  snapshot and the HUD are about to be built from. Any earlier and a rule reacts
  to last frame's knowability.
- **Before `Intent`** — this is what makes a trigger *fast* rather than merely
  automatic: the intent it submits is compiled in the **same frame**, so the
  whole distance from "the hall took damage" to "the squad is moving" is one
  tick plus the cadence.
- **`Think`, not `Input`** — `Think` is where standing policy lives. It also
  gives the right precedence: doctrine.rs writes `Order`s *inside* `Think` and
  the compiler runs *after* it, so a fired trigger overrules the posture
  executor for that tick. Correct — a rule written for this exact situation
  should beat the continuous policy it was written to interrupt.

One honest consequence, stated rather than discovered: an intent submitted in
`Think` is read by the compiler *after* the ones ui.rs and bridge.rs submitted
in `Input` this frame. On the rare tick where a player clicks in the same 250ms
window their own rule fires, the rule lands last. `Order` is a component and
last writer wins everywhere here (*source is descriptive, never authoritative*),
so this is the existing rule rather than a new one, and speaking again is a
quarter of a second away.

### Latency: a trigger pays nothing, and that is the point

docs/TEMPO.md's verb table exempts every doctrine verb on one rule — *standing
orders are local; direct orders travel* — because a unit under standing policy
already has its orders and does not need to ask. A trigger is standing policy
whose condition came true: **its author paid the reach when they armed it**, and
charging the link again on firing would price one reach twice.

It also restores the mechanism's own incentive one rung further out. C4 says
doctrine must be strictly better than micro at range; with triggers exempt,
*pre-arming a rule is strictly better than hand-answering an alarm at range*,
which is the same argument applied to contingent work. The exemption is spelled
as its own constructor (`CommandLink::exempt_issuer`) rather than a boolean, so
"who is allowed to skip the link" is a question with a findable answer.

### Provenance: a new rung, because it is a different answer

A trigger-fired order answering `order:move by bridge` would be claiming that
somebody decided to move this unit *just now* — exactly what did not happen. So
`Cause` gained a rung:

```text
order:move by bridge t=123            a player said so, and when
trigger:home-guard move by ui t=41    a rule they armed earlier fired
```

The seat is still named. A trigger has an author and the engine is only its
executor, so `source` stays descriptive in the way every other rung is. The
name is a `TriggerName` — bounded ASCII in a `Copy` scalar — because `Cause` is
an allocation-free `Copy` enum by design and a `String` there would have cost
every unit in the game a heap pointer to answer a question about one of them.

The join with the replay log still holds character-for-character: the log
renders its `why` at `t + link` through the same `IntentMark`, so
`why.at == t + link` remains true for a trigger the same way it is for a click.

### Both renderers hear it fire

Every fire pushes one `GameEvents` line on the **owner's** feed:

```
trigger home-guard fired: squad 1 defends (-70.0, -70.0) within 26
```

`Info`, not `Warning` — whatever the rule reacted to has already raised its own
line at its own severity, and this is the calmer follow-up saying what was done
about it. One producer, two renderers: the human reads it in the alert stack and
the commander reads the identical string in its snapshot's `events`. A trigger
would otherwise be the one thing in the game that changes the board without
saying so.

A refusal is routed the same way every refusal is, with one word changed: a
`Ui`-armed rule that bounces raises `trigger home-guard refused: …` rather than
`order refused: …`, because the player made no gesture and sending them to look
for a click they never made is a worse answer than none.

### The seats, honestly

| seat | authoring surface |
|---|---|
| bridge / copilot | full — fourteen predicates × any ordinary verb, as JSON |
| `tools/intent_compile.py` | full-ish — "when X, Y" over the same fourteen, in English |
| human at the keyboard | **one preset**: `[I][H] Home guard`, plus a readout of every armed rule |

**This is a real asymmetry and it is the first one this document has had to
report in that direction since docs/TEMPO.md §2.0.** The human's `[I][H]` is a
toggle that arms `base_under_attack → posture defend` on the selection's squad
at the nearest hall, repeating every 30s, and presses again to clear it. The
selection panel lists every rule the team has armed with its state
(`armed` / `cooling` / `spent`), including ones a co-commander set. That is the
whole surface.

What the preset is *not* is a capability gap in the sense that matters: every
intent it produces is one a commander could have typed, byte-identical, and the
test that proves it is `the_home_guard_preset_is_a_trigger_a_commander_could_have_typed`.
What the human lacks is the *authoring* — fourteen predicates and a free choice of
action — and a full custom authoring UI (predicate picker, action picker,
click-to-place) is v3-later work, deliberately not attempted here.

Two things make the gap narrower than the table looks. The English compiler is
not a bridge-only tool — it emits `Intent` values against a snapshot, and a
human running it against their own seat writes the same rules a commander does.
And the *reading* half is symmetric already: the HUD line and the snapshot's
`triggers` array are built from the same resource and say the same things.

The gap that would actually matter is the reverse one — the human unable to see
or stop a rule their partner armed — and that one is closed: the readout is
team-wide, and `trigger_clear` with no name is one keystroke away from being
bound if it is ever wanted.

### Co-command: `trigger_set` needs the human's approval

The direct/propose split (below) puts doctrine on the direct side because a
posture is *advice you can overwrite in a second*. Triggers sit on the
**propose** side, and the reason is the action rather than the rule: a trigger
whose `then` is `train` or `attack` is an irreversible act that has merely been
postponed, and it is *harder* to veto than the immediate version because it
happens when nobody is looking. The line stays where the cost of being wrong is,
which is where it always was. `DOCTRINE_VERBS` is untouched.

### The snapshot

```json
"triggers": [
  {"name":"home-guard",
   "when":{"type":"base_under_attack"},
   "then":{"type":"posture","id":1,"posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}},
   "repeat":30.0, "status":"armed", "last_fired":112.4,
   "sentence":"when the base is attacked: squad 1 defends (-70.0, -70.0) within 26 (trigger: home-guard, repeating every 30s)"}
]
```

Own team only, and for a stronger reason than the usual one: **a trigger is a
plan**, and an opponent's contingency plans are the single most valuable thing a
snapshot could leak. In the engine the resource is split by team and
`write_seat_snapshot` is handed a pre-sliced `&[TriggerRule]`, so it cannot read
the other faction's even by accident.

`when` and `then` are the **same JSON you sent**, round-tripped through `Intent`
rather than re-described in prose — so a commander reads a rule out of the
snapshot, edits one number, and sends it back under the same name. A prose
summary would have been a second spelling of the language.

Not sorted, unlike every other list in the snapshot: the order is the order they
were set, which is the order they fire in when two come true on the same tick.
Sorting would hide the one thing about the list that is load-bearing.

`skip_serializing_if` empty, like `command_nodes` — a seat that has never spoken
the word sends exactly the historical sixteen keys.

### What this leaves open

- **A full authoring UI for the human.** Named above; the honest gap.
- **No predicate composition.** No `and`, no `or`, no `not`. One condition per
  rule, deliberately: a boolean algebra is the point where this stops being
  doctrine. Two rules with the same action is the workaround and it costs two
  of the eight slots, which is the right price.
- **No "when it stops being true".** Every predicate is level-triggered, so a
  repeating rule re-fires on its cooldown while the condition holds rather than
  once per crossing. For `base_under_attack` (an 8s window) that is what you
  want; for a long-lived condition like `tier_reached` a repeating rule is
  almost certainly a mistake, and nothing stops you making it.
- **A trigger cannot name a unit that does not exist yet.** `then` is frozen at
  arm time, so "when I have 8 footmen, attack-move them" has to say a squad
  rather than a list of ids. Squads are the right answer and the reason
  `squad_below` and the `squad N defends X` NL rule are here — but it is a real
  edge and the plans bead met it again — see § Plans, "The late-binding
  problem", which answers it with the squad idiom rather than a new selector.

---

## Plans: `then` as a first-class word

*`wc3clone-c5b`. Two verbs, three advance-conditions, one new file
(`plan.rs`), and no new way to change the game.*

Doctrine relocated **continuous** fast work into the engine. Triggers
relocated **reaction**. Neither of them can say ORDER, and order is what a
build order is:

> "Barracks, then the keep, then a sanctum, then sorcerers" is a sequence a
> commander settles before the match starts and then spends the first six
> minutes hand-feeding to the engine, one command per poll. For a language
> model that is ten to fifteen seconds *per step of a sequence with no
> decisions left in it*. A human at a keyboard pays a keystroke.

That difference is not judgment either — it is transcription. A plan is named
ordered steps the engine walks for you, submitting each step's intent through
the ordinary compiler when its turn comes.

### The step/advance grammar

A step is `{intent, advance}`. The intent is any verb; the advance
says how the engine knows it is time for the next one. Three forms, and the
middle one is the seam that matters:

| `advance` | means |
|---|---|
| omitted / `{"type":"on_applied"}` | as soon as this step is **accepted**. The plain meaning of "then". |
| `{"type":"when","when":{…}}` | when a **`TriggerWhen` predicate** holds — the *same* predicates triggers use (fourteen of them as of the chain bead, and whatever the next one adds), level-triggered, evaluated by the same function at the same 4 Hz. |
| `{"type":"after","secs":30}` | 30 seconds after this step was accepted. |

*Accepted*, not *completed*: the engine does not wait for the barracks to
finish, it waits for the order to be legal and taken. Waiting on completion is
what the `when` form is for (`tier_reached`, `unit_count`), and conflating the
two would make "then" mean something different for every verb.

**The advance-condition of step *k* governs the move to step *k+1*.** The last
step's advance decides when the plan reports itself finished.

### The predicate seam is the whole reason `when` is not its own vocabulary

`PlanAdvance::When` carries a `TriggerWhen`, and `plan.rs` answers it by calling
`trigger::holds` — the same function, on the same world, at the same cadence.
This was the one design decision worth being careful about, and it pays off
twice:

* A plan and a trigger cannot disagree about what "we reached tier 2" means.
* **Any predicate a later bead adds is a plan advance-condition for free.** The
  territory and intel beads landing beside this one add `TriggerWhen` arms; the
  moment they do, `plan_set` accepts them with no work in `plan.rs`, because
  `holds` is the only thing that reads the enum and `validate_predicate` is the
  only thing that checks it. Neither lives here.

### Failure semantics: blocked, then halted, **never skipped**

A step's intent is frozen at `plan_set` time and compiled when it runs, so it
can be refused. The engine has three options and only one is defensible:

* **Skip and carry on.** Refused. A plan that quietly drops the Blacksmith and
  goes on to research at it is worse than one that stopped, because its owner
  reads `running` and believes the sequence they wrote is the sequence that ran.
* **Halt immediately.** Too brittle. Most refusals are *timing*: forty gold
  short, a worker mid-walk, a hall one tick from finishing. Halting on those
  would make plans useless for exactly the economic sequencing they exist for.
* **Block, retry, then halt.** What it does. The plan stops advancing, its
  status becomes `blocked: <the compiler's own error, verbatim>`, it re-submits
  the same step every `PLAN_RETRY_S` (5s), and if it is still refused after
  `PLAN_BLOCK_GRACE_S` (60s) it becomes `halted: <error>` and stops for good —
  **on the step that failed**, which is where a reader needs to find it.

The grace window was **ten seconds until the canonical opening was run against a
live seat and died of it**: the plan reached its `upgrade` step short on lumber,
blocked correctly, and halted twenty seconds before the income that would have
paid for it arrived. Ten seconds is right for the reason it was chosen (a worker
mid-walk) and wrong for the reason plans exist — economic sequencing, where the
dominant refusal is "not affordable yet" and money moves on a scale of tens of
seconds. Halting later costs nothing, because the *owner* is told at the first
bounce; the constant governs only how long the engine keeps trying.

**A step that reached some of its targets is a partial success, not a refusal.**
`own_units` reports every dead id and returns the survivors, so `move [a,b]`
with `b` a corpse really does move `a`. Treating "any error" as "refused" made a
plan block on the most ordinary event in the game — a squad member dying between
`plan_set` and the step — and then halt a sequence that was running correctly.
The compiler therefore reports whether it *reached* anything, and only a step
that reached nothing blocks. The error still goes to every other channel.

### Chains: a plan whose steps are stances

*`wc3clone-0uu.6`, and the shortest bead in this document, because the answer
was "you can already say that".*

docs/AFFORDANCES.md asks for a way to write *"turtle until the hero is healed,
then secure the northwest mine"* — steps that are stance transitions with
wait-conditions, the **pre-armed policy** tier that moves a commit/withdraw
decision from fire time to arm time. That is a `plan_set` whose steps are
`stance` intents and whose `advance` conditions are predicates, and every part
of it already existed: a `PlanStep` carries any `Intent`, `stance` is an
`Intent`, and `PlanAdvance::When` carries a whole `TriggerWhen`. So there is no
`stance_plan` verb. A second spelling of a sentence the language already has is
the second implementation this project keeps refusing to write, and it would
have needed its own cap, its own snapshot array and its own failure semantics
to say what two words of documentation say instead.

What the bead added was the one word the wait was missing (`hero_above`, above)
and one rule about *legibility*:

**Per-step readiness is reported at arm time, and reports nothing else.** A
chain is written precisely when its target is not knowable yet — the expansion
you have not scouted, the anchor you have not named — so refusing a plan whose
step names unresolvable ground would refuse the sentences the feature exists
for. Instead `plan_set` **dry-runs `resolve_places`** over each step's intent
against the world as it stands and, for each one that cannot resolve *yet*, adds

    chain holds at step 2: no region named 'their-expansion' - known places: … —
    plan hold is armed anyway; the step resolves when its turn comes, and blocks
    there if it still cannot

to the setter's error channel. The plan is armed either way; nothing returns.
Three properties are load-bearing:

* **It is the same resolver, so it cannot disagree with fire time.** The reason
  printed at arm time is character-for-character the reason the step would block
  with. A second "is this resolvable?" predicate would be a second opinion about
  the first one, and the two would drift.
* **It is an edge, not a level.** One line, at the moment of arming, about a
  step's readiness *then*. The continuous rendering of "this step cannot run" is
  what `plans[].status` already does, once the step is the current one.
* **It teaches without gating** (§ Legibility, and the compiler/payer split):
  the message names the fix — `region_set`, or the menu of places this seat can
  already speak — and the *step* remains the thing that decides, when its turn
  comes, whether the sentence is true.

### Arm time and late binding

*`wc3clone-8hu`. Which half of a sentence gets judged now, and which gets judged
when its turn comes.*

A plan step has two places a name can appear: its **target** (`stance ... target:
"staging"`) and its **advance predicate** (`enemy_in` over `"staging"`). For one
release they were judged by opposite rules — the target armed-and-taught, per the
block above; the predicate refused the entire plan. A commander writing

> push until twelve of them are in staging, then stage there

got a notice for one half of one sentence and a refusal for the other, over the
same word about the same ground. Worse, a plan step *may* `region_set` — that
verb is banned from a trigger's `then` and deliberately not from a plan step — so
"name the staging ground, then wait for twelve of them to reach it" was a
coherent sequence the compiler would not accept.

So the rule is now split by **what kind of thing is wrong**, not by which channel
it is in:

| | judged at arm time | late-bound |
|---|---|---|
| plan step | the predicate's **shape** — a count of zero, a misspelt target class, a fraction outside (0,1] | the predicate's **place**, and the step's own target |
| trigger | shape **and** place | its `then`, entirely |

Shape is a typo: no amount of later world turns `"count":0` into a sentence, so
refusing it is the only honest answer. A place is *vocabulary*, and vocabulary
grows during a match.

**A trigger keeps the arm-time refusal**, and that is not an oversight. A trigger
has no earlier step to name ground in; its `then` may not name ground at all; and
it is one statement a commander re-sends in one line, so a refusal costs a line
rather than a sequence. A plan is the construct explicitly written before the
world it describes exists.

**Late binding is only honest because the hold is audible.** `holds()` answers
`false` for a region it cannot find — correct, and completely silent — so a plan
armed over an unnamed place would otherwise sit `running` on step 2 forever,
which is precisely the 3 a.m. failure the arm-time refusal used to prevent. The
lesson is therefore paid twice:

* at arm time, the same `chain holds at step k: …` notice the target channel
  gives, with the resolver's own words and the menu of places;
* at the step's turn, `PlanState::Held` — announced once on the edge, carried
  continuously in `plans[].status` as `held: <why>`, and announced once more as
  `no longer held` when the name becomes a place.

`Held` is deliberately **not** `blocked`. `blocked` means the compiler refused
the step, so retrying it is right and halting after `PLAN_BLOCK_GRACE_S` is the
honest end. Here the step *ran*: re-submitting its intent would re-order an army
that is already doing as it was told, and a missing place name is a thing a
commander can still supply at minute ten. A held plan waits, and says so.

### Transitions are announced; states are displayed

*`wc3clone-vax`, and the sharpest edge any vocabulary in this document has cut
its own user on.*

Both states carry the reason in the status itself, so nothing has to be
correlated against `errors`. What they do **not** do is repeat themselves. A
blocked step is announced exactly **once** — on the transition into `blocked` —
plus once more if the reason changes to different words, once as
`plan <name> step k/n unblocked` when it recovers, and once on `halted`. The
twelve retries in between emit nothing, on any channel: not the seat's `errors`
array, not the event feed, not the replay log, not the alert stack.

The retry cadence itself is unchanged. `PLAN_RETRY_S` still re-submits the same
step every five seconds, because the dominant refusal is timing and retrying is
how a plan survives it. Only the **emission** changed.

This is not a cosmetic preference; it is the mechanism that lost arena round 17.
BLUE's `army` plan sat on `cannot afford Footman (135g 0l)`. The compiler
re-appended that string to the seat's `errors` array on every retry,
`tools/bridge_wait.py` woke on every one of them, and the commander's event loop
became a fire hose. They escaped it the only way the tooling allowed — chaining
`bridge_wait` calls — and went ~100 game seconds without issuing an order, with
2280 gold banked and supply hard capped. The match was decided in that gap. The
AAR's first complaint reads: *"that punished me for using the feature well."*

The fix is the distinction between an **event** and a **condition**, applied to
one channel each:

* `events` and `errors` are **edge**-triggered. They interrupt. A thing that is
  still true is not a new interruption.
* `plans[].status` is **level**-triggered. It reads `blocked: <why>` in every
  snapshot for exactly as long as it is true, so nothing is hidden by the
  silence and a reader who wants to know can always look.

`Plans::report` returns a [`PlanVerdict`] so the compiler can tell the two
apart — `Blocked` (news) from `BlockedAgain` (the cadence talking) — and a
`BlockedAgain` submission stops before any channel sees it. `bridge_wait.py`
independently fingerprints the error **set** and refuses to wake on one it has
already shown, which is belt and braces on purpose: the engine fix is the right
one, and the pacing tool should not have been trusting its input to be
well-behaved either.

The one thing edge-emission owes its reader in return is the **other** edge: a
plan that announced a block and then recovers says so. Told once that the army
plan is stuck and never told it came unstuck, a commander would have to poll
`plans[].status` — which is the polling this whole layer exists to delete.

The verdict reaches the plan through `SubmitIntent::plan` and `Plans::report`,
in the same frame the step was compiled — not by scraping the error channel for
a tag. A plan that could not tell "accepted" from "refused" would have to either
skip or wedge, and both are the failure above.

### Once through, never looping

A plan does not repeat. Repetition is a trigger's `repeat`, and a construct with
sequencing *and* iteration is a programming language with no debugger — which is
the thing the caps exist to refuse.

### The caps: two plans of eight steps

Eight steps matches the trigger cap for the identical reason: it is the length
of a sequence a person can recite. **Two plans** is one notch tighter than the
eight triggers, and deliberately: a plan is a *sequence* running unattended, and
two plans stepping over each other's build sites and squad ids is much harder to
read out of a snapshot than two triggers that each fire once. Two is also what
commanders actually want — an economic opening and a military follow-up.

Replacing a plan by name is free and restarts it from step 1, so iterating on an
opening never costs the other slot. The cap counts *live* plans: a `done` or
`halted` plan is history rather than policy and stops holding a slot, while
staying readable in the list.

### Plans get their own storage, not eight chained triggers

The obvious implementation is "a plan is N chained triggers". It is wrong.
`MAX_TRIGGERS_PER_TEAM` is 8 and it is doctrine, not a budget — the number of
rules a commander can hold in their head. A five-step plan that ate five of
those slots would make two features compete for one scarce thing while being
about different halves of the same sentence. `Plans` is its own resource with
its own cap and its own evaluator, so arming a plan never costs a trigger and
reading your triggers never means reading a plan's internals.

### The deferral graph is two rungs deep and never points back up

| from | may defer | may not |
|---|---|---|
| a trigger's `then` | any ordinary intent | `trigger_set`, `trigger_clear`, `plan_set`, `plan_clear` |
| a plan's step | any ordinary intent, **including `trigger_set`** | `plan_set`, `plan_clear` |

A plan step arming a trigger is a real idiom — "build the barracks, then arm the
home guard" — and it stays bounded because a trigger cannot defer anything
further and `Triggers::set` still refuses the ninth. Remove the trigger→plan
refusal and a trigger could set a plan whose step re-armed the trigger, forever;
that is why the trigger refusal was widened rather than left alone.

### The late-binding problem, and the idiom that answers it

A step's intent is frozen at `plan_set` time, exactly like a trigger's `then`.
So a step **cannot** say "the eight footmen I will have by then" — there is no
id to write, and there is no selector in the language that would produce one.

The trigger chapter above flagged this as "a real edge the plans bead will meet
again". It does, and the answer is that **the language already has a
late-binding selector: the squad.** `template` stamps every unit a building
trains into squad 2; `posture` addresses squad 2 *by number* and its membership
resolves when the step executes. So the idiom is:

```json
{"type":"plan_set","name":"army","steps":[
  {"intent":{"type":"template","building":<barracks>,"squad":2}},
  {"intent":{"type":"train","building":<barracks>,"unit":"Footman"}},
  {"intent":{"type":"train","building":<barracks>,"unit":"Footman"}},
  {"intent":{"type":"train","building":<barracks>,"unit":"Footman"},
   "advance":{"type":"when","when":{"type":"unit_count","kind":"Footman","count":3}}},
  {"intent":{"type":"posture","id":2,"posture":{"type":"push","x":70,"z":70}}}]}
```

Note the three separate `train` steps: `train` queues **one** unit, so a
`unit_count` wait must be for a number the plan actually produces. A single
`train` step under `count: 8` is a plan that waits forever, and the engine will
not warn you — it is `running`, correctly, on a step whose condition is simply
never going to hold.

The last step moves whoever is in squad 2 when it runs. **No new selector was
invented**, and that is the decision rather than an omission: a `{"squad":2}`
form of every unit-taking verb would be a second way to spell membership, and
this document exists because two spellings of one language is two languages. The
answer to "I want to act on units I do not have yet" is *put them in a squad on
the way in*, which is a thing a commander should be doing anyway.

The NL layer says the same thing in English: `"the barracks units join squad 2,
then when I have 8 footmen, squad 2 pushes their base"`.

### The frame slot, and the one new determinism edge

`SimSet::Think`, after `FogSet`, at 4 Hz, upstream of `SimSet::Intent` — all
four of trigger.rs's reasons, unchanged, so a step submitted this tick is
compiled this tick.

One thing is new. plan.rs and trigger.rs both write `GameEvents` and
`SubmitIntent`, and Bevy leaves two systems in one set unordered unless
something forces an edge. Something has to, because `Order` is a component and
last writer wins. **Plans are ordered `.before` triggers**, so a trigger lands
last and wins a same-tick tie. That is the same ranking trigger.rs already
argued for against doctrine, one rung along: a trigger is a rule written for the
exact situation that just occurred; a plan is a sequence written before the
match for the general case. If your opening says "push mid" on the tick your
home guard says "the base is burning", the base is burning.

### Latency: a plan step pays nothing

Same row as a trigger in docs/TEMPO.md's verb table, same argument: a plan is
standing policy the engine executes unattended, its author paid the reach when
they wrote it down, and charging the link per step would make a plan strictly
worse than typing the same commands by hand — which inverts C4.

### Provenance: a third rung

`Cause::Plan { plan, verb, source }` renders `plan:opening step 2/5 move by
bridge t=41`. Its own rung beside `Cause::Trigger` because the step number is
the part that makes the answer usable: it tells a reader where in the sequence
they are without opening the plan.

### The seats, honestly

Same asymmetry triggers documented, and it is a *rendering* decision rather than
a routing one — the one place this document permits the two seats to differ.

* **The bridge** gets both verbs and a `plans` array with `step`/`of`/`status`,
  the current step's sentence, and `steps` round-tripped as the JSON that was
  sent (read it out, change one step, send it back under the same name).
* **The human** gets a status line in the selection panel — `Plans: opening 2/5
  boom 3/3 (blocked: not enough gold)` — and no authoring UI. That is not a
  privileged seat: `plan_set` is one verb in one language, and
  `intent_compile.py` compiles a person's English into exactly the JSON a
  commander writes. It is that a person at a keyboard *already has* sequencing —
  they press the keys in order at 200ms each, and a mouse-driven step editor
  would be strictly slower than the thing it automates. What they did not have
  is a way to *see* a sequence their co-commander set running unattended, and
  that is the line.

### Co-command: a proposed plan is one reviewable line

This is the thing co-command wanted and could not have. A partner's opening used
to arrive as five separate commands — five queue entries, each approvable alone,
so the human could approve the barracks, veto the keep, and end up holding an
incoherent half-sequence nobody proposed. `plan_set` makes the whole sequence
one `Intent`, so it is one proposal, one sentence, one `[Enter]`.

Nothing in copilot.rs knew a plan was coming: it wraps any command. And a plan's
`sentence()` renders **every** step joined by "then", which is why — the human
answering has to see what they are agreeing to on the line they are answering.

### What this leaves open

- **`plan_pause` / `plan_resume`.** Deferred, and not because it is hard to
  implement but because its semantics are not honest. Pausing cannot un-issue
  what the engine has already ordered, so "pause" would promise a rollback the
  engine cannot deliver. `plan_clear` plus a re-stated `plan_set` is the idiom,
  and re-stating restarts cleanly rather than resuming into a world that moved.
- **No branching.** One sequence, no `else`. A plan that could branch is a
  program; the composition you want is a plan *and* a trigger, which is why both
  exist.
- **A step still cannot name a unit that does not exist.** Answered by the squad
  idiom above rather than solved. The residue is real: a plan cannot say "the
  *specific* Catapult I am about to build".
- ~~**And there is no squad idiom for BUILDINGS.**~~ **Closed** by the building
  channel above (`wc3clone-3ji`). It was the sharper half of the same gap: a
  step could not name a building the plan itself was about to put up, because
  step 1 minted the id step 3 would need, so "build a Barracks, then train
  Footmen at it" was unspellable in one plan and the working idiom was two
  plans a poll apart. `{"intent":{"type":"train","select":"my Barracks",
  "unit":"Footman"}}` is now one plan, and `tools/intent_compile.py` compiles
  exactly that English into exactly that shape. The `{"kind":..,"nth":..}`
  object this bullet declined to invent is still not invented — the handle is a
  phrase in the channel that already existed, which is why the argument against
  the object did not apply to it.
- **A plan whose wait can never be satisfied is indistinguishable from one that
  is merely early.** `unit_count >= 8` behind a single `train` step (which
  queues one unit) is `running` forever, honestly and uselessly. Nothing
  type-checks a plan against the world it will meet.
- **Nothing watches for a plan whose premise died.** A plan waiting on
  `unit_count` for an army whose barracks was razed waits forever, `running`,
  and nothing tells you. The status is honest but passive.

---

## Territory: named places and regions

Every verb in this language that touches ground has spoken it as two floats.
`{"type":"posture","id":2,"posture":{"type":"defend","x":-60,"z":60,
"radius":18}}` is a legal sentence and an unreadable one. Three rounds of replay
logs say `squad 2 defends (-60.0, 60.0) within 18`, and nobody — human or model
— can tell from that line whether the commander meant the northwest ford or a
patch of grass twelve units from it.

The evidence that this was a real gap is that a workaround already existed and
was *invisible to the engine*. `tools/intent_compile.py` carried a private table
of fords, `mid`, the two bases and the four mines, and resolved names to
coordinates on the READ side, in Python, before anything reached the wire. It
worked, and it meant the vocabulary a commander spoke in English did not exist
in the protocol, could not be spoken by the human, could not be referenced by a
trigger, and could not be extended by either player. This section makes it
first-class and gives it to both seats.

### Two kinds of name

**Built-in places** are map facts. Derived per map, read-only, identical for
both teams except two seat-relative aliases, and they exist *without anybody
arming anything* — `"region":"center ford"` is a legal sentence in the first
second of a match. They are the shared half: both snapshots carry the same list
under `map.places`, so when one seat says `northwest ford` and the other reads
`northwest ford`, the two are demonstrably talking about the same ground.

| name | derived from |
|---|---|
| `our base` / `their base` | the two starting halls, per seat |
| `mid` | the map centre |
| `<compass> mine` | one per `GOLD_MINE_POSITIONS`, named for its nearest compass anchor |
| `<name> ford` | one per `MapKind::chokepoints()` — the map names these itself, so `open` has none and `crossings` has three |

The mine names are the **inverse of `intent_compile.py`'s `pick_mine`**, on
purpose and with a test pinning it: the mine the engine calls `northwest mine`
is the mine that tool hands back for the words "northwest mine". Two
vocabularies that disagree about where a word points would be worse than one
vocabulary.

**Regions** are what a commander names. `region_set` gives a circle a name; from
then on every verb that takes `x`/`z` takes `"region":"<name>"` instead. They are
**doctrine, not information**: a region appears in its owner's snapshot only
(`regions`), and naming ground is never a way to tell the enemy something. The
cap (8) and the replace-by-name rule are copied verbatim from triggers, for the
identical reason — eight named places is a map a commander can hold in their
head; eighty is a database.

### Circles only

A region is `{center, radius}` and nothing else. Polygons are more expressive
and there is no evidence anybody needs one: every shape this game already speaks
— `leash`, `defend`, `MINE_HOME_RADIUS`, ability areas, fog reveal — is a point
and a radius; `contains` is one distance test the 4 Hz trigger sweep can afford;
and a circle is drawable on a 100px minimap in a way a polygon is not. If a
match is ever lost because a ford was square, the shape becomes an enum with a
second variant. Until then the extra vocabulary is cost without a buyer.

### One resolution point

`intent::resolve_places` runs once, at the top of `compile_intent`, before any
verb arm sees the intent. It turns every `region` into coordinates or refuses
with the list of names this seat may speak. Everything downstream sees the
language it has always seen.

*(It resolves the **role** channel too, now — `"select":"my hero"` and its
siblings. Same function, same moment, same precedence rule; see "Selectors:
roles, on the same footing as places" below for why they had to share one pass
rather than get one each.)*

*(One resolution point, **two readers**. copilot.rs's conflict preview calls the
same function to ask what a proposal would reach, submitting nothing and keeping
nothing — see "Roles are expanded for the readout" above. A second reader is not
a second path: the rule still has exactly one implementation, and the preview's
whole honesty argument rests on it being the implementation approval will use.)*

The alternative — each verb resolving its own place — is how you get `defend`
accepting a name `push` does not, and two spellings of the unknown-name error.
Here there is one function and one refusal:

```
cmd 3: no region named 'the-perimiter' - known places: the-perimeter, our base,
their base, mid, southwest mine, northeast mine, northwest mine, southeast mine,
northwest ford, center ford, southeast ford
```

That is the teaching error the rest of this document argues for, applied to
geography: a commander who mistyped gets the menu back, not a "no".

**Precedence**, stated once so no verb can disagree: a `region` given alongside
`x`/`z` **wins**. The name is the decision; numbers next to it are not.

**What each verb does with a region** - every mapping stated rather than left to
be discovered:

| verb | mapping |
|---|---|
| `move` / `attackmove` / `build` / `rally` / `retreat` | the centre |
| `posture` `defend` | centre is the anchor, and **the region's own radius is the ring** unless `radius` is given |
| `posture` `push` | the centre. A push is a direction, not an area; the radius is dropped |
| `posture` `forage` | the centre is the muster point held while no cache is up |
| `posture` `escort` | **no region form.** It names a unit, and a region that followed a hero would be a second, moving vocabulary for one word |
| `leash` | anchor at the centre; the region's own radius is the leash unless `radius` is given |
| `stance` | the centre is the anchor. The region's radius is **dropped** — a stance's ring is its own, because a preset whose numbers moved with its target would not be a preset. Omit the place entirely and the anchor is the issuing team's base |

`x`/`z` became `Option` on the verbs that required them, and that is a
deliberate improvement rather than a cost: "this sentence names no ground at
all" is now a thing the language can say, and it earns
`move needs x/z or a region name` instead of serde's `missing field x`.

### What a trigger stores

`resolve_places` deliberately does **not** recurse into a trigger's `then`. That
follows the rule already stated in the Triggers section - the action is
validated when it *fires*, against the world that fired it - and applying it to
territory buys something specific: an armed rule keeps naming *the perimeter*, so

* moving a region with a second `region_set` **re-aims every standing order and
  every armed rule that mentions it**, in one command, at zero polls;
* clearing a region makes those rules refuse *out loud*, into the arming seat's
  own error channel, rather than silently acting on stale coordinates.

The `when` half is the opposite case and is checked immediately: a predicate's
parameters are constants the commander typed, so **for a trigger** `enemy_in`'s
region is validated at **arm time**, with the same menu attached. A plan step's
advance predicate late-binds the same field instead — the split, and the reason
for it, are in § *Arm time and late binding*.

A trigger may not `region_set` or `region_clear`, for the same reason it may not
arm another trigger and one step further out: a rule that renamed ground while
the match ran would make every other rule's meaning depend on firing order, and
"what does `north-pass` mean right now?" would stop being answerable by reading
the snapshot.

### `enemy_in` - the territorial predicate

```json
{"type":"enemy_in","region":"north-pass","class":"Siege","count":5}
```

Fog-honest in **both** directions: it counts bodies the arming team's own
`FogGrid::sees` admits AND that are inside the circle. Both filters, always. A
region is ground you are *watching*, not a sensor bolted to the map - an army
walking unseen through your named pass does not trip the rule, which is the same
knowability rule `enemy_sighted` obeys, applied to a smaller piece of the board.

This is the predicate that makes regions pay for themselves. `enemy_sighted`
fires on a lone scout wandering past a tower; "five enemies are in north-pass"
is the sentence a commander can actually sleep behind.

If a rule's region is cleared after arming, the rule goes **quiet** rather than
falling back to the whole map. An unresolvable name is not a bigger question; it
is no question, and firing a defence of nowhere is strictly worse than firing
nothing.

Quiet is the right answer for a *trigger*, which is a standing offer to act. It
is the wrong answer for a plan **step**, which parks on it: a sequence stopped
for good has to be able to say so, and `PlanState::Held` is how it does. See §
*Arm time and late binding*.

### Both seats, again

The human names ground with `[I][M]`: an armed marker whose next ground click is
the centre, `;`/`'` tuning the radius on the same free-entry helper the fallback
and leash numbers use, and `[N]` forgetting the lot. The engine picks the name
(`mark 1`..`mark 8`) because there is no text entry anywhere in this HUD - a
poorer name than `north-pass`, and a real one: it round-trips through the wire,
the snapshot and a co-commander's directive unchanged. That last part is the
point. A human and their LLM co-commander share a team and therefore share its
regions, so `[I][M]` on a ford is a sentence the partner can read and answer
with `squad 2 defends mark 1`.

Both kinds of place render: the map's built-ins as permanent faint circles, own
regions in amber, on the ground and on the minimap, with the armed marks listed
on the panel. The built-ins being drawn at all was the harder call - seven faint
circles could easily be seven pieces of clutter - and they are drawn because the
vocabulary is only *shared* if the human can see what the words mean.

The English compiler learned both halves. `map.places` and `regions` join its
place resolution, and an exact name beats every heuristic below it, because a
commander who called some ground "the perimeter" must not have that word
re-read as a compass direction. A **user** region is passed to the wire *by
name*, unresolved - it can move, so the engine should decide where it is - while
a built-in is resolved in the tool, because `mid` is the middle of the map for
the whole match. Authoring is the deterministic form only:
`name the northwest ford "north-pass" radius 20`. The tempting spelling,
`call this the perimeter`, is a trap: `the perimeter` is both a name and a
phrase that file resolves as a place, so the parse is ambiguous in exactly the
sentences a commander would write.

## Selectors: roles, on the same footing as places

*(arena r21–r23; docs/AFFORDANCES.md § "Chains: stance plans with late-bound
references", plan item 1)*

An entity id is a fact about one instant. `{"units":[4294967297]}` names the
hero that was alive when the sentence was written — and a sentence the engine
*stores and runs later* is precisely where that instant has already passed.
Three arena rounds produced five distinct failure classes from this one cause:

| what happened | round |
|---|---|
| a hero-save trigger armed with `"units":[]` because the hero was not trained yet; it fired as "move 0 units", was rejected, and the hero died three seconds later | r21 |
| dead hero ids in triggers | r23 red |
| stale unit lists in `priority`, refreshed by hand every poll | r23 red |
| a memorized tree chopped out from under a repeating harvest order | r23 red |
| the wrong worker frozen into a repeating trigger | r23 red |
| a farm trigger on fixed coordinates that reported `site blocked` on every retry for the whole match | r23 blue |

Both r23 commanders, asked independently how they would prioritize the decision
space for a smaller model, put late-bound references first.

### The shape was already in the language

Regions are late-bound **places**. `"region":"north-pass"` travels in the stored
intent and becomes coordinates when the intent is compiled, so moving the region
re-aims every rule that names it. A selector is a late-bound **role**, on the
identical footing, resolved at the identical moment — `intent::resolve_places`,
at the top of the one compiler, which a trigger reaches only when it *fires*.

So the wire grew one optional key beside `units`, exactly as it once grew one
beside `x`/`z`:

```json
{"type":"move","select":"my hero","region":"home"}
{"type":"harvest","select":"workers","target_select":"nearest tree"}
{"type":"build","select":"workers","kind":"Farm","region":"home","site":"nearest legal site"}
```

The vocabulary is deliberately small: five roles a commander already thinks in
(`my hero`, `all army`, `all units`, `workers`, `squad <n>`) and three
"nearest X" phrases that answer the questions ids answered badly
(`nearest tree`, `nearest mine`, `nearest legal site`). Phrases fold through
`normalize_place`, so case, dashes and underscores are noise and `squad 2` keeps
its space.

`build`, `cast` and `follow`'s leader need exactly one unit and take the
**lowest-id match** — the same documented tie-break `buy` and `use_item` already
use for an omitted `hero`, so there is one rule for "which one did you mean"
rather than two.

### The building channel

*(`wc3clone-3ji`, from the 0uu.3 handoff)*

The first three channels answered "which units", "which node" and "which
ground". A fourth answers **which building**, for the four verbs that act on
one — `train`, `template`, `rally`, `cancel`:

```json
{"type":"train","select":"idle barracks","unit":"Footman"}
{"type":"train","select":"my hall","unit":"Worker"}
{"type":"rally","select":"my barracks","region":"north-pass"}
```

It closes the gap the plan section below names in as many words: production —
the one thing a small commander does every cycle — required an entity id read
out of the snapshot, and a repeating `train` rule armed with that id trained
nothing the moment the barracks was razed and rebuilt.

Three phrases, and the shape of the family is the argument for each:

* **`my <building>`** — this seat's own FINISHED buildings of one kind. The kind
  is any `catalog.buildings[].id`, folded through `normalize_name` like every
  other name on the wire, singular or plural. Unfinished is excluded because a
  Barracks with scaffolding on it trains nothing, and resolving to one would
  turn a good sentence into an `under construction` refusal nobody wrote.
* **`idle <building>`** — the same, narrowed to an empty training queue. This is
  the one that wins games: a repeating "train a Footman" rule wants a producer
  that is actually free, and when they are all busy it says so — `all 2 of your
  Barracks already have something queued; drop 'idle' to queue behind it` —
  rather than stacking six deep on one building.
* **`my hall`** — whichever rung of the ladder is standing. A hall UPGRADES in
  place, so a rule that said `my town hall` would stop matching the moment it
  became a Keep. That is the author-time-fact bug this whole feature deletes,
  wearing a different hat, and the fix is a role rather than a kind.

All four verbs act on exactly one building, so all four take the **lowest-id
match** — the same tie-break as `build`'s worker, not a second rule. Their
`building` key widened to `Option` to make room for the phrase, which is
additive in both directions on the same argument as `build.worker` before it.

The empty match teaches by naming what the seat *does* have — `'my workshop'
matches none of your finished buildings — you have: Barracks ×2, Keep` — which
is `Regions::unknown`'s rule applied to buildings. Own buildings only, so it
leaks nothing the seat's own snapshot did not already print.

**Why not `{"building":{"kind":"Barracks","nth":0}}`.** That object was
considered in the plan section below and rejected for being a second way to name
a building. It still is. A phrase in the `select` channel is not a second way —
it is the way that already existed, reaching one more kind of thing.

### The four rules

1. **Precedence.** A `select` given alongside `units` **wins**, and the
   overruled list is not even reported. Same sentence as territory's: the name
   is the decision; ids next to it are not.
2. **An empty resolution is a refusal, not a quiet nothing.** A phrase that
   currently matches nobody returns `Err` from the resolver, so the intent never
   reaches its verb's arm, `reached` stays false, and the seat is told
   `'all army' matches none of your units right now — nothing was ordered`.
   **"Move 0 units" is now inexpressible**: the only way to order nobody used to
   be to name nobody, and naming a currently-empty role teaches instead of
   firing. In a plan this blocks the step rather than skipping it, because a
   step that reached nothing is a refusal by the rule already written down.
3. **Nothing is written back.** Resolution is per submission and discarded; the
   armed rule still says `"my hero"` on its hundredth firing. A hero can die and
   be revived with a brand-new entity id and the rule keeps working.
4. **A selector is bounded by the seat that speaks it and by fog.** The
   resolver's context struct holds this seat's own units, their squads, the
   neutral resource nodes and the nav grid — and no enemy query at all. There is
   deliberately no `"nearest enemy"`: that is an intel question wearing a
   convenience hat, and fog decides intel (docs/FOG.md), not the resolver.

### Determinism

A resolved unit list is sorted by entity bits before it leaves the resolver.
This is not cosmetic: `ground_order` hands out formation offsets by index, so an
unsorted resolution would arrange the same squad on the same ground differently
depending on Bevy's archetype order. `nearest_node` breaks ties on
`(distance, x, z, id)` for the same reason `nearest_free_site` does — two seats
reading the same board must be given the same answer.

### Both seats

The human's answer to "which units" is the mouse, and it is already late-bound:
a gesture resolves its selection at press time, which for a *direct* order is
fire time. The asymmetry to watch is in the *deferred* half, and the one preset
the HUD can arm — `[H]` home guard — was already written against a squad rather
than a roster (`Intent::Posture { id }`, whose membership doctrine.rs resolves
every second). So the human's deferred authoring surface is late-bound today by
a different mechanism, and neither seat can currently arm a rule the other
cannot express.

What the human seat does **not** have is a way to *type* a role into a rule,
because there is no text entry in this HUD. That is the same limitation that
makes the human's regions `mark 1`..`mark 8` rather than `north-pass`, and it is
a rendering difference rather than a capability one. A selector-picker on the
doctrine page would close it; nothing about the wire needs to change for that.

### Wire compatibility

Every new key is optional and `skip_serializing_if`, and every one is declared
last in its variant so the serialized key *order* did not move either. Four
required scalars widened to `Option` — `build.worker`, `cast.hero`,
`harvest.target`, `follow.target` — which is additive in both directions: a
historical command always carries them, so it parses as `Some` and serializes
identically. `bridge.rs` echoes armed triggers and set plans into `state.json`
by re-serializing the stored `Intent`, so a stray `"select":null` would have
appeared in every snapshot of every match that never used the feature;
`the_selector_keys_are_absent_from_a_sentence_that_does_not_use_them` pins that
it does not.

## What this unlocks

- ~~**`wc3clone-hre` (co-command).**~~ **Done** — see above. The prediction was
  right: it was two producers and one `IntentSource` variant, and the design
  work was all in the conflict policy.
- ~~**Closing the ghost-attack gap.**~~ **Done** — the picker reads
  `FogGrid::ghosts()` and produces the same `Intent::Attack` against the same
  id (see "The residual asymmetry" above). There is no longer a place where the
  AI can express something the human cannot. It is also, as of Chain of
  Command, a direct order like any other: attacking a remembered building from
  across the map costs the link, because what is slow is reaching your own
  soldier, not reaching the enemy.
- ~~**Ability ids parse inconsistently.**~~ **Done** — `normalize_name` moved
  to shared.rs, next to the catalog whose names it folds, and now backs
  `ability_index_by_id` and `parse_target_class` as well. There is one name
  matcher, and `"Call to Arms"` works.
- **Chain of Command (docs/TEMPO.md) — shipped, and this layer is why it was
  cheap.** The spike asked for "a single choke point where player commands
  become engine orders" and budgeted 23 call sites across three files. Because
  there was one function, latency for both player seats is a substitution
  inside `compile_intent`'s order arms: eight verbs now issue through
  `command::OrderIssuer` instead of `try_insert`, and the rest are documented
  as instant with a reason each (docs/TEMPO.md §7). The compiler still
  validates in the frame the intent arrives — only *application* is deferred —
  so error strings, the wire format and the `cmd N:` prefixes are untouched.
  The log gained one thing: a `(+N.Ns link)` suffix on any sentence the chain
  of command delayed.
