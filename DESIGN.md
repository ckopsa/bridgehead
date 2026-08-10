# WC3 Clone — Module Contract

A Warcraft-3-style 3D RTS in Rust + **Bevy 0.16** (pinned — do not change Cargo.toml).
Two factions: **Human** (blue, the player, base SW) vs **Claude** (red, AI, base NE).
Win by destroying all enemy buildings.

## Ground rules for module agents

- You own **exactly one file** in `src/`. Replace its stub entirely. Do **not** edit
  any other file (`shared.rs`, `main.rs`, other modules, `Cargo.toml`).
- `use crate::shared::*;` gives you the full cross-module contract — read
  `src/shared.rs` top to bottom before writing code.
- Anything module-private (helper components, resources, timers) you define inside
  your own file. If a shared type seems missing, solve it locally — do not invent
  new cross-module contracts.
- Your file must expose exactly `pub struct <Name>Plugin` implementing `Plugin`
  (the name already in your stub); `main.rs` already registers it.
- All gameplay happens on the Y=0 ground plane. Y is up. Map spans ±100 on X/Z.
- Keep it simple and working over fancy and broken. Procedural primitive meshes
  (cuboids, spheres, cylinders, capsules) — no assets, no textures.

## Architecture

- `shared.rs` (integrator-owned): Team, UnitKind/BuildingKind + stats tables,
  Health, Order, MoveTo, NavGrid, Economies, spawn events, GameOver, CorePlugin
  (initial town hall + 5 workers per team, death cleanup, supply recount, win check),
  and `GameEvents` — the per-team alert feed (losses, hero milestones, base
  threats, squad wipes, bounties), diffed once per team per second. It lives
  here and not in a consumer because it has two renderers: bridge.rs serializes
  it into `state.json` for an external commander, ui.rs draws it as HUD
  notifications for the player. A feed with two producers is two feeds, and two
  feeds is an information advantage for whichever side has the better one.
  It also owns two frameworks every content bead builds on:
  * **Status effects** — `StatusKind` (Slow/Haste/ArmorBuff/DamageBuff/
    HealOverTime), `StatusEffect`, the `StatusEffects` component, and
    `effective_stats(BaseStats, Option<&StatusEffects>)`, **the one modifier
    function**: units.rs and combat.rs ask it for move speed, attack cooldown
    and damage dealt/taken instead of reading the stat tables. Debuffs refresh,
    buffs stack, magnitudes are capped per kind, and `tick_status_effects`
    expires everything centrally (combat.rs draws the ground ring).
  * **Abilities v2** — `abilities_of_unit` / `abilities_of_building` return a
    LIST per caster; each `AbilityDef` carries an `AbilityUnlock` predicate
    (always / hero level / `TechTier`) and an effect that may be
    `ApplyStatus`. `CastAbility { caster, ability: Option<AbilitySelector> }`
    picks a slot (`None` = first unlocked); cooldowns live per slot in
    `AbilityCooldowns` and auto-cast rules per slot in `AutoCastPolicy`.
    `tech_tier_for` derives the team's `TechTier` from the highest hall rung it
    has standing (`is_hall` + `building_tier`), so a completed Keep opens every
    `TeamTier(T2)` ability and losing it closes them again.
  * **Research** — `ResearchKind` (Attack/Armor), the `research_step` cost
    ladder, the `TeamResearch` resource and the `Researching` component. The
    game's one purchase that buys a *property of the faction* rather than a
    thing: +1/+2/+3 flat damage on every unit attack and -1/-2/-3 flat damage
    off every hit a unit takes, retroactive to units already alive and
    surviving the Blacksmith that bought them. It reaches the simulation
    through the same one stat law as everything else —
    `effective_stats_with(base, status, ResearchBonus)` — as two flat terms
    (`bonus_damage`, `flat_armor`) applied OUTSIDE every multiplier, so a
    Catapult's 6x siege bonus can never scale an army upgrade. `combat.rs`
    adds the first at the swing and subtracts the second at `apply_damage`,
    floored by `damage_after_armor` so armour is a discount and never an
    immunity. Structures are deliberately excluded from both.
