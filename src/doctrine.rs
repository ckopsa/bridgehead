//! doctrine.rs — the tactical-doctrine executor.
//!
//! The strategic layer (bridge commander / scripted AI) SETS doctrine
//! components (`RetreatPolicy`, `LeashPolicy`, `AutoCastPolicy`, `SquadId`) and
//! the `SquadOrders` resource; this module carries them out every tick, acting
//! ONLY through the standard primitives:
//!
//!   * `Order` (via `try_insert`) — units.rs turns those into movement,
//!   * `MoveTo` — a plain re-path nudge for members already on the right order,
//!   * `CastAbility` events — combat.rs validates and executes.
//!
//! Nothing here pays, spawns, damages, or moves a Transform. Policies are just
//! components, so this applies to ANY team's units — whoever sets them gets the
//! behavior (a retreating worker is perfectly fine).
//!
//! `TargetPriority` and the acquisition/chase side of `LeashPolicy` live in
//! combat.rs; everything else is here.
//!
//! One thing here is NOT waiting to be told: the default squad. Every living
//! army unit of either team that nobody has assigned elsewhere is enrolled in
//! `DEFAULT_SQUAD`, and each team's `DEFAULT_SQUAD` gets a home `Defend`
//! posture seeded the moment it has none. A commander that says nothing (or
//! thinks for fifteen seconds between commands) therefore still fields a
//! pooled, reactive army instead of a field of statues; a commander that does
//! speak simply overwrites the seeded posture, and the seeding never writes
//! over an entry that already exists.

use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use std::time::Duration;

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

/// Retreat check cadence (~4 Hz) — HP can fall fast, so this is the quickest.
const RETREAT_MS: u64 = 250;
/// Leash recall cadence (~2 Hz).
const LEASH_MS: u64 = 500;
/// Hero auto-cast cadence (~2 Hz).
const AUTOCAST_MS: u64 = 500;
/// Squad re-tasking cadence (~1 Hz) — formations only need a slow heartbeat.
const SQUAD_MS: u64 = 1000;

/// Slack when asking "is this still the order we issued?" (world units).
const ORDER_EPS: f32 = 1.0;
/// How far from a squad's posture point still counts as "not there yet" when
/// deciding whether a stalled member deserves a fresh path.
const SQUAD_ARRIVE: f32 = 3.0;

/// Radius of the `Defend` posture seeded onto an untasked `DEFAULT_SQUAD`.
/// Wide enough to cover a base and the ground the scripted AI rallies on, so
/// an army waiting to move out is never yanked around by the seeded order.
const DEFAULT_DEFEND_RADIUS: f32 = 22.0;
/// How tightly foragers hold their muster point when the map has no bounty
/// left to hunt.
const FORAGE_MUSTER_RADIUS: f32 = 12.0;

/// Cohesive advance: when a Push/Forage squad's widest member strays farther
/// than this from the squad's centre of mass, the whole squad gathers before
/// pressing on — leaders wait, stragglers close up, the blob moves at the
/// slowest unit's pace. Solves the series' most repeated death: fast units
/// arriving at a defended position in packets, 30s ahead of their catapults
/// ("defeat in detail", R5 & R7 AARs).
const COHESION_SPREAD: f32 = 14.0;
/// How far ahead of the centroid the regroup point sits — keeps the gathered
/// blob creeping toward the objective instead of standing still.
const COHESION_STEP: f32 = 8.0;

// ---------------------------------------------------------------------------
// Module-private components
// ---------------------------------------------------------------------------

/// Present while a `RetreatPolicy` unit is falling back. Holds the rally we
/// sent it to so `rearm_retreat` can tell "still obeying our retreat" from
/// "the strategic layer re-tasked this unit".
#[derive(Component, Clone, Copy, Debug)]
struct Retreating {
    rally: Vec3,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct DoctrinePlugin;

impl Plugin for DoctrinePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Change-detection driven: must see every Order change, so it
                // runs every frame (the query is tiny — only retreaters).
                rearm_retreat,
                recover_retreaters.run_if(on_timer(Duration::from_millis(RETREAT_MS))),
                trigger_retreat.run_if(on_timer(Duration::from_millis(RETREAT_MS))),
                enforce_leash.run_if(on_timer(Duration::from_millis(LEASH_MS))),
                auto_cast_abilities.run_if(on_timer(Duration::from_millis(AUTOCAST_MS))),
                // Shares the squad heartbeat, and runs first so a unit born
                // this second is already a member when postures are executed.
                default_squad_autonomy.run_if(on_timer(Duration::from_millis(SQUAD_MS))),
                run_squad_postures.run_if(on_timer(Duration::from_millis(SQUAD_MS))),
            )
                .chain(),
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    Vec2::new(d.x, d.z).length()
}

