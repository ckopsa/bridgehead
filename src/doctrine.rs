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

use crate::command::PendingOrder;
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

/// How far past a known emplacement's attack range a squad treats the ground
/// as covered. A squad that stops exactly on the range ring is already being
/// shot at by the time it works out that it is: the margin is the approach.
const DEFENSE_MARGIN: f32 = 6.0;
/// Cohesion demanded before a Forage squad walks onto ground it knows is
/// covered by static defense. Tighter than `COHESION_SPREAD` deliberately —
/// the general rule only has to stop a squad arriving in packets, this one has
/// to stop it arriving in ones.
const DEFENDED_SPREAD: f32 = 7.0;

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
                // Also change-detection driven, and for the same reason: the
                // frame a unit falls idle is the frame its old reason expired.
                idle_instinct,
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
                // `SimSet::Think` runs after the scripted commander has
                // written its postures and BEFORE the intent compiler, which
                // is the same rule command.rs states for its latency
                // dispatcher: standing orders execute first so an explicit
                // order issued in the same frame can still overrule them.
                .in_set(SimSet::Think)
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

/// Centre of mass on the ground plane, and the distance of the widest member
/// from it. `(from, 0.0)` for an empty formation.
fn formation(positions: &[Vec3], fallback: Vec3) -> (Vec3, f32) {
    if positions.is_empty() {
        return (fallback, 0.0);
    }
    let centroid = positions.iter().copied().sum::<Vec3>() / positions.len() as f32;
    let centroid = Vec3::new(centroid.x, 0.0, centroid.z);
    let spread = positions
        .iter()
        .map(|p| xz_dist(*p, centroid))
        .fold(0.0_f32, f32::max);
    (centroid, spread)
}

/// Distance from `p` to the segment `a`-`b`, on the ground plane.
fn point_segment_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = Vec2::new(b.x - a.x, b.z - a.z);
    let ap = Vec2::new(p.x - a.x, p.z - a.z);
    let len_sq = ab.length_squared();
    if len_sq <= f32::EPSILON {
        return ap.length();
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    (ap - ab * t).length()
}

/// Static defense `team` has reason to believe is standing there, as
/// `(position, covered radius)` discs.
///
/// It takes BOTH halves of "known" — what the team can see right now and what
/// it remembers — and the second half is the point. `FogGrid::ghosts()` drops
/// a record the instant the team can see the spot again ("sight wins", per
/// docs/FOG.md), so a memory-only list would evaporate exactly as the squad
/// got close enough for it to matter: the tower would guard the approach and
/// then stop guarding the doorstep, which is a worse behaviour than having no
/// opinion at all. A tower nobody has ever looked at is in neither half, and
/// that is the fog rule holding — doctrine reads the grid, it does not peek
/// around it.
fn known_defenses(
    fog: &FogGrid,
    team: Team,
    live: &Query<(&Team, &Transform, &Building), Without<UnderConstruction>>,
) -> Vec<(Vec3, f32)> {
    let covered = |kind: BuildingKind| {
        building_stats(kind)
            .attack
            .map(|a| a.range + DEFENSE_MARGIN)
    };
    let mut out: Vec<(Vec3, f32)> = Vec::new();
    // Remembered: a scout saw it, nobody is watching it now. `done` matters —
    // a foundation the scout caught mid-build was not shooting at anything.
    for ghost in fog.ghosts() {
        if ghost.team == team || !ghost.done {
            continue;
        }
        if let Some(r) = covered(ghost.kind) {
            out.push((ghost.pos, r));
        }
    }
    // Seen: in sight this instant, so it is not in `ghosts()` and would
    // otherwise be missing from the list at the worst possible moment.
    for (btm, tf, building) in live {
        if *btm == team || !fog.sees(tf.translation) {
            continue;
        }
        if let Some(r) = covered(building.kind) {
            out.push((tf.translation, r));
        }
    }
    out
}

/// Does the straight run from `from` to `to` pass through covered ground?
///
/// Deliberately the whole *approach* rather than just the destination: R10's
/// tower was not standing on the treasure, it was standing on the way to it,
/// and a rule that only asked about the cache would have marched the squad
/// past the guns to reach an "uncovered" prize.
fn approach_is_covered(from: Vec3, to: Vec3, defenses: &[(Vec3, f32)]) -> bool {
    defenses
        .iter()
        .any(|(centre, radius)| point_segment_dist(*centre, from, to) <= *radius)
}

/// Slide `p` out of any covered disc it landed in, straight away from the
/// emplacement. A staging point is where a squad *waits*, and waiting under
/// the guns is precisely the death this rule exists to prevent.
fn clear_of_defenses(p: Vec3, defenses: &[(Vec3, f32)]) -> Vec3 {
    let mut out = Vec3::new(p.x, 0.0, p.z);
    for (centre, radius) in defenses {
        let d = xz_dist(out, *centre);
        if d < *radius {
            let away = Vec3::new(out.x - centre.x, 0.0, out.z - centre.z);
            let dir = if away.length_squared() > f32::EPSILON {
                away.normalize()
            } else {
                Vec3::X
            };
            out = Vec3::new(centre.x, 0.0, centre.z) + dir * *radius;
        }
    }
    out
}

/// What a Forage squad does this tick, once known static defense has had its
/// say. Two shapes, and the difference between them is the bead:
#[derive(Clone, Debug, PartialEq)]
enum ForagePlan {
    /// Free hunting. Every member walks to the cache nearest to ITSELF, so a
    /// spread-out squad splits across the map instead of queueing behind one
    /// pile. The original, and still the common case.
    Scatter(Vec<Vec3>),
    /// One point for the whole squad — a regroup, a staging point short of the
    /// guns, or a single cache being taken as a body.
    Together(Vec3),
}

