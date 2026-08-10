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
sends of `CastAbility` / `BuyItem` / `UseItem` / `Surrender`. Both write
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
  path except intents". Routing `ai.rs` through the compiler is follow-up work
  and is a prerequisite for docs/TEMPO.md's Chain of Command, which explicitly
  requires all three seats to pay latency identically or "autopilot becomes a
  cheat and C1 is violated at the third seat".

One more honest edge: `ui.rs::update_rally_flag` still removes a `RallyPoint`
whose target has died. That is a validator reacting to a world event, not a
player expressing anything, so it stays where it is.

---

## The vocabulary

23 verbs, grouped by what they are for. The serde shape **is** the bridge's
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
| `cancel` | `{building:id, index:n}` |
| `rally` | `{building:id, x, z}` or `{building:id, target:id}` |

### Abilities & items
| Verb | Shape |
|---|---|
| `cast` | `{hero:id}` (alias `caster`) — hero or own ability building |
| `buy` | `{shop:id, item:"HealingPotion"}` — buyer implied by team |
| `use_item` | `{slot:0}` |

### Doctrine — standing policy the engine executes at machine speed
| Verb | Shape | Clears when |
|---|---|---|
| `priority` | `{units:[id], classes:["Hero","Siege"]}` | `classes` empty |
| `retreat` | `{units:[id], below:0.35, x, z}` | `below` 0/absent |
| `leash` | `{units:[id], x, z, radius:20}` | `radius` ≤ 0 |
| `autocast` | `{units:[id], min_enemies:3}` | `min_enemies` 0/absent |
| `squad` | `{units:[id], id:1}` | `id` absent |
| `posture` | `{id:1, posture:{type:"defend"\|"push"\|"escort"\|"forage", …}}` | `posture` absent |
| `template` | `{building:id, squad, retreat, priority, autocast}` | all pieces absent |

### Match level
| Verb | Shape |
|---|---|
| `autopilot` | `{on:true}` — hand this faction to the scripted AI |
| `surrender` | `{}` |

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

---

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
(`errors: ["cmd 3: …"]`) and the `cmd <i>:` prefix are unchanged.

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
  `cast` and the `use_item` rename (`intent::tests::legacy_wire_commands_parse`
  covers all 23 verbs and their optional-field forms).
- `seq` gating, `last_seq`, the 4 Hz poll and the 1 Hz snapshot are untouched.
- `tools/bridge_send.py`, `tools/bridge_view.py`, `tools/bridge_wait.py` and
  every COMMANDER_BRIEF.md flow work without modification.

No new commands were added, and none were removed. **This bead changed no game
behaviour** — it changed how many places can cause it.

---

## What this unlocks

- **`wc3clone-ge4` (NL→intent compiler).** Its target is now a concrete type
  with a fixed serde shape and a `sentence()` renderer to check itself against:
  compile English to `Intent`, print the sentence back, and the round trip is
  the confirmation dialogue. It should emit `Intent` values, never JSON strings.
- **`wc3clone-hre` (co-command).** Two authors submitting into one team is now
  a matter of two `SubmitIntent` producers with different `IntentSource`s —
  the compiler already tags, logs and attributes every intent, and already
  treats source as descriptive rather than authoritative. `IntentSource` will
  want a third variant, and conflict policy (last-writer-wins vs. veto) is the
  real design question, not plumbing.
- **Chain of Command (docs/TEMPO.md).** The spike asked for "a single choke
  point where player commands become engine orders" and budgeted 23 call sites
  across three files. There is now one function. `PendingOrder` latency becomes
  a change inside `compile_intent`'s order arms rather than a 23-site refactor
  — with the `ai.rs` caveat above.
