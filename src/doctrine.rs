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
//!
//! Who this runs for: **whoever set the policy.** Retreat, leash and auto-cast
//! were always team-blind; squad postures were not, and until docs/TEMPO.md
//! §2.0 found it, `run_squad_postures` early-returned on every team a machine
//! was not driving. A human's posture was stored and ignored. The gate is now
//! the posture map itself — a squad with a live entry executes — with one
//! carve-out for `DEFAULT_SQUAD`, which is the machine's anti-idle floor
//! rather than anything a player said. See `run_squad_postures`.

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
                // Postures read this frame's fog (Forage targeting, Defend
                // threat response), so the whole chain sits after the one
                // producer of it.
                .chain()
                .after(FogSet),
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
    fog: &FogGrid,
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
        // Doctrine is the strategic layer running at machine speed on the
        // player's behalf, so it obeys the player's fog. (The squad's own
        // units still swing at whatever closes on them — that is combat.rs's
        // aggro, which models their senses and is deliberately not gated.)
        if !fog.sees(tf.translation) {
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

/// Cast whenever an ability is up and enough bodies are inside its radius.
///
/// The policy is per-ability now (`AutoCastPolicy::rules`), so a hero with two
/// spells auto-casts each on its own trigger count, in slot order. Every rule
/// naming a locked or non-existent slot is simply skipped.
///
/// "Enough bodies" depends on what the ability DOES:
///   * Damage / ApplyStatus at enemies — enemy units in the radius;
///   * Heal — OWN units in the radius that are actually hurt (< 70% HP), so a
///     Priestess doesn't burn mana topping up scratches;
///   * ApplyStatus at allies — own units in the radius.
/// Building casters have no policy component, so they are never auto-cast.
fn auto_cast_abilities(
    tiers: Res<TechTiers>,
    mut casts: EventWriter<CastAbility>,
    // `Option<&Hero>`: casters, not heroes. The Sorcerer's Slow is the first
    // ability on a unit that has no level and no mana, and an auto-caster that
    // only knows how to drive heroes would have left it a statue.
    casters: Query<(
        Entity,
        &AutoCastPolicy,
        Option<&Hero>,
        &Unit,
        &Team,
        &Transform,
        Option<&AbilityCooldowns>,
    )>,
    others: Query<(&Team, &Transform, &Health, &Unit)>,
) {
    /// A hurt ally is one below this fraction of max HP.
    const HEAL_FRAC: f32 = 0.7;
    /// A heal-over-time is a 60s commitment, not a 12s top-up, so Sanctuary
    /// waits for allies that are properly in trouble rather than scratched.
    const HOT_FRAC: f32 = 0.6;

    for (entity, policy, hero, unit, team, tf, cooldowns) in &casters {
        let list = abilities_of_unit(unit.kind);
        let ctx = UnlockCtx::new(hero.map_or(0, |h| h.level), tiers.get(*team));
        for (index, min_targets) in policy.rules.iter().copied() {
            let Some(def) = list.get(index) else {
                continue;
            };
            if !ability_unlocked(def, ctx) {
                continue;
            }
            // Same gate combat.rs applies when it executes the cast.
            if !ability_ready(def, hero, cooldowns, index) {
                continue;
            }
            let count = others
                .iter()
                .filter(|(other_team, other_tf, health, other_unit)| {
                    if health.current <= 0.0
                        || xz_dist(tf.translation, other_tf.translation) > def.radius
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
                        AbilityEffect::ApplyStatus { targets, .. } => match targets {
                            AbilityTargets::Enemies => *other_team != team,
                            // An ally buff that HEALS counts only allies who
                            // need healing — same reasoning as `Heal` above,
                            // asked of what the effect does rather than of
                            // which ability it is.
                            AbilityTargets::Allies => {
                                *other_team == team
                                    && (!def.effect.heals()
                                        || (health.max > 0.0
                                            && health.current < health.max * HOT_FRAC))
                            }
                            AbilityTargets::OwnWorkers => {
                                *other_team == team && other_unit.kind == UnitKind::Worker
                            }
                        },
                    }
                })
                .count() as u32;
            // A trigger of 0 still needs someone to affect — never cast at air.
            if count < min_targets.max(1) {
                continue;
            }
            // An OFFENSIVE ally buff (Warcry) is worth nothing without a
            // fight: without this a Champion walking past its own worker line
            // would burn its 45s ultimate on mining. So the same threshold is
            // asked twice — enough allies to buff AND enough enemies to fight.
            // Defensive and healing ally buffs need no such second opinion.
            if matches!(
                def.effect,
                AbilityEffect::ApplyStatus {
                    status: StatusKind::DamageBuff,
                    targets: AbilityTargets::Allies,
                    ..
                }
            ) {
                let enemies = others
                    .iter()
                    .filter(|(other_team, other_tf, health, other_unit)| {
                        *other_team != team
                            && health.current > 0.0
                            && xz_dist(tf.translation, other_tf.translation) <= def.radius
                            && (def.hits_air || !is_flying_kind(other_unit.kind))
                    })
                    .count() as u32;
                if enemies < min_targets.max(1) {
                    continue;
                }
            }
            casts.write(CastAbility::index(entity, index));
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
    fog: Res<FogGrids>,
    ai: Res<AiControlled>,
    external: Res<ExternallyCommanded>,
) {
    if squad_orders.0.is_empty() {
        return;
    }
    // Every cache on the map. Which of them a given squad may hunt is decided
    // per team below — the same list filtered two ways, never two lists.
    let all_bounties: Vec<Vec3> = bounties.iter().map(|tf| tf.translation).collect();
    // Snapshot so we can drop lapsed escorts while iterating.
    let postures: Vec<((Team, u8), SquadPosture)> =
        squad_orders.0.iter().map(|(k, v)| (*k, *v)).collect();
    let mut lapsed: Vec<(Team, u8)> = Vec::new();

    for ((team, squad), posture) in postures {
        // THE OPT-IN TEST (docs/TEMPO.md §2.0, follow-up 2). This used to be
        // `!machine_driven(...) { continue }`, which meant a human's squad
        // posture was stored and then ignored — THESIS.md's "standing orders
        // run at machine speed for whichever player set them" was true for one
        // seat only. Now the posture map IS the opt-in: a squad with a live
        // entry executes, whoever wrote it, and a unit with no squad (or a
        // squad with no posture) is never touched. A human who assigns nothing
        // still keeps their units exactly where they put them.
        //
        // The one exception is `DEFAULT_SQUAD`: that entry is not something a
        // player said, it is the anti-idle floor `default_squad_autonomy`
        // seeds to compensate for a slow machine commander. It stays
        // machine-only, which is also the F9 handback rule — take a team back
        // from the AI and you inherit its squads but not its autopilot floor.
        if squad == DEFAULT_SQUAD && !machine_driven(&ai, &external, team) {
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

        let team_fog = fog.get(team);
        // Treasure THIS team can see. Forage used to hunt every cache on the
        // map, which was the doctrine layer quietly handing whoever set it a
        // map-wide treasure radar. Now a forager chases what its team has eyes
        // on — which is the symmetric rule doing its job, and a real change to
        // how the posture plays: Forage has become a posture for an army that
        // is already out on the map, not a homing beacon.
        let bounty_points: Vec<Vec3> = all_bounties
            .iter()
            .copied()
            .filter(|p| team_fog.sees(*p))
            .collect();

        // A Forage squad with nothing left to hunt IS a Defend squad sitting on
        // its muster point, so rewrite it into one and let the Defend path (and
        // its threat response) do the work — no second implementation to skew.
        // "Nothing left to hunt" now means "nothing we can SEE to hunt", so a
        // blind forager musters instead of marching on treasure it has no
        // business knowing about.
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
            SquadPosture::Defend { pos, radius } => {
                threat_point(&hostiles, team_fog, team, pos, radius)
            }
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
                // The see-nothing case became Defend above, so `muster` here is
                // only a defensive fallback.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A world with just the doctrine resources and no scripted AI on either
    /// seat, so `machine_driven` is false for `Team::Human` — i.e. the human is
    /// at the keyboard, which is exactly the case the old gate refused to serve.
    fn world() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SquadOrders>()
            .init_resource::<FogGrids>()
            .init_resource::<ExternallyCommanded>()
            .insert_resource(AiControlled { human: false, claude: false });
        app
    }

    fn spawn_footman(app: &mut App, team: Team, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                team,
                Transform::from_translation(at),
                Health::new(100.0),
                Order::Idle,
            ))
            .id()
    }

    fn order_of(app: &App, entity: Entity) -> Order {
        app.world().entity(entity).get::<Order>().cloned().unwrap()
    }

    /// docs/TEMPO.md §2.0, the bug this bead exists to fix: a posture set by a
    /// HUMAN team (nothing machine-driven anywhere) must actually execute. On
    /// master this assertion fails — the member sits on `Order::Idle` forever.
    #[test]
    fn a_human_teams_posture_executes() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);

        let objective = Vec3::new(30.0, 0.0, 30.0);
        let unit = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        app.world_mut().entity_mut(unit).insert(SquadId(1));
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: objective });

        app.update();

        match order_of(&app, unit) {
            Order::AttackMove(p) => assert!(
                xz_dist(p, objective) <= SQUAD_ARRIVE,
                "human squad ordered to {p:?}, expected the objective {objective:?}"
            ),
            other => panic!("human squad posture did not execute: {other:?}"),
        }
    }

    /// The other half of the opt-in test: a human unit that nobody enrolled in
    /// anything is never yanked around, even while a squad posture exists for
    /// some other squad on the same team.
    #[test]
    fn an_unassigned_human_unit_is_left_alone() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);

        let stray = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        let member = spawn_footman(&mut app, Team::Human, Vec3::new(-58.0, 0.0, -60.0));
        app.world_mut().entity_mut(member).insert(SquadId(2));
        app.world_mut().resource_mut::<SquadOrders>().0.insert(
            (Team::Human, 2),
            SquadPosture::Push { pos: Vec3::new(30.0, 0.0, 30.0) },
        );

        app.update();

        assert!(
            matches!(order_of(&app, stray), Order::Idle),
            "a human unit with no squad was re-tasked by doctrine"
        );
        assert!(matches!(order_of(&app, member), Order::AttackMove(_)));
    }

    /// The auto-enrol + seed floor exists to compensate for a slow MACHINE
    /// commander, so it stays machine-only: a human's idle units must not
    /// self-organise into `DEFAULT_SQUAD` behind their back.
    #[test]
    fn machine_seeding_stays_machine_only() {
        let mut app = world();
        app.add_systems(Update, default_squad_autonomy);

        let human = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        let claude = spawn_footman(&mut app, Team::Claude, Vec3::new(60.0, 0.0, 60.0));
        app.world_mut().resource_mut::<AiControlled>().claude = true;

        app.update();

        assert!(
            app.world().entity(human).get::<SquadId>().is_none(),
            "a human unit was auto-enrolled in the default squad"
        );
        assert_eq!(
            app.world().entity(claude).get::<SquadId>().copied(),
            Some(SquadId(DEFAULT_SQUAD)),
            "a machine-driven unit should still land in the default squad"
        );
        let orders = app.world().resource::<SquadOrders>();
        assert!(
            !orders.0.contains_key(&(Team::Human, DEFAULT_SQUAD)),
            "the human team got a seeded posture it never asked for"
        );
        assert!(orders.0.contains_key(&(Team::Claude, DEFAULT_SQUAD)));
    }

    /// F9 handback. Autopilot enrols the team's units and seeds the floor;
    /// hand the team back and the *player's own* squads keep running at machine
    /// speed (that is the whole point of the bead), while the autopilot's
    /// anti-idle floor stops — you inherit the army, not the autopilot.
    #[test]
    fn f9_handback_keeps_human_squads_and_drops_the_machine_floor() {
        let mut app = world();
        app.add_systems(Update, (default_squad_autonomy, run_squad_postures).chain());
        app.world_mut().resource_mut::<AiControlled>().human = true;

        let home = Team::Human.base_pos();
        let floor_member = spawn_footman(&mut app, Team::Human, home + Vec3::new(80.0, 0.0, 0.0));
        let mine = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        app.world_mut().entity_mut(mine).insert(SquadId(1));
        let objective = Vec3::new(30.0, 0.0, 30.0);
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: objective });

        // --- under autopilot: both the floor and the player's squad run -----
        app.update();
        assert_eq!(
            app.world().entity(floor_member).get::<SquadId>().copied(),
            Some(SquadId(DEFAULT_SQUAD))
        );
        assert!(matches!(order_of(&app, floor_member), Order::AttackMove(_)));
        assert!(matches!(order_of(&app, mine), Order::AttackMove(_)));

        // --- F9: the human takes the team back ------------------------------
        app.world_mut().resource_mut::<AiControlled>().human = false;
        app.world_mut().entity_mut(floor_member).insert(Order::Idle);
        app.world_mut().entity_mut(mine).insert(Order::Idle);
        app.update();

        assert!(
            matches!(order_of(&app, floor_member), Order::Idle),
            "the autopilot's anti-idle floor kept commanding a team the human took back"
        );
        match order_of(&app, mine) {
            Order::AttackMove(p) => assert!(xz_dist(p, objective) <= SQUAD_ARRIVE),
            other => panic!("a human-set posture stopped executing after handback: {other:?}"),
        }
    }
}
