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

27 verbs, grouped by what they are for. The serde shape **is** the bridge's
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

### Triggers — contingent standing policy (v3)
| Verb | Shape | Clears when |
|---|---|---|
| `trigger_set` | `{name, when:{…}, then:{<any intent>}, repeat?:secs}` | — |
| `trigger_clear` | `{name}` or `{}` for every trigger | — |

Full treatment below (§ Triggers). One line here: doctrine is what the engine
does *continuously*; a trigger is what it does *when something happens*.

### Match level
| Verb | Shape |
|---|---|
| `autopilot` | `{on:true}` — hand this faction to the scripted AI |
| `surrender` | `{}` |

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
  (`intent::tests::legacy_wire_commands_parse` covers all 27 verbs and their
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
mutation path. What it gains is a shorter way to write the same 27 verbs.

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
latency, obeys the same fog and speaks the same 27 verbs; the scripted `ai.rs`
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

#### Making the loop measurable: `WC3_COPILOT_AUTOAPPROVE`

*`wc3clone-3f7`, closing the "headless has no approver" follow-up below.*
Approval is a human act, so a headless sim has nobody to give it and every
proposal lapses. That is *correct* — and it means the one part of co-command
that is genuinely new was the one part no sim could measure.

Two knobs make it observable without a person:

- **`WC3_COPILOT_TRUST=full`** — the control case. No loop at all: every verb
  goes direct, so a sim measures a co-commander *without* the negotiation cost.
  (It already existed as a policy toggle; it works headless because
  `CopilotPlugin` is registered in both branches of `main.rs` and the seat is
  opened by `WC3_BRIDGE=copilot` either way.)
- **`WC3_COPILOT_AUTOAPPROVE=1`** — a scripted approver that says yes to each
  proposal `WC3_COPILOT_APPROVE_DELAY` seconds after it arrives (default 3s).

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
`WC3_COMMAND_LATENCY` a co-commander's orders travel exactly as the human's do,
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
  `WC3_COPILOT_TRUST=full` as the no-loop control, and
  `WC3_COPILOT_AUTOAPPROVE=1` as a scripted approver with a deliberate delay.
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

Nine, and the constraint that produced the list is worth stating: **every one is
answerable from state the frame already has, for the arming team, with no new
bookkeeping.** No event subscriptions, no history, no memo. A predicate that
needed its own record-keeping would be a predicate whose truth could drift from
the world, and the whole value of firing at machine speed is that the world is
what fired it.

| `when` | True when |
|---|---|
| `base_under_attack` | Any of **your buildings** has a `LastDamaged` inside `BASE_ATTACK_WINDOW_S` (8s). Buildings only — a skirmish in midfield is not the base being attacked. Half-built ones count: losing an expansion under construction is exactly this raid. |
| `hero_below {frac}` | **Any** of your living heroes is under `frac` of max health. Any, not "the" — hero slots climb the hall ladder, and the useful reading of "save my hero" is "whichever one is dying". |
| `squad_below {id, frac}` | Squad `id`'s living members hold, **pooled**, less than `frac` of their combined max HP. Pooled because a squad is a formation: one wounded footman in a healthy line is not a squad in trouble. **False for a squad with no living members** — a squad that is gone cannot be hurt, and firing a rescue at a corpse pile is worse than firing nothing. |
| `enemy_sighted {class?, count}` | You can **see** at least `count` enemy units right now, optionally of one `TargetClass`. Fog-honest: counted against your own `FogGrid::sees`. Remembered buildings do **not** count — remembering where a barracks stood is not the news that an army came out of it. |
| `bounty_spawned` | A neutral cache exists **and you can see it**. The same `fog.sees` filter the snapshot's `bounties` array uses, so the rule sees exactly the caches its owner is shown. |
| `mine_dry` | A gold node with `remaining == 0` lies within `MINE_HOME_RADIUS` (40) of one of your **completed** halls. Mines are neutral and unowned, so "our mine" is defined by geometry: the one your hall was placed to work. |
| `tier_reached {tier}` | `TechTiers::get(you).level() >= tier`. |
| `unit_count {kind, count}` | You field at least `count` living units of `kind`. |
| `game_time {at}` | The match clock has passed `at` seconds. The one predicate about nothing in the world — it is here because "expand at six minutes" is a plan every commander already writes, and as a trigger it stops depending on remembering. |

**What is deliberately missing** is anything about the *enemy's* internals — their
gold, their tech, their hero's health. Not an oversight and not a fog problem
you could scout your way around: those are facts the snapshot does not carry for
either seat, so a predicate over them would be an information right the human
does not have. `tools/intent_compile.py` therefore still **defers** "strike when
their hero falls" — with `trigger_set` sitting right there — because the nearest
predicate that exists reads *your own* hero and arming that would mean the
opposite of what was asked.

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
| bridge / copilot | full — nine predicates × 27 verbs, as JSON |
| `tools/intent_compile.py` | full-ish — "when X, Y" over the same nine, in English |
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
What the human lacks is the *authoring* — nine predicates and a free choice of
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
  edge and the plans bead will meet it again.

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