- `intent.rs` (v2): the **intent compiler** — the single place a player's
  meaning becomes game state. `shared::Intent` is the vocabulary (25 verbs,
  serde-serializable, wire-identical to the bridge protocol); `ui.rs` compiles
  mouse gestures into it and `bridge.rs` deserializes `commands.json` into it,
  and neither may mutate the world any other way. It is also where fog stops
  being a rendering choice and becomes a rule: an `attack` on a target the
  issuing team cannot see or remember is refused for BOTH interfaces. Writes
  the per-match intent log (`bridge/intent_log.jsonl`): every intent as an
  English sentence plus its serialized form, so a replay reads the same
  regardless of who was playing. Engine follow-through (economy/combat/
  doctrine) and the scripted `ai.rs` are not players and still write components
  directly. Ordering is via the `IntentApply` system set, itself
  `.after(FogSet)`. See docs/INTENT.md.
- `terrain.rs`: ground, doodads, resource nodes (gold mines at
  `GOLD_MINE_POSITIONS`, tree clusters), lighting, **RTS camera** (spawns the
  `MainCamera`), blocks trees/mines in `NavGrid`, and owns the **map layout**:
  `WC3_MAP=open|crossings` (default `open`) picks one, `crossings` blocking a
  canyon with three fords in the `NavGrid`. Both players learn the layout the
  same way — bridge.rs ships `map` (name, summary, chokepoints) in every
  snapshot, ui.rs paints the barrier on the minimap.
- `units.rs`: handles `SpawnUnitEvent` (meshes per kind/team), executes
  `Order::Move`/`Order::AttackMove` by inserting `MoveTo`, implements `MoveTo`
  pathfinding (A* over `NavGrid`) + steering + local separation, removes `MoveTo`
  on arrival.
- `combat.rs`: target acquisition (aggro radius when Idle/AttackMove),
  `Order::Attack`, chase via `MoveTo`, attack cooldowns, projectiles for
  `projectile: true` units, damage to `Health`, floating health bars.
  Death/despawn itself is handled centrally in shared.rs.
- `economy.rs`: handles `SpawnBuildingEvent`, construction progress, research
  (`StartResearch` pays and starts the clock, `Researching` ticks down, the
  team's `TeamResearch` level rises on completion — and this module, not the
  compiler, is what enforces one job per forge; see docs/INTENT.md),
  `Order::Build` (worker walks to site, pays, spawns UnderConstruction building,
  blocks NavGrid), `Order::Harvest`/`ReturnResources` loop (gold mines & trees →
  nearest own TownHall), processes `TrainingQueue` (pays at enqueue time — the
  enqueuer only checks affordability; economy deducts, checks supply, spawns via
  `SpawnUnitEvent` near the building when done, refunds nothing on death).
- `data.rs`: the content data loader. Reads `assets/data/*.ron` into the stat
  tables shared.rs's accessors hand out (`unit_stats`, `building_stats`,
  `abilities_of_unit`, `item_def`, `research_step`, …), validates them, and
  panics at startup naming the offending row if they do not hold up. See
  "Content data files" below for the contract.
- `ui.rs`: selection (left click + drag box over own units/buildings), right-click
  context orders (enemy → Attack, resource node → Harvest for workers, ground →
  Move; A+click → AttackMove), building placement mode with ghost + affordability
  training hotkeys/buttons on selected production buildings, `[U]` tier-up,
  hero/building ability hotkeys, doctrine toggles — all of which *compile to
  `Intent` values and submit them*; ui.rs mutates no game state itself. The
  command card has two pages, toggled by `[I]`: the classic orders card, and a
  doctrine card carrying squad postures (click-to-place, with a ground disc
  showing the pending point; Escort clicks a unit), a free-entry retreat
  threshold and leash radius (`[F]`/`[G]` step presets, `-`/`=` and `[`/`]`
  nudge), a per-ability auto-cast toggle, and `DoctrineTemplate` stamping on a
  production building. `Ctrl+1-3` assigns a control group *and* submits the
  `squad` verb, so a control group and a squad are one object (docs/TEMPO.md).
  Plus the top resource bar,
  selection info panel, game-over banner from `GameOver`, top-right alert stack
  rendering `GameEvents::feed(Team::Human)` (severity colours, fade-out, Space
  or click focuses the camera via `CameraFocus`).
