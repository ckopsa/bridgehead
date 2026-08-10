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
  (initial town hall + 5 workers per team, death cleanup, supply recount, win check).
- `terrain.rs`: ground, doodads, resource nodes (gold mines at
  `GOLD_MINE_POSITIONS`, tree clusters), lighting, **RTS camera** (spawns the
  `MainCamera`), blocks trees/mines in `NavGrid`.
- `units.rs`: handles `SpawnUnitEvent` (meshes per kind/team), executes
  `Order::Move`/`Order::AttackMove` by inserting `MoveTo`, implements `MoveTo`
  pathfinding (A* over `NavGrid`) + steering + local separation, removes `MoveTo`
  on arrival.
- `combat.rs`: target acquisition (aggro radius when Idle/AttackMove),
  `Order::Attack`, chase via `MoveTo`, attack cooldowns, projectiles for
  `projectile: true` units, damage to `Health`, floating health bars.
  Death/despawn itself is handled centrally in shared.rs.
- `economy.rs`: handles `SpawnBuildingEvent`, construction progress,
  `Order::Build` (worker walks to site, pays, spawns UnderConstruction building,
  blocks NavGrid), `Order::Harvest`/`ReturnResources` loop (gold mines & trees →
  nearest own TownHall), processes `TrainingQueue` (pays at enqueue time — the
  enqueuer only checks affordability; economy deducts, checks supply, spawns via
  `SpawnUnitEvent` near the building when done, refunds nothing on death).
- `ui.rs`: selection (left click + drag box over own units/buildings), right-click
  context orders (enemy → Attack, resource node → Harvest for workers, ground →
  Move; A+click → AttackMove), building placement mode with ghost + affordability
  (writes `Order::Build`), training hotkeys/buttons on selected production
  buildings (push to `TrainingQueue` if affordable), top resource bar,
  selection info panel, game-over banner from `GameOver`.
- `ai.rs`: the Claude faction. Assigns idle workers to harvest, maintains build
  order (farms before supply block, barracks, more workers), trains army,
  attack-moves waves at the human base. Acts ONLY through the same primitives the
  UI uses: writing `Order`, pushing to `TrainingQueue` (after checking/paying the
  same way), sending events. Never teleports or cheats resources.

## Cross-module conventions

- **Orders**: `ui.rs`/`ai.rs` set `Order` on entities. Executors react via
  `Changed<Order>` and then own the follow-through. A module handling one order
  variant must tolerate the order being overwritten at any time (always re-check).
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

## Verification

When done, run `cargo check` from the repo root (timeout 600s — first run may
wait on a build lock held by another agent; that's fine, wait it out). Fix your
own file's errors. If errors are clearly in someone else's file, ignore them.
