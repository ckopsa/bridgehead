//! command.rs — Chain of Command: **orders propagate; standing orders are local.**
//!
//! docs/TEMPO.md §3 decided v2's tempo-equity mechanism. This file is its core.
//! A *direct* order — the kind a player addresses to a named unit — does not
//! take effect the instant it is spoken. It arrives after a latency that grows
//! with the ordered unit's distance from the nearest **command node** of its
//! own team (a finished hall, or a living hero). An order the *engine* executes
//! on a player's behalf — doctrine's squad postures, retreat triggers, leash
//! recalls, economy's harvest follow-through, combat's chase — arrives
//! instantly, because the unit already has its standing orders and does not
//! need to ask.
//!
//! That asymmetry is the whole mechanism. It makes pre-positioned policy
//! strictly better than live intervention *at range*, for every seat, by
//! construction rather than by rule — which is THESIS.md's own answer to the
//! tempo problem ("relocate fast work into the game itself") rather than a
//! referee slowing the human down.
//!
//! ## Where it attaches
//!
//! intent.rs is the single choke point where every player command becomes an
//! `Order` (docs/INTENT.md). So latency is not a 23-site refactor: it is one
//! decision inside `compile_intent`'s order arms, expressed through
//! [`OrderIssuer`]. The compiler still **validates immediately** — a bad target
//! is still an error in the same frame, on the same wire — and only the
//! *application* of a valid direct order is deferred.
//!
//! ## All three seats pay
//!
//! `ui.rs` and `bridge.rs` reach this through intent submission. The scripted
//! `ai.rs` does not go through the compiler (that is a known, documented
//! asymmetry), so it calls [`OrderIssuer`] **directly** at its own unit-order
//! sites. docs/TEMPO.md is explicit that if autopilot is exempt it becomes a
//! cheat and C1 is violated at the third seat, so it is not exempt.
//!
//! ## Verb classification
//!
//! The one table that decides what is a "direct order". Kept here rather than
//! in intent.rs because *this* is the module whose behaviour it describes.
//!
//! | Verb | Latency | Why |
//! |---|---|---|
//! | `move`, `attackmove`, `attack`, `harvest`, `return`, `follow`, `stop` | **pays** | A direct order to a named unit standing somewhere. This is the set docs/TEMPO.md means by "a direct `Order` written by a player interface". |
//! | `build` | exempt | docs/TEMPO.md's open question, answered as it recommends: the worker walks to the site anyway, so the latency is invisible and just taxes the economy. |
//! | `train`, `upgrade`, `cancel`, `research`, `rally` | exempt | Addressed to a *building*, which is standing in your base next to a command node by definition. Production is not micro. |
//! | `cast`, `use_item`, `buy` | exempt | Every caster the game has **is** a command node (a hero) or **sits on** one (`abilities_of_building` is `is_hall`-only), and items live in a hero's inventory. The computed latency would be identically zero, so charging it would be ceremony. `every_caster_is_a_command_node` pins that claim: add a caster that is not a node and the test fails, which is the signal to revisit this row. |
//! | `priority`, `retreat`, `leash`, `autocast`, `squad`, `posture`, `template` | exempt | Doctrine. Standing orders ARE the fast path — that is the mechanism, not an exception to it. |
//! | `autopilot`, `surrender` | exempt | Match level, not a unit order. |
//!
//! ## The curve
//!
//! Step-plus-ramp, as docs/TEMPO.md §4 recommends ("start with a step plus a
//! short ramp, and let the sweep decide"): free inside a node's radius, then a
//! fixed step the moment you leave it plus a linear ramp per world unit beyond,
//! clamped at a maximum. The step is what makes it legible to an LLM commander
//! ("inside my radius or outside it"); the ramp is what keeps it from being a
//! cliff a player can stand exactly on.
//!
//! Every constant is env-tunable so the calibration bead can sweep without a
//! rebuild. Defaults put a midfield engagement (~100 units from a base) at
//! about 2.0s and cap the far corners at 3.0s, which is the 1.5–3s docs/TEMPO.md
//! §C1 asks for at the point of contact.
//!
//! ## Default OFF
//!
//! `WC3_COMMAND_LATENCY` defaults off, and with it off this module is inert:
//! the node cache is never built, `OrderIssuer::issue` is literally the
//! `try_insert(order)` it replaced, and no `PendingOrder` can exist. v1
//! behaviour is unchanged, which is what lets the whole thing ship before it is
//! calibrated.