- `ai.rs`: the Claude faction. Assigns idle workers to harvest, maintains build
  order (farms before supply block, barracks, more workers), trains army,
  attack-moves waves at the human base. Acts ONLY through the same primitives the
  UI uses: writing `Order`, pushing to `TrainingQueue` (after checking/paying the
  same way), sending events. Never teleports or cheats resources. It is engine
  baseline rather than a seat, so it still writes those directly instead of
  going through `intent.rs` — a known asymmetry, noted in docs/INTENT.md.

## Cross-module conventions

- **Orders**: `intent.rs` (for the two player interfaces) and `ai.rs` set
  `Order` on entities. Executors react via `Changed<Order>` and then own the
  follow-through. A module handling one order variant must tolerate the order
  being overwritten at any time (always re-check).
- **Player intent**: `ui.rs` and `bridge.rs` never mutate game state. They
  build `shared::Intent` values and write `SubmitIntent`; `intent.rs` validates
  and applies. Adding a player-facing capability means adding a verb to
  `Intent`, which adds it to both seats at once.
- **Movement**: only `units.rs` moves unit Transforms. Everyone else inserts
  `MoveTo { target }` (or an Order that units.rs turns into movement). Absence of
  `MoveTo` == not moving. Insert a fresh `MoveTo` to re-path.
- **Damage**: only `combat.rs` subtracts unit/building `Health`. Anything at
  `current <= 0` is despawned centrally by shared.rs (buildings auto-unblock nav).
- **Money**: all spending goes through `Economies::get_mut(team).pay(...)`.
- **Buildings under construction** have `UnderConstruction`; they don't train or
  provide supply until it's removed by economy.rs.
- **Selection**: `Selected` marker is written only by ui.rs.
- Teams: every unit/building/projectile-owner entity has a `Team` component.

## Content data files (`assets/data/*.ron`)

Every stat table in the game is a RON file, not a `match` arm. This is a merge
decision before it is anything else: row literals inside one big `match`
interleave silently — git merges two agents' hunks cleanly because they touch
different lines — and the damage only surfaces as a missing-field compile error
some commits later, if at all. One record per row in a data file either
conflicts loudly (both edited the same record) or not at all.

| File | Table | Accessors in `shared.rs` |
| --- | --- | --- |
| `units.ron` | `UnitStats`, name, description, tech gate | `unit_stats`, `kind_name`, `unit_description`, `unit_requires` |
| `buildings.ron` | `BuildingStats`, name, description, `requires`, `trains`, `researches`, `upgrades_to` | `building_stats`, `building_name`, `building_description`, `building_requires`, `trainable`, `building_researches`, `building_upgrades_to` |
| `abilities.ron` | `AbilityDef` rows + per-caster slot lists + default auto-cast | `abilities_of_unit`, `abilities_of_building`, `default_autocast` |
| `items.ron` | `ItemDef` (Shop shelf) | `item_def` |
| `research.ron` | ladder ids/labels/descriptions, the shared price list, the forge | `ResearchKind::{id,label,description}`, `research_step`, `research_building` |

**What is data and what is code.** *Every number and every flag* is data.
*Identity and rules* stay in Rust:

- `UnitKind` / `BuildingKind` / `ItemId` / `ResearchKind` / `StatusKind` stay
  enums. A KIND is code identity — it needs a variant and a mesh arm in
  `units.rs` regardless — and making kinds dynamic would buy nothing but the
  loss of exhaustiveness everywhere else.