/// Is the unit already carrying (effectively) the order we are about to give?
/// Used to avoid re-issuing an identical order every tick, which would reset
/// `Changed<Order>` and make combat.rs drop and re-acquire its target.
fn order_matches(order: &Order, wanted: &Order) -> bool {
    match (order, wanted) {
        (Order::Move(a), Order::Move(b)) | (Order::AttackMove(a), Order::AttackMove(b)) => {
            xz_dist(*a, *b) <= ORDER_EPS
        }
        (Order::Follow(a), Order::Follow(b)) => a == b,
        _ => false,
    }
}

/// A squad member is available for re-tasking when it is idle, or when it is
/// on a Move/AttackMove it has already finished (no `MoveTo` == arrived or
/// drifting). Units mid-fight (`Order::Attack`) or busy with economy orders
/// are left alone.
fn re_taskable(order: &Order, moving: bool) -> bool {
    match order {
        Order::Idle => true,
        Order::Move(_) | Order::AttackMove(_) => !moving,
        _ => false,
    }
}

/// If a squad is strung out past COHESION_SPREAD, the point it should gather
/// at (centre of mass, nudged toward `target` so the regroup itself advances).
/// `None` = squad is cohesive (or trivially small); press on to the target.
fn cohesion_point(positions: &[Vec3], target: Vec3) -> Option<Vec3> {
    if positions.len() < 2 {
        return None;
    }
    let centroid = positions.iter().copied().sum::<Vec3>() / positions.len() as f32;
    let spread = positions
        .iter()
        .map(|p| xz_dist(*p, centroid))
        .fold(0.0_f32, f32::max);
    if spread <= COHESION_SPREAD {
        return None;
    }
    let dir = Vec3::new(target.x - centroid.x, 0.0, target.z - centroid.z).normalize_or_zero();
    Some(Vec3::new(centroid.x, 0.0, centroid.z) + dir * COHESION_STEP)
}