use crate::shared::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Tuning knobs — all env-overridable for the calibration sweep
// ---------------------------------------------------------------------------

/// The master switch. Default off; v1 behaviour until the sweep justifies a
/// default. Same idiom as `WC3_SPEED` / `WC3_AI_BOTH` / `WC3_BRIDGE`.
pub const LATENCY_ENV: &str = "WC3_COMMAND_LATENCY";
/// Free radius around a hall — your base is where your hands work.
pub const HALL_RADIUS_ENV: &str = "WC3_LINK_HALL_RADIUS";
/// Free radius around a living hero. Smaller than a hall's on purpose: the
/// hero is the *mobile* node, and where you put it is the judgment call
/// docs/TEMPO.md §C5 wants to preserve. Buying fast hands at the front means
/// putting your most valuable unit in the most dangerous place.
pub const HERO_RADIUS_ENV: &str = "WC3_LINK_HERO_RADIUS";
/// The step: what an order costs the moment it has to leave a node's radius.
pub const STEP_ENV: &str = "WC3_LINK_STEP";
/// The ramp: seconds added per world unit beyond the radius.
pub const PER_UNIT_ENV: &str = "WC3_LINK_PER_UNIT";
/// The clamp. Also the delay charged to a team with no command nodes at all.
pub const MAX_ENV: &str = "WC3_LINK_MAX";

pub const DEFAULT_HALL_RADIUS: f32 = 30.0;
pub const DEFAULT_HERO_RADIUS: f32 = 18.0;
pub const DEFAULT_STEP: f32 = 0.6;
pub const DEFAULT_PER_UNIT: f32 = 0.02;
pub const DEFAULT_MAX: f32 = 3.0;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// A player-issued order in transit. `ready_at` is absolute game seconds — the
/// same idiom as `Militia.until`, `Bounty.expires_at` and `LastDamaged.at`.
///
/// Its presence is also the "this unit is busy" signal doctrine.rs reads: a
/// unit awaiting a delayed order looks exactly like an idle one to
/// `re_taskable`, so without a `Without<PendingOrder>` guard the squad executor
/// would re-task it and the order would silently vanish before it ever landed
/// (docs/TEMPO.md §4, "integration hazard").
#[derive(Component, Clone, Debug)]
pub struct PendingOrder {
    pub order: Order,
    /// Game seconds at which it lands.
    pub ready_at: f32,
    /// Game seconds at which it was spoken. `ready_at - issued_at` is the
    /// realised link latency, which is what the HUD and the snapshot report.
    pub issued_at: f32,
}

impl PendingOrder {
    /// Seconds of latency this order is paying, in total.
    pub fn link(&self) -> f32 {
        self.ready_at - self.issued_at
    }
}

/// Inserted on every completed hall and every living hero: the entities a
/// team's orders radiate from. Descriptive — [`CommandNodes`] is the cache the
/// latency function actually reads — but it makes "why is this a node" a
/// component query for the HUD and the snapshot, and it is the seam the
/// phase-3 forward Outpost drops into ([`building_node_radius`]).
#[derive(Component, Clone, Copy, Debug)]
pub struct CommandNode {
    pub radius: f32,
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// The curve, as data, so the headless sweep can vary it without a rebuild.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CommandLatency {
    /// The master switch. `false` ⇒ every delay is zero and this module is a
    /// no-op.
    pub on: bool,
    pub hall_radius: f32,
    pub hero_radius: f32,
    pub step: f32,
    pub per_world_unit: f32,
    pub max: f32,
}

impl Default for CommandLatency {
    fn default() -> Self {
        CommandLatency {
            on: false,
            hall_radius: DEFAULT_HALL_RADIUS,
            hero_radius: DEFAULT_HERO_RADIUS,
            step: DEFAULT_STEP,
            per_world_unit: DEFAULT_PER_UNIT,
            max: DEFAULT_MAX,
        }
    }
}