- Derived facts stay derived: `building_tier`, `upgrade_root`, `is_hall`,
  `building_placeable`, `upgrade_cost`, `unit_tier`, `tech_tier_for`. The
  upgrade LADDER is one data field (`upgrades_to`) and everything else is walked
  from it, so a fourth hall rung is a data change.
- Formulas stay code: `research_bonus`, `effective_stats`, `damage_after_armor`,
  `upkeep_rate`, `bounty_value`, hero level curves.
- Singleton constants stay code (`POTION_HEAL`, `BOOTS_HASTE`,
  `HERO_XP_RADIUS`, `RESEARCH_MAX_LEVEL`, `MIN_DAMAGE_PER_HIT`, the
  `StatusKind` caps and tints). They are one line each, so a merge conflicts on
  them loudly already; a file per scalar would be ceremony without a payoff.
  The line is *tables move, scalars stay*.

**How to add a row.** Open the file, copy the nearest record, change the values,
and put the balance rationale in a `//` comment next to the number it explains —
the data files carry the design commentary that used to live beside the literals.
Adding a whole new KIND is: the enum variant, the entry in `ALL_*_KINDS`, the
mesh/colour arm in `units.rs`, and the record. The loader refuses to start if
the last one is missing and names the variant.

**Load mechanism.** Each table is compiled in with `include_str!` and is the
default, so `cargo run` works from any working directory and a shipped binary
carries its own content. `WC3_DATA_DIR=<dir>` makes the loader prefer
`<dir>/<file>.ron` for any file present there and fall back to the built-in copy
for the rest, so a modder or a balance pass ships only the files they changed:

```bash
WC3_DATA_DIR=assets/data cargo run          # edits to assets/data/*.ron take
                                            # effect on the next launch, no rebuild
```

Without `WC3_DATA_DIR` the built-in copy wins, and editing a `.ron` triggers a
recompile of the crate (cargo tracks `include_str!` inputs) — correct, but a
rebuild. The override path is the one to use while tuning.

The tables are `LazyLock`s, so "loaded before anything reads a stat" is
structural rather than a system-ordering promise: the first read is the load, in
a windowed run, a headless run or a unit test. `CorePlugin::build` additionally
forces the load during `App` construction so a bad file is a startup panic.

**Validation.** The loader refuses to start, listing every problem it found, if
a variant has no row or has two, a referenced ability name does not exist, an
auto-cast names an ability its own caster does not have, a name collides under
`normalize_name` (the intent parser would then be ambiguous), a unit is trained
by nothing, the upgrade ladder is not a tree, the research steps do not cover
`1..=RESEARCH_MAX_LEVEL`, or any of hp / vision / speed / range /
attack_cooldown / train_time / footprint / multipliers is not positive. Costs
are `u32`, so "no negative costs" is the type system's job. `src/data.rs`'s
tests prove the validator bites by handing it deliberately broken tables.

## Determinism — the canonical frame order

Same seed + same intent stream + same tick = same match. This is the
foundation WeGo turns and replays are built on, and it rests on three things.

### 1. One explicit set order

`shared::SimSet` names every phase of a simulation frame and `CorePlugin`
chains them pairwise straight out of `shared::SIM_ORDER`, so the constant below
*is* the schedule — it cannot drift from this document without failing
`the_frame_order_names_every_phase_exactly_once`.

```
Deaths → Fog → Input → CoCommand → AiThink → Think → Intent
       → Movement → Combat → Bounty → Economy → Upkeep → Feed → Cosmetic
```

