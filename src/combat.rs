//! combat.rs — target acquisition, engagement, damage, projectiles, health bars.
//!
//! Owns: `Order::Attack` execution, auto-acquisition while Idle/AttackMove,
//! chasing (via `MoveTo`), attack cooldowns, melee + projectile damage, and the
//! floating health bars on everything with `Health` + `Team`.
//!
//! Does NOT despawn dead entities — shared.rs does that centrally.

use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use std::time::Duration;

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

/// Default aggro radius; a unit with a longer weapon range uses that instead.
const AGGRO_RADIUS: f32 = 12.0;
/// Approximate collision radius of a unit (buildings use `size * 0.5`).
// Unit body radius comes from shared::UNIT_RADIUS (scaled by UNIT_SCALE).
const PROJECTILE_SPEED: f32 = 30.0;
const PROJECTILE_HIT_DIST: f32 = 0.8;
const PROJECTILE_LIFETIME: f32 = 6.0;
/// Minimum seconds between re-paths while chasing (~4x/sec).
const REPATH_INTERVAL: f32 = 0.25;
/// Re-path immediately if the target drifted more than this since last path.
const REPATH_MOVE_EPS: f32 = 2.0;
/// How far a panicking worker runs toward its base.
const FLEE_DISTANCE: f32 = 8.0;
/// units.rs drops `MoveTo` once a unit is within ~1.5 world units of the spot
/// it was sent to, so a chase must aim this much INSIDE its own reach or the
/// attacker can "arrive" still out of range and re-path forever (a Worker's
/// 1.8 reach against a 4-wide Farm has no room for the slack otherwise).
const ARRIVAL_SLACK: f32 = 1.8;
/// How far inside max range a ranged unit's chase stand-off point sits.
/// Covers arrival slop while preserving the outranging advantage (a catapult
/// must NEVER stand inside tower fire).
const STANDOFF_MARGIN: f32 = 2.0;
/// A unit that only picked its target up on instinct (Order::Idle — auto
/// acquisition or retaliation) gives up once the target is this far away.
/// Without it a chase can cross the whole map — and a hero that just town
/// portalled home would immediately walk all the way back to its old fight.
/// Explicit `Order::Attack` and attack-move pushes are never abandoned here.
const CHASE_GIVE_UP: f32 = 40.0;
/// Lifetime of the expanding ring drawn when a hero slams.
const SHOCKWAVE_TIME: f32 = 0.35;
/// Radius the shockwave ring starts at before expanding to the slam radius.
const SHOCKWAVE_START: f32 = 2.0;

// ---------------------------------------------------------------------------
// Module-private components / events / resources
// ---------------------------------------------------------------------------

/// The entity this unit is currently trying to kill. Combat-module private.
#[derive(Component, Clone, Copy, Debug)]
struct AttackTarget(Entity);

/// Per-unit combat bookkeeping (attack cooldown + chase re-path throttle).
#[derive(Component, Debug)]
struct CombatState {
    cooldown: f32,
    repath: f32,
    last_target_pos: Vec3,
}

impl Default for CombatState {
    fn default() -> Self {
        CombatState { cooldown: 0.0, repath: 0.0, last_target_pos: Vec3::splat(f32::MAX) }
    }
}

/// Per-tower bookkeeping: fire cooldown + the unit it is shooting at.
/// Attached lazily (like `CombatState` on units) to every building whose
/// `building_stats(kind).attack` is `Some`, construction site or not — the
/// firing system is what refuses to shoot while `UnderConstruction`.
#[derive(Component, Debug, Default)]
struct TowerState {
    cooldown: f32,
    target: Option<Entity>,
}

/// Bolts leave a tower from the platform on top of its shaft, not from the
/// ground — see the Tower mesh in economy.rs.
const TOWER_MUZZLE_HEIGHT: f32 = 5.0;

/// A flying arrow/bolt homing on `target`.
#[derive(Component, Debug)]
struct Projectile {
    target: Entity,
    owner: Entity,
    damage: f32,
    speed: f32,
    life: f32,
}

/// Damage is routed through this event so retaliation logic lives in one place.
#[derive(Event, Debug)]
struct DamageEvent {
    victim: Entity,
    attacker: Entity,
    amount: f32,
}

/// An ability's expanding ground ring. Despawns itself after `SHOCKWAVE_TIME`;
/// `radius` is the ability's radius, so every ability draws its true footprint.
#[derive(Component)]
struct Shockwave {
    age: f32,
    radius: f32,
}

/// Marker: this entity already has a floating health bar child.
#[derive(Component)]
struct HasHealthBar;

/// The status ring currently worn by a buffed/debuffed unit: which child
/// entity draws it and which kind it is coloured for. One ring per unit —
/// stacking five glows would be less legible, not more.
#[derive(Component)]
struct StatusRing {
    ring: Entity,
    kind: StatusKind,
}

/// Root of a health bar (child of the owner, billboarded each frame).
#[derive(Component)]
struct HealthBarRoot {
    owner: Entity,
}

/// The green/red foreground quad of a health bar.
#[derive(Component)]
struct HealthBarFill {
    width: f32,
}

#[derive(Resource)]
struct CombatAssets {
    quad: Handle<Mesh>,
    bar_bg: Handle<StandardMaterial>,
    /// Index 0 = empty (red) .. last = full (green).
    hp_mats: Vec<Handle<StandardMaterial>>,
    proj_mesh: Handle<Mesh>,
    proj_human: Handle<StandardMaterial>,
    proj_claude: Handle<StandardMaterial>,
    /// Priestess bolts read as light, not arrows, whichever side casts them.
    proj_holy: Handle<StandardMaterial>,
    /// Flat ring (unit radius, XZ plane) used for the hero slam shockwave.
    ring_mesh: Handle<Mesh>,
    shock_mat: Handle<StandardMaterial>,
    /// Same ring, tinted per ability effect (heal = green, militia = yellow).
    shock_heal_mat: Handle<StandardMaterial>,
    shock_militia_mat: Handle<StandardMaterial>,
    /// One material per `StatusKind`, in `ALL_STATUS_KINDS` order — the ring
    /// worn by a buffed or debuffed unit.
    status_mats: Vec<Handle<StandardMaterial>>,
}

impl CombatAssets {
    fn proj_mat(&self, team: Team, kind: Option<UnitKind>) -> &Handle<StandardMaterial> {
        // The Priestess throws light; everyone else throws team-coloured shafts.
        if kind == Some(UnitKind::Priestess) {
            return &self.proj_holy;
        }
        match team {
            Team::Human => &self.proj_human,
            Team::Claude => &self.proj_claude,
        }
    }
    fn shock_mat(&self, effect: AbilityEffect) -> &Handle<StandardMaterial> {
        match effect {
            AbilityEffect::Damage => &self.shock_mat,
            AbilityEffect::Heal => &self.shock_heal_mat,
            AbilityEffect::Militia => &self.shock_militia_mat,
            // A status ability paints its ring in the colour its effect wears
            // on the affected units — one legend for the cast and the buff.
            AbilityEffect::ApplyStatus { status, .. } => self.status_mat(status),
        }
    }
    /// Ring material for a status kind (also used for the persistent ring under
    /// an affected unit). Built once at startup, one per kind.
    fn status_mat(&self, kind: StatusKind) -> &Handle<StandardMaterial> {
        let i = ALL_STATUS_KINDS
            .iter()
            .position(|k| *k == kind)
            .unwrap_or(0);
        &self.status_mats[i]
    }
    fn hp_mat(&self, frac: f32) -> &Handle<StandardMaterial> {
        let last = self.hp_mats.len().saturating_sub(1);
        let idx = ((frac.clamp(0.0, 1.0) * last as f32).round() as usize).min(last);
        &self.hp_mats[idx]
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<DamageEvent>()
            .add_systems(Startup, setup_combat_assets)
            .add_systems(
                Update,
                (
                    ensure_combat_state,
                    ensure_tower_state,
                    handle_attack_orders,
                    acquire_targets.run_if(on_timer(Duration::from_millis(200))),
                    tower_acquire.run_if(on_timer(Duration::from_millis(200))),
                    engagement,
                    tower_fire,
                    update_projectiles,
                    cast_abilities,
                    use_items,
                    apply_damage,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    attach_health_bars.run_if(on_timer(Duration::from_millis(150))),
                    update_health_bars,
                    update_status_rings,
                )
                    .chain(),
            )
            .add_systems(Update, update_shockwaves);
    }
}