impl CommandLatency {
    pub fn from_env() -> Self {
        let flag = |name: &str| {
            std::env::var(name)
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false)
        };
        let num = |name: &str, fallback: f32| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(fallback)
        };
        CommandLatency {
            on: flag(LATENCY_ENV),
            hall_radius: num(HALL_RADIUS_ENV, DEFAULT_HALL_RADIUS),
            hero_radius: num(HERO_RADIUS_ENV, DEFAULT_HERO_RADIUS),
            step: num(STEP_ENV, DEFAULT_STEP),
            per_world_unit: num(PER_UNIT_ENV, DEFAULT_PER_UNIT),
            max: num(MAX_ENV, DEFAULT_MAX),
        }
    }

    /// **The curve.** `slack` is how far outside the nearest node's radius the
    /// unit is standing (`None` = the team has no command nodes at all).
    ///
    /// * inside any node's radius (`slack <= 0`) ⇒ free;
    /// * outside ⇒ `step + per_world_unit * slack`, clamped at `max`;
    /// * no nodes at all ⇒ `max`. Losing every hall and your hero severs the
    ///   arm, which is the same fact the phase-3 Outpost will make attackable.
    pub fn delay_for_slack(&self, slack: Option<f32>) -> f32 {
        if !self.on {
            return 0.0;
        }
        match slack {
            None => self.max,
            Some(slack) if slack <= 0.0 => 0.0,
            Some(slack) => (self.step + self.per_world_unit * slack).min(self.max),
        }
    }
}

/// Cache of `(team, position, radius)` for every command node on the map,
/// rebuilt each frame before the compiler runs. A cache rather than a query so
/// that the issue path stays a pure function every caller can afford —
/// intent.rs's compiler and ai.rs's planner both take it by reference and
/// neither needs a new `Query`.
#[derive(Resource, Default, Debug, Clone)]
pub struct CommandNodes {
    pub nodes: Vec<(Team, Vec3, f32)>,
    /// False until `refresh_command_nodes` has run once. Distinguishes "this
    /// team has no nodes" (charge `max`) from "the cache is not built yet"
    /// (charge nothing) — without it, any order issued on the first frame
    /// would pay the severed-arm penalty.
    pub ready: bool,
}

impl CommandNodes {
    /// How far outside its nearest own node this position sits, in world units
    /// on the ground plane. `0.0` = inside a radius. `None` = the team has no
    /// command nodes (or the cache is not built, which reads as "no latency").
    pub fn slack(&self, team: Team, pos: Vec3) -> Option<f32> {
        if !self.ready {
            return Some(0.0);
        }
        let mut best: Option<f32> = None;
        for (node_team, node_pos, radius) in &self.nodes {
            if *node_team != team {
                continue;
            }
            let d = xz_dist(pos, *node_pos) - radius;
            let d = d.max(0.0);
            best = Some(best.map_or(d, |b: f32| b.min(d)));
        }
        best
    }

    /// This team's own nodes — the information right the snapshot and the HUD
    /// both report, symmetrically (docs/TEMPO.md §4: own team only).
    pub fn own(&self, team: Team) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.nodes
            .iter()
            .filter(move |(t, _, _)| *t == team)
            .map(|(_, pos, r)| (*pos, *r))
    }
}

/// Slack when asking "is this the order that is already on its way?" — the same
/// question, and the same tolerance, as `doctrine::order_matches`. Two ground
/// orders a hair apart are one intention repeated, not a change of mind.
const ORDER_EPS: f32 = 1.0;

/// Is `wanted` the order already travelling? Deliberately conservative: when in
/// doubt it answers "no", the clock restarts, and the player simply pays again
/// — the failure mode is a slower order, never a lost one.
fn same_order(current: &Order, wanted: &Order) -> bool {
    match (current, wanted) {
        (Order::Idle, Order::Idle) | (Order::ReturnResources, Order::ReturnResources) => true,
        (Order::Move(a), Order::Move(b)) | (Order::AttackMove(a), Order::AttackMove(b)) => {
            xz_dist(*a, *b) <= ORDER_EPS
        }
        (Order::Attack(a), Order::Attack(b))
        | (Order::Harvest(a), Order::Harvest(b))
        | (Order::Follow(a), Order::Follow(b)) => a == b,
        (
            Order::Build { kind: ka, pos: pa },
            Order::Build { kind: kb, pos: pb },
        ) => ka == kb && xz_dist(*pa, *pb) <= ORDER_EPS,
        _ => false,
    }
}

/// Ground-plane distance. Altitude is never a tactical variable in this game,
/// and a Gryphon's height must not inflate its link.
fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

// ---------------------------------------------------------------------------
// The node table
// ---------------------------------------------------------------------------

/// Which buildings are command nodes, and how far their authority reaches.
/// Data-driven so docs/TEMPO.md's phase-3 forward Outpost is one arm here
/// rather than a code change spread across the module.
///
/// Asked as `is_hall` rather than by naming the three rungs, so upgrading a
/// TownHall into a Keep never costs you the node you paid for.
pub fn building_node_radius(kind: BuildingKind, latency: &CommandLatency) -> Option<f32> {
    is_hall(kind).then_some(latency.hall_radius)
}