| Set | Who lives there |
| --- | --- |
| `Deaths` | `apply_death` |
| `Fog` | `update_fog` (wraps the older `FogSet` handle) |
| `Input` | ui.rs gesture chain, `bridge::poll_commands`, command.rs dispatchers, hotkeys, `status_probe` |
| `CoCommand` | copilot.rs `CopilotSet` |
| `AiThink` | `ai::ai_think`, `seed_machine_autocast` |
| `Think` | doctrine.rs — postures, retreat, leash, auto-cast; trigger.rs — the trigger evaluator, the contingent member of the same family |
| `Intent` | `intent::apply_intents` (wraps `IntentApply`) |
| `Movement` | units.rs — spawn, path, steer, separate |
| `Combat` | combat.rs — acquire, engage, projectiles, abilities, damage |
| `Bounty` | bounty.rs — spawn, claim, expire |
| `Economy` | economy.rs — bank, build, research, harvest, train, buy |
| `Upkeep` | xp, regen, cooldowns, status, supply, tech, win check |
| `Feed` | `produce_game_events`, `write_snapshot`, fingerprint, logging, headless exit |
| `Cosmetic` | health bars, rings, shockwaves, orb pulses, camera — outside the contract |

Before this, the schedule had exactly two ordering handles — `FogSet` and
`IntentApply` — and everything else was left to Bevy's multi-threaded
executor, which resolves conflicting systems against whatever is running on
another thread. Movement, combat and separation all take `&mut Transform`, so
two runs of the same binary could step the same units in different orders.

Three edges were already in the code and are **re-encoded**, not invented:
`Deaths → Fog` (`update_fog.after(apply_death)` — the dead stop seeing),
`Fog → Intent` (an order is judged against the visibility its issuer has now),
and `Input`/`CoCommand → Intent` (the bridge poll and the co-command layer both
declared `.before(IntentApply)`).

The rest was genuinely ambiguous and was **chosen** here:

- **`Deaths`/`Fog` lead the frame.** Forced: fog must follow death and intent
  must follow fog. The consequence is that damage dealt in `Combat` becomes a
  despawn at the top of the *next* frame — a one-tick lag the old schedule
  already had about half the time, now had consistently.
- **`Think` before `Intent`.** command.rs already states the rule: "a fresh
  direct order issued in the same frame still wins." Standing orders execute
  first so an explicit order can overrule them, never the reverse. The cost is
  that a posture set *through* the compiler takes effect one tick later.
- **`AiThink` before `Think`**, because the scripted commander writes the
  `SquadOrders` doctrine then executes.
- **`Movement` before `Combat`**, so a unit shoots from where it now stands.
- **`Bounty` before `Economy`**, so treasure claimed this frame banks this
  frame.
- **`Upkeep`/`Feed` last**, so recounts, the win check and the snapshot all
  describe the frame that just finished.

Parallelism *within* a set is fine where systems don't conflict; where they do
(`regen_health` vs `tick_status_effects`, `bank_bounties` vs `harvest_loop`)
they are explicitly `.chain()`ed rather than left to the executor.

The five older named sets are **nested** inside `SimSet` with `configure_sets`
rather than restated per system — `FogSet` in `Fog`, `CommandNodeRefresh` and
`BridgePoll` in `Input`, `CopilotSet` in `CoCommand`, `IntentApply` in
`Intent`. That is what keeps the guarantee from rotting: a system added later
carrying only `.in_set(IntentApply)` inherits the frame order instead of
quietly landing outside it.

### 2. One seeded RNG

`shared::SimRng` is the only source of gameplay randomness in a running match.
`WC3_SEED=<u64>` sets it; the default is a fresh random seed **logged at
startup**, so a normal match stays unpredictable and any match can be replayed
from its own log. Terrain was already deterministic (`terrain.rs` seeds
`StdRng` from the fixed `MAP_SEED`); bounty placement was not, and now is.

Iteration order counts as randomness too. `SquadOrders`, the fog `ghosts` map
and the whole `GameEvents` memo are `BTreeMap`s, because std's `HashMap`
reseeds its hasher **per process** — a hash map on any of those paths means
squads execute, ghosts are targeted and event lines are emitted in a different
order in every run.

### 3. One fixed tick