fn setup_combat_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let quad = meshes.add(Rectangle::new(1.0, 1.0));

    let bar_bg = materials.add(StandardMaterial {
        base_color: Color::srgb(0.04, 0.04, 0.05),
        unlit: true,
        ..default()
    });

    const STEPS: usize = 9;
    let hp_mats = (0..STEPS)
        .map(|i| {
            let f = i as f32 / (STEPS - 1) as f32; // 0 = empty, 1 = full
            let r = (2.0 * (1.0 - f)).min(1.0);
            let g = (2.0 * f).min(1.0);
            materials.add(StandardMaterial {
                base_color: Color::srgb(r * 0.95, g * 0.85, 0.12),
                unlit: true,
                ..default()
            })
        })
        .collect();

    let proj_mesh = meshes.add(Cuboid::new(0.12, 0.12, 0.7));
    let proj_human = materials.add(StandardMaterial {
        base_color: Color::srgb(0.75, 0.9, 1.0),
        unlit: true,
        ..default()
    });
    let proj_claude = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.85, 0.35),
        unlit: true,
        ..default()
    });

    let proj_holy = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 1.0, 0.95),
        unlit: true,
        ..default()
    });

    let ring_mesh = meshes.add(Torus::new(0.86, 1.0));
    let mut ring_mat = |r: f32, g: f32, b: f32| {
        materials.add(StandardMaterial {
            base_color: Color::srgba(r, g, b, 0.75),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })
    };
    let shock_mat = ring_mat(1.0, 0.86, 0.35);
    let shock_heal_mat = ring_mat(0.35, 1.0, 0.5);
    let shock_militia_mat = ring_mat(1.0, 0.95, 0.2);
    // Status rings sit on the ground under a unit all the time, so they are a
    // touch more solid than a shockwave that flashes and is gone.
    let status_mats: Vec<Handle<StandardMaterial>> = ALL_STATUS_KINDS
        .iter()
        .map(|kind| {
            materials.add(StandardMaterial {
                base_color: kind.tint().with_alpha(0.85),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })
        })
        .collect();

    commands.insert_resource(CombatAssets {
        quad,
        bar_bg,
        hp_mats,
        proj_mesh,
        proj_human,
        proj_claude,
        proj_holy,
        ring_mesh,
        shock_mat,
        shock_heal_mat,
        shock_militia_mat,
        status_mats,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn xz(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    xz(a - b).length()
}

fn xz_dist_sq(a: Vec3, b: Vec3) -> f32 {
    xz(a - b).length_squared()
}

/// Is this candidate airborne? Buildings never are.
fn is_air(unit: Option<&Unit>) -> bool {
    target_is_air(unit.map(|u| u.kind))
}

/// Radius used for range checks against a target.
fn target_radius(building: Option<&Building>) -> f32 {
    match building {
        Some(b) => building_stats(b.kind).size * 0.5,
        None => UNIT_RADIUS,
    }
}

/// Drop the current target and pick a sensible follow-up for the given order.
fn clear_target(commands: &mut Commands, entity: Entity, order: &Order) {
    let mut ec = commands.entity(entity);
    ec.try_remove::<AttackTarget>();
    match order {
        // Explicit attack finished -> go idle (and auto-acquire again later).
        Order::Attack(_) => {
            ec.try_insert(Order::Idle);
        }
        // Keep marching toward the attack-move destination.
        Order::AttackMove(p) => {
            ec.try_insert(MoveTo { target: *p });
        }
        _ => {}
    }
}

fn clamp_to_map(p: Vec3) -> Vec3 {
    Vec3::new(
        p.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
        0.0,
        p.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
    )
}

// ---------------------------------------------------------------------------
// Order handling & acquisition
// ---------------------------------------------------------------------------

/// Every unit carries combat bookkeeping so cooldowns survive target switches.
fn ensure_combat_state(
    mut commands: Commands,
    query: Query<Entity, (With<Unit>, Without<CombatState>)>,
) {
    for entity in &query {
        commands.entity(entity).try_insert(CombatState::default());
    }
}

/// React to `Changed<Order>`: adopt explicit attack targets, drop stale ones.
fn handle_attack_orders(
    mut commands: Commands,
    query: Query<(Entity, &Order, Option<&AttackTarget>), (Changed<Order>, With<Unit>)>,
) {
    for (entity, order, current) in &query {
        match order {
            Order::Attack(target) => {
                commands
                    .entity(entity)
                    .try_insert((AttackTarget(*target), CombatState::default()));
            }
            // Idle is also what we reset to ourselves after a kill, and what a
            // retaliating unit sits in — never yank the target away here, the
            // acquisition pass owns idle units.
            Order::Idle => {}
            // Anything else (Move / AttackMove / Harvest / Build / Return /
            // Follow) means "stop fighting this"; AttackMove re-acquires on
            // its own, Follow deliberately never does.
            _ => {
                if current.is_some() {
                    commands.entity(entity).try_remove::<AttackTarget>();
                }
            }
        }
    }
}

/// Rank of a candidate's class in a focus-fire list: lower is juicier,
/// unlisted classes sort after everything named.
/// The counter-triangle, in one place: how much of an attacker's listed damage
/// actually lands on this target.
///
/// Keyed off `TargetClass` rather than off `UnitKind` equality, so adding a
/// second siege engine or a second mounted kind is a line in
/// `TargetClass::of`, not a new branch here. At most one multiplier applies —
/// a building is never cavalry — so they can never compound.
fn type_damage_mult(stats: &UnitStats, target_kind: Option<UnitKind>, is_building: bool) -> f32 {
    // Structures are classified structure-first, as they were before this was
    // a table: whatever else is true of the thing, if it has a Building
    // component the siege multiplier is the one that matters.
    let class = if is_building {
        Some(TargetClass::Building)
    } else {
        TargetClass::of(target_kind, false)
    };
    match class {
        Some(TargetClass::Building) => stats.vs_building_mult,
        Some(TargetClass::Siege) => stats.vs_siege_mult,
        Some(TargetClass::Cavalry) => stats.vs_cavalry_mult,
        _ => 1.0,
    }
}

fn priority_rank(class: Option<TargetClass>, priority: &TargetPriority) -> usize {
    match class {
        Some(class) => priority
            .0
            .iter()
            .position(|listed| *listed == class)
            .unwrap_or(priority.0.len()),
        None => priority.0.len(),
    }
}

/// Idle / attack-moving combat units grab the best enemy in aggro range —
/// nearest by default, or the highest-priority class first when the unit
/// carries a `TargetPriority` (doctrine). A `LeashPolicy` hides everything
/// outside the anchor radius, so leashed units never take the bait.
#[allow(clippy::type_complexity)]
fn acquire_targets(
    mut commands: Commands,
    seekers: Query<
        (
            Entity,
            &Unit,
            &Team,
            &Transform,
            &Order,
            Option<&MoveTo>,
            Option<&TargetPriority>,
            Option<&LeashPolicy>,
            Option<&Militia>,
        ),
        Without<AttackTarget>,
    >,
    candidates: Query<
        (Entity, &Team, &Transform, &Health, Option<&Unit>, Option<&Building>),
        Or<(With<Unit>, With<Building>)>,
    >,
) {
    let list: Vec<(Entity, Team, Vec3, Option<TargetClass>, bool, bool)> = candidates
        .iter()
        .filter(|(_, _, _, health, _, _)| health.current > 0.0)
        .map(|(entity, team, tf, _, unit, building)| {
            (
                entity,
                *team,
                tf.translation,
                TargetClass::of(unit.map(|u| u.kind), building.is_some()),
                building.is_some(),
                is_air(unit),
            )
        })
        .collect();
    if list.is_empty() {
        return;
    }

    for (entity, unit, team, tf, order, move_to, priority, leash, militia) in &seekers {
        // Workers have damage but never pick fights on their own — unless the
        // Town Hall called them to arms, in which case they are soldiers.
        if unit.kind == UnitKind::Worker && militia.is_none() {
            continue;
        }
        // Aggro while idle, attack-moving, or standing around after a
        // completed Move order (a finished Move shouldn't leave a unit
        // permanently passive — and Stop is implemented as a zero-length Move).
        let aggro_ok = match order {
            Order::Idle | Order::AttackMove(_) => true,
            Order::Move(_) => move_to.is_none(),
            // Escorts stick to their followee instead of wandering off after
            // whatever walks past.
            Order::Follow(_) => false,
            _ => false,
        };
        if !aggro_ok {
            continue;
        }
        let stats = unit_stats(unit.kind);
        let radius = AGGRO_RADIUS.max(stats.range);
        let max_dist_sq = radius * radius;
        // Siege engines are structure-killers, not skirmishers: they reach for
        // the nearest enemy BUILDING in range and only swing at units when
        // there is no building to break. An explicit `TargetPriority` from the
        // strategic layer still wins — doctrine outranks the default instinct.
        let siege = unit.kind == UnitKind::Catapult && priority.is_none();
        // (priority rank, distance²) — lowest rank wins, distance breaks ties.
        let mut best: Option<(usize, f32, Entity)> = None;
        for (cand, cand_team, cand_pos, cand_class, cand_is_building, cand_is_air) in &list {
            if *cand == entity || *cand_team == *team {
                continue;
            }
            // A weapon that cannot reach this altitude never even sees the
            // target: no acquisition means no chase, no deadlock, and no
            // footman jogging under a Gryphon for the rest of the match.
            if !unit_can_hit(unit.kind, *cand_is_air) {
                continue;
            }
            // Leashed units simply cannot see anything past their tether.
            if let Some(leash) = leash {
                if xz_dist(*cand_pos, leash.anchor) > leash.radius {
                    continue;
                }
            }
            let d = xz_dist_sq(tf.translation, *cand_pos);
            if d >= max_dist_sq {
                continue;
            }
            let rank = match priority {
                Some(p) => priority_rank(*cand_class, p),
                None if siege => usize::from(!*cand_is_building),
                None => 0,
            };
            let better = match best {
                None => true,
                Some((best_rank, best_dist, _)) => {
                    rank < best_rank || (rank == best_rank && d < best_dist)
                }
            };
            if better {
                best = Some((rank, d, *cand));
            }
        }
        if let Some((_, _, target)) = best {
            commands
                .entity(entity)
                .try_insert((AttackTarget(target), CombatState::default()));
        }
    }
}

// ---------------------------------------------------------------------------
// Engagement: chase, face, swing
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn engagement(
    mut commands: Commands,
    time: Res<Time>,
    assets: Res<CombatAssets>,
    mut damage: EventWriter<DamageEvent>,
    // `Option<&Hero>` is read-only here; the only `&mut Hero` access in this
    // module lives in `cast_abilities`, a separate system.
    mut attackers: Query<(
        Entity,
        &Unit,
        &Team,
        &mut Transform,
        &Order,
        &AttackTarget,
        &mut CombatState,
        Option<&MoveTo>,
        Option<&Hero>,
        Option<&LeashPolicy>,
        Option<&Militia>,
        Option<&StatusEffects>,
    )>,
    // GlobalTransform (not Transform) so this never conflicts with the mutable
    // attacker query — attackers can themselves be targets.
    targets: Query<(&GlobalTransform, &Team, &Health, Option<&Building>, Option<&Unit>)>,
    // The attacker's team research. Read here and handed to the stat law, so
    // no arm below ever reaches for a research level on its own.
    research: Res<TeamResearch>,
) {
    let dt = time.delta_secs();

    for (
        entity,
        unit,
        team,
        mut tf,
        order,
        at,
        mut state,
        move_to,
        hero,
        leash,
        militia,
        status,
    ) in &mut attackers
    {
        state.cooldown = (state.cooldown - dt).max(0.0);
        state.repath = (state.repath - dt).max(0.0);

        // Someone gave this unit a non-combat order: stop fighting.
        if !matches!(order, Order::Attack(_) | Order::AttackMove(_) | Order::Idle) {
            commands.entity(entity).try_remove::<AttackTarget>();
            continue;
        }

        // Target may have been despawned this very frame.
        let Ok((target_gt, target_team, target_hp, target_building, target_unit)) =
            targets.get(at.0)
        else {
            clear_target(&mut commands, entity, order);
            continue;
        };
        if target_hp.current <= 0.0 || *target_team == *team {
            clear_target(&mut commands, entity, order);
            continue;
        }
        // THE deadlock guard. Acquisition already refuses unreachable
        // altitudes, but an EXPLICIT `Order::Attack` from a player or a
        // commander bypasses acquisition entirely — and a footman told to kill
        // a Gryphon would otherwise chase it across the map forever, never in
        // range, never giving up. Drop it instead: `clear_target` turns the
        // order back into Idle, and the unit re-acquires something it can
        // actually kill on the next acquisition tick.
        if !unit_can_hit(unit.kind, is_air(target_unit)) {
            clear_target(&mut commands, entity, order);
            continue;
        }

        let stats = unit_stats(unit.kind);
        // Everything a buff or debuff can touch comes from the ONE modifier
        // function — never off `stats` directly. `stats.range`,
        // `vs_building_mult` and friends are unmodifiable, so they stay raw.
        // Attack research rides in through the same door as a damage buff:
        // one call, one struct, and the flat term lands in `bonus_damage`.
        let effective = effective_unit_stats_with(unit.kind, status, research.get(*team).bonus());
        let target_pos = target_gt.translation();
        let my_pos = tf.translation;
        let reach = stats.range + target_radius(target_building);

        // Instinct has a leash of its own: stop chasing something that is now
        // half a map away (teleports, kited runners).
        if matches!(order, Order::Idle) && xz_dist(my_pos, target_pos) > CHASE_GIVE_UP {
            clear_target(&mut commands, entity, order);
            continue;
        }

        if xz_dist(my_pos, target_pos) > reach {
            // --- chase ---
            // Stand just outside the target's footprint rather than inside it.
            let away = xz(my_pos - target_pos).normalize_or_zero();
            let away = if away.length_squared() < 1e-6 { Vec3::X } else { away };
            // Ranged units engage from near MAX range — outranging the target
            // is the whole identity of siege (catapult 20 vs tower 16), and
            // kiting distance for archers. Melee still hugs the footprint.
            // STANDOFF_MARGIN absorbs pathing arrival slop so the stand point
            // stays comfortably within reach after the unit stops; the old
            // range*0.6 walked catapults INSIDE tower fire.
            let radius = target_radius(target_building);
            let effective = (stats.range - STANDOFF_MARGIN).max(stats.range * 0.5);
            let stand_dist = (radius + effective)
                .min(reach - ARRIVAL_SLACK)
                .max(radius * 0.9);
            let stand = clamp_to_map(target_pos + away * stand_dist);
            // Doctrine: a leashed unit refuses to be pulled past its tether.
            if let Some(leash) = leash {
                if xz_dist(stand, leash.anchor) > leash.radius {
                    clear_target(&mut commands, entity, order);
                    continue;
                }
            }
            let drifted = xz_dist(state.last_target_pos, target_pos) > REPATH_MOVE_EPS;
            if move_to.is_none() || state.repath <= 0.0 || drifted {
                commands.entity(entity).try_insert(MoveTo { target: stand });
                state.repath = REPATH_INTERVAL;
                state.last_target_pos = target_pos;
            }
            continue;
        }

        // --- in range: stop, face, swing ---
        if move_to.is_some() {
            commands.entity(entity).try_remove::<MoveTo>();
        }
        let look = Vec3::new(target_pos.x, my_pos.y, target_pos.z);
        if look.distance_squared(my_pos) > 1e-4 {
            tf.look_at(look, Vec3::Y);
        }

        if state.cooldown > 0.0 {
            continue;
        }
        // A slowed unit swings slower: the cooldown it re-arms with is the
        // effective one, so the debuff shows up in dps without a second rule.
        state.cooldown = effective.attack_cooldown.max(0.1);

        // Heroes hit harder every level; the type multipliers stack the
        // counter-triangle on top of that. The target type is known here, so
        // projectiles are minted carrying the already-multiplied damage — a
        // catapult boulder homing on a structure lands for the full siege
        // amount.
        let type_mult = type_damage_mult(
            &stats,
            target_unit.map(|u| u.kind),
            target_building.is_some(),
        );
        // A worker under Call to Arms swings a militia weapon, not a pickaxe.
        let base_damage = if unit.kind == UnitKind::Worker && militia.is_some() {
            MILITIA_DAMAGE
        } else {
            stats.damage
        };
        let damage_amount = base_damage
            * hero.map_or(1.0, |h| Hero::damage_mult(h.level))
            * type_mult
            // Outgoing damage buffs (Warcry and friends) land here.
            * effective.damage_mult
            // ...and attack research lands HERE, outside every multiplier, so
            // +3 is +3 whether the swinger is a level-10 hero or a militia
            // worker, and whether the thing being hit is a man or a wall.
            + effective.bonus_damage;

        if stats.projectile {
            let origin = my_pos + Vec3::Y * 1.3;
            let aim = target_pos + Vec3::Y * 1.0;
            let mut ptf = Transform::from_translation(origin);
            if aim.distance_squared(origin) > 1e-4 {
                ptf.look_at(aim, Vec3::Y);
            }
            commands.spawn((
                Mesh3d(assets.proj_mesh.clone()),
                MeshMaterial3d(assets.proj_mat(*team, Some(unit.kind)).clone()),
                ptf,
                Projectile {
                    target: at.0,
                    owner: entity,
                    damage: damage_amount,
                    speed: PROJECTILE_SPEED,
                    life: PROJECTILE_LIFETIME,
                },
            ));
        } else {
            damage.write(DamageEvent {
                victim: at.0,
                attacker: entity,
                amount: damage_amount,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Towers: static defense. Buildings with `BuildingStats::attack`.
//
// Deliberately much simpler than unit combat: a tower cannot move, cannot be
// ordered, has no `Order`/`MoveTo`/doctrine, and never retaliates or flees —
// it only ever holds a target and fires on cooldown.
//
// Towers shoot UNITS ONLY. Letting them plink at enemy buildings would be
// worse than useless: buildings can't be killed fast enough at tower dps to
// matter, and a tower distracted by the far side of an enemy expansion would
// ignore the army walking past it. Enemy structures near a tower are the
// attacking army's problem, not the tower's.
// ---------------------------------------------------------------------------

/// Give every armed building its firing state (mirrors `ensure_combat_state`).
fn ensure_tower_state(
    mut commands: Commands,
    query: Query<(Entity, &Building), Without<TowerState>>,
) {
    for (entity, building) in &query {
        if building_stats(building.kind).attack.is_some() {
            commands.entity(entity).try_insert(TowerState::default());
        }
    }
}

/// Keep or replace each finished tower's target: hold fire on a live enemy
/// unit still inside `attack.range`, otherwise grab the nearest one.
fn tower_acquire(
    mut towers: Query<(&Building, &Team, &Transform, &mut TowerState), Without<UnderConstruction>>,
    // Units only (see the module note above); `Without<Building>` keeps this
    // provably disjoint from the tower query.
    candidates: Query<
        (Entity, &Team, &GlobalTransform, &Health, &Unit),
        (With<Unit>, Without<Building>),
    >,
) {
    for (building, team, tf, mut state) in &mut towers {
        let Some(attack) = building_stats(building.kind).attack else {
            continue;
        };
        let max_dist_sq = attack.range * attack.range;
        let pos = tf.translation;

        // Current target still worth shooting?
        if let Some(current) = state.target {
            let still_good = candidates.get(current).is_ok_and(|(_, t, gt, hp, unit)| {
                hp.current > 0.0
                    && *t != *team
                    && xz_dist_sq(pos, gt.translation()) < max_dist_sq
                    && (attack.can_hit_air || !is_flying_kind(unit.kind))
            });
            if still_good {
                continue;
            }
            state.target = None;
        }

        let mut best: Option<(f32, Entity)> = None;
        for (cand, cand_team, cand_gt, health, cand_unit) in &candidates {
            if *cand_team == *team || health.current <= 0.0 {
                continue;
            }
            // Emplacements that cannot elevate simply do not see air.
            if !attack.can_hit_air && is_flying_kind(cand_unit.kind) {
                continue;
            }
            let d = xz_dist_sq(pos, cand_gt.translation());
            if d >= max_dist_sq {
                continue;
            }
            if best.map_or(true, |(best_d, _)| d < best_d) {
                best = Some((d, cand));
            }
        }
        state.target = best.map(|(_, cand)| cand);
    }
}

/// Tick tower cooldowns and rain bolts off the platform. Under-construction
/// towers are filtered out, so a half-built tower is a free kill.
fn tower_fire(
    mut commands: Commands,
    time: Res<Time>,
    assets: Res<CombatAssets>,
    mut towers: Query<
        (
            Entity,
            &Building,
            &Team,
            &Transform,
            &mut TowerState,
            Option<&StatusEffects>,
        ),
        Without<UnderConstruction>,
    >,
    targets: Query<(&Team, &GlobalTransform, &Health, &Unit), (With<Unit>, Without<Building>)>,
) {
    let dt = time.delta_secs();

    for (entity, building, team, tf, mut state, status) in &mut towers {
        state.cooldown = (state.cooldown - dt).max(0.0);

        let Some(attack) = building_stats(building.kind).attack else {
            continue;
        };
        let Some(target) = state.target else {
            continue;
        };
        // The target may have died or been despawned since acquisition.
        let Ok((target_team, target_gt, target_hp, target_unit)) = targets.get(target) else {
            state.target = None;
            continue;
        };
        if target_hp.current <= 0.0 || *target_team == *team {
            state.target = None;
            continue;
        }
        if !attack.can_hit_air && is_flying_kind(target_unit.kind) {
            state.target = None;
            continue;
        }
        let target_pos = target_gt.translation();
        if xz_dist(tf.translation, target_pos) > attack.range {
            state.target = None;
            continue;
        }
        if state.cooldown > 0.0 {
            continue;
        }
        // Emplacements are buffable too — one law, one call, whether the thing
        // holding the weapon has legs or foundations.
        let effective = effective_stats(BaseStats::of_building_attack(&attack), status);
        state.cooldown = effective.attack_cooldown.max(0.1);

        let origin = tf.translation + Vec3::Y * TOWER_MUZZLE_HEIGHT;
        let aim = target_pos + Vec3::Y * 1.0;
        let mut ptf = Transform::from_translation(origin);
        if aim.distance_squared(origin) > 1e-4 {
            ptf.look_at(aim, Vec3::Y);
        }
        commands.spawn((
            Mesh3d(assets.proj_mesh.clone()),
            MeshMaterial3d(assets.proj_mat(*team, None).clone()),
            ptf,
            Projectile {
                target,
                // Owner is the tower itself: `apply_damage` looks the attacker's
                // Team up through `Or<(With<Unit>, With<Building>)>`, so a unit
                // shot by a tower retaliates INTO the tower — melee units path
                // to it and start sieging, which is the behaviour we want.
                owner: entity,
                damage: attack.damage * effective.damage_mult,
                speed: PROJECTILE_SPEED,
                life: PROJECTILE_LIFETIME,
            },
        ));
    }
}

// ---------------------------------------------------------------------------
// Abilities: one generic executor, driven by the `AbilityDef` tables
// ---------------------------------------------------------------------------

/// One `CastAbility` resolved down to "who, where, which ability, how strong".
struct ResolvedCast {
    def: AbilityDef,
    team: Team,
    center: Vec3,
    /// `def.power` after hero level scaling (Militia and ApplyStatus ignore the
    /// scaling — theirs is a duration and a magnitude, not a damage number).
    power: f32,
}

/// Execute `CastAbility` for BOTH caster families, v2 style:
///
///   * units — the ability list comes from `abilities_of_unit(kind)`; the
///     event's selector picks a slot (`None` = first unlocked), and the cast is
///     gated on that slot's `AbilityCooldowns` entry plus the hero's mana;
///   * buildings — `abilities_of_building(kind)`, same list/selector/cooldown
///     machinery, no mana and no level scaling.
///
/// Unlocks are evaluated here, once, against the caster's own level and its
/// team's tech tier — so a locked ability cannot be cast by the UI, the AI, the
/// auto-caster or a bridge commander, and none of them need their own copy of
/// the rule.
///
/// The `&mut Hero` here is the module's only mutable hero access; `engagement`
/// reads heroes in a different system, so the two can never alias.
#[allow(clippy::type_complexity)]
fn cast_abilities(
    mut commands: Commands,
    time: Res<Time>,
    assets: Res<CombatAssets>,
    tiers: Res<TechTiers>,
    mut events: EventReader<CastAbility>,
    mut damage: EventWriter<DamageEvent>,
    // `Without<Building>` is load-bearing: heroes and ability buildings now
    // share the `AbilityCooldowns` component, so the two caster queries need an
    // explicit disjointness proof to both take it mutably (B0001).
    mut unit_casters: Query<
        (&Unit, &mut Hero, &Team, &Transform, Option<&mut AbilityCooldowns>),
        Without<Building>,
    >,
    mut building_casters: Query<
        (&Building, &Team, &Transform, Option<&mut AbilityCooldowns>),
        Without<UnderConstruction>,
    >,
    mut affected: Query<
        (
            Entity,
            &Team,
            &GlobalTransform,
            &mut Health,
            Option<&Unit>,
            Option<&mut StatusEffects>,
        ),
        Or<(With<Unit>, With<Building>)>,
    >,
) {
    let now = time.elapsed_secs();

    for ev in events.read() {
        // --- resolve the caster, then the ability --------------------------
        let resolved = if let Ok((unit, mut hero, team, tf, cooldowns)) =
            unit_casters.get_mut(ev.caster)
        {
            let list = abilities_of_unit(unit.kind);
            let ctx = UnlockCtx::new(hero.level, tiers.get(*team));
            let Some(index) = resolve_ability(list, ev.ability.as_ref(), ctx) else {
                continue;
            };
            let def = list[index];
            if !ability_ready(&def, Some(&hero), cooldowns.as_deref(), index) {
                continue;
            }
            hero.mana = (hero.mana - def.mana_cost).max(0.0);
            start_cooldown(&mut commands, ev.caster, cooldowns, index, def.cooldown);
            ResolvedCast {
                def,
                team: *team,
                center: tf.translation,
                power: def.power * Hero::damage_mult(hero.level),
            }
        } else if let Ok((building, team, tf, cooldowns)) = building_casters.get_mut(ev.caster) {
            let list = abilities_of_building(building.kind);
            let ctx = UnlockCtx::building(tiers.get(*team));
            let Some(index) = resolve_ability(list, ev.ability.as_ref(), ctx) else {
                continue;
            };
            let def = list[index];
            if !ability_ready(&def, None, cooldowns.as_deref(), index) {
                continue;
            }
            start_cooldown(&mut commands, ev.caster, cooldowns, index, def.cooldown);
            ResolvedCast { def, team: *team, center: tf.translation, power: def.power }
        } else {
            continue;
        };

        let ResolvedCast { def, team, center, power } = resolved;

        // --- apply the effect ----------------------------------------------
        for (entity, other_team, gt, mut health, unit, mut status) in &mut affected {
            if health.current <= 0.0 || xz_dist(center, gt.translation()) > def.radius {
                continue;
            }
            // Ground AoE stops at the ground: the Champion's Slam passes
            // harmlessly under a flyer, while the Priestess's Heal reaches up
            // to one. Both are `hits_air` in the ability table, not code here.
            if !def.hits_air && is_air(unit) {
                continue;
            }
            match def.effect {
                AbilityEffect::Damage => {
                    if other_team == &team {
                        continue;
                    }
                    // Health is only ever subtracted in `apply_damage`.
                    damage.write(DamageEvent { victim: entity, attacker: ev.caster, amount: power });
                }
                AbilityEffect::Heal => {
                    if other_team != &team || unit.is_none() {
                        continue;
                    }
                    health.current = (health.current + power).min(health.max);
                }
                AbilityEffect::Militia => {
                    if other_team != &team || unit.map(|u| u.kind) != Some(UnitKind::Worker) {
                        continue;
                    }
                    // `power` is a duration here, so it is never level-scaled.
                    commands
                        .entity(entity)
                        .try_insert(Militia { until: now + def.power });
                }
                // The whole point of (A) meeting (B): a status ability is a
                // table row. `power` is the magnitude, `duration` the seconds,
                // `targets` says who — and shared.rs expires it.
                AbilityEffect::ApplyStatus { status: kind, targets, also } => {
                    // (`status` is the target's component, rebound mutable.)
                    let matches = match targets {
                        AbilityTargets::Enemies => other_team != &team && unit.is_some(),
                        AbilityTargets::Allies => other_team == &team && unit.is_some(),
                        AbilityTargets::OwnWorkers => {
                            other_team == &team && unit.map(|u| u.kind) == Some(UnitKind::Worker)
                        }
                    };
                    if !matches {
                        continue;
                    }
                    // One cast, one or two statuses. `also` shares this cast's
                    // duration and targets and brings only its own magnitude —
                    // Sanctuary's heal-over-time and its armour arrive
                    // together, expire together, and are still two ordinary
                    // instances the moment they land.
                    let mut fresh = StatusEffects::new();
                    let sink: &mut StatusEffects = match status {
                        Some(ref mut existing) => &mut *existing,
                        None => &mut fresh,
                    };
                    sink.apply(StatusEffect::new(
                        kind,
                        def.power,
                        now,
                        def.duration,
                        StatusSource::Ability,
                    ));
                    if let Some((extra, magnitude)) = also {
                        sink.apply(StatusEffect::new(
                            extra,
                            magnitude,
                            now,
                            def.duration,
                            StatusSource::Ability,
                        ));
                    }
                    if status.is_none() {
                        commands.entity(entity).try_insert(fresh);
                    }
                }
            }
        }

        commands.spawn((
            Mesh3d(assets.ring_mesh.clone()),
            MeshMaterial3d(assets.shock_mat(def.effect).clone()),
            Transform::from_xyz(center.x, 0.15, center.z)
                .with_scale(Vec3::new(SHOCKWAVE_START, 0.5, SHOCKWAVE_START)),
            Shockwave { age: 0.0, radius: def.radius },
        ));
    }
}

/// Put one ability slot on cooldown, creating the store on the caster's first
/// cast. Heroes and buildings share it, so this is written once.
fn start_cooldown(
    commands: &mut Commands,
    caster: Entity,
    cooldowns: Option<Mut<AbilityCooldowns>>,
    index: usize,
    secs: f32,
) {
    match cooldowns {
        Some(mut cooldowns) => cooldowns.start(index, secs),
        None => {
            let mut fresh = AbilityCooldowns::default();
            fresh.start(index, secs);
            commands.entity(caster).try_insert(fresh);
        }
    }
}

/// Expand each shockwave ring out to its ability's radius, then despawn it.
fn update_shockwaves(
    mut commands: Commands,
    time: Res<Time>,
    mut waves: Query<(Entity, &mut Transform, &mut Shockwave)>,
) {
    let dt = time.delta_secs();
    for (entity, mut tf, mut wave) in &mut waves {
        wave.age += dt;
        if wave.age >= SHOCKWAVE_TIME {
            commands.entity(entity).try_despawn();
            continue;
        }
        let t = (wave.age / SHOCKWAVE_TIME).clamp(0.0, 1.0);
        let radius = SHOCKWAVE_START + (wave.radius - SHOCKWAVE_START) * t;
        tf.scale = Vec3::new(radius, 0.5, radius);
    }
}

// ---------------------------------------------------------------------------
// Hero items
// ---------------------------------------------------------------------------

/// Execute `UseItem`: consume the slot and apply the effect. Potions heal here
/// (combat owns `Health`); Town Portals delegate to units.rs, which owns
/// Transforms, via a `TeleportRequest`.
fn use_items(
    mut commands: Commands,
    time: Res<Time>,
    mut events: EventReader<UseItem>,
    mut teleports: EventWriter<TeleportRequest>,
    // Health lives on the SHARED `buffed` query, not here: a hero is one of
    // the units an item can buff, and two queries cannot both hold `&mut
    // Health` (B0001). So the potion heals through the same handle the banner
    // buffs through.
    mut heroes: Query<(&Team, &Transform, &mut Inventory)>,
    halls: Query<(&Building, &Team, &Transform), Without<UnderConstruction>>,
    mut buffed: StatusTargets,
) {
    let now = time.elapsed_secs();

    for ev in events.read() {
        let Ok((team, tf, mut inventory)) = heroes.get_mut(ev.hero) else {
            continue;
        };
        if !buffed.get(ev.hero).is_ok_and(|(_, _, _, hp, _)| hp.current > 0.0) {
            continue;
        }
        let Some(Some(item)) = inventory.0.get(ev.slot).copied() else {
            continue;
        };
        let team = *team;
        let hero_pos = tf.translation;
        // Every item below is consumed. Doing it once, up front, is also the
        // rule: a scroll that finds no hall still burns — no free retries.
        inventory.0[ev.slot] = None;

        // The nearest rung of our own hall ladder. Both teleport items home in
        // on the same spot, so the search is written once.
        let nearest_hall = || {
            halls
                .iter()
                .filter(|(building, hall_team, _)| is_hall(building.kind) && **hall_team == team)
                .map(|(_, _, hall_tf)| hall_tf.translation)
                .min_by(|a, b| {
                    xz_dist_sq(hero_pos, *a)
                        .partial_cmp(&xz_dist_sq(hero_pos, *b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        };

        match item {
            ItemId::HealingPotion => {
                if let Ok((_, _, _, mut health, _)) = buffed.get_mut(ev.hero) {
                    health.current = (health.current + POTION_HEAL).min(health.max);
                }
            }
            // Two shop items are now nothing but a status application. They go
            // through the SAME `StatusEffects::apply` an ability uses, tagged
            // `StatusSource::Item` so a future dispel can tell them apart, and
            // shared.rs expires them — no item ever grows an expiry system.
            ItemId::BootsOfSpeed => {
                apply_status_around(
                    &mut commands,
                    &mut buffed,
                    now,
                    hero_pos,
                    team,
                    0.0,
                    Some(ev.hero),
                    StatusKind::Haste,
                    BOOTS_HASTE,
                    BOOTS_DURATION,
                );
            }
            ItemId::BannerOfCommand => {
                apply_status_around(
                    &mut commands,
                    &mut buffed,
                    now,
                    hero_pos,
                    team,
                    BANNER_RADIUS,
                    None,
                    StatusKind::ArmorBuff,
                    BANNER_ARMOR,
                    BANNER_DURATION,
                );
            }
            ItemId::TownPortal => match nearest_hall() {
                Some(dest) => {
                    teleports.write(TeleportRequest {
                        center: ev.hero,
                        radius: PORTAL_RADIUS,
                        dest,
                        army_only: false,
                    });
                }
                None => warn!("TownPortal used with no completed TownHall to return to"),
            },
            // The late-game map-control item. THE RULE: hero + every own
            // non-worker unit anywhere on the map, to the hall nearest the
            // HERO (not nearest each unit — one destination, so an army
            // arrives together). Workers stay on the gold. Expressed entirely
            // as a `TeleportRequest` with a map-spanning radius, so units.rs
            // needed one new flag and no new code path.
            ItemId::ScrollOfMassTeleport => match nearest_hall() {
                Some(dest) => {
                    teleports.write(TeleportRequest {
                        center: ev.hero,
                        radius: MASS_TELEPORT_RADIUS,
                        dest,
                        army_only: true,
                    });
                }
                None => warn!("ScrollOfMassTeleport used with no completed hall to return to"),
            },
        }
    }
}

/// Every unit an item may buff: the status framework's write side, as a query.
type StatusTargets<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Team,
        &'static GlobalTransform,
        &'static mut Health,
        Option<&'static mut StatusEffects>,
    ),
    With<Unit>,
>;

/// Lay `kind` at `magnitude` for `duration` on own living units — either one
/// named entity (`only`), or everything within `radius` of `center`.
///
/// This is `cast_abilities`'s ApplyStatus arm, minus the ability: items are the
/// second producer of statuses, and they must land the same way abilities do
/// (same `apply`, same stacking policy, same central expiry) or the two would
/// drift. Buildings are never targets — a banner steadies soldiers, not walls.
#[allow(clippy::too_many_arguments)]
fn apply_status_around(
    commands: &mut Commands,
    targets: &mut StatusTargets,
    now: f32,
    center: Vec3,
    team: Team,
    radius: f32,
    only: Option<Entity>,
    kind: StatusKind,
    magnitude: f32,
    duration: f32,
) {
    for (entity, other_team, gt, health, status) in targets.iter_mut() {
        if health.current <= 0.0 || *other_team != team {
            continue;
        }
        match only {
            Some(wanted) if wanted != entity => continue,
            None if xz_dist(center, gt.translation()) > radius => continue,
            _ => {}
        }
        let effect = StatusEffect::new(kind, magnitude, now, duration, StatusSource::Item);
        match status {
            Some(mut existing) => existing.apply(effect),
            None => {
                let mut fresh = StatusEffects::new();
                fresh.apply(effect);
                commands.entity(entity).try_insert(fresh);
            }
        }
    }
}

/// Home projectiles onto their target; despawn on hit, expiry or target loss.
fn update_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    mut damage: EventWriter<DamageEvent>,
    mut projectiles: Query<(Entity, &mut Transform, &mut Projectile)>,
    targets: Query<(&GlobalTransform, &Health), Without<Projectile>>,
) {
    let dt = time.delta_secs();

    for (entity, mut tf, mut proj) in &mut projectiles {
        proj.life -= dt;
        if proj.life <= 0.0 {
            commands.entity(entity).try_despawn();
            continue;
        }
        let Ok((target_gt, target_hp)) = targets.get(proj.target) else {
            commands.entity(entity).try_despawn();
            continue;
        };
        if target_hp.current <= 0.0 {
            commands.entity(entity).try_despawn();
            continue;
        }

        let aim = target_gt.translation() + Vec3::Y * 1.0;
        let delta = aim - tf.translation;
        let dist = delta.length();
        let step = proj.speed * dt;

        if dist <= PROJECTILE_HIT_DIST.max(step) {
            damage.write(DamageEvent {
                victim: proj.target,
                attacker: proj.owner,
                amount: proj.damage,
            });
            commands.entity(entity).try_despawn();
            continue;
        }

        tf.translation += delta / dist * step;
        let aim_now = aim;
        if aim_now.distance_squared(tf.translation) > 1e-4 {
            tf.look_at(aim_now, Vec3::Y);
        }
    }
}

/// The one place `Health` is subtracted. Also drives retaliation.
#[allow(clippy::type_complexity)]
fn apply_damage(
    mut commands: Commands,
    time: Res<Time>,
    mut events: EventReader<DamageEvent>,
    mut healths: Query<&mut Health>,
    // Read-only status lookup, disjoint from `healths` (different component),
    // so damage reduction costs nothing but a get.
    shields: Query<&StatusEffects>,
    victims: Query<(
        &Unit,
        &Team,
        &Transform,
        &Order,
        Option<&AttackTarget>,
        Option<&Militia>,
    )>,
    attackers: Query<(&Team, Option<&Unit>), Or<(With<Unit>, With<Building>)>>,
    research: Res<TeamResearch>,
) {
    for event in events.read() {
        // Everything the victim brings to the hit, resolved before the
        // subtraction. `victims` requires `Unit`, so a successful get is also
        // the test for "is this a unit?" — which is exactly the question armor
        // research asks. A building falls through with `ResearchBonus::NONE`
        // and takes the hit unreduced: research equips the army, and masonry
        // is what a Keep upgrade buys.
        let victim = victims.get(event.victim).ok();
        let bonus = victim
            .map(|(_, team, ..)| research.get(*team).bonus())
            .unwrap_or(ResearchBonus::NONE);
        // Incoming damage goes through the same law as outgoing damage:
        // whatever armour buffs the victim is carrying are applied HERE, once,
        // at the single point where health is subtracted.
        let effective =
            effective_stats_with(BaseStats::STATIC, shields.get(event.victim).ok(), bonus);
        let Ok(mut health) = healths.get_mut(event.victim) else {
            continue;
        };
        if health.current <= 0.0 {
            continue;
        }
        health.current -= damage_after_armor(
            event.amount,
            effective.damage_taken_mult,
            effective.flat_armor,
        );
        // Everything that takes a hit — unit, hero, building — is stamped, so
        // shared.rs's out-of-combat regen restarts its clock from here.
        commands
            .entity(event.victim)
            .try_insert(LastDamaged { at: time.elapsed_secs() });

        // --- retaliation (buildings never fight back) ---
        // Already resolved above for the armor lookup; `None` here means the
        // victim was a building, which is the same reason it does not retaliate.
        let Some((unit, team, tf, order, current_target, militia)) = victim else {
            continue;
        };
        if current_target.is_some() || !matches!(order, Order::Idle) {
            continue;
        }

        if unit.kind == UnitKind::Worker && militia.is_none() {
            // Workers panic and run home instead of fighting — militia stand
            // their ground and hit back like everyone else.
            let home = team.base_pos();
            let dir = xz(home - tf.translation).normalize_or_zero();
            let dir = if dir.length_squared() < 1e-6 { Vec3::X } else { dir };
            let flee = clamp_to_map(tf.translation + dir * FLEE_DISTANCE);
            commands
                .entity(event.victim)
                .try_insert(MoveTo { target: flee });
            continue;
        }

        // Fight back if the attacker is still alive, hostile, and actually
        // reachable — being strafed from the air is not a reason for a footman
        // to lock onto something it can never swing at.
        if let Ok((attacker_team, attacker_unit)) = attackers.get(event.attacker) {
            if *attacker_team != *team && unit_can_hit(unit.kind, is_air(attacker_unit)) {
                commands
                    .entity(event.victim)
                    .try_insert((AttackTarget(event.attacker), CombatState::default()));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Status rings: what a buff or a debuff LOOKS like
// ---------------------------------------------------------------------------
//
// A status effect that only exists in the numbers is a status effect the
// player cannot play around. The cheapest legible marker with the primitives
// already here is the shockwave torus, shrunk to body size and parked under the
// unit's feet as a child (so it inherits position and dies with its owner).
//
// One ring per unit, coloured by `StatusEffects::dominant` — debuffs outrank
// buffs, because "why is my footman crawling" is the more urgent question.

/// World radius of the ring drawn under an affected unit.
const STATUS_RING_RADIUS: f32 = 1.35;
/// Local Y of the ring inside the unit's (scaled) root — just above the ground.
const STATUS_RING_DROP: f32 = -0.42;

/// Attach, recolour and remove status rings. Runs every frame but touches only
/// units whose effect set actually changed state, so an unbuffed army costs one
/// query iteration and nothing else.
fn update_status_rings(
    mut commands: Commands,
    assets: Res<CombatAssets>,
    affected: Query<(Entity, &Transform, Option<&StatusEffects>, Option<&StatusRing>), With<Unit>>,
    mut ring_mats: Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    for (entity, tf, status, ring) in &affected {
        let wanted = status.and_then(|s| s.dominant());
        match (wanted, ring) {
            // Nothing to show, nothing shown.
            (None, None) => {}
            // Effects all gone: take the ring away.
            (None, Some(ring)) => {
                commands.entity(ring.ring).try_despawn();
                commands.entity(entity).try_remove::<StatusRing>();
            }
            // Newly affected: hang a ring under it.
            (Some(kind), None) => {
                // The root is scaled by UNIT_SCALE (heroes more), so the child
                // divides that back out to land at a constant world radius.
                let scale = STATUS_RING_RADIUS / tf.scale.x.max(0.01);
                let ring = commands
                    .spawn((
                        Mesh3d(assets.ring_mesh.clone()),
                        MeshMaterial3d(assets.status_mat(kind).clone()),
                        Transform::from_xyz(0.0, STATUS_RING_DROP, 0.0)
                            .with_scale(Vec3::new(scale, scale * 0.35, scale)),
                    ))
                    .id();
                commands
                    .entity(entity)
                    .try_insert(StatusRing { ring, kind })
                    .add_child(ring);
            }
            // Still affected: only repaint when the dominant kind changed.
            (Some(kind), Some(ring)) => {
                if ring.kind == kind {
                    continue;
                }
                if let Ok(mut material) = ring_mats.get_mut(ring.ring) {
                    material.0 = assets.status_mat(kind).clone();
                }
                commands
                    .entity(entity)
                    .try_insert(StatusRing { ring: ring.ring, kind });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Floating health bars
// ---------------------------------------------------------------------------

fn attach_health_bars(
    mut commands: Commands,
    assets: Res<CombatAssets>,
    query: Query<(Entity, Option<&Building>), (With<Health>, With<Team>, Without<HasHealthBar>)>,
) {
    for (entity, building) in &query {
        let (height, width, thickness) = match building {
            Some(b) => {
                let size = building_stats(b.kind).size;
                (size * 0.85 + 1.6, (size * 0.6).clamp(2.0, 5.0), 0.3)
            }
            // Local child coords — the unit root is scaled by UNIT_SCALE,
            // so these are divided down to land just above the bigger body.
            None => (1.55, 0.9, 0.12),
        };

        let background = commands
            .spawn((
                Mesh3d(assets.quad.clone()),
                MeshMaterial3d(assets.bar_bg.clone()),
                Transform::from_scale(Vec3::new(width + 0.1, thickness + 0.1, 1.0)),
            ))
            .id();
        let fill = commands
            .spawn((
                Mesh3d(assets.quad.clone()),
                MeshMaterial3d(assets.hp_mat(1.0).clone()),
                Transform {
                    translation: Vec3::new(0.0, 0.0, 0.02),
                    scale: Vec3::new(width, thickness, 1.0),
                    ..default()
                },
                HealthBarFill { width },
            ))
            .id();
        let root = commands
            .spawn((
                Transform::from_xyz(0.0, height, 0.0),
                Visibility::Hidden,
                HealthBarRoot { owner: entity },
            ))
            .id();
        commands.entity(root).add_children(&[background, fill]);
        commands.entity(entity).try_insert(HasHealthBar).add_child(root);
    }
}

#[allow(clippy::type_complexity)]
fn update_health_bars(
    mut commands: Commands,
    assets: Res<CombatAssets>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    children: Query<&Children>,
    owners: Query<(&Health, &GlobalTransform)>,
    mut roots: Query<(Entity, &HealthBarRoot, &mut Transform, &mut Visibility)>,
    mut fills: Query<
        (
            &HealthBarFill,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        Without<HealthBarRoot>,
    >,
) {
    let Ok(cam_gt) = camera.single() else {
        return;
    };
    let cam_rot = cam_gt.rotation();

    for (root_entity, root, mut root_tf, mut visibility) in &mut roots {
        // Owner gone (shared.rs despawns are recursive, but be defensive).
        let Ok((health, owner_gt)) = owners.get(root.owner) else {
            commands.entity(root_entity).try_despawn();
            continue;
        };

        let frac = if health.max > 0.0 {
            (health.current / health.max).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let wanted = if frac >= 0.999 {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
        if wanted == Visibility::Hidden {
            continue;
        }

        // Billboard: match the camera's world rotation, undoing the parent's.
        root_tf.rotation = owner_gt.rotation().inverse() * cam_rot;

        let Ok(kids) = children.get(root_entity) else {
            continue;
        };
        for kid in kids.iter() {
            if let Ok((fill, mut fill_tf, mut material)) = fills.get_mut(kid) {
                // Shrink from the right: scale down and shift left by the loss.
                fill_tf.scale.x = (fill.width * frac).max(0.0001);
                fill_tf.translation.x = -fill.width * (1.0 - frac) * 0.5;
                let wanted_mat = assets.hp_mat(frac).clone();
                if material.0 != wanted_mat {
                    material.0 = wanted_mat;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Balance probe
// ---------------------------------------------------------------------------
//
// A melee engagement in this game is arithmetic: two blocks close, then trade
// `damage * type_mult` every `attack_cooldown` until one block is gone.
// Movement decides *when* that starts, not who wins it. So the counter
// triangle can be checked without a World — this steps the real stat tables
// through the real multiplier rule and reports who is left standing.
//
// The numbers it guards are the ones that make the Spearman a counter rather
// than a strictly-better Footman: it must beat cavalry it could never catch,
// and it must lose to the same gold spent on Footmen.

#[cfg(test)]
mod tests {
    use super::*;

    /// One combatant's mutable state.
    struct Fighter {
        hp: f32,
        cooldown: f32,
    }

    /// Outcome of a block-vs-block fight: surviving HP on each side.
    struct Outcome {
        a_hp: Vec<f32>,
        b_hp: Vec<f32>,
    }

    impl Outcome {
        fn a_alive(&self) -> usize {
            self.a_hp.iter().filter(|hp| **hp > 0.0).count()
        }
        fn b_alive(&self) -> usize {
            self.b_hp.iter().filter(|hp| **hp > 0.0).count()
        }
        /// Winning side's remaining HP as a fraction of what it started with —
        /// "convincingly" should be a number, not a vibe.
        fn a_hp_fraction(&self, kind: UnitKind, n: usize) -> f32 {
            self.a_hp.iter().sum::<f32>() / (unit_stats(kind).hp * n as f32)
        }
    }

    /// Step `a_n` units of one kind against `b_n` of another until one side is
    /// wiped (or two game-minutes pass, meaning neither side can finish).
    ///
    /// Both sides resolve simultaneously, so nobody wins on initiative, and
    /// each attacker round-robins onto a living enemy rather than focus-firing
    /// — the game has no focus-fire order either, and perfect focus fire would
    /// hand every fight to whichever side merely brought more bodies.
    fn engage(a_kind: UnitKind, a_n: usize, b_kind: UnitKind, b_n: usize) -> Outcome {
        const DT: f32 = 0.02;
        const TIMEOUT: f32 = 120.0;

        let (a_stats, b_stats) = (unit_stats(a_kind), unit_stats(b_kind));
        let a_hit = a_stats.damage * type_damage_mult(&a_stats, Some(b_kind), false);
        let b_hit = b_stats.damage * type_damage_mult(&b_stats, Some(a_kind), false);

        let mut a: Vec<Fighter> = (0..a_n)
            .map(|_| Fighter { hp: a_stats.hp, cooldown: 0.0 })
            .collect();
        let mut b: Vec<Fighter> = (0..b_n)
            .map(|_| Fighter { hp: b_stats.hp, cooldown: 0.0 })
            .collect();

        let mut t = 0.0;
        while t < TIMEOUT {
            let a_live: Vec<usize> = (0..a.len()).filter(|i| a[*i].hp > 0.0).collect();
            let b_live: Vec<usize> = (0..b.len()).filter(|i| b[*i].hp > 0.0).collect();
            if a_live.is_empty() || b_live.is_empty() {
                break;
            }

            // Collect both sides' swings before applying either, so a fighter
            // that dies this tick still lands the blow it had already earned.
            let mut into_b = vec![0.0f32; b.len()];
            let mut into_a = vec![0.0f32; a.len()];
            for (slot, i) in a_live.iter().enumerate() {
                if a[*i].cooldown <= 0.0 {
                    a[*i].cooldown = a_stats.attack_cooldown;
                    into_b[b_live[slot % b_live.len()]] += a_hit;
                } else {
                    a[*i].cooldown -= DT;
                }
            }
            for (slot, i) in b_live.iter().enumerate() {
                if b[*i].cooldown <= 0.0 {
                    b[*i].cooldown = b_stats.attack_cooldown;
                    into_a[a_live[slot % a_live.len()]] += b_hit;
                } else {
                    b[*i].cooldown -= DT;
                }
            }
            for (f, dmg) in a.iter_mut().zip(into_a) {
                f.hp -= dmg;
            }
            for (f, dmg) in b.iter_mut().zip(into_b) {
                f.hp -= dmg;
            }
            t += DT;
        }

        Outcome {
            a_hp: a.iter().map(|f| f.hp.max(0.0)).collect(),
            b_hp: b.iter().map(|f| f.hp.max(0.0)).collect(),
        }
    }

    /// The multiplier table is keyed off `TargetClass`, and exactly one
    /// multiplier may ever apply to a swing.
    #[test]
    fn type_multipliers_follow_target_class() {
        let spear = unit_stats(UnitKind::Spearman);
        assert_eq!(
            type_damage_mult(&spear, Some(UnitKind::Raider), false),
            5.0,
            "the Spearman's whole reason to exist"
        );
        assert_eq!(type_damage_mult(&spear, Some(UnitKind::Footman), false), 1.0);
        assert_eq!(type_damage_mult(&spear, Some(UnitKind::Catapult), false), 1.0);
        assert_eq!(type_damage_mult(&spear, None, true), 1.0);

        // The pre-existing counters must survive the move onto TargetClass.
        let raider = unit_stats(UnitKind::Raider);
        assert_eq!(type_damage_mult(&raider, Some(UnitKind::Catapult), false), 2.0);
        assert_eq!(type_damage_mult(&raider, Some(UnitKind::Footman), false), 1.0);
        let catapult = unit_stats(UnitKind::Catapult);
        assert_eq!(type_damage_mult(&catapult, None, true), 6.0);
    }

    /// A 90g Spearman beats a 170g Raider one-on-one, and not by a hair —
    /// a counter nobody trusts is not a counter.
    #[test]
    fn spearman_beats_raider_one_on_one() {
        let out = engage(UnitKind::Spearman, 1, UnitKind::Raider, 1);
        assert_eq!(out.b_alive(), 0, "the Raider should die");
        assert_eq!(out.a_alive(), 1, "the Spearman should live");
        let left = out.a_hp_fraction(UnitKind::Spearman, 1);
        assert!(
            // Measured: 0.30 of its 160 hp. The margin is the design — wide
            // enough that a player believes the counter, narrow enough that
            // walking one Spearman at a Raider is not free.
            left > 0.25,
            "Spearman should finish comfortably ahead, had {left:.3} left",
        );
    }

    /// ...and is not a general-purpose upgrade: the same gold spent on
    /// Footmen beats a bigger block of Spearmen, so the cheap unit only pays
    /// off against the thing it is pointed at.
    #[test]
    fn equal_gold_footmen_beat_spearmen() {
        // 270 gold each way: 2 Footmen (135g) vs 3 Spearmen (90g).
        assert_eq!(unit_stats(UnitKind::Footman).cost_gold * 2, 270);
        assert_eq!(unit_stats(UnitKind::Spearman).cost_gold * 3, 270);
        let out = engage(UnitKind::Footman, 2, UnitKind::Spearman, 3);
        assert_eq!(out.b_alive(), 0, "the Spearmen should be wiped");
        // Measured: 1 of the 2 Footmen survives, on 24% of the pair's HP. Not
        // a rout in either direction — Spearmen are cheap enough that massing
        // them is a real option, just never a *free* one.
        assert!(out.a_alive() > 0, "at least one Footman should live");
    }

    /// The straight duel, for the record: a Footman handles a Spearman easily.
    #[test]
    fn footman_beats_spearman_one_on_one() {
        let out = engage(UnitKind::Footman, 1, UnitKind::Spearman, 1);
        assert_eq!(out.b_alive(), 0);
        assert!(
            out.a_hp_fraction(UnitKind::Footman, 1) > 0.5,
            "and without dropping below half"
        );
    }

    /// The same gold pointed at what it counters: 270g of Spearmen erases the
    /// cavalry it meets without losing a body.
    #[test]
    fn equal_gold_spearmen_beat_raiders() {
        // 270g buys 1.59 Raiders; 1 Raider vs 3 Spearmen is the closest
        // whole-unit trade, and it is still a rout.
        let out = engage(UnitKind::Spearman, 3, UnitKind::Raider, 1);
        assert_eq!(out.b_alive(), 0);
        assert_eq!(out.a_alive(), 3, "cavalry should not even take one with it");
    }
}
