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
  sentence plus its serialized form.

---

## The fairness invariant

> **No player-facing mutation path exists except intent submission.**

`ui.rs` and `bridge.rs` contain zero writes to `Order`, `TrainingQueue`,
`RallyPoint`, `TargetPriority`, `RetreatPolicy`, `LeashPolicy`,
`AutoCastPolicy`, `SquadId`, `DoctrineTemplate` or `SquadOrders`, and zero
sends of `CastAbility` / `BuyItem` / `UseItem` / `UpgradeBuilding` /
`Surrender`. Both files used to carry a *field-for-field identical* four-writer
`SystemParam` bundle (`CardActions` in ui.rs, `CmdEvents` in bridge.rs) —
independent convergence on the same shape, which is exactly the duplication
this layer removes. Both are gone; intent.rs owns the four writers. Both write
`SubmitIntent` events and nothing else. This is grep-checkable, and checking it
is the point: the invariant is only worth having if a regression is visible.

What the interfaces still own is the *gesture*: deciding which units a
right-click meant, which worker is nearest the build site, what "guard" implies
as an anchor and a radius. That is the human interface's real job. What comes
out the other side is a value a commander could have typed.

### Who is not a player

Two categories deliberately keep writing components directly, and neither
weakens the invariant:

- **Engine systems.** `economy.rs`'s harvest follow-through, `combat.rs`'s
  chase, `doctrine.rs`'s squad re-tasking and retreat triggers. These are the
  engine executing standing policy at machine speed, for whichever player set
  it. Routing them through intents would be a category error — and it is
  exactly the line THESIS.md principle 3 draws ("the engine does what is fast;
  the player does what is wise").
- **The scripted `ai.rs`.** *This is a known asymmetry.* `ai.rs` still writes
  `Order`s and training-queue pushes directly, from ~9 call sites. It is engine
  baseline rather than a seat — nothing is measuring fairness against it today
  — but it means the invariant currently reads "no *human or bridge* mutation
  path except intents". Routing `ai.rs` through the compiler is follow-up work.

  It is **not**, as this document originally guessed, a prerequisite for
  docs/TEMPO.md's Chain of Command. That bead needed all three seats to pay
  latency identically, and got it by having `ai.rs` call the same
  `command::OrderIssuer` the compiler calls — the mechanism lives one layer
  below the compiler, so the third seat can reach it without speaking the
  language first. What routing `ai.rs` through intents would still buy is
  attribution: its decisions would appear in `intent_log.jsonl` as sentences,
  and the fairness invariant would read without a footnote.

One more honest edge: `ui.rs::update_rally_flag` still removes a `RallyPoint`
whose target has died. That is a validator reacting to a world event, not a
player expressing anything, so it stays where it is.

---

## The vocabulary

25 verbs, grouped by what they are for. The serde shape **is** the bridge's
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
| `cast` | `{hero:id, ability?}` (alias `caster`) — any own CASTER: hero, Sorcerer, or ability building |
| `buy` | `{shop:id, item:"HealingPotion", hero?:id}` — `hero` optional, see below |
| `use_item` | `{slot:0, hero?:id}` |

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
| `template` | `{building:id, squad, retreat, priority, autocast}` | all pieces absent |

### Match level
| Verb | Shape |
|---|---|
| `autopilot` | `{on:true}` — hand this faction to the scripted AI |
| `surrender` | `{}` |

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
`bridge/intent_log.jsonl` (override with `WC3_INTENT_LOG`; set it to `0` or
empty to disable). The file is truncated at the first intent of a run, so it is
one file per match. It is opened lazily, so a run in which nobody says anything
leaves no file behind — an AI-vs-AI headless sim writes nothing, because
`ai.rs` is not a player.

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

Verified end-to-end against a live `WC3_BRIDGE=1` seat driven by
`tools/bridge_send.py`:

- `state.json`'s top-level key set is unchanged (15 keys), as are `UnitOut`,
  `BuildingOut`, `MeOut`, `MapOut`, `SquadOut` and every other snapshot struct
  — the diff against master touches none of them.
- Every historical command shape still parses, including the `caster` alias on
  `cast`, the `use_item` rename and the untagged ability selector
  (`intent::tests::legacy_wire_commands_parse` covers all 25 verbs and their
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

## Speaking it: English, and "why are you doing that?"

*`wc3clone-ge4`. Two additions, both built on the fact that `Intent` is a value
with a `sentence()` renderer.*

### English is a third spelling, and it lives outside the engine

`tools/intent_compile.py` compiles a natural-language directive plus a snapshot
into a batch of `Intent` objects. It is a **tool, not an engine feature**, and
that placement is the design: the game gains no NLP, no new verb, and no new
mutation path. What it gains is a shorter way to write the same 25 verbs.

```
"hold the northwest ford, forage mid with the cavalry, retreat at 35%"
  -> {"type":"squad","units":[…],"id":1}
     {"type":"posture","id":1,"posture":{"type":"defend","x":-60.0,"z":60.0,"radius":18.0}}
     …
```

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
errors, never a silently different order. The one structural refusal is
**conditionals**: "strike when their hero falls" has no verb, because the
engine has no trigger system. The tool compiles the action, marks it deferred,
and prints the command to run when the commander sees the condition in
`events` — which is the honest shape of that request, not a limitation to
paper over.

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
| scripted baseline | `ai.rs` (not a seat, so its own rung) | `script:wave` |
| engine default | auto-enrolment, idle instinct | `instinct:auto-enroll`, `idle` |

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
- **`ai.rs` gets `Cause::Script`, not an `IntentSource`.** Correct today, since
  it is engine baseline rather than a seat — but if `ai.rs` is ever routed
  through the compiler (the prerequisite for TEMPO.md's Chain of Command), that
  rung should collapse into `order:… by ai`.
- **The tool cannot see money.** It happily compiles a `build` you cannot
  afford; economy.rs refuses it. That is the documented division below, but a
  `me.gold` check would turn a rejected batch into a better error.

---

---

## Co-command: one faction, two authors

*`wc3clone-hre`. The bead THESIS.md was written for — "co-command, where a
human and an AI run one faction and negotiate strategy in a language both speak
natively". Implemented in `copilot.rs`.*

`WC3_BRIDGE=copilot` opens **one** seat on `Team::Human` — beside the player at
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

The rung is a **seat**, not `Cause::Script`. A co-commander pays the same
latency, obeys the same fog and speaks the same 25 verbs; the scripted `ai.rs`
does none of those things, and collapsing the two would have made "who moved
this unit" unanswerable in exactly the case it is asked.

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
| **direct** | `squad` `posture` `template` `priority` `retreat` `leash` `autocast` | doctrine is *advice-shaped already* |
| **propose** | every unit order, all production, all spending, `autopilot`, `surrender` | irreversible, or spends what is not the proposer's |

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

`WC3_COPILOT_TRUST` moves the line for experiments: `full` (everything direct)
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
  `proposals` and `partner_log` are `Option` and skipped when absent; `red` and
  `blue` snapshots keep exactly the keys they had.

### Follow-ups co-command leaves open

- **A proposal's conflict tags can go stale.** They are computed at arrival and
  describe a board up to 20 seconds old. The sentences cannot go stale (they
  are the batch), and approval re-validates everything against the live world,
  so the cost is a tag that over- or under-states — never an order that lands
  differently than it read. Recomputing per frame is the fix if it ever bites.
- **The queue is not prioritised.** Four pending proposals answer oldest-first,
  which is right when they are about the same fight and wrong when one is
  urgent. A `severity` on the wrapper is the obvious next rung.
- **A veto tells the partner nothing but "no".** The human has no way to reply
  *why*, so a co-commander can only guess whether to re-propose. A one-key
  "not now" vs "never" distinction, or a canned reason list, is the smallest
  thing that would make the negotiation two-sided in both directions.
- **Headless has no approver.** `WC3_BRIDGE=copilot` under `WC3_HEADLESS=1`
  queues proposals nobody can answer, so they all lapse. That is *correct* —
  approval is a human act and headless has no human — but it means an
  AI-vs-(AI+AI) sim needs either full-trust mode or a scripted approver seat.

---

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