`WC3_FIXED_DT=0.05` (headless only) installs Bevy's
`TimeUpdateStrategy::ManualDuration`, so each frame advances the clock by a
constant instead of by however long the frame took. Without it every
accumulator in the sim — attack cooldowns, construction, projectile flight,
every `on_timer` gate — integrates a wall-clock delta and no two runs agree.
It drives `Time<Real>` too, so bridge.rs's poll/snapshot cadence stops being a
property of the host. `WC3_SPEED` is ignored while it is set.

### Proving it

`WC3_FINGERPRINT=<seconds>` logs a hash of the entire world (raw IEEE bits of
every unit's and building's position and health, entity ids, both economies) at
fixed game-time intervals. `tools/determinism_check.sh` runs two headless
AI-vs-AI matches with one seed and diffs those lines; it exits 0 only if every
sample matches. **All three envs are opt-in — with none of them set, behaviour
is exactly what it was.**

## Bevy 0.16 API notes (avoid stale idioms)

- Spawning visible meshes: `commands.spawn((Mesh3d(meshes.add(Cuboid::new(1.0,1.0,1.0))), MeshMaterial3d(materials.add(StandardMaterial { base_color: Color::srgb(0.2,0.4,0.9), ..default() })), Transform::from_translation(pos)))` — no `PbrBundle` (bundles are gone).
- `Color::srgb(r,g,b)` / `Color::srgba` — `Color::rgb` does not exist.
- Camera: `commands.spawn((Camera3d::default(), Transform::from_xyz(..).looking_at(..), MainCamera))`.
- Lights: `DirectionalLight { illuminance: 8_000.0, shadows_enabled: true, ..default() }` + `Transform`; `AmbientLight` is a `Resource` (`app.insert_resource(AmbientLight { color: Color::WHITE, brightness: 300.0, ..default() })`).
- Events: `EventWriter::write(event)`, `EventReader::read()`.
- Time: `time.delta_secs()`, `time.elapsed_secs()`.
- Input: `Res<ButtonInput<KeyCode>>`, `Res<ButtonInput<MouseButton>>`,
  `KeyCode::KeyA` style. Cursor: `Query<&Window, With<PrimaryWindow>>` →
  `window.cursor_position()` (needs `use bevy::window::PrimaryWindow;`).
- Picking ray: use the provided `shared::cursor_to_ground(camera, cam_global_tf, cursor)`.
  For clicking entities, do your own distance test in XZ against unit positions
  (units ~0.7 radius, buildings `size/2`) — no physics engine, no mesh raycasts.
- UI: `Node { width: Val::Px(..), position_type: PositionType::Absolute, .. }`,
  `BackgroundColor(..)`, `Text::new("..")`, `TextFont { font_size: 18.0, ..default() }`,
  `TextColor(..)`, `Button` + `Interaction`. Text spans: parent `Text` entity with
  `TextSpan` children, or just rewrite `Text` each frame (fine here).
- `commands.entity(e).despawn()` is recursive in 0.16. Entities can be despawned
  by other systems the same frame — use `try_insert` when unsure the entity is
  alive; when following `Order::Attack(target)`/`Harvest(target)`, always
  `if let Ok(..) = query.get(target)` and fall back to Idle if the target is gone.
- `Query::single()`/`single_mut()` return `Result` in 0.16 — use `let Ok(x) = q.single() else { return; }`.
- **`GlobalTransform` is only propagated in `PostUpdate`.** Any ROOT entity you
  spawn *or* teleport during `Update` must seed/update its own
  `GlobalTransform::from(transform)` in the same statement that writes the
  `Transform` — otherwise every `GlobalTransform` reader that frame (combat.rs
  reads positions that way) sees the origin for a fresh spawn, or the stale
  pre-teleport position for a mover.

## Verification

When done, run `cargo check` from the repo root (timeout 600s — first run may
wait on a build lock held by another agent; that's fine, wait it out). Fix your
own file's errors. If errors are clearly in someone else's file, ignore them.