// ---------------------------------------------------------------------------
// The issue path — the one function every seat writes a direct order through
// ---------------------------------------------------------------------------

/// Read-side bundle for systems that issue orders. intent.rs and ai.rs each
/// take one of these and build an [`OrderIssuer`] from it.
#[derive(SystemParam)]
pub struct CommandLink<'w> {
    pub nodes: Res<'w, CommandNodes>,
    pub latency: Res<'w, CommandLatency>,
}

impl CommandLink<'_> {
    /// Seconds a direct order to a unit of `team` standing at `pos` would take
    /// to arrive — the number the snapshot reports as `link` and the HUD will
    /// show in the selection panel. Asking costs nothing and changes nothing.
    pub fn delay(&self, team: Team, pos: Vec3) -> f32 {
        self.latency.delay_for_slack(self.nodes.slack(team, pos))
    }

    pub fn issuer(&self, now: f32) -> OrderIssuer<'_> {
        OrderIssuer {
            nodes: &self.nodes,
            latency: &self.latency,
            now,
            max_delay: 0.0,
        }
    }
}

/// Issues direct orders, delaying them by the link latency of the unit they are
/// addressed to. Borrowed for the duration of one system's work; `max_delay`
/// accumulates the worst delay it charged, which is what the replay log
/// annotates a sentence with.
pub struct OrderIssuer<'a> {
    pub nodes: &'a CommandNodes,
    pub latency: &'a CommandLatency,
    pub now: f32,
    pub max_delay: f32,
}