/// Pick the Forage objective, respecting emplacements the team knows about.
///
/// **R10 (Red):** six Footmen went into a tower on a forage path one at a
/// time and died one at a time, because `Scatter` gives every member its own
/// nearest cache and nothing in the posture had any opinion about a building
/// the squad could not currently see. Two rules fix it, in order:
///
/// 1. **Divert.** Treasure whose approach nothing known is shooting at is
///    strictly better treasure. If any cache qualifies, hunt only those and
///    the tower never enters the story.
/// 2. **Gather, then go in as one.** When every cache left is behind the guns,
///    the squad stops scattering: it takes the nearest one as a single body,
///    and it must be gathered to `DEFENDED_SPREAD` first — staging on the near
///    side of the covered ground, never inside it.
///
/// What it deliberately does not do is refuse to go. A forager that will not
/// walk past a tower it once saw is a forager that never leaves home, and the
/// posture's whole job is to be out on the map.
fn plan_forage(
    positions: &[Vec3],
    bounties: &[Vec3],
    defenses: &[(Vec3, f32)],
    muster: Vec3,
) -> ForagePlan {
    let (centroid, spread) = formation(positions, muster);

    // 1. Divert: prefer caches with a clean run to them.
    let open: Vec<Vec3> = bounties
        .iter()
        .copied()
        .filter(|p| !approach_is_covered(centroid, *p, defenses))
        .collect();
    if !open.is_empty() {
        let objective = nearest_point(&open, centroid).unwrap_or(muster);
        return match cohesion_point(positions, objective) {
            Some(p) => ForagePlan::Together(p),
            None => ForagePlan::Scatter(open),
        };
    }

    // 2. Everything left is covered. One objective for everybody, and nobody
    //    crosses the line until the squad is actually a squad.
    let objective = nearest_point(bounties, centroid).unwrap_or(muster);
    if spread > DEFENDED_SPREAD {
        let dir =
            Vec3::new(objective.x - centroid.x, 0.0, objective.z - centroid.z).normalize_or_zero();
        let stage = centroid + dir * COHESION_STEP;
        return ForagePlan::Together(clear_of_defenses(stage, defenses));
    }
    ForagePlan::Together(objective)
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
///
/// **No `PendingOrder` guard, deliberately** (docs/TEMPO.md follow-up 5): a
/// unit bleeding out is not "busy waiting", and a retreat threshold is the
/// commander's own standing order — the fast path this whole mechanism exists
/// to reward. It also never cancels what is in transit, so the order still
/// lands; `rearm_retreat` un-latches on arrival, and if the unit is still under
/// its threshold the policy simply fires again on the next 250ms tick. The net
/// effect is C4 in miniature: a stale order bought at range loses the argument
/// with a policy set in advance, and loses it within a quarter of a second.
fn trigger_retreat(
    mut commands: Commands,
    time: Res<Time>,
    query: Query<(Entity, &RetreatPolicy, &Health), (With<Unit>, Without<Retreating>)>,
) {
    let now = time.elapsed_secs();
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
        commands.entity(entity).try_insert((
            Order::Move(rally),
            Retreating { rally },
            // "Why am I running?" — because the threshold the commander set
            // fired, not because anyone said so just now.
            Provenance::new(Cause::Policy { policy: "retreat" }, now),
        ));
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

/// Expire a unit's reason the moment its behaviour does.
///
/// `Order::Idle` is written from eight scattered places — a finished attack in
/// combat.rs, an exhausted mine or a missing drop-off in economy.rs, a dead
/// followee in units.rs — and every one of them means the same thing: whatever
/// the unit was doing is over and nothing has replaced it. Stamping that at
/// each site would be eight edits to keep in sync forever; catching the
/// transition once is one system with a `Changed` filter, and it cannot fall
/// out of date when a ninth site appears.
///
/// Nothing player-facing writes `Order::Idle` — the compiler's `stop` re-issues
/// a Move to the unit's own spot — so this only ever overwrites a reason that
/// has genuinely lapsed, never a live directive.
///
/// **No `PendingOrder` guard, deliberately** (docs/TEMPO.md follow-up 5): a
/// unit whose last order finished while a new one is still travelling has
/// genuinely stopped doing anything, and "idle" is the true answer for those
/// seconds. Suppressing it would make the unit claim it was still obeying an
/// order it had already completed, to hide a latency window — the exact
/// dishonesty the `at`-on-arrival convention was chosen to avoid. The pending
/// order carries its own `Provenance` and stamps it when it lands.
fn idle_instinct(
    mut commands: Commands,
    time: Res<Time>,
    query: Query<(Entity, &Order, Option<&Provenance>), Changed<Order>>,
) {
    let now = time.elapsed_secs();
    for (entity, order, why) in &query {
        if !matches!(order, Order::Idle) {
            continue;
        }
        // Already answering "idle": leave the original timestamp alone rather
        // than restamping it every time something else touches the Order.
        if matches!(
            why,
            Some(Provenance {
                cause: Cause::Instinct { what: "idle" },
                ..
            })
        ) {
            continue;
        }
        commands
            .entity(entity)
            .try_insert(Provenance::instinct("idle", now));
    }
}

// ---------------------------------------------------------------------------
// 2. Leash (~2 Hz)
// ---------------------------------------------------------------------------

/// Recall anyone who wandered (or was baited) outside their anchor radius.
/// Retreat outranks the leash — a fleeing unit is allowed to leave.
#[allow(clippy::type_complexity)]
fn enforce_leash(
    mut commands: Commands,
    time: Res<Time>,
    // `Without<PendingOrder>`: a unit waiting on a delayed direct order is
    // BUSY, not adrift. Without this guard a leash would haul it home and the
    // order the player actually gave would land on a unit that had already
    // been dragged somewhere else — the "my orders sometimes just vanish" bug
    // docs/TEMPO.md §4 warns about. Same idiom as `Without<Retreating>`
    // alongside it: another claim on this unit outranks the leash.
    query: Query<
        (Entity, &LeashPolicy, &Transform, &Order),
        (With<Unit>, Without<Retreating>, Without<PendingOrder>),
    >,
) {
    let now = time.elapsed_secs();
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
            commands
                .entity(entity)
                .try_insert((wanted, Provenance::new(Cause::Policy { policy: "leash" }, now)));
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
///
/// **No `PendingCast` guard, deliberately** (docs/TEMPO.md follow-up 5). The
/// tempting guard — "don't auto-cast a caster whose hand-fired cast is still
/// travelling" — is backwards: it would let a player *suppress* the fast path
/// by reaching for the slow one, which is C4 upside down. Left alone, the
/// standing policy fires now and the hand-fired copy arrives to find the
/// ability on cooldown and fizzles, which is exactly the honest-fizzle rule
/// `PendingCast` was built around. Doctrine wins the race; that is the point.
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
            // Everything this ability could affect, ANYWHERE — the distance
            // test comes after, because for a targeted ability the distance
            // that matters is not measured from the caster.
            let reachable: Vec<Vec3> = others
                .iter()
                .filter(|(other_team, _, health, other_unit)| {
                    if health.current <= 0.0 {
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
                .map(|(_, other_tf, _, _)| other_tf.translation)
                .collect();

            // **How many bodies this cast would actually catch** — and for a
            // targeted ability that is a question about the best AIM, not
            // about the caster's surroundings. `best_cast_focus` is the same
            // function combat.rs uses to aim the resulting `CastAbility`, so
            // the trigger count and the cast agree by construction: doctrine
            // cannot promise four victims and then deliver a spell centred
            // somewhere that catches one.
            //
            // This is what lets a Sorcerer on auto-cast stand BEHIND its line.
            // Under the old caster-centred count it had to be within `radius`
            // of the enemy for the rule to fire at all, so "auto-cast Slow"
            // and "walk into the front rank" were the same instruction.
            let count = match def.target.range() {
                Some(range) => {
                    match best_cast_focus(tf.translation, range, def.radius, &reachable) {
                        Some((_, _, caught)) => caught,
                        // Nothing reachable: no aim, no cast.
                        None => continue,
                    }
                }
                None => reachable
                    .iter()
                    .filter(|pos| xz_dist(tf.translation, **pos) <= def.radius)
                    .count() as u32,
            };
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
///
/// **No `PendingOrder` guard, deliberately** (docs/TEMPO.md follow-up 5):
/// enrolment writes a `SquadId`, never an `Order`, so it cannot clobber
/// anything in transit — it only decides which squad may re-task the unit
/// *later*, and `run_squad_postures` skips it for as long as the order is
/// travelling anyway. The same reasoning the existing `Provenance` carve-out
/// already makes: enrolment changes who owns the unit, not what it is doing.
fn default_squad_autonomy(
    mut commands: Commands,
    time: Res<Time>,
    mut squad_orders: ResMut<SquadOrders>,
    ai: Res<AiControlled>,
    external: Res<ExternallyCommanded>,
    strays: Query<(Entity, &Unit, &Team, &Health, Option<&Provenance>), Without<SquadId>>,
) {
    let now = time.elapsed_secs();
    // Autonomy floors exist to compensate for slow machine commanders. A
    // human with a mouse keeps full authority: their idle units stay exactly
    // where they were put.
    for (entity, unit, team, health, why) in &strays {
        if unit.kind == UnitKind::Worker
            || health.current <= 0.0
            || !machine_driven(&ai, &external, *team)
        {
            continue;
        }
        let mut enrolled = commands.entity(entity);
        enrolled.try_insert(SquadId(DEFAULT_SQUAD));
        // Nobody asked for this — the engine pooled an uncommanded unit so the
        // army is not a field of statues, and it says so rather than passing
        // the floor off as a decision.
        //
        // Only when the unit had no better answer, though: enrolment changes
        // which squad may re-task it LATER, not what it is doing now, so a
        // unit still walking to its barracks rally keeps the truer reason.
        if !matches!(
            why,
            Some(Provenance { cause: Cause::Instinct { what }, .. }) if *what != "idle"
        ) && !matches!(why, Some(Provenance { cause: Cause::Stamp { .. }, .. }))
        {
            enrolled.try_insert(Provenance::instinct("auto-enroll", now));
        }
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
    time: Res<Time>,
    mut squad_orders: ResMut<SquadOrders>,
    // `Option<&PendingOrder>` rather than `Without<PendingOrder>`, and the
    // distinction is the whole of docs/TEMPO.md follow-up 5's second half.
    //
    // The guard itself is not optional: `re_taskable` reads a unit awaiting a
    // delayed direct order as idle (it IS idle — its order has not arrived
    // yet), so the squad executor would fold it back into the posture and the
    // direct order would be clobbered before it ever landed. docs/TEMPO.md §4
    // calls this the single most likely source of a "my orders vanish" report.
    // The guard also states the design: doctrine owns a unit until a player
    // reaches past it, and reaching past it takes time.
    //
    // But a query *filter* would have applied that guard twice over, and the
    // second application was wrong. This query is read for two different
    // questions: "who may I re-task?" and "where is this squad standing?"
    // (`squad_positions`, below). An in-transit member is still a body in the
    // formation — it has not moved, it is still going to get shot, and
    // cohesion measures physical spread. Filtering it out made a squad's
    // centre of mass jump the moment a player spoke to half of it, so the
    // free half would regroup on a point that ignored the squadmates standing
    // right next to them. So: in-transit members COUNT for cohesion and are
    // SKIPPED for re-tasking, which is one `continue` in the member loop.
    //
    // Retreaters stay filtered out entirely, and that asymmetry is deliberate:
    // a retreater is deliberately *leaving* the formation under a policy the
    // commander set, so the squad must not gather around a unit running for
    // home. An in-transit unit is going nowhere yet.
    members: Query<
        (
            Entity,
            &SquadId,
            &Team,
            &Transform,
            &Order,
            Option<&MoveTo>,
            Option<&PendingOrder>,
        ),
        (With<Unit>, Without<Retreating>),
    >,
    // Read-only, so it may freely overlap `members` (defenders can themselves
    // be somebody else's threat).
    hostiles: Query<(&Team, &Transform, &Health), With<Unit>>,
    // Emplacements standing right now. Read-only, so it may overlap freely;
    // `Without<UnderConstruction>` is the same filter combat.rs's
    // `tower_acquire` uses, because a foundation shoots at nothing. What the
    // team is allowed to KNOW about these is decided by the fog grid in
    // `known_defenses`, not by this query.
    emplacements: Query<(&Team, &Transform, &Building), Without<UnderConstruction>>,
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
    let now = time.elapsed_secs();
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

        // The word the commander actually set, captured before the
        // Forage->Defend rewrite below. A blind forager holding its muster
        // point is still a forage squad: `squads[].posture` in the snapshot
        // says so, and a unit that answered "defend" here would contradict the
        // very readout the commander is looking at.
        let commanded = posture.word();

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
        // Every body in the formation, including the ones with an order still
        // travelling — see the `members` query. They have not moved yet.
        let squad_positions: Vec<Vec3> = members
            .iter()
            .filter(|(_, ms, mt, ..)| **mt == team && ms.0 == squad)
            .map(|(_, _, _, tf, ..)| tf.translation)
            .collect();
        let regroup = match posture {
            SquadPosture::Push { pos } => cohesion_point(&squad_positions, pos),
            _ => None,
        };

        // Forage gets its own planner, because cohesion is only half of what a
        // treasure hunt owes the squad: the other half is *which* treasure, and
        // that question now has an answer about static defense in it. See
        // `plan_forage` for the R10 story.
        let forage_plan = match posture {
            SquadPosture::Forage { muster } if !bounty_points.is_empty() => {
                let defenses = known_defenses(team_fog, team, &emplacements);
                Some(plan_forage(
                    &squad_positions,
                    &bounty_points,
                    &defenses,
                    muster,
                ))
            }
            _ => None,
        };

        // Every order this posture mints answers "why" the same way, so build
        // the stamp once per squad rather than once per member. Reactive
        // defense below is included deliberately: a defend squad diving a
        // trespasser is still doing it *because* it is a defend squad.
        let why = Provenance::new(
            Cause::Posture {
                squad,
                posture: commanded,
            },
            now,
        );

        for (entity, member_squad, member_team, tf, order, move_to, pending) in &members {
            if *member_team != team || member_squad.0 != squad {
                continue;
            }

            // The guard, applied where it belongs: this member counted toward
            // the cohesion point above, and is untouchable until the order a
            // player spoke to it actually arrives. Placed ahead of the
            // reactive-defense branch deliberately — answering a trespasser is
            // still a re-task, and it would clobber the order just the same.
            if pending.is_some() {
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
                    commands.entity(entity).try_insert((wanted, why));
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
                // Treasure hunt. `plan_forage` has already decided whether this
                // is a free scatter (each member walks to the cache nearest to
                // ITSELF, so a spread-out squad splits across the map instead
                // of queueing behind one pile) or one point for everybody.
                // AttackMove, not Move, because a forager that meets an enemy
                // on the way should fight it — the contested middle is exactly
                // where bounties spawn.
                //
                // The see-nothing case became Defend above, so `muster` here is
                // only a defensive fallback.
                SquadPosture::Forage { muster } => {
                    let target = match &forage_plan {
                        Some(ForagePlan::Together(point)) => *point,
                        Some(ForagePlan::Scatter(targets)) => {
                            nearest_point(targets, tf.translation).unwrap_or(muster)
                        }
                        None => muster,
                    };
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
            commands.entity(entity).try_insert((wanted, why));
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
///
/// **No `PendingOrder` guard, deliberately** (docs/TEMPO.md follow-up 5): this
/// only removes a marker, and removing it hands the unit back to the posture
/// executor — which then declines to touch it while an order is in transit. The
/// two guards compose, so a healed unit awaiting a delayed order stops being a
/// retreater and still is not re-tasked.
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
    use crate::command::{CommandLatency, CommandNodes};

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

    /// The unit's own answer to "why are you doing that?", exactly as the
    /// snapshot and the selection panel print it.
    fn why_of(app: &App, entity: Entity) -> String {
        app.world()
            .entity(entity)
            .get::<Provenance>()
            .map_or_else(|| NO_PROVENANCE.to_string(), Provenance::why)
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

    // -----------------------------------------------------------------
    // Provenance: every unit answers "why are you doing that?"
    // -----------------------------------------------------------------

    /// The posture rung of the chain. A unit moved by its squad's standing
    /// order names that squad and that posture — not the commander, who may
    /// have set it minutes ago and is not what is moving it now.
    #[test]
    fn a_squad_member_names_the_posture_that_is_moving_it() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);

        let unit = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        app.world_mut().entity_mut(unit).insert(SquadId(3));
        app.world_mut().resource_mut::<SquadOrders>().0.insert(
            (Team::Human, 3),
            SquadPosture::Push { pos: Vec3::new(30.0, 0.0, 30.0) },
        );

        app.update();

        assert!(matches!(order_of(&app, unit), Order::AttackMove(_)));
        assert_eq!(why_of(&app, unit), "posture:push sq3");
    }

    /// A forage squad with nothing it can see to hunt is executed through the
    /// Defend path — but it is still a FORAGE squad, and that is what
    /// `squads[].posture` reports to the commander. The unit must not answer
    /// with the implementation detail and contradict the readout above it.
    #[test]
    fn a_blind_forager_names_the_posture_the_commander_set() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);

        let muster = Vec3::new(0.0, 0.0, 0.0);
        let unit = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        app.world_mut().entity_mut(unit).insert(SquadId(2));
        // No `Bounty` entities exist, so there is nothing to hunt.
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 2), SquadPosture::Forage { muster });

        app.update();

        assert!(matches!(order_of(&app, unit), Order::AttackMove(_)));
        assert_eq!(why_of(&app, unit), "posture:forage sq2");
    }

    // -----------------------------------------------------------------
    // Forage vs known static defense (R10: six Footmen, one tower, one
    // at a time)
    // -----------------------------------------------------------------

    /// Where the guns are and how wide their covered disc is, in the geometry
    /// both tests below share. The tower sits between the squad's muster and
    /// the far cache; the near cache is off to one side with a clean run.
    const T_TOWER: Vec3 = Vec3::new(40.0, 0.0, 0.0);
    const T_COVERED: Vec3 = Vec3::new(70.0, 0.0, 0.0);
    const T_OPEN: Vec3 = Vec3::new(0.0, 0.0, 70.0);

    fn tower_radius() -> f32 {
        building_stats(BuildingKind::Tower)
            .attack
            .expect("a Tower shoots")
            .range
            + DEFENSE_MARGIN
    }

    /// A dark grid with just the cache cells lit. Everything else stays unseen,
    /// which is what keeps the planted tower a *ghost*: `FogGrid::ghosts()`
    /// drops any record whose cell is currently visible.
    fn forage_fog(app: &mut App, caches: &[Vec3]) {
        let mut fog = FogGrids::test_dark();
        for c in caches {
            let (cx, cz) = NavGrid::world_to_cell(*c).expect("cache is on the map");
            fog.test_set_cell(Team::Human, cx, cz, CellVis::Visible);
        }
        app.insert_resource(fog);
    }

    fn remember_tower(app: &mut App) {
        app.world_mut()
            .resource_mut::<FogGrids>()
            .test_remember(
                Team::Human,
                RememberedBuilding {
                    id: 7,
                    team: Team::Claude,
                    kind: BuildingKind::Tower,
                    pos: T_TOWER,
                    hp: 550.0,
                    max_hp: 550.0,
                    done: true,
                    last_seen: 0.0,
                },
            );
    }

    fn spawn_cache(app: &mut App, at: Vec3) {
        app.world_mut()
            .spawn((Bounty { gold: 270, expires_at: 999.0 }, Transform::from_translation(at)));
    }

    /// Three bodies strung out by 10 units: cohesive by the general rule
    /// (`COHESION_SPREAD` is 14) and NOT cohesive by the stricter rule that
    /// applies under known guns (`DEFENDED_SPREAD` is 7). Any test using this
    /// formation is therefore testing the new gate specifically.
    fn spawn_strung_out_squad(app: &mut App) -> Vec<Entity> {
        let squad: Vec<Entity> = [
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(-1.0, 0.0, 0.0),
        ]
        .into_iter()
        .map(|p| spawn_footman(app, Team::Human, p))
        .collect();
        for e in &squad {
            app.world_mut().entity_mut(*e).insert(SquadId(2));
        }
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 2), SquadPosture::Forage { muster: Vec3::ZERO });
        squad
    }

    fn move_target(app: &App, e: Entity) -> Vec3 {
        match order_of(app, e) {
            Order::AttackMove(p) | Order::Move(p) => p,
            other => panic!("expected a move order, got {other:?}"),
        }
    }

    /// Rule 1, the cheap one: treasure with a clean run to it is strictly
    /// better treasure. With a tower remembered on the path to the far cache,
    /// a Forage squad hunts the other one and the tower never enters the story.
    #[test]
    fn a_forage_squad_diverts_to_the_cache_no_remembered_tower_is_covering() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);
        forage_fog(&mut app, &[T_COVERED, T_OPEN]);
        remember_tower(&mut app);
        spawn_cache(&mut app, T_COVERED);
        spawn_cache(&mut app, T_OPEN);
        let squad = spawn_strung_out_squad(&mut app);

        app.update();

        for e in &squad {
            let target = move_target(&app, *e);
            assert!(
                xz_dist(target, T_OPEN) < 0.1,
                "a forager walked toward {target:?} with an uncovered cache at \
                 {T_OPEN:?} available"
            );
        }
    }

    /// Rule 2, the one R10 needed: when the only treasure left is behind the
    /// guns, the squad stops trickling. Every member is sent to ONE point, that
    /// point is short of the covered ground, and it stays there until the squad
    /// has actually gathered.
    ///
    /// The control half is the whole test: with nothing remembered, the same
    /// three bodies walk straight at the cache — which is exactly the
    /// single-file entry that killed six Footmen.
    #[test]
    fn a_forage_squad_gathers_short_of_a_remembered_tower_instead_of_trickling_in() {
        // Control: no memory of the tower, so no opinion about it.
        let mut app = world();
        app.add_systems(Update, run_squad_postures);
        forage_fog(&mut app, &[T_COVERED]);
        spawn_cache(&mut app, T_COVERED);
        let squad = spawn_strung_out_squad(&mut app);
        app.update();
        for e in &squad {
            assert!(
                xz_dist(move_target(&app, *e), T_COVERED) < 0.1,
                "control: without the memory a forager should march at the cache"
            );
        }

        // Same map, same squad, one scouted tower on the path.
        let mut app = world();
        app.add_systems(Update, run_squad_postures);
        forage_fog(&mut app, &[T_COVERED]);
        remember_tower(&mut app);
        spawn_cache(&mut app, T_COVERED);
        let squad = spawn_strung_out_squad(&mut app);
        app.update();

        let targets: Vec<Vec3> = squad.iter().map(|e| move_target(&app, *e)).collect();
        // ONE point for everybody — the end of single file.
        for t in &targets {
            assert!(
                xz_dist(*t, targets[0]) < 0.1,
                "the squad was sent to {targets:?}: still entering piecemeal"
            );
        }
        let stage = targets[0];
        assert!(
            xz_dist(stage, T_COVERED) > 0.1,
            "the squad walked onto the cache without gathering first"
        );
        assert!(
            xz_dist(stage, T_TOWER) >= tower_radius() - 0.01,
            "the squad staged at {stage:?}, inside the tower's covered ground"
        );
    }

    /// The rule must not turn Forage into a posture that never leaves home:
    /// once the squad IS gathered, it goes in — as a body, to the cache.
    #[test]
    fn a_gathered_forage_squad_still_takes_the_defended_cache() {
        let defenses = vec![(T_TOWER, tower_radius())];
        let tight = [
            Vec3::new(0.0, 0.0, -2.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(-1.0, 0.0, 0.0),
        ];
        assert_eq!(
            plan_forage(&tight, &[T_COVERED], &defenses, Vec3::ZERO),
            ForagePlan::Together(T_COVERED),
            "a gathered squad must commit to the cache, not sit outside forever"
        );
    }

    /// The policy rung. A unit running for home is not obeying an order anyone
    /// gave just now — it is obeying a threshold — and saying so is the whole
    /// difference between "my commander pulled me back" and "I broke".
    #[test]
    fn a_retreating_unit_blames_the_threshold() {
        let mut app = world();
        app.add_systems(Update, trigger_retreat);

        let unit = spawn_footman(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));
        app.world_mut().entity_mut(unit).insert(RetreatPolicy {
            below_frac: 0.5,
            rally: Vec3::new(-70.0, 0.0, -70.0),
        });
        app.world_mut().entity_mut(unit).get_mut::<Health>().unwrap().current = 20.0;

        app.update();

        assert!(matches!(order_of(&app, unit), Order::Move(_)));
        assert_eq!(why_of(&app, unit), "policy:retreat t=0");
    }

    /// The engine-default rung. `Order::Idle` is written from eight scattered
    /// engine systems and always means the same thing: the old reason expired.
    /// One `Changed<Order>` system catches all of them.
    #[test]
    fn falling_idle_expires_whatever_reason_came_before() {
        let mut app = world();
        app.add_systems(Update, idle_instinct);

        let done = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        let busy = spawn_footman(&mut app, Team::Human, Vec3::new(5.0, 0.0, 0.0));
        let stale = Provenance::new(
            Cause::Order { verb: "move", source: IntentSource::Bridge },
            12.0,
        );
        app.world_mut().entity_mut(done).insert(stale);
        app.world_mut()
            .entity_mut(busy)
            .insert((Order::Move(Vec3::new(40.0, 0.0, 40.0)), stale));

        app.update();

        assert_eq!(why_of(&app, done), "idle", "an idle unit kept a dead reason");
        assert_eq!(
            why_of(&app, busy),
            "order:move by bridge t=12",
            "a unit still carrying out its order lost its reason"
        );
    }

    /// Auto-enrolment is a floor the engine applies, not a decision anyone
    /// made. It says so rather than passing itself off as a command.
    #[test]
    fn auto_enrolment_admits_nobody_asked_for_it() {
        let mut app = world();
        app.insert_resource(AiControlled { human: false, claude: true });
        app.add_systems(Update, default_squad_autonomy);

        let unit = spawn_footman(&mut app, Team::Claude, Vec3::new(60.0, 0.0, 60.0));

        app.update();

        assert_eq!(
            app.world().entity(unit).get::<SquadId>().map(|s| s.0),
            Some(DEFAULT_SQUAD)
        );
        assert_eq!(why_of(&app, unit), "instinct:auto-enroll");
    }

    /// A unit nobody has said anything to answers plainly rather than
    /// inventing a reason, and the string is the one the snapshot uses too.
    #[test]
    fn a_unit_with_no_reason_says_idle() {
        let app = world();
        let mut app = app;
        let unit = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        assert_eq!(why_of(&app, unit), NO_PROVENANCE);
        assert_eq!(NO_PROVENANCE, "idle");
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

    // -----------------------------------------------------------------------
    // Chain of Command (docs/TEMPO.md §3) — the doctrine side of the contract
    // -----------------------------------------------------------------------

    /// docs/TEMPO.md §4's named integration hazard, as a test. A unit waiting
    /// on a delayed direct order sits on `Order::Idle` with no `MoveTo`, which
    /// is *exactly* what `re_taskable` calls available — so without the
    /// `Without<PendingOrder>` guard the squad executor folds it back into the
    /// posture and the player's order is clobbered before it ever arrives.
    /// This is the "my orders sometimes just vanish" bug, pinned.
    #[test]
    fn a_squad_does_not_clobber_an_order_still_in_transit() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);

        let objective = Vec3::new(30.0, 0.0, 30.0);
        let waiting = spawn_footman(&mut app, Team::Human, Vec3::new(-60.0, 0.0, -60.0));
        let free = spawn_footman(&mut app, Team::Human, Vec3::new(-58.0, 0.0, -60.0));
        for unit in [waiting, free] {
            app.world_mut().entity_mut(unit).insert(SquadId(1));
        }
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: objective });
        // A direct order the player gave, still travelling.
        app.world_mut().entity_mut(waiting).insert(PendingOrder {
            order: Order::Move(Vec3::new(-70.0, 0.0, 0.0)),
            provenance: Provenance::instinct("idle", 97.0),
            ready_at: 99.0,
            issued_at: 97.0,
        });

        app.update();

        assert!(
            matches!(order_of(&app, waiting), Order::Idle),
            "the squad re-tasked a unit whose direct order had not arrived yet — \
             the order would have been silently lost"
        );
        assert!(
            app.world().entity(waiting).get::<PendingOrder>().is_some(),
            "the in-transit order must survive a posture tick"
        );
        // The guard is narrow: a squadmate with nothing in transit is still
        // commanded exactly as before.
        assert!(
            matches!(order_of(&app, free), Order::AttackMove(_)),
            "the guard leaked and stopped the posture from executing at all"
        );
    }

    /// Same hazard, the leash half. A leashed unit awaiting a direct order out
    /// of its anchor radius must not be hauled home first — the recall would
    /// win the race against the order that supersedes it.
    #[test]
    fn a_leash_does_not_recall_a_unit_with_an_order_in_transit() {
        let mut app = world();
        app.add_systems(Update, enforce_leash);

        let anchor = Vec3::new(-60.0, 0.0, -60.0);
        let far = Vec3::new(20.0, 0.0, 20.0);
        let waiting = spawn_footman(&mut app, Team::Human, far);
        let stray = spawn_footman(&mut app, Team::Human, far);
        for unit in [waiting, stray] {
            app.world_mut()
                .entity_mut(unit)
                .insert(LeashPolicy { anchor, radius: 10.0 });
        }
        app.world_mut().entity_mut(waiting).insert(PendingOrder {
            order: Order::AttackMove(far),
            provenance: Provenance::instinct("idle", 97.0),
            ready_at: 99.0,
            issued_at: 97.0,
        });

        app.update();

        assert!(
            matches!(order_of(&app, waiting), Order::Idle),
            "the leash recalled a unit whose direct order was still travelling"
        );
        assert!(
            matches!(order_of(&app, stray), Order::Move(_)),
            "the guard leaked and the leash stopped working"
        );
    }

    /// An order a player spoke, still travelling. The provenance is the "idle"
    /// one a unit genuinely answers with during a latency window — see
    /// `a_unit_waiting_on_a_delayed_order_still_answers_idle`.
    fn in_transit(order: Order) -> PendingOrder {
        PendingOrder {
            order,
            provenance: Provenance::instinct("idle", 97.0),
            ready_at: 99.0,
            issued_at: 97.0,
        }
    }

    /// **In-transit members still count as bodies in the formation.**
    ///
    /// The `Without<PendingOrder>` guard shipped as a query filter, which
    /// applied it to two different questions at once: "who may I re-task?"
    /// (right) and "where is this squad standing?" (wrong). A unit awaiting a
    /// delayed order has not moved an inch — it is standing in the blob, in
    /// range of whatever the blob is in range of — so dropping it from the
    /// centroid made the squad's centre of mass lurch the moment a player spoke
    /// to part of it, and the rest would then regroup on a point that ignored
    /// the squadmates standing right beside them.
    ///
    /// Here the straggler is the one with an order in transit. Counted, the
    /// squad is strung out and gathers; uncounted, the two survivors look
    /// perfectly cohesive and march on the objective without it.
    #[test]
    fn an_in_transit_member_still_counts_toward_squad_cohesion() {
        let mut app = world();
        app.add_systems(Update, run_squad_postures);

        let objective = Vec3::new(60.0, 0.0, 60.0);
        let a = Vec3::new(-60.0, 0.0, -60.0);
        let b = Vec3::new(-58.0, 0.0, -60.0);
        let c = Vec3::new(-20.0, 0.0, -20.0);

        // The discriminator, stated before the fact: these two alone are
        // cohesive, so if the third body is dropped there is nothing to gather
        // and this test cannot tell the two implementations apart.
        assert!(
            cohesion_point(&[a, b], objective).is_none(),
            "the two free members must be cohesive on their own, or this test \
             proves nothing"
        );

        let free_a = spawn_footman(&mut app, Team::Human, a);
        let free_b = spawn_footman(&mut app, Team::Human, b);
        let waiting = spawn_footman(&mut app, Team::Human, c);
        for unit in [free_a, free_b, waiting] {
            app.world_mut().entity_mut(unit).insert(SquadId(1));
        }
        app.world_mut()
            .entity_mut(waiting)
            .insert(in_transit(Order::Move(Vec3::new(-70.0, 0.0, 0.0))));
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: objective });

        app.update();

        let expected = cohesion_point(&[a, b, c], objective)
            .expect("all three bodies together are strung out");
        for unit in [free_a, free_b] {
            match order_of(&app, unit) {
                Order::AttackMove(p) => {
                    assert!(
                        xz_dist(p, expected) <= 0.01,
                        "a free member advanced to {p:?}; with the in-transit \
                         squadmate counted the squad should gather at {expected:?}"
                    );
                    assert!(
                        xz_dist(p, objective) > SQUAD_ARRIVE,
                        "the squad pressed on to the objective and left its \
                         in-transit member behind"
                    );
                }
                other => panic!("a free member was not commanded at all: {other:?}"),
            }
        }
        // And the guard it was split out of still holds: the waiting member is
        // untouched and its order is still on its way.
        assert!(matches!(order_of(&app, waiting), Order::Idle));
        assert!(app.world().entity(waiting).get::<PendingOrder>().is_some());
    }

    /// A **retreat threshold fires for a unit with an order in transit**, and
    /// does not cancel it.
    ///
    /// The guard question asked of every doctrine consumer in turn
    /// (docs/TEMPO.md follow-up 5), answered "no guard" here: a unit bleeding
    /// out is not busy waiting, and the threshold is a standing order the
    /// commander set in advance — the fast path. Cancelling the traveller
    /// instead would silently eat an order the player really did give.
    #[test]
    fn a_retreat_threshold_fires_for_a_unit_with_an_order_in_transit() {
        let mut app = world();
        app.add_systems(Update, trigger_retreat);

        let rally = Vec3::new(-70.0, 0.0, -70.0);
        let unit = spawn_footman(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        app.world_mut()
            .entity_mut(unit)
            .insert(RetreatPolicy { below_frac: 0.5, rally });
        app.world_mut()
            .entity_mut(unit)
            .insert(in_transit(Order::AttackMove(Vec3::new(60.0, 0.0, 60.0))));
        app.world_mut().entity_mut(unit).get_mut::<Health>().unwrap().current = 20.0;

        app.update();

        match order_of(&app, unit) {
            Order::Move(p) => assert!(xz_dist(p, rally) <= ORDER_EPS),
            other => panic!("a wounded unit did not run because it was waiting: {other:?}"),
        }
        assert_eq!(why_of(&app, unit), "policy:retreat t=0");
        assert!(
            app.world().entity(unit).get::<PendingOrder>().is_some(),
            "the retreat swallowed an order the player had already spoken"
        );
    }

    /// The other end of that decision, and the reason it is safe: when the
    /// stale order finally lands it un-latches the retreat (`rearm_retreat`, at
    /// dispatch time — docs/TEMPO.md §4's "assert this deliberately"), the unit
    /// is still under its threshold, and the policy simply fires again on the
    /// next tick.
    ///
    /// C4 in miniature: an order bought at range loses the argument with a
    /// policy set in advance, and loses it in a quarter of a second.
    #[test]
    fn a_landed_order_unlatches_the_retreat_and_the_policy_wins_anyway() {
        let mut app = world();
        app.add_systems(Update, (rearm_retreat, trigger_retreat).chain());

        let rally = Vec3::new(-70.0, 0.0, -70.0);
        let unit = spawn_footman(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        app.world_mut()
            .entity_mut(unit)
            .insert(RetreatPolicy { below_frac: 0.5, rally });
        app.world_mut().entity_mut(unit).get_mut::<Health>().unwrap().current = 20.0;

        app.update();
        assert!(app.world().entity(unit).get::<Retreating>().is_some());

        // The delayed order arrives — exactly what `command::dispatch_pending`
        // does when `ready_at` comes due: it writes the Order and drops the
        // PendingOrder.
        let front = Vec3::new(60.0, 0.0, 60.0);
        app.world_mut().entity_mut(unit).insert(Order::AttackMove(front));

        app.update();
        app.update();

        match order_of(&app, unit) {
            Order::Move(p) => assert!(
                xz_dist(p, rally) <= ORDER_EPS,
                "the unit is running to {p:?}, not to its rally {rally:?}"
            ),
            // If `rearm_retreat` had failed to un-latch, `trigger_retreat`
            // (Without<Retreating>) could never have re-fired and this would
            // still read AttackMove.
            other => panic!(
                "a still-wounded unit kept obeying an order that reached it \
                 after it broke: {other:?}"
            ),
        }
        assert!(app.world().entity(unit).get::<Retreating>().is_some());
    }

    /// **A unit waiting on a delayed order answers "idle", and says so.**
    ///
    /// docs/TEMPO.md records this as a live capture from the `why`-layer
    /// reconciliation — "while the order was in transit the unit answered
    /// `why: idle`" — and calls it the two layers agreeing rather than merely
    /// coexisting. It was never pinned. It is now, because the tempting
    /// "fix" (suppress `idle_instinct` while something is in transit) would
    /// make the unit claim it was still obeying an order it had finished, to
    /// paper over a latency window.
    #[test]
    fn a_unit_waiting_on_a_delayed_order_still_answers_idle() {
        let mut app = world();
        app.add_systems(Update, idle_instinct);

        let unit = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        app.world_mut().entity_mut(unit).insert(Provenance::new(
            Cause::Order { verb: "attackmove", source: IntentSource::Ui },
            12.0,
        ));
        app.world_mut()
            .entity_mut(unit)
            .insert(in_transit(Order::AttackMove(Vec3::new(60.0, 0.0, 60.0))));

        app.update();

        assert_eq!(
            why_of(&app, unit),
            "idle",
            "a unit whose last order finished must not claim to be obeying one \
             that has not reached it yet"
        );
    }

    /// Auto-enrolment writes a `SquadId`, never an `Order`, so it needs no
    /// guard: it decides who may re-task the unit *later*, and the posture
    /// executor then declines to while the order is travelling. The two
    /// compose, which is the whole reason the floor can stay unguarded.
    #[test]
    fn auto_enrolment_leaves_an_order_in_transit_alone() {
        let mut app = world();
        app.insert_resource(AiControlled { human: false, claude: true });
        app.add_systems(Update, (default_squad_autonomy, run_squad_postures).chain());

        let unit = spawn_footman(&mut app, Team::Claude, Vec3::new(60.0, 0.0, 60.0));
        app.world_mut()
            .entity_mut(unit)
            .insert(in_transit(Order::Move(Vec3::new(0.0, 0.0, 0.0))));

        app.update();

        assert_eq!(
            app.world().entity(unit).get::<SquadId>().map(|s| s.0),
            Some(DEFAULT_SQUAD),
            "an in-transit unit should still be enrolled — enrolment is not an order"
        );
        assert!(
            app.world().entity(unit).get::<PendingOrder>().is_some(),
            "the anti-idle floor ate an order that was still travelling"
        );
        assert!(
            matches!(order_of(&app, unit), Order::Idle),
            "the seeded posture re-tasked a unit whose order had not arrived"
        );
    }

    /// **Standing orders are local.** The doctrine executor is the fast path,
    /// and it stays instant for every seat no matter how far from a command
    /// node the unit is standing and no matter what the latency curve says.
    /// That asymmetry IS the mechanism (docs/TEMPO.md §3), so it is worth a
    /// test that fails loudly if anyone ever routes doctrine through the
    /// issuer "for consistency".
    #[test]
    fn doctrine_dispatches_instantly_however_far_from_home_it_reaches() {
        let mut app = world();
        // Latency at maximum everywhere: on, and this team owns no command
        // nodes at all.
        app.insert_resource(CommandLatency { on: true, ..Default::default() })
            .insert_resource(CommandNodes { nodes: Vec::new(), ready: true })
            .add_systems(Update, run_squad_postures);

        let objective = Vec3::new(30.0, 0.0, 30.0);
        // Standing in the far corner, as disconnected as a unit can be.
        let member = spawn_footman(&mut app, Team::Human, Vec3::new(-95.0, 0.0, -95.0));
        app.world_mut().entity_mut(member).insert(SquadId(1));
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: objective });

        app.update();

        assert!(
            app.world().entity(member).get::<PendingOrder>().is_none(),
            "doctrine's own order went into transit — standing orders must be local"
        );
        assert!(
            matches!(order_of(&app, member), Order::AttackMove(_)),
            "the engine's standing order did not dispatch in the same frame"
        );
    }

    // -----------------------------------------------------------------------
    // v3: auto-cast under targeted geometry
    // -----------------------------------------------------------------------

    /// The auto-caster on its own, with no timer gate, so one `update` is one
    /// decision.
    fn autocast_world() -> App {
        let mut app = world();
        app.init_resource::<TechTiers>()
            .add_event::<CastAbility>()
            .add_systems(Update, auto_cast_abilities);
        app
    }

    fn spawn_sorcerer(app: &mut App, team: Team, at: Vec3) -> Entity {
        let mut policy = AutoCastPolicy::default();
        let (slot, min_targets) = default_autocast(UnitKind::Sorcerer).unwrap();
        policy.set(slot, min_targets);
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Sorcerer },
                team,
                Transform::from_translation(at),
                Health::new(unit_stats(UnitKind::Sorcerer).hp),
                Order::Idle,
                policy,
            ))
            .id()
    }

    /// Every cast the auto-caster asked for this frame.
    fn casts_fired(app: &mut App) -> Vec<CastAbility> {
        app.world_mut()
            .resource_mut::<Events<CastAbility>>()
            .drain()
            .collect()
    }

    /// **The arena finding, answered.** The fog/arena report was that
    /// Sorcerers die because auto-cast Slow required them to be within the
    /// spell's radius of the enemy — i.e. in the front rank. This is that
    /// exact board: the Sorcerer stands BEHIND its own line, the enemy is
    /// beyond the old bubble entirely, and the auto-caster fires anyway.
    #[test]
    fn a_sorcerer_behind_its_own_line_auto_casts_without_walking_in() {
        let mut app = autocast_world();

        // The Sorcerer at the back; its own footmen screening at z = 6; the
        // enemy charge arriving at z = 11-13.
        let sorcerer = spawn_sorcerer(&mut app, Team::Human, Vec3::ZERO);
        for x in [-2.0, 0.0, 2.0] {
            spawn_footman(&mut app, Team::Human, Vec3::new(x, 0.0, 6.0));
        }
        let enemies: Vec<Vec3> = vec![
            Vec3::new(-1.0, 0.0, 11.0),
            Vec3::new(1.0, 0.0, 11.5),
            Vec3::new(0.0, 0.0, 13.0),
        ];
        for at in &enemies {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Raider },
                Team::Claude,
                Transform::from_translation(*at),
                Health::new(unit_stats(UnitKind::Raider).hp),
                Order::Idle,
            ));
        }

        // The nearest enemy is further away than the WHOLE of the old
        // caster-centred bubble, so on master this board produces no cast at
        // all and the Sorcerer stands there doing nothing until it is charged.
        const OLD_RADIUS: f32 = 8.0;
        let nearest = enemies
            .iter()
            .map(|e| xz_dist(Vec3::ZERO, *e))
            .fold(f32::MAX, f32::min);
        assert!(
            nearest > OLD_RADIUS,
            "the scenario must put the enemy out of the old bubble (nearest {nearest:.1})"
        );

        app.update();

        let fired = casts_fired(&mut app);
        assert_eq!(fired.len(), 1, "the Sorcerer should have cast exactly once");
        assert_eq!(fired[0].caster, sorcerer);
        assert!(
            fired[0].target.is_none(),
            "auto-cast names no target: it hands the aim to the engine, which is \
             what makes the standing policy and a bare bridge `cast` identical"
        );
        // ...and it did all of that from behind its own screen: no order, no
        // step forward, still at the back.
        assert!(matches!(order_of(&app, sorcerer), Order::Idle));
    }

    /// The other half of the same rule: reach is not infinite. An enemy past
    /// `range + radius` produces no cast, so the Sorcerer does not burn its
    /// cooldown at a rumour on the far side of the map.
    #[test]
    fn auto_cast_still_needs_something_within_reach() {
        let mut app = autocast_world();
        spawn_sorcerer(&mut app, Team::Human, Vec3::ZERO);
        app.world_mut().spawn((
            Unit { kind: UnitKind::Raider },
            Team::Claude,
            Transform::from_translation(Vec3::new(0.0, 0.0, 30.0)),
            Health::new(unit_stats(UnitKind::Raider).hp),
            Order::Idle,
        ));

        app.update();
        assert!(casts_fired(&mut app).is_empty());
    }

    /// A caster-centred ability's trigger is unchanged: the Champion still
    /// counts the bodies around ITSELF, because that is still where its Slam
    /// goes. Geometry became data without redefining the abilities that never
    /// used it.
    #[test]
    fn a_caster_centred_autocast_still_counts_its_own_bubble() {
        let mut app = autocast_world();
        let mut policy = AutoCastPolicy::default();
        policy.set(0, 2);
        let champion = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Hero::from_record(None),
                Transform::from_translation(Vec3::ZERO),
                Health::new(unit_stats(UnitKind::Hero).hp),
                Order::Idle,
                policy,
            ))
            .id();

        let slam = abilities_of_unit(UnitKind::Hero)[0];
        // Two enemies just OUTSIDE the Slam's radius: no cast.
        for x in [0.0, 1.0] {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_translation(Vec3::new(x, 0.0, slam.radius + 2.0)),
                Health::new(100.0),
                Order::Idle,
            ));
        }
        app.update();
        assert!(
            casts_fired(&mut app).is_empty(),
            "a caster-centred ability must not have gained reach it never had"
        );

        // Two inside: cast.
        for x in [0.0, 1.0] {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_translation(Vec3::new(x, 0.0, 2.0)),
                Health::new(100.0),
                Order::Idle,
            ));
        }
        app.update();
        let fired = casts_fired(&mut app);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].caster, champion);
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    // The R10 shape, restated so this module needs nothing from `mod tests`.
    const T_TOWER: Vec3 = Vec3::new(40.0, 0.0, 0.0);
    const T_COVERED: Vec3 = Vec3::new(70.0, 0.0, 0.0);
    const T_OPEN: Vec3 = Vec3::new(0.0, 0.0, 70.0);

    /// A measurement first and an assertion second. Run with `--nocapture` to
    /// read the numbers; it also fails if either headline result drifts back.
    ///
    /// R10's death was not six Footmen picking six different destinations — with
    /// one cache they all picked the same one. It was six Footmen *arriving*
    /// separately, because a strung-out squad was ordered onto the treasure and
    /// then trickled into the tower's range in whatever order they got there.
    /// So the quantity that matters is: **when the squad is not gathered, is it
    /// ordered under the guns anyway?**
    #[test]
    fn probe_r10_forage_entry() {
        let radius = building_stats(BuildingKind::Tower)
            .attack
            .expect("a Tower shoots")
            .range
            + DEFENSE_MARGIN;
        let defenses = vec![(T_TOWER, radius)];
        // Strictly inside. A point ON the ring is where `clear_of_defenses`
        // deliberately puts a staging squad, and counting the ring as covered
        // would score the fix as the bug.
        let covered = |p: Vec3| xz_dist(p, T_TOWER) < radius - 0.05;

        let mut seed: u64 = 0x5EED_1234;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 40) as f32) / (0xFF_FFFF as f32)
        };

        // (a) One cache, behind the guns. Ungathered squads only.
        let (mut n, mut old_in, mut new_in) = (0usize, 0usize, 0usize);
        // (b) Two caches, one with a clean approach. All squads.
        let (mut m, mut old_bad, mut new_bad) = (0usize, 0usize, 0usize);

        // The squad approaches from outside the guns. A formation already
        // standing inside the covered disc is not a decision anyone gets to
        // make — it is already being shot at — so it is not sampled.
        for step in 0..40 {
            let centre = Vec3::new(step as f32 * 0.45 - 4.0, 0.0, 0.0);
            for _ in 0..12 {
                let spread = 2.0 + rnd() * 22.0;
                let squad: Vec<Vec3> = (0..6)
                    .map(|_| {
                        centre
                            + Vec3::new(
                                (rnd() - 0.5) * 2.0 * spread,
                                0.0,
                                (rnd() - 0.5) * 2.0 * spread,
                            )
                    })
                    .collect();
                let (real_centroid, real_spread) = formation(&squad, centre);
                if xz_dist(real_centroid, T_TOWER) <= radius {
                    continue;
                }

                // What master did: no opinion about defense whatsoever.
                let old = |caches: &[Vec3]| -> Vec3 {
                    let (c, _) = formation(&squad, centre);
                    let obj = nearest_point(caches, c).unwrap();
                    cohesion_point(&squad, obj).unwrap_or(obj)
                };
                let new = |caches: &[Vec3]| -> Vec3 {
                    match plan_forage(&squad, caches, &defenses, centre) {
                        ForagePlan::Together(p) => p,
                        ForagePlan::Scatter(ts) => nearest_point(&ts, centre).unwrap(),
                    }
                };

                // (a) An UNGATHERED squad must not be pointed under the guns.
                if real_spread > DEFENDED_SPREAD {
                    n += 1;
                    if covered(old(&[T_COVERED])) {
                        old_in += 1;
                    }
                    if covered(new(&[T_COVERED])) {
                        new_in += 1;
                    }
                }

                // (b) With a clean cache available, taking the covered one is
                //     simply the wrong call.
                m += 1;
                let pick_bad = |t: Vec3| xz_dist(t, T_OPEN) > xz_dist(t, T_COVERED);
                if pick_bad(old(&[T_COVERED, T_OPEN])) {
                    old_bad += 1;
                }
                if pick_bad(new(&[T_COVERED, T_OPEN])) {
                    new_bad += 1;
                }
            }
        }

        let pct = |a: usize, b: usize| 100.0 * a as f32 / b.max(1) as f32;
        println!("\n=== R10 forage probe — one remembered Tower, covered disc {radius:.0} units ===");
        println!("(a) UNGATHERED squads ordered onto covered ground  (n = {n})");
        println!("      master {old_in:4}/{n}  ({:5.1}%)", pct(old_in, n));
        println!("      now    {new_in:4}/{n}  ({:5.1}%)", pct(new_in, n));
        println!("(b) squads that chose the covered cache with a clean one available  (n = {m})");
        println!("      master {old_bad:4}/{m}  ({:5.1}%)", pct(old_bad, m));
        println!("      now    {new_bad:4}/{m}  ({:5.1}%)", pct(new_bad, m));
        println!();

        // The measurement is the point, but it is worth nothing if it can
        // silently drift back, so the two headline numbers are also the bar.
        assert_eq!(
            new_in, 0,
            "a strung-out squad was ordered onto covered ground {new_in} times"
        );
        assert!(
            pct(new_bad, m) < 25.0,
            "the divert rule only fires {:.1}% of the time",
            100.0 - pct(new_bad, m)
        );
        // …and the bar has to be one master actually fails, or it proves nothing.
        assert!(old_in > 0 && pct(old_bad, m) > 50.0);
    }
}