/// Closest of `points` to `from` on the ground plane. `None` = empty slice.
fn nearest_point(points: &[Vec3], from: Vec3) -> Option<Vec3> {
    points.iter().copied().min_by(|a, b| {
        xz_dist(*a, from)
            .partial_cmp(&xz_dist(*b, from))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Centre of mass of the living enemy units standing inside a defended area,
/// clamped to stay within `radius` of `pos` so answering an incursion can never
/// turn into chasing the bait off the position. `None` = nothing to answer.
fn threat_point(
    hostiles: &Query<(&Team, &Transform, &Health), With<Unit>>,
    team: Team,
    pos: Vec3,
    radius: f32,
) -> Option<Vec3> {
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    for (hostile_team, tf, health) in hostiles {
        if *hostile_team == team || health.current <= 0.0 {
            continue;
        }
        if xz_dist(tf.translation, pos) > radius {
            continue;
        }
        sum += tf.translation;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let centroid = sum / count as f32;
    let offset = Vec3::new(centroid.x - pos.x, 0.0, centroid.z - pos.z);
    let len = offset.length();
    let clamped = if len > radius { pos + offset * (radius / len) } else { pos + offset };
    Some(Vec3::new(clamped.x, pos.y, clamped.z))
}

// ---------------------------------------------------------------------------
// 1. Retreat (~4 Hz)
// ---------------------------------------------------------------------------

/// Wounded units with a `RetreatPolicy` break off and fall back to their rally.
/// The `Retreating` marker keeps this from re-firing every tick.
fn trigger_retreat(
    mut commands: Commands,
    query: Query<(Entity, &RetreatPolicy, &Health), (With<Unit>, Without<Retreating>)>,
) {
    for (entity, policy, health) in &query {
        // Dead-but-not-yet-despawned units are shared.rs's problem.
        if health.max <= 0.0 || health.current <= 0.0 {
            continue;
        }
        if health.current / health.max >= policy.below_frac {
            continue;
        }
        let rally = policy.rally;
        // Order::Move disengages for free: combat.rs drops the AttackTarget on
        // any non-combat order, and units.rs paths us home.
        commands
            .entity(entity)
            .try_insert((Order::Move(rally), Retreating { rally }));
    }
}

/// Re-arm the retreat: the moment anyone gives a retreating unit an order that
/// is not our fall-back Move, the unit is back on duty and may retreat again
/// later. (Our own insert above matches, so it never self-cancels.)
fn rearm_retreat(
    mut commands: Commands,
    query: Query<(Entity, &Order, &Retreating), Changed<Order>>,
) {
    for (entity, order, retreating) in &query {
        if !order_matches(order, &Order::Move(retreating.rally)) {
            commands.entity(entity).try_remove::<Retreating>();
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Leash (~2 Hz)
// ---------------------------------------------------------------------------

/// Recall anyone who wandered (or was baited) outside their anchor radius.
/// Retreat outranks the leash — a fleeing unit is allowed to leave.
fn enforce_leash(
    mut commands: Commands,
    query: Query<(Entity, &LeashPolicy, &Transform, &Order), (With<Unit>, Without<Retreating>)>,
) {
    for (entity, leash, tf, order) in &query {
        if xz_dist(tf.translation, leash.anchor) <= leash.radius {
            continue;
        }
        let wanted = Order::Move(leash.anchor);
        if order_matches(order, &wanted) {
            // Already walking home; a fresh MoveTo re-paths it without
            // touching Order (so combat/change-detection stay undisturbed).
            commands
                .entity(entity)
                .try_insert(MoveTo { target: leash.anchor });
        } else {
            commands.entity(entity).try_insert(wanted);
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Hero auto-cast (~2 Hz)
// ---------------------------------------------------------------------------

/// Cast whenever the ability is up and enough bodies are inside its radius.
///
/// "Enough bodies" depends on what the ability DOES (`ability_of_unit`):
///   * Damage — enemy units in the blast (the Champion's slam);
///   * Heal — OWN units in the radius that are actually hurt (< 70% HP), so a
///     Priestess doesn't burn mana topping up scratches.
/// Building casters have no policy component, so they are never auto-cast.
fn auto_cast_abilities(
    mut casts: EventWriter<CastAbility>,
    heroes: Query<(Entity, &AutoCastPolicy, &Hero, &Unit, &Team, &Transform)>,
    others: Query<(&Team, &Transform, &Health, &Unit)>,
) {
    /// A hurt ally is one below this fraction of max HP.
    const HEAL_FRAC: f32 = 0.7;

    for (entity, policy, hero, unit, team, tf) in &heroes {
        let Some(def) = ability_of_unit(unit.kind) else {
            continue;
        };
        // Same gate combat.rs applies when it executes the cast.
        if hero.ability_cooldown > 0.0 || hero.mana < def.mana_cost {
            continue;
        }
        let count = others
            .iter()
            .filter(|(other_team, other_tf, health, other_unit)| {
                if health.current <= 0.0 || xz_dist(tf.translation, other_tf.translation) > def.radius
                {
                    return false;
                }
                // Only count what the ability can actually affect. Three
                // flyers overhead must not trip a Champion's auto-slam: the
                // shockwave would miss and the mana would be gone.
                if !def.hits_air && is_flying_kind(other_unit.kind) {
                    return false;
                }
                match def.effect {
                    AbilityEffect::Damage => *other_team != team,
                    AbilityEffect::Heal => {
                        *other_team == team
                            && health.max > 0.0
                            && health.current < health.max * HEAL_FRAC
                    }
                    // No auto-cast path for militia (buildings cast it).
                    AbilityEffect::Militia => false,
                }
            })
            .count() as u32;
        // `min_enemies` of 0 still needs someone to affect — never cast at air.
        if count >= policy.min_enemies.max(1) {
            casts.write(CastAbility { caster: entity });
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Squad postures (~1 Hz)
// ---------------------------------------------------------------------------

/// The anti-idle floor, run on the squad heartbeat.
///
/// 1. Enrolment: every living army unit with no `SquadId` joins `DEFAULT_SQUAD`
///    — including heroes, which benefit most from never standing still.
///    Workers are exempt: economy.rs owns them, and an attack-moving harvester
///    is a lost harvester. A commander that wants a unit elsewhere just sets
///    `SquadId`, and this never takes it back.
/// 2. Seeding: each team's `DEFAULT_SQUAD` gets a `Defend` at its own base if —
///    and only if — it currently has no posture at all. Commanders overwrite
///    freely and their posture sticks; this is a floor, not a leash.
fn default_squad_autonomy(
    mut commands: Commands,
    mut squad_orders: ResMut<SquadOrders>,
    ai: Res<AiControlled>,
    external: Res<ExternallyCommanded>,
    strays: Query<(Entity, &Unit, &Team, &Health), Without<SquadId>>,
) {
    // Autonomy floors exist to compensate for slow machine commanders. A
    // human with a mouse keeps full authority: their idle units stay exactly
    // where they were put.
    for (entity, unit, team, health) in &strays {
        if unit.kind == UnitKind::Worker
            || health.current <= 0.0
            || !machine_driven(&ai, &external, *team)
        {
            continue;
        }
        commands.entity(entity).try_insert(SquadId(DEFAULT_SQUAD));
    }

    for team in [Team::Human, Team::Claude] {
        if !machine_driven(&ai, &external, team) {
            continue;
        }
        // Read first: touching the ResMut mutably every tick would be a
        // pointless change-detection storm, and an existing entry (including
        // one a commander just wrote) must never be overwritten.
        if squad_orders.0.contains_key(&(team, DEFAULT_SQUAD)) {
            continue;
        }
        squad_orders.0.insert(
            (team, DEFAULT_SQUAD),
            SquadPosture::Defend {
                pos: team.base_pos(),
                radius: DEFAULT_DEFEND_RADIUS,
            },
        );
    }
}

/// Keep each squad acting like a standing formation: members that have gone
/// idle (or finished their last move) are folded back into the squad's posture.
/// Members whose squad has no posture entry are never touched.
#[allow(clippy::type_complexity)]
fn run_squad_postures(
    mut commands: Commands,
    mut squad_orders: ResMut<SquadOrders>,
    members: Query<
        (Entity, &SquadId, &Team, &Transform, &Order, Option<&MoveTo>),
        (With<Unit>, Without<Retreating>),
    >,
    // Read-only, so it may freely overlap `members` (defenders can themselves
    // be somebody else's threat).
    hostiles: Query<(&Team, &Transform, &Health), With<Unit>>,
    healths: Query<&Health>,
    // Live treasure. bounty.rs despawns a cache the instant it is claimed or
    // expires, so "still in this query" is the whole liveness test.
    bounties: Query<&Transform, With<Bounty>>,
    ai: Res<AiControlled>,
    external: Res<ExternallyCommanded>,
) {
    if squad_orders.0.is_empty() {
        return;
    }
    let bounty_points: Vec<Vec3> = bounties.iter().map(|tf| tf.translation).collect();
    // Snapshot so we can drop lapsed escorts while iterating.
    let postures: Vec<((Team, u8), SquadPosture)> =
        squad_orders.0.iter().map(|(k, v)| (*k, *v)).collect();
    let mut lapsed: Vec<(Team, u8)> = Vec::new();

    for ((team, squad), posture) in postures {
        // A team a human took back mid-game (F9) keeps its posture entries in
        // the map, but they stop EXECUTING — the mouse outranks the doctrine.
        if !machine_driven(&ai, &external, team) {
            continue;
        }
        // Escort dies -> the posture lapses so the strategic layer sees it
        // gone in the next snapshot and can issue something new.
        if let SquadPosture::Escort { unit } = posture {
            let alive = healths.get(unit).map(|h| h.current > 0.0).unwrap_or(false);
            if !alive {
                lapsed.push((team, squad));
                continue;
            }
        }

        // A Forage squad with nothing left to hunt IS a Defend squad sitting on
        // its muster point, so rewrite it into one and let the Defend path (and
        // its threat response) do the work — no second implementation to skew.
        let posture = match posture {
            SquadPosture::Forage { muster } if bounty_points.is_empty() => SquadPosture::Defend {
                pos: muster,
                radius: FORAGE_MUSTER_RADIUS,
            },
            other => other,
        };

        // A Defend squad that only walks stragglers home is furniture. If enemy
        // bodies are inside the held ground, everyone turns and answers them.
        let threat = match posture {
            SquadPosture::Defend { pos, radius } => threat_point(&hostiles, team, pos, radius),
            _ => None,
        };

        // Cohesive advance for offensive postures: a strung-out squad gathers
        // (leaders hold, stragglers close) instead of trickling into a defended
        // position in packets. Defend/Escort are exempt — defense is urgent and
        // short-ranged, escorts follow one unit anyway.
        let squad_positions: Vec<Vec3> = members
            .iter()
            .filter(|(_, ms, mt, ..)| **mt == team && ms.0 == squad)
            .map(|(_, _, _, tf, _, _)| tf.translation)
            .collect();
        let regroup = match posture {
            SquadPosture::Push { pos } => cohesion_point(&squad_positions, pos),
            SquadPosture::Forage { muster } if !bounty_points.is_empty() => {
                let centroid = squad_positions.iter().copied().sum::<Vec3>()
                    / (squad_positions.len().max(1)) as f32;
                let objective = nearest_point(&bounty_points, centroid).unwrap_or(muster);
                cohesion_point(&squad_positions, objective)
            }
            _ => None,
        };

        for (entity, member_squad, member_team, tf, order, move_to) in &members {
            if *member_team != team || member_squad.0 != squad {
                continue;
            }

            // Reactive defense outranks whatever a member was doing (retreaters
            // are already filtered out of the query — fleeing still wins).
            if let Some(threat_pos) = threat {
                let wanted = Order::AttackMove(threat_pos);
                if order_matches(order, &wanted) {
                    if move_to.is_none() && xz_dist(tf.translation, threat_pos) > SQUAD_ARRIVE {
                        commands
                            .entity(entity)
                            .try_insert(MoveTo { target: threat_pos });
                    }
                } else {
                    commands.entity(entity).try_insert(wanted);
                }
                continue;
            }

            if !re_taskable(order, move_to.is_some()) {
                continue;
            }

            let (wanted, path_point) = match posture {
                SquadPosture::Defend { pos, radius } => {
                    // Only stragglers get pulled back to the held ground.
                    if xz_dist(tf.translation, pos) <= radius {
                        continue;
                    }
                    (Order::AttackMove(pos), Some(pos))
                }
                // Keep pressing after every kill — as one blob, not a queue.
                SquadPosture::Push { pos } => {
                    let advance = regroup.unwrap_or(pos);
                    (Order::AttackMove(advance), Some(advance))
                }
                SquadPosture::Escort { unit } => {
                    if unit == entity {
                        continue; // never escort yourself
                    }
                    (Order::Follow(unit), None)
                }
                // Treasure hunt: each member walks to the cache nearest to
                // ITSELF, so a spread-out squad splits across the map instead
                // of queueing behind one pile. AttackMove, not Move, because a
                // forager that meets an enemy on the way should fight it — the
                // contested middle is exactly where bounties spawn.
                //
                // The empty-map case became Defend above, so `muster` here is
                // only a defensive fallback that should never be reached.
                SquadPosture::Forage { muster } => {
                    let target = regroup.unwrap_or_else(|| {
                        nearest_point(&bounty_points, tf.translation).unwrap_or(muster)
                    });
                    (Order::AttackMove(target), Some(target))
                }
            };

            if order_matches(order, &wanted) {
                // Already on this posture. Only nudge if it stalled short of
                // the destination (unreachable path, shoved off course).
                if let Some(point) = path_point {
                    if move_to.is_none() && xz_dist(tf.translation, point) > SQUAD_ARRIVE {
                        commands.entity(entity).try_insert(MoveTo { target: point });
                    }
                }
                continue;
            }
            commands.entity(entity).try_insert(wanted);
        }
    }

    for key in lapsed {
        squad_orders.0.remove(&key);
    }
}

/// A retreat is a trip to the infirmary, not permanent leave: once regen
/// lifts a retreater back above its threshold (+10% hysteresis so it doesn't
/// flap at the boundary), the marker clears and squad postures re-task it.
/// Playtest round 7: a latched retreat froze an entire army for five minutes
/// because postures skip `Retreating` members and nothing ever released them.
fn recover_retreaters(
    mut commands: Commands,
    query: Query<(Entity, &RetreatPolicy, &Health), With<Retreating>>,
) {
    for (entity, policy, health) in &query {
        if health.max <= 0.0 || health.current <= 0.0 {
            continue;
        }
        let threshold = (policy.below_frac + 0.10).min(1.0);
        if health.current / health.max >= threshold {
            commands.entity(entity).try_remove::<Retreating>();
        }
    }
}