impl OrderIssuer<'_> {
    /// Seconds a direct order to a unit of `team` standing at `pos` would take
    /// to arrive. This is the number the snapshot reports as `link` and the HUD
    /// shows in the selection panel.
    pub fn delay(&self, team: Team, pos: Vec3) -> f32 {
        self.latency.delay_for_slack(self.nodes.slack(team, pos))
    }

    /// **A direct order.** Delayed by `delay(team, pos)`; applied immediately
    /// when that is zero — which it always is with the feature off, so this is
    /// the `try_insert(order)` it replaced, byte for byte.
    pub fn issue(
        &mut self,
        commands: &mut Commands,
        team: Team,
        pos: Vec3,
        entity: Entity,
        order: Order,
    ) {
        let delay = self.delay(team, pos);
        if delay <= 0.0 {
            self.issue_instant(commands, entity, order);
            return;
        }
        self.max_delay = self.max_delay.max(delay);
        let pending = PendingOrder {
            order,
            ready_at: self.now + delay,
            issued_at: self.now,
        };
        // Deferred to flush time so it can read the unit's TRUE current state,
        // and so no caller needs to carry a `PendingOrder` query it otherwise
        // has no use for.
        commands.queue(move |world: &mut World| {
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                return; // died between the order and the flush
            };
            if let Some(existing) = entity.get::<PendingOrder>() {
                // SAYING THE SAME THING AGAIN DOES NOT RESTART THE JOURNEY.
                //
                // Without this, any issuer that re-asserts a standing decision
                // faster than the link takes to travel can never land an order
                // at all: each repeat replaces the last and resets the clock.
                // `ai.rs` re-issues its wave target every second, so a team cut
                // off from its command nodes (halls razed, hero dead — the
                // `max` case) livelocked its whole army. A human holding down
                // right-click, or a commander re-sending an unchanged batch,
                // would have hit exactly the same wall.
                //
                // The rule that fixes it is also the honest one: latency is the
                // cost of changing your mind at range, not a tax per click.
                if same_order(&existing.order, &pending.order) {
                    return;
                }
            }
            // Otherwise the newest thing said IS the thing that arrives — a
            // superseded order is dropped, never queued behind.
            entity.insert(pending);
        });
    }

    /// **An exempt direct order** (see the verb table): applied now. It still
    /// cancels anything in transit, because an order a player has superseded
    /// must not land on top of the one that replaced it.
    pub fn issue_instant(&mut self, commands: &mut Commands, entity: Entity, order: Order) {
        let mut ec = commands.entity(entity);
        ec.try_insert(order);
        if self.latency.on {
            ec.try_remove::<PendingOrder>();
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CommandPlugin;

/// The node refresh, as a set, so every issuer can order itself after it and
/// read a cache built from this frame's positions.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CommandNodeRefresh;

impl Plugin for CommandPlugin {
    fn build(&self, app: &mut App) {
        let latency = CommandLatency::from_env();
        if latency.on {
            info!(
                "chain of command: ON — free within {:.0} of a hall / {:.0} of your hero, \
                 then +{:.2}s and +{:.3}s per unit beyond, capped at {:.1}s ({LATENCY_ENV}=0 to disable)",
                latency.hall_radius,
                latency.hero_radius,
                latency.step,
                latency.per_world_unit,
                latency.max
            );
        }
        app.insert_resource(latency)
            .init_resource::<CommandNodes>()
            .add_systems(
                Update,
                (
                    // Built from this frame's positions, before anyone asks it
                    // a question.
                    refresh_command_nodes
                        .in_set(CommandNodeRefresh)
                        .before(crate::intent::IntentApply),
                    // Before the compiler too: an order that comes due this
                    // frame lands first, so a fresh direct order issued in the
                    // same frame still wins.
                    dispatch_pending.before(crate::intent::IntentApply),
                )
                    .run_if(latency_enabled),
            )
            .add_systems(
                Update,
                report_link_load
                    .run_if(latency_enabled)
                    .run_if(on_timer(Duration::from_secs(30))),
            );
    }
}

fn latency_enabled(latency: Res<CommandLatency>) -> bool {
    latency.on
}

/// Rebuild the node cache and keep the descriptive [`CommandNode`] markers in
/// step with it. Cheap: a team fields a handful of halls and one hero.
#[allow(clippy::type_complexity)]
fn refresh_command_nodes(
    mut commands: Commands,
    latency: Res<CommandLatency>,
    mut cache: ResMut<CommandNodes>,
    buildings: Query<
        (Entity, &Building, &Team, &Transform, Option<&CommandNode>),
        Without<UnderConstruction>,
    >,
    heroes: Query<(Entity, &Team, &Transform, &Health, Option<&CommandNode>), With<Hero>>,
) {
    let mut nodes = Vec::new();
    for (entity, building, team, tf, marker) in &buildings {
        let want = building_node_radius(building.kind, &latency);
        sync_marker(&mut commands, entity, marker, want);
        if let Some(radius) = want {
            nodes.push((*team, tf.translation, radius));
        }
    }
    for (entity, team, tf, health, marker) in &heroes {
        // A dead-but-not-yet-despawned hero commands nothing.
        let want = (health.current > 0.0).then_some(latency.hero_radius);
        sync_marker(&mut commands, entity, marker, want);
        if let Some(radius) = want {
            nodes.push((*team, tf.translation, radius));
        }
    }
    cache.nodes = nodes;
    cache.ready = true;
}

/// Insert/remove/update the marker only when it actually differs — a write per
/// node per frame would be a pointless change-detection storm.
fn sync_marker(
    commands: &mut Commands,
    entity: Entity,
    current: Option<&CommandNode>,
    want: Option<f32>,
) {
    match (current, want) {
        (None, Some(radius)) => {
            commands.entity(entity).try_insert(CommandNode { radius });
        }
        (Some(_), None) => {
            commands.entity(entity).try_remove::<CommandNode>();
        }
        (Some(node), Some(radius)) if node.radius != radius => {
            commands.entity(entity).try_insert(CommandNode { radius });
        }
        _ => {}
    }
}

/// Orders that have finished travelling become real orders. Downstream systems
/// (`Changed<Order>` in units.rs, combat.rs, economy.rs) see the change here,
/// at arrival — which is also why `doctrine::rearm_retreat` un-latches a
/// retreating unit at *dispatch* time rather than issue time. That is the
/// intended reading: the unit is back on duty when the order actually reaches
/// it, not when it was spoken.
fn dispatch_pending(
    mut commands: Commands,
    time: Res<Time>,
    pending: Query<(Entity, &PendingOrder)>,
) {
    let now = time.elapsed_secs();
    for (entity, order) in &pending {
        if now < order.ready_at {
            continue;
        }
        commands
            .entity(entity)
            .try_insert(order.order.clone())
            .try_remove::<PendingOrder>();
    }
}

/// Periodic telemetry: how much of the map is currently out of arm's reach.
///
/// The scripted `ai.rs` is not a player, so it writes nothing to
/// `intent_log.jsonl` — which means an AI-vs-AI headless sweep would otherwise
/// produce no evidence that latency was charged at all. This line is that
/// evidence, and it is the series docs/TEMPO.md §5's calibration bead reads:
/// if mean link across a match is near zero the curve is not binding, and if
/// it pins at `max` the armies have marched off the end of their own chain of
/// command.
fn report_link_load(
    time: Res<Time>,
    latency: Res<CommandLatency>,
    nodes: Res<CommandNodes>,
    pending: Query<(&PendingOrder, &Team)>,
) {
    let mut count = 0u32;
    let mut total = 0.0f32;
    let mut worst = 0.0f32;
    for (order, _) in &pending {
        count += 1;
        total += order.link();
        worst = worst.max(order.link());
    }
    if count == 0 {
        return;
    }
    info!(
        "chain of command @{:.0}s: {count} orders in transit, mean link {:.2}s, worst {:.2}s \
         (cap {:.1}s), {} command nodes standing",
        time.elapsed_secs(),
        total / count as f32,
        worst,
        latency.max,
        nodes.nodes.len()
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tuned() -> CommandLatency {
        CommandLatency {
            on: true,
            ..Default::default()
        }
    }

    fn cache(nodes: Vec<(Team, Vec3, f32)>) -> CommandNodes {
        CommandNodes { nodes, ready: true }
    }

    fn at(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.0, z)
    }

    /// The curve, as a curve: free inside the radius, a step the moment you
    /// leave it, a linear ramp beyond, a clamp at the end. This is the shape
    /// docs/TEMPO.md §4 asked for and the one the calibration bead will sweep.
    #[test]
    fn the_latency_curve_is_a_step_plus_a_ramp() {
        let lat = tuned();
        // Inside the radius, at the edge of it: free.
        assert_eq!(lat.delay_for_slack(Some(0.0)), 0.0);
        // One world unit outside: the step dominates. It is a step, not a
        // ramp from zero — that discontinuity is what makes it legible.
        let just_outside = lat.delay_for_slack(Some(1.0));
        assert!(
            just_outside >= lat.step && just_outside < lat.step + 0.05,
            "one unit outside should cost about the step, got {just_outside}"
        );
        // The ramp is linear in distance beyond the radius.
        let a = lat.delay_for_slack(Some(20.0));
        let b = lat.delay_for_slack(Some(40.0));
        assert!(
            ((b - a) - lat.per_world_unit * 20.0).abs() < 1e-4,
            "ramp is not linear: {a} -> {b}"
        );
        // And it clamps rather than growing without bound.
        assert_eq!(lat.delay_for_slack(Some(100_000.0)), lat.max);
        // A team with no command nodes at all pays the maximum: losing every
        // hall AND your hero severs the arm.
        assert_eq!(lat.delay_for_slack(None), lat.max);
    }

    /// Midfield contact — the place the v1 duel was actually decided — should
    /// land in the 1.5–3s band docs/TEMPO.md §C1 names, with the defaults.
    #[test]
    fn default_constants_put_midfield_contact_in_the_intended_band() {
        let lat = tuned();
        let nodes = cache(vec![(Team::Human, Team::Human.base_pos(), lat.hall_radius)]);
        let midfield = lat.delay_for_slack(nodes.slack(Team::Human, Vec3::ZERO));
        assert!(
            (1.5..=3.0).contains(&midfield),
            "a unit at the centre of the map should pay 1.5-3s, got {midfield}"
        );
        // And the far corner is capped, not unbounded.
        let corner = lat.delay_for_slack(nodes.slack(Team::Human, Team::Claude.base_pos()));
        assert_eq!(corner, lat.max);
    }

    /// A unit standing inside its own base — or next to its hero, wherever the
    /// hero is — issues orders for free. This is docs/TEMPO.md §C5: hero micro
    /// survives, relocated to "where you put your hero is where your hands
    /// work".
    #[test]
    fn orders_are_free_near_a_command_node() {
        let lat = tuned();
        let base = Team::Human.base_pos();
        // Hero parked at the front, far from home.
        let front = at(40.0, 40.0);
        let nodes = cache(vec![
            (Team::Human, base, lat.hall_radius),
            (Team::Human, front, lat.hero_radius),
        ]);

        for spot in [base, base + at(10.0, 0.0), front, front + at(0.0, 10.0)] {
            let d = lat.delay_for_slack(nodes.slack(Team::Human, spot));
            assert_eq!(d, 0.0, "{spot:?} is inside a node radius and must be free");
        }
        // Just past the hero's smaller radius, it is not free any more.
        let outside = front + at(lat.hero_radius + 5.0, 0.0);
        assert!(
            lat.delay_for_slack(nodes.slack(Team::Human, outside)) > 0.0,
            "outside every radius must cost something"
        );
    }

    /// Nodes are per-team: standing inside the ENEMY's base is not standing
    /// inside your own command structure.
    #[test]
    fn an_enemy_node_is_not_your_node() {
        let lat = tuned();
        let enemy_base = Team::Claude.base_pos();
        let nodes = cache(vec![
            (Team::Claude, enemy_base, lat.hall_radius),
            (Team::Human, Team::Human.base_pos(), lat.hall_radius),
        ]);
        // A human unit standing on the enemy's hall is as far from home as it
        // gets — this is the deep raid, and it pays for it.
        assert_eq!(
            lat.delay_for_slack(nodes.slack(Team::Human, enemy_base)),
            lat.max
        );
        assert_eq!(
            lat.delay_for_slack(nodes.slack(Team::Claude, enemy_base)),
            0.0
        );
    }

    /// With the flag off, `slack` may say whatever it likes: the curve is flat
    /// zero. This is the off-switch at the level of the function, and the
    /// reason the whole feature can ship default-off.
    #[test]
    fn the_curve_is_identically_zero_when_the_flag_is_off() {
        let lat = CommandLatency::default();
        assert!(!lat.on, "the feature must default OFF");
        for slack in [None, Some(0.0), Some(50.0), Some(1e6)] {
            assert_eq!(lat.delay_for_slack(slack), 0.0);
        }
    }

    /// The off-flag identity at the level of the issue path: with the feature
    /// off, `issue` writes an `Order` exactly as the code it replaced did, and
    /// a `PendingOrder` can never come into existence.
    #[test]
    fn issuing_with_the_flag_off_writes_the_order_directly() {
        let mut app = App::new();
        app.insert_resource(CommandLatency::default())
            .init_resource::<CommandNodes>();

        let far = at(90.0, 90.0);
        let entity = app.world_mut().spawn((Team::Human, Order::Idle)).id();
        // A cache that WOULD charge the maximum if the flag were on.
        app.world_mut().resource_mut::<CommandNodes>().ready = true;

        let world = app.world_mut();
        let latency = *world.resource::<CommandLatency>();
        let nodes = world.resource::<CommandNodes>().clone();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, world);
            let mut issuer = OrderIssuer {
                nodes: &nodes,
                latency: &latency,
                now: 10.0,
                max_delay: 0.0,
            };
            issuer.issue(&mut commands, Team::Human, far, entity, Order::Move(far));
            assert_eq!(issuer.max_delay, 0.0);
        }
        queue.apply(world);

        assert!(
            world.entity(entity).get::<PendingOrder>().is_none(),
            "the feature is off; nothing may be left in transit"
        );
        assert!(
            matches!(world.entity(entity).get::<Order>(), Some(Order::Move(_))),
            "the order must land immediately with the feature off"
        );
    }

    /// A far-from-home order is held, then dispatched — and while it is in
    /// transit the unit's `Order` is untouched, so it keeps doing what it was
    /// doing. This is the mechanic in one test.
    #[test]
    fn a_distant_order_arrives_late_and_then_arrives() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(tuned())
            .init_resource::<CommandNodes>()
            .add_systems(Update, dispatch_pending);

        let lat = tuned();
        let base = Team::Human.base_pos();
        app.insert_resource(cache(vec![(Team::Human, base, lat.hall_radius)]));

        let far = at(60.0, 60.0);
        let entity = app
            .world_mut()
            .spawn((Team::Human, Order::Move(base), Transform::from_translation(far)))
            .id();

        // Issue from far away.
        let world = app.world_mut();
        let latency = *world.resource::<CommandLatency>();
        let nodes = world.resource::<CommandNodes>().clone();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let delay;
        {
            let mut commands = Commands::new(&mut queue, world);
            let mut issuer = OrderIssuer {
                nodes: &nodes,
                latency: &latency,
                now: 0.0,
                max_delay: 0.0,
            };
            delay = issuer.delay(Team::Human, far);
            issuer.issue(&mut commands, Team::Human, far, entity, Order::AttackMove(far));
            assert_eq!(issuer.max_delay, delay, "the issuer reports what it charged");
        }
        queue.apply(world);

        assert!(delay > 0.0, "a unit this far from home must pay something");
        let pending = world
            .entity(entity)
            .get::<PendingOrder>()
            .expect("the order is in transit")
            .clone();
        assert!((pending.link() - delay).abs() < 1e-5);
        assert!(
            matches!(world.entity(entity).get::<Order>(), Some(Order::Move(_))),
            "an in-transit order must not disturb what the unit is doing now"
        );

        // Not yet.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(delay * 0.5));
        app.update();
        assert!(
            app.world().entity(entity).get::<PendingOrder>().is_some(),
            "dispatched early"
        );

        // Now.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(delay * 0.6));
        app.update();
        assert!(
            app.world().entity(entity).get::<PendingOrder>().is_none(),
            "the order never arrived — this is the 'my orders vanish' bug"
        );
        assert!(
            matches!(
                app.world().entity(entity).get::<Order>(),
                Some(Order::AttackMove(_))
            ),
            "the delayed order must actually land"
        );
    }

    /// **The livelock.** Found in a headless `crossings` run, not in review: a
    /// team that had lost every hall and its hero paid the `max` link on every
    /// order, and `ai.rs` re-issues its standing decision once a second — so
    /// each repeat replaced the last, the clock restarted, and not one order
    /// ever landed. Two workers sat frozen for twenty minutes of game time
    /// while the telemetry read "2 orders in transit, mean link 3.00s" forever.
    ///
    /// Repeating an order you have already given must therefore be free. A
    /// changed order still supersedes, and still pays.
    #[test]
    fn repeating_an_order_does_not_restart_its_journey() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(tuned())
            // The severed-arm case: no command nodes at all, so every order
            // pays the cap — much longer than the interval a caller re-asserts
            // its standing decision on.
            .insert_resource(cache(Vec::new()))
            .add_systems(Update, dispatch_pending);

        let far = at(60.0, 60.0);
        let entity = app
            .world_mut()
            .spawn((Team::Human, Order::Idle, Transform::from_translation(far)))
            .id();

        let repeat = |app: &mut App, now: f32, order: Order| {
            let world = app.world_mut();
            let latency = *world.resource::<CommandLatency>();
            let nodes = world.resource::<CommandNodes>().clone();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            {
                let mut commands = Commands::new(&mut queue, world);
                let mut issuer = OrderIssuer {
                    nodes: &nodes,
                    latency: &latency,
                    now,
                    max_delay: 0.0,
                };
                issuer.issue(&mut commands, Team::Human, far, entity, order);
            }
            queue.apply(world);
        };

        // Said once at t=0, then said again every second — the exact cadence
        // that used to reset the clock forever.
        repeat(&mut app, 0.0, Order::AttackMove(far));
        let first_ready = app
            .world()
            .entity(entity)
            .get::<PendingOrder>()
            .expect("in transit")
            .ready_at;

        for step in 1..=3 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0));
            repeat(&mut app, step as f32, Order::AttackMove(far));
            app.update();
        }

        assert!(
            app.world().entity(entity).get::<PendingOrder>().is_none(),
            "the order never landed — the repeat kept resetting its clock"
        );
        assert!(
            matches!(
                app.world().entity(entity).get::<Order>(),
                Some(Order::AttackMove(_))
            ),
            "the repeated order must eventually arrive"
        );
        assert!(
            first_ready <= tuned().max + 1e-3,
            "the first utterance should have set the arrival time"
        );

        // Changing your mind, on the other hand, does restart it — that is the
        // cost the mechanic exists to charge.
        let elsewhere = at(-60.0, 60.0);
        repeat(&mut app, 10.0, Order::AttackMove(elsewhere));
        let pending = app
            .world()
            .entity(entity)
            .get::<PendingOrder>()
            .expect("a genuinely new order travels");
        assert!(pending.link() > 0.0);
    }

    /// The claim the verb table makes about `cast`/`use_item`: every caster in
    /// the game either IS a command node or SITS on one, so charging those
    /// verbs latency would always compute zero. If someone adds an ability to a
    /// building that is not a hall — or to a non-hero unit — this test fails,
    /// which is the signal to move that row of the table.
    #[test]
    fn every_caster_is_a_command_node() {
        for kind in ALL_BUILDING_KINDS {
            if abilities_of_building(kind).is_empty() {
                continue;
            }
            assert!(
                is_hall(kind),
                "{} casts but is not a command node — the `cast` row of \
                 command.rs's verb table no longer holds",
                building_name(kind)
            );
        }
        for kind in ALL_UNIT_KINDS {
            if abilities_of_unit(kind).is_empty() {
                continue;
            }
            assert!(
                is_hero_kind(kind),
                "{} casts but is not a hero, so it is not a command node — the \
                 `cast` row of command.rs's verb table no longer holds",
                kind_name(kind)
            );
        }
    }
}
