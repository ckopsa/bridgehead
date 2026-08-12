//! trigger.rs — `when` as a first-class word.
//!
//! doctrine.rs runs **continuous** standing policy for whoever set it: retreat
//! below 35%, hold this ring, focus the siege. This module runs the
//! **contingent** half — a predicate the engine watches and an intent it
//! submits the instant the predicate holds.
//!
//! The two are the same argument. THESIS.md's tempo answer is "relocate fast
//! work into the game itself", and eight rounds of doctrine did that for the
//! work that never stops. What it never covered was *reaction*: a commander who
//! wanted to answer a base raid had to read `events`, notice, and speak — and
//! for a language model that loop costs ten to fifteen seconds, every time,
//! forever. A human at a keyboard pays 200ms for the same answer. That gap is
//! not judgment; it is polling latency, and this module deletes it for
//! **whichever** player armed the rule.
//!
//! ## What this module is allowed to do
//!
//! Exactly one thing: write [`SubmitIntent`]. It mints no `Order`, spends
//! nothing, and moves nothing. A fired trigger is an ordinary intent through
//! the ordinary compiler — same validation, same ownership checks, same fog
//! rule, same replay log, same error channel. The only thing that marks it out
//! is [`SubmitIntent::trigger`], and everything downstream that reads that
//! field is telling the truth about *authorship*, never about legality.
//!
//! That is deliberate and it is the whole reason the action is `Intent` rather
//! than a small private list of "things a trigger may do". A private list would
//! be a second vocabulary, and docs/INTENT.md exists because two implementations
//! of one language is two languages.
//!
//! ## Where it sits in the frame
//!
//! `SimSet::Think`, after `FogSet`, at 4 Hz. Reasoned out against
//! `shared::SIM_ORDER` (`Deaths → Fog → Input → CoCommand → AiThink → Think →
//! Intent → …`):
//!
//! * **After `Deaths`** — a predicate must not count a corpse. `hero_below`
//!   over a hero who died this frame would fire a rescue at nothing.
//! * **After `Fog`** — `enemy_sighted` and `bounty_spawned` are fog-honest, so
//!   they must read the grid the snapshot and the HUD are about to be built
//!   from. Any earlier and a trigger reacts to last frame's knowability.
//! * **Before `Intent`** — this is what makes a trigger *fast* rather than
//!   merely automatic. The intent it submits is compiled in the same frame's
//!   `SimSet::Intent`, so the whole distance from "the building took damage" to
//!   "the squad is moving" is one tick plus the evaluator's cadence.
//! * **In `Think` rather than in `Input`** — `Think` is where standing policy
//!   lives, and a trigger is standing policy. It also gives the right
//!   precedence: doctrine.rs writes `Order` components directly *in* `Think`,
//!   and a trigger's intent is compiled *after* `Think`, so a fired trigger
//!   overrules the squad posture executor for that tick. That is the correct
//!   ranking — a rule the commander wrote for this exact situation should beat
//!   the continuous policy it was written to interrupt.
//!
//! One honest consequence of the slot, stated rather than discovered: an intent
//! submitted here is read by the compiler *after* the ones ui.rs and bridge.rs
//! submitted in `SimSet::Input` this frame, so on the rare tick where a player
//! clicks in the same 250ms window that their own trigger fires, the trigger
//! lands last. `Order` is a component and last writer wins everywhere in this
//! codebase (docs/INTENT.md, co-command: *source is descriptive, never
//! authoritative*), so this is the existing rule rather than a new one — and
//! the player can always speak again, which is a quarter of a second away.
//!
//! ## Determinism inside the set
//!
//! `SimSet::Think` also holds doctrine.rs's seven systems, and Bevy leaves two
//! systems in one set unordered unless something forces an edge. One thing
//! does: plan.rs's evaluator writes the same two things this system does, and
//! it declares itself `.before` this one so that a trigger's answer to the
//! situation at hand lands AFTER a plan's answer to the general case, and
//! therefore wins. That edge is argued in plan.rs's module docs.
//!
//! Against doctrine, nothing forces an edge and nothing needs to: this system's
//! only writes are `ResMut<Triggers>` (nobody else touches it),
//! `ResMut<GameEvents>` and `EventWriter<SubmitIntent>` (whose only other
//! `Think` writer is plan.rs, ordered explicitly). Everything it reads is
//! read-only, and doctrine's `Order` writes go through `Commands`, which flush
//! after the set either way. So the two are genuinely commutative rather than
//! merely usually-fine.
//!
//! What IS ordered, deliberately, is the sweep itself: teams in a fixed order,
//! and each team's rules in the order they were armed (`Triggers` is a `Vec`
//! and `trigger_set` replaces in place). Two rules coming true on the same tick
//! submit in the order the commander wrote them, on every run.
//!
//! ## Cadence
//!
//! 250ms, the same heartbeat as `doctrine::trigger_retreat` and for the same
//! reason: it is the fastest thing in `Think` because it is the one watching
//! for a threshold to be crossed, and a threshold crossed is news that goes
//! stale. The sweep is at most `MAX_TRIGGERS_PER_TEAM * 2` predicates over
//! queries the frame already has resident.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use std::time::Duration;

use crate::intent::{parse_target_class, parse_unit_kind};
use crate::shared::*;

/// Trigger evaluation cadence (~4 Hz). Matches `doctrine::RETREAT_MS`.
const TRIGGER_MS: u64 = 250;

pub struct TriggerPlugin;

impl Plugin for TriggerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            evaluate_triggers
                .run_if(on_timer(Duration::from_millis(TRIGGER_MS)))
                // The module docs above reason this slot out in full. Short
                // version: after the one producer of knowability, inside the
                // set where standing policy lives, and upstream of the compiler
                // so a fired trigger is executed in the frame it fired.
                .in_set(SimSet::Think)
                .after(FogSet),
        );
    }
}

// ---------------------------------------------------------------------------
// The world a predicate may consult
// ---------------------------------------------------------------------------

/// Everything with a team, a position and health — units and buildings alike.
/// One query rather than two because half the predicates ask about "any of
/// ours" and the split would only be re-joined at every call site.
type TriggerUnits<'w, 's> = Query<
    'w,
    's,
    (
        &'static Unit,
        &'static Team,
        &'static Transform,
        &'static Health,
        Option<&'static Hero>,
        Option<&'static SquadId>,
    ),
>;

type TriggerBuildings<'w, 's> = Query<
    'w,
    's,
    (
        &'static Building,
        &'static Team,
        &'static Transform,
        Option<&'static LastDamaged>,
        Option<&'static UnderConstruction>,
    ),
>;

type TriggerNodes<'w, 's> = Query<'w, 's, (&'static ResourceNode, &'static Transform)>;

/// Production queues, by owner. Its own query rather than a sixth column on
/// [`TriggerBuildings`]: only `supply_capped` reads it, and widening the
/// buildings tuple would re-spell the destructuring in every other arm to buy
/// nothing.
type TriggerQueues<'w, 's> = Query<'w, 's, (&'static Team, &'static TrainingQueue)>;

type TriggerBounties<'w, 's> = Query<'w, 's, (&'static Bounty, &'static Transform)>;

/// The read-only world one predicate sweep consults. Bundled because
/// `evaluate_triggers` would otherwise sit on Bevy's parameter ceiling for the
/// sake of five queries nobody outside this file cares about.
#[derive(SystemParam)]
pub struct TriggerWorld<'w, 's> {
    units: TriggerUnits<'w, 's>,
    buildings: TriggerBuildings<'w, 's>,
    nodes: TriggerNodes<'w, 's>,
    bounties: TriggerBounties<'w, 's>,
    /// What is standing in production right now — the half of "am I supply
    /// blocked?" that the ledger has not been told about yet.
    queues: TriggerQueues<'w, 's>,
    tiers: Res<'w, TechTiers>,
    /// Read-only, and only ever for the ASKING team's own row: a predicate
    /// that could read the other side's bank would be fog laundering with
    /// extra steps.
    economies: Res<'w, Economies>,
    fog: Res<'w, FogGrids>,
    /// Read-only here. `enemy_in` is the one predicate that asks WHERE, and
    /// the answer is the arming team's own vocabulary — built-ins included, so
    /// "5 enemies in the center ford" is armable in the first second of a
    /// match with nothing named first.
    regions: Res<'w, Regions>,
}

// ---------------------------------------------------------------------------
// The predicates
// ---------------------------------------------------------------------------

fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    Vec2::new(d.x, d.z).length()
}

/// Does `when` hold for `me`, right now?
///
/// `pub` because plan.rs asks the same question about the same vocabulary: a
/// plan step's `advance` condition IS a [`TriggerWhen`], and two evaluators
/// with two readings of "we reached tier 2" would be two languages. One
/// definition, two callers — and any predicate a later bead adds here is a plan
/// advance-condition the moment it lands, with no work in plan.rs.
///
/// Every arm is a fold over state the frame already has. Nothing here writes,
/// remembers, or subscribes: a predicate that needed its own bookkeeping would
/// be a predicate whose truth could drift from the world, and the whole value
/// of firing at machine speed is that the world is what fired it.
pub fn holds(when: &TriggerWhen, me: Team, now: f32, world: &TriggerWorld) -> bool {
    match when {
        // Our own BUILDINGS, damaged inside the window. Buildings only: a
        // skirmish in midfield is not the base being attacked, and a predicate
        // that fired for it would be an alarm nobody could act on. Buildings
        // still going up count — losing a half-built expansion is exactly the
        // raid this is for.
        TriggerWhen::BaseUnderAttack => world
            .buildings
            .iter()
            .any(|(_, team, _, hit, _)| {
                *team == me && hit.is_some_and(|h| now - h.at <= BASE_ATTACK_WINDOW_S)
            }),

        // ANY of our living heroes. A team may field two; "the hero" stopped
        // being a well-defined phrase when hero slots started climbing the hall
        // ladder (docs/INTENT.md), and the useful reading of "save my hero" is
        // "whichever one is dying".
        TriggerWhen::HeroBelow { frac } => world.units.iter().any(|(_, team, _, hp, hero, _)| {
            *team == me && hero.is_some() && hp.max > 0.0 && hp.current / hp.max < *frac
        }),

        // The wait-condition half of the pair, and NOT the negation of the arm
        // above it — see the doc on `TriggerWhen::HeroAbove`. Written as a fold
        // that has to see a hero before it can be true, so "we have no hero" and
        // "our hero is healed" can never be confused: an empty roster leaves
        // `found` false and the whole predicate false, which is what keeps a
        // chain from advancing over a corpse.
        TriggerWhen::HeroAbove { frac } => {
            let mut found = false;
            for (_, team, _, hp, hero, _) in world.units.iter() {
                if *team != me || hero.is_none() || hp.max <= 0.0 {
                    continue;
                }
                found = true;
                if hp.current / hp.max < *frac {
                    return false;
                }
            }
            found
        }

        // POOLED health, not per-member: a squad is a formation, and one
        // wounded footman in a healthy line is not a squad in trouble. False
        // for an empty squad — see the doc on `TriggerWhen::SquadBelow`.
        TriggerWhen::SquadBelow { id, frac } => {
            let (current, max) = world
                .units
                .iter()
                .filter(|(_, team, _, _, _, squad)| {
                    **team == me && squad.is_some_and(|s| s.0 == *id)
                })
                .fold((0.0f32, 0.0f32), |(c, m), (_, _, _, hp, _, _)| {
                    (c + hp.current, m + hp.max)
                });
            max > 0.0 && current / max < *frac
        }

        // FOG-HONEST. The count is taken against this team's own grid, so a
        // trigger can never react to something its owner was not shown. A
        // remembered building is deliberately not "sighted" — remembering where
        // a barracks stood is not the same news as seeing an army come out of
        // it, and the alarm this predicate exists to raise is the second one.
        TriggerWhen::EnemySighted { class, count } => {
            let want = class.as_deref().and_then(parse_target_class);
            // An unparseable class is refused by the compiler at set time, so
            // reaching here with one is impossible; counting nothing is the
            // safe answer if it ever happens.
            if class.is_some() && want.is_none() {
                return false;
            }
            let fog = world.fog.get(me);
            let seen = world
                .units
                .iter()
                .filter(|(unit, team, tf, _, _, _)| {
                    **team == me.enemy()
                        && fog.sees(tf.translation)
                        && want.is_none_or(|w| TargetClass::of(Some(unit.kind), false) == Some(w))
                })
                .count();
            seen as u32 >= (*count).max(1)
        }

        // The territorial `enemy_sighted`. TWO filters, and both are load
        // bearing:
        //
        //   * the arming team's own `FogGrid::sees` — a region is ground you
        //     are WATCHING, not ground you are told about, so an army walking
        //     unseen through your named pass does not trip the rule. That is
        //     the same knowability rule `enemy_sighted` obeys, applied to a
        //     smaller piece of the map, and it is what keeps a region from
        //     becoming a free sensor.
        //   * the circle itself, on XZ.
        //
        // A region cleared after arming makes this go QUIET rather than fall
        // back to the whole map: an unresolvable name is not a bigger question,
        // it is no question. The compiler refuses unknown names at arm time, so
        // reaching here with one means the commander cleared the region out
        // from under their own rule — and firing a defence of nowhere would be
        // strictly worse than not firing.
        TriggerWhen::EnemyIn {
            region,
            class,
            count,
        } => {
            let Some(shape) = world.regions.find(me, region) else {
                return false;
            };
            let want = class.as_deref().and_then(parse_target_class);
            if class.is_some() && want.is_none() {
                return false;
            }
            let fog = world.fog.get(me);
            let seen = world
                .units
                .iter()
                .filter(|(unit, team, tf, _, _, _)| {
                    **team == me.enemy()
                        && fog.sees(tf.translation)
                        && shape.contains(tf.translation)
                        && want.is_none_or(|w| TargetClass::of(Some(unit.kind), false) == Some(w))
                })
                .count();
            seen as u32 >= (*count).max(1)
        }

        // INTEL, not sight. The only two predicates in this file that read the
        // ledger rather than the world, and they are fog-honest a step further
        // out than the rest: the ledger itself cannot contain anything this
        // team did not observe, so a predicate over it inherits the property
        // instead of re-deriving it. There is no `world.units` access in
        // either arm, which is the structural version of that claim.
        TriggerWhen::EnemyArmySeen { size, within_s } => {
            let fog = world.fog.get(me);
            fog.army_groups().iter().any(|g| {
                g.size as u32 >= (*size).max(1)
                    && within_s.is_none_or(|w| (now - g.t_seen).max(0.0) <= w)
            })
        }

        // A LEVEL predicate: "their hero is currently believed dead". See the
        // doc on `TriggerWhen::EnemyHeroDown` for why it is not an edge, and
        // what a once vs. a repeating rule does with it.
        TriggerWhen::EnemyHeroDown { class } => {
            let want = class.as_deref().and_then(parse_unit_kind);
            // An unparseable or non-hero class is refused by the compiler at
            // set time, so reaching here with one is impossible; believing
            // nothing is the safe answer if it ever happens.
            if class.is_some() && !want.is_some_and(is_hero_kind) {
                return false;
            }
            let fog = world.fog.get(me);
            let down = |h: &HeroIntel| h.status == HeroStatus::SeenDying;
            match want {
                // A named class asks about exactly one belief, so ask for it
                // rather than scanning and filtering.
                Some(kind) => fog.hero_intel_of(kind).is_some_and(down),
                None => fog.hero_intel().iter().any(down),
            }
        }

        // Fog-honest for the same reason and through the same call the
        // snapshot's `bounties` array uses.
        TriggerWhen::BountySpawned => {
            let fog = world.fog.get(me);
            world
                .bounties
                .iter()
                .any(|(_, tf)| fog.sees(tf.translation))
        }

        // Mines are neutral and unowned, so "our mine" is defined by geometry:
        // a dry gold node inside `MINE_HOME_RADIUS` of one of our COMPLETED
        // halls. Completed, because a hall still going up has not started
        // working anything.
        TriggerWhen::MineDry => {
            let halls: Vec<Vec3> = world
                .buildings
                .iter()
                .filter(|(b, team, _, _, uc)| {
                    **team == me && is_hall(b.kind) && uc.is_none()
                })
                .map(|(_, _, tf, _, _)| tf.translation)
                .collect();
            if halls.is_empty() {
                return false;
            }
            world.nodes.iter().any(|(node, tf)| {
                node.kind == ResourceKind::Gold
                    && node.remaining == 0
                    && halls
                        .iter()
                        .any(|hall| xz_dist(*hall, tf.translation) <= MINE_HOME_RADIUS)
            })
        }

        // No free supply, counting what is already in the queues. A fold over
        // state the frame already has, like every arm here: the queues are
        // components, the ledger is a resource, and nothing is remembered
        // between sweeps.
        //
        // `supply_cap > 0` is the guard that keeps this from being true before
        // the match starts. The cap is recomputed every frame from completed
        // supply buildings, so a team with none — frame one, or a team whose
        // last hall just fell — reads zero cap and zero headroom. That is "you
        // have no base", not "you are supply blocked", and the answer to it is
        // not a farm. ui.rs's supply-blocked badge draws the line in the same
        // place, and two readings of one phrase would be two languages.
        TriggerWhen::SupplyCapped => {
            let economy = world.economies.get(me);
            let queued: u32 = world
                .queues
                .iter()
                .filter(|(team, _)| **team == me)
                .map(|(_, q)| {
                    q.queue
                        .iter()
                        .map(|kind| unit_stats(*kind).supply)
                        .sum::<u32>()
                })
                .sum();
            economy.supply_cap > 0 && supply_headroom(economy, queued) == 0
        }

        TriggerWhen::TierReached { tier } => world.tiers.get(me).level() >= u32::from(*tier),

        TriggerWhen::UnitCount { kind, count } => {
            let Some(want) = parse_unit_kind(kind) else {
                return false;
            };
            let have = world
                .units
                .iter()
                .filter(|(unit, team, _, _, _, _)| **team == me && unit.kind == want)
                .count();
            have as u32 >= (*count).max(1)
        }

        TriggerWhen::GameTime { at } => now >= *at,
    }
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

/// Sweep both teams' armed triggers; fire the ones whose predicate holds.
///
/// A once-trigger disarms as it fires and stays in the list, spent — so
/// "did my rule ever go off?" is answerable from the snapshot rather than from
/// an absence. A repeating one stamps `last_fired` and goes quiet for its
/// cooldown.
/// `pub` so plan.rs can declare a hard ordering edge against it. Both systems
/// live in `SimSet::Think` and both write `SubmitIntent` and `GameEvents`, and
/// Bevy would otherwise leave them unordered — see plan.rs's module docs for
/// why plans go first and a trigger therefore wins a same-tick tie.
pub fn evaluate_triggers(
    time: Res<Time>,
    mut triggers: ResMut<Triggers>,
    mut submissions: EventWriter<SubmitIntent>,
    mut feed: ResMut<GameEvents>,
    world: TriggerWorld,
) {
    let now = time.elapsed_secs();
    // Fixed team order so two teams' triggers can never interleave differently
    // between runs. `Triggers` keeps each team's list in the order it was
    // written for the same reason.
    for me in [Team::Human, Team::Claude] {
        // Decide first, mutate second: `holds` borrows the world immutably and
        // the fire loop needs `&mut` on the resource.
        let firing: Vec<(usize, TriggerName, IntentSource, Intent)> = triggers
            .get(me)
            .iter()
            .enumerate()
            .filter(|(_, t)| t.ready(now) && holds(&t.when, me, now, &world))
            .map(|(i, t)| (i, t.name, t.source, t.then.clone()))
            .collect();
        for (index, name, source, intent) in firing {
            {
                let list = triggers.get_mut(me);
                let trigger = &mut list[index];
                trigger.last_fired = Some(now);
                if trigger.repeat.is_none() {
                    trigger.armed = false;
                }
            }
            // Both renderers, one producer. The human reads this in the alert
            // stack; the commander that armed it reads the identical line in
            // its snapshot's `events`. Without it a trigger would be the one
            // thing in the game that changes the board without saying so.
            //
            // `Info`, not `Warning`: whatever the trigger reacted to has
            // already raised its own line at its own severity, and this is the
            // calmer follow-up that says what was done about it.
            feed.push(
                me,
                now,
                format!("trigger {name} fired: {}", intent.sentence()),
                EventSeverity::Info,
                None,
            );
            submissions.write(SubmitIntent::fired(me, source, name, intent));
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
    use crate::intent::{IntentLog, IntentPlugin};

    /// The evaluator against a bare world. `Time` is hand-driven and
    /// `evaluate_triggers` is registered WITHOUT its `on_timer` — the same
    /// idiom doctrine.rs's tests use for the same reason: the cadence is a
    /// tuning constant, and a test that waited 250ms of wall clock for it would
    /// be testing the clock.
    fn trigger_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<Triggers>()
            .init_resource::<Regions>()
            .init_resource::<TechTiers>()
            // `supply_capped` reads the ledger, so `TriggerWorld` needs it —
            // and defaults to a zero cap, which is exactly the "no base yet"
            // reading that predicate refuses to fire on.
            .init_resource::<Economies>()
            .init_resource::<GameEvents>()
            .add_event::<SubmitIntent>()
            .add_systems(Update, evaluate_triggers);
        // Pin the fog mode rather than inheriting `BH_FOG`: two predicates
        // here are ABOUT knowability, so the ambient env must not decide them.
        app.insert_resource(FogGrids::test_dark());
        app
    }

    /// The evaluator and the real compiler in one app, so a test can arm a
    /// trigger the way a commander does and watch the engine act on it.
    /// `evaluate_triggers` runs `.before(IntentApply)`, which is what
    /// `SimSet::Think` → `SimSet::Intent` means in the real schedule.
    fn full_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<Triggers>()
            .init_resource::<Regions>()
            .init_resource::<Economies>()
            .init_resource::<HeroRecords>()
            .init_resource::<TechTiers>()
            .init_resource::<NavGrid>()
            .init_resource::<TeamResearch>()
            .init_resource::<SquadOrders>()
            .init_resource::<AiControlled>()
            .init_resource::<GameEvents>()
            .init_resource::<CommandNodes>()
            .init_resource::<CommandLatency>()
            .add_event::<CastAbility>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .add_plugins(IntentPlugin)
            .add_systems(Update, evaluate_triggers.before(crate::intent::IntentApply));
        app.insert_resource(IntentLog::disabled());
        app.insert_resource(FogGrids::test_dark());
        app
    }

    fn name(s: &str) -> TriggerName {
        TriggerName::new(s).expect("test name is legal")
    }

    fn armed(when: TriggerWhen, then: Intent, repeat: Option<f32>) -> TriggerRule {
        TriggerRule {
            name: name("t"),
            when,
            then,
            repeat,
            source: IntentSource::Bridge,
            armed: true,
            last_fired: None,
        }
    }

    fn stop_intent() -> Intent {
        Intent::Stop {
            units: vec![],
            select: None,
        }
    }

    fn fired(app: &mut App) -> Vec<SubmitIntent> {
        let mut events = app.world_mut().resource_mut::<Events<SubmitIntent>>();
        let out: Vec<SubmitIntent> = events.drain().collect();
        out
    }

    fn advance(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
    }

    // -- the once/cooldown rule, as pure arithmetic -----------------------

    #[test]
    fn a_once_trigger_is_ready_until_it_is_disarmed() {
        let mut t = armed(TriggerWhen::GameTime { at: 0.0 }, stop_intent(), None);
        assert!(t.ready(0.0));
        t.armed = false;
        t.last_fired = Some(0.0);
        assert!(!t.ready(1000.0), "a spent once-trigger never fires again");
        assert_eq!(t.status(1000.0), "spent");
    }

    #[test]
    fn a_repeating_trigger_waits_out_its_cooldown() {
        let mut t = armed(TriggerWhen::GameTime { at: 0.0 }, stop_intent(), Some(30.0));
        assert!(t.ready(0.0), "never fired, so nothing to wait for");
        t.last_fired = Some(10.0);
        assert!(!t.ready(39.9), "still cooling");
        assert_eq!(t.status(39.9), "cooling");
        assert!(t.ready(40.0), "cooldown is inclusive at its edge");
        assert_eq!(t.status(40.0), "armed");
    }

    // -- firing, once vs repeating, through the real system ----------------

    #[test]
    fn a_once_trigger_fires_exactly_once() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::GameTime { at: 0.0 }, stop_intent(), None));

        app.update();
        assert_eq!(fired(&mut app).len(), 1, "fires on the first sweep");
        for _ in 0..5 {
            advance(&mut app, 60.0);
            app.update();
            assert!(fired(&mut app).is_empty(), "and never again");
        }
        let triggers = app.world().resource::<Triggers>();
        let t = &triggers.get(Team::Human)[0];
        assert!(!t.armed, "spent, but still listed so the seat can see it");
        assert_eq!(t.status(999.0), "spent");
    }

    #[test]
    fn a_repeating_trigger_fires_again_only_after_its_cooldown() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::GameTime { at: 0.0 },
                stop_intent(),
                Some(30.0),
            ));

        app.update();
        assert_eq!(fired(&mut app).len(), 1);
        advance(&mut app, 10.0);
        app.update();
        assert!(fired(&mut app).is_empty(), "10s into a 30s cooldown");
        advance(&mut app, 25.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "35s later it goes again");
    }

    #[test]
    fn a_fired_trigger_announces_itself_on_its_own_teams_feed_only() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::GameTime { at: 0.0 }, stop_intent(), None));
        app.update();

        let feed = app.world().resource::<GameEvents>();
        assert!(
            feed.feed(Team::Human)
                .iter()
                .any(|e| e.message.starts_with("trigger t fired:")),
            "the owner is told"
        );
        assert!(
            feed.feed(Team::Claude).is_empty(),
            "and the opponent is not — a trigger is not intelligence"
        );
    }

    #[test]
    fn a_fired_intent_carries_the_arming_seat_and_the_trigger_name() {
        let mut app = trigger_app();
        let mut t = armed(TriggerWhen::GameTime { at: 0.0 }, stop_intent(), None);
        t.name = name("home-guard");
        t.source = IntentSource::Ui;
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(t);
        app.update();

        let out = fired(&mut app);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, IntentSource::Ui, "the AUTHOR, not the engine");
        assert_eq!(out[0].trigger.map(|n| n.as_str().to_string()).as_deref(), Some("home-guard"));
        assert_eq!(out[0].tag, "trigger:home-guard");
        assert_eq!(out[0].team, Team::Human);
    }

    #[test]
    fn triggers_fire_in_the_order_they_were_armed() {
        let mut app = trigger_app();
        {
            let mut triggers = app.world_mut().resource_mut::<Triggers>();
            let list = triggers.get_mut(Team::Human);
            for label in ["first", "second", "third"] {
                let mut t = armed(TriggerWhen::GameTime { at: 0.0 }, stop_intent(), None);
                t.name = name(label);
                list.push(t);
            }
        }
        app.update();
        let order: Vec<String> = fired(&mut app)
            .iter()
            .map(|s| s.trigger.unwrap().as_str().to_string())
            .collect();
        assert_eq!(order, vec!["first", "second", "third"]);
    }

    // -- fog honesty -------------------------------------------------------

    /// `enemy_sighted` counts against the ARMING TEAM'S OWN fog grid, so a
    /// trigger can never react to something its owner was never shown. This is
    /// the same rule the compiler applies to `attack` and the snapshot applies
    /// to `units` — one rule of knowability, now also governing what a rule may
    /// notice.
    #[test]
    fn enemy_sighted_is_fog_honest() {
        let mut app = trigger_app();
        let spot = Vec3::new(20.0, 0.0, 20.0);
        app.world_mut().spawn((
            Unit { kind: UnitKind::Footman },
            Team::Claude,
            Transform::from_translation(spot),
            Health::new(100.0),
        ));
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemySighted {
                    class: None,
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));

        // Nothing revealed: the enemy is standing right there and the rule does
        // not know it.
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "unseen is unknown, for a trigger exactly as for an order"
        );

        // Light the map for both seats and the IDENTICAL world fires it. The
        // only thing that changed is what the arming team was shown.
        app.insert_resource(FogGrids::test_revealed());
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "seen is known");
    }

    #[test]
    fn enemy_sighted_counts_only_the_named_class_and_only_enough_of_them() {
        let mut app = trigger_app();
        for i in 0..2 {
            let spot = Vec3::new(20.0 + i as f32, 0.0, 20.0);
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_translation(spot),
                Health::new(100.0),
            ));
        }
        app.insert_resource(FogGrids::test_revealed());
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemySighted {
                    class: Some("Siege".to_string()),
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(fired(&mut app).is_empty(), "two footmen are not a siege train");

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemySighted {
            class: Some("Footman".to_string()),
            count: 3,
        };
        advance(&mut app, 5.0);
        app.update();
        assert!(fired(&mut app).is_empty(), "two is fewer than three");

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemySighted {
            class: Some("Footman".to_string()),
            count: 2,
        };
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1);
    }

    // -- intel: predicates that read MEMORY rather than sight --------------

    fn sighting(id: u64, kind: UnitKind, x: f32, t_seen: f32) -> Sighting {
        Sighting {
            id,
            team: Team::Claude,
            kind,
            pos: Vec3::new(x, 0.0, 0.0),
            hp_frac: 1.0,
            heading: None,
            t_seen,
        }
    }

    /// The difference between `enemy_sighted` and `enemy_army_seen` in one
    /// test: an army standing in the dark fires NEITHER, and an army recorded
    /// in the ledger fires only the second — which is what makes it survive
    /// the death of the scout that found it.
    #[test]
    fn enemy_army_seen_reads_the_ledger_not_the_board() {
        let mut app = trigger_app();
        // Four enemies really standing there, and no vision of them at all.
        for i in 0..4 {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_translation(Vec3::new(20.0 + i as f32, 0.0, 20.0)),
                Health::new(100.0),
            ));
        }
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyArmySeen {
                    size: 3,
                    within_s: None,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "an army nobody has ever seen is an army no rule may react to"
        );

        // Now the same four are in this team's ledger — scouted at t=0 by a
        // rider that is long since dead. The board has not changed.
        let mut grids = FogGrids::test_dark();
        for i in 0..4u64 {
            grids.test_sight(Team::Human, sighting(i, UnitKind::Footman, i as f32, 0.0));
        }
        app.insert_resource(grids);
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(
            fired(&mut app).len(),
            1,
            "what we know outlives what we can see — that is the whole feature"
        );
    }

    #[test]
    fn enemy_army_seen_counts_the_group_and_honours_a_staleness_bound() {
        let mut app = trigger_app();
        let mut grids = FogGrids::test_dark();
        // Three together, seen at t=0.
        for i in 0..3u64 {
            grids.test_sight(Team::Human, sighting(i, UnitKind::Footman, i as f32, 0.0));
        }
        // A fourth, far away — a separate group, so no group reaches four.
        grids.test_sight(Team::Human, sighting(9, UnitKind::Footman, 90.0, 0.0));
        app.insert_resource(grids);

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyArmySeen {
                    size: 4,
                    within_s: None,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "four scattered units are not a force of four"
        );

        // Three IS enough, and the sighting is 30s old.
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemyArmySeen {
            size: 3,
            within_s: None,
        };
        advance(&mut app, 30.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "a known army, however old");

        // The same rule, now demanding a sighting from the last ten seconds.
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemyArmySeen {
            size: 3,
            within_s: Some(10.0),
        };
        advance(&mut app, 5.0);
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "35s of memory is not a current army; `within_s` is how you say so"
        );
    }

    /// `enemy_hero_down` is a LEVEL predicate over a belief, not an edge over
    /// an event. `Unknown` and `Alive` are both "not down", and only a death
    /// this team WATCHED sets the belief.
    #[test]
    fn enemy_hero_down_fires_on_the_belief_and_only_on_the_belief() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyHeroDown { class: None },
                stop_intent(),
                Some(1.0),
            ));

        // Unknown: never met. NOT the same as dead, and the commonest way to
        // get this wrong would be to treat an empty belief as a true one.
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "never having seen their hero is not knowing it is dead"
        );

        // Alive.
        let mut grids = FogGrids::test_dark();
        grids.test_hero_intel(Team::Human, UnitKind::Hero, HeroStatus::Alive, Vec3::ZERO);
        app.insert_resource(grids);
        advance(&mut app, 5.0);
        app.update();
        assert!(fired(&mut app).is_empty(), "alive is not down");

        // Watched it die.
        let mut grids = FogGrids::test_dark();
        grids.test_hero_intel(
            Team::Human,
            UnitKind::Hero,
            HeroStatus::SeenDying,
            Vec3::ZERO,
        );
        app.insert_resource(grids);
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "we watched it fall");
    }

    #[test]
    fn enemy_hero_down_can_name_one_class() {
        let mut app = trigger_app();
        let mut grids = FogGrids::test_dark();
        // Their Champion is down; their Priestess is alive and well.
        grids.test_hero_intel(
            Team::Human,
            UnitKind::Hero,
            HeroStatus::SeenDying,
            Vec3::ZERO,
        );
        grids.test_hero_intel(
            Team::Human,
            UnitKind::Priestess,
            HeroStatus::Alive,
            Vec3::ZERO,
        );
        app.insert_resource(grids);

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyHeroDown {
                    class: Some("Priestess".to_string()),
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "a rule about their Priestess must not fire on their Champion"
        );

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemyHeroDown {
            class: Some("Hero".to_string()),
        };
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1);
    }

    /// The documented once-vs-repeating interaction, pinned. The predicate
    /// stays TRUE for as long as the belief stands, so a `once` rule fires
    /// exactly one time (the edge a commander means by "when their hero
    /// falls") and a repeating one keeps going while they have no hero.
    #[test]
    fn a_once_rule_on_a_standing_belief_still_fires_exactly_once() {
        let mut app = trigger_app();
        let mut grids = FogGrids::test_dark();
        grids.test_hero_intel(
            Team::Human,
            UnitKind::Hero,
            HeroStatus::SeenDying,
            Vec3::ZERO,
        );
        app.insert_resource(grids);
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyHeroDown { class: None },
                stop_intent(),
                None,
            ));

        app.update();
        assert_eq!(fired(&mut app).len(), 1);
        for _ in 0..4 {
            advance(&mut app, 30.0);
            app.update();
            assert!(
                fired(&mut app).is_empty(),
                "the belief still holds, but the rule is spent"
            );
        }
    }

    // -- the other predicates ---------------------------------------------

    #[test]
    fn base_under_attack_means_our_buildings_and_only_recently() {
        let mut app = trigger_app();
        let hall = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::TownHall },
                Team::Human,
                Transform::from_xyz(-70.0, 0.0, -70.0),
                Health::new(1500.0),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::BaseUnderAttack, stop_intent(), Some(1.0)));

        app.update();
        assert!(fired(&mut app).is_empty(), "an unhurt base is not under attack");

        advance(&mut app, 10.0);
        let now = app.world().resource::<Time>().elapsed_secs();
        app.world_mut()
            .entity_mut(hall)
            .insert(LastDamaged { at: now });
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "hit just now");

        // Walk past the window with no further damage and it goes quiet.
        advance(&mut app, BASE_ATTACK_WINDOW_S + 1.0);
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "a raid repelled a minute ago must stop arming the rule"
        );
    }

    #[test]
    fn base_under_attack_ignores_units_and_the_enemys_buildings() {
        let mut app = trigger_app();
        // Our unit bleeding in midfield.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Footman },
            Team::Human,
            Transform::from_xyz(0.0, 0.0, 0.0),
            Health::new(100.0),
            LastDamaged { at: 0.0 },
        ));
        // Their building burning.
        app.world_mut().spawn((
            Building { kind: BuildingKind::TownHall },
            Team::Claude,
            Transform::from_xyz(70.0, 0.0, 70.0),
            Health::new(1500.0),
            LastDamaged { at: 0.0 },
        ));
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::BaseUnderAttack, stop_intent(), Some(1.0)));
        app.update();
        assert!(fired(&mut app).is_empty());
    }

    #[test]
    fn hero_below_reads_any_of_our_heroes() {
        let mut app = trigger_app();
        let hero = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Transform::default(),
                Health { current: 100.0, max: 100.0 },
                Hero { level: 1, xp: 0.0, mana: 80.0 },
            ))
            .id();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::HeroBelow { frac: 0.35 },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(fired(&mut app).is_empty(), "a healthy hero is not in trouble");

        app.world_mut().entity_mut(hero).insert(Health {
            current: 34.0,
            max: 100.0,
        });
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1);
    }

    /// **`hero_above` is a wait-condition, not a negation.** The predicate
    /// "turtle until the hero is healed" needs, and the three cases that make it
    /// different from `not hero_below` — each of which is a way a chain could
    /// advance at the worst possible moment.
    #[test]
    fn hero_above_is_healed_and_a_dead_hero_is_not_healed() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::HeroAbove { frac: 0.8 },
                stop_intent(),
                Some(1.0),
            ));

        // 1. NO hero. `not hero_below` would be true here and would release a
        //    chain over a corpse; this must not.
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "a hero you do not have is not a hero that is healed"
        );

        let hero = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Transform::default(),
                Health { current: 30.0, max: 100.0 },
                Hero { level: 1, xp: 0.0, mana: 80.0 },
            ))
            .id();
        advance(&mut app, 5.0);
        app.update();
        assert!(fired(&mut app).is_empty(), "30% is not healed");

        app.world_mut().entity_mut(hero).insert(Health {
            current: 85.0,
            max: 100.0,
        });
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "85% is at or above 80%");

        // 2. ALL, not any. A fresh second hero must not release the wait while
        //    the first one is still crawling home.
        let second = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Priestess },
                Team::Human,
                Transform::default(),
                Health { current: 20.0, max: 100.0 },
                Hero { level: 1, xp: 0.0, mana: 80.0 },
            ))
            .id();
        advance(&mut app, 5.0);
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "one hero at 20% means the roster is not ready"
        );

        // 3. Ours, not theirs — the same rule every other predicate obeys.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Hero },
            Team::Claude,
            Transform::default(),
            Health { current: 5.0, max: 100.0 },
            Hero { level: 1, xp: 0.0, mana: 80.0 },
        ));
        app.world_mut().entity_mut(second).insert(Health {
            current: 90.0,
            max: 100.0,
        });
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(
            fired(&mut app).len(),
            1,
            "both of OUR heroes are up; theirs is not our question"
        );
    }

    /// With at least one hero alive the pair really are complements, and with
    /// none alive both are false. Stated as a test because it is the property
    /// the doc on `TriggerWhen::HeroAbove` promises, and the one a future
    /// "simplification" into `!hero_below` would break.
    #[test]
    fn hero_above_and_hero_below_partition_a_living_roster() {
        let mut app = trigger_app();
        let hero = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Transform::default(),
                Health { current: 100.0, max: 100.0 },
                Hero { level: 1, xp: 0.0, mana: 80.0 },
            ))
            .id();
        app.update();

        for current in [5.0f32, 49.0, 50.0, 99.0, 100.0] {
            app.world_mut().entity_mut(hero).insert(Health {
                current,
                max: 100.0,
            });
            advance(&mut app, 1.0);
            app.update();
            let (below, above) = probe(&mut app, 0.5);
            assert_ne!(
                below, above,
                "at {current}% exactly one of below/above 50% holds"
            );
        }

        app.world_mut().despawn(hero);
        advance(&mut app, 1.0);
        app.update();
        let (below, above) = probe(&mut app, 0.5);
        assert!(
            !below && !above,
            "with no hero at all, neither question has a yes"
        );
    }

    /// Ask both hero predicates about the live world, through the real
    /// evaluator: two rules, one sweep, and which of them fired is the answer.
    fn probe(app: &mut App, frac: f32) -> (bool, bool) {
        {
            let mut triggers = app.world_mut().resource_mut::<Triggers>();
            let list = triggers.get_mut(Team::Human);
            list.clear();
            let mut below = armed(TriggerWhen::HeroBelow { frac }, stop_intent(), Some(0.1));
            below.name = name("below");
            list.push(below);
            let mut above = armed(TriggerWhen::HeroAbove { frac }, stop_intent(), Some(0.1));
            above.name = name("above");
            list.push(above);
        }
        advance(app, 1.0);
        app.update();
        let names: Vec<String> = fired(app)
            .iter()
            .map(|s| s.trigger.unwrap().as_str().to_string())
            .collect();
        (
            names.iter().any(|n| n == "below"),
            names.iter().any(|n| n == "above"),
        )
    }

    #[test]
    fn squad_below_pools_the_squads_health() {
        let mut app = trigger_app();
        // Four footmen in squad 1: one nearly dead, three untouched. Pooled,
        // the squad is at 77% and is NOT in trouble — which is the whole point
        // of pooling rather than asking "is any member hurt".
        let mut ids = Vec::new();
        for _ in 0..4 {
            ids.push(
                app.world_mut()
                    .spawn((
                        Unit { kind: UnitKind::Footman },
                        Team::Human,
                        Transform::default(),
                        Health::new(100.0),
                        SquadId(1),
                    ))
                    .id(),
            );
        }
        app.world_mut().entity_mut(ids[0]).insert(Health {
            current: 10.0,
            max: 100.0,
        });
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::SquadBelow { id: 1, frac: 0.5 },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(fired(&mut app).is_empty(), "310/400 is not below half");

        for id in &ids[1..] {
            app.world_mut().entity_mut(*id).insert(Health {
                current: 40.0,
                max: 100.0,
            });
        }
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "130/400 is");
    }

    #[test]
    fn squad_below_is_false_for_a_squad_that_no_longer_exists() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::SquadBelow { id: 3, frac: 0.9 },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "a squad that is gone cannot be hurt — firing a rescue at a corpse \
             pile is worse than firing nothing"
        );
    }

    #[test]
    fn tier_and_unit_count_and_clock() {
        let mut app = trigger_app();
        {
            let mut triggers = app.world_mut().resource_mut::<Triggers>();
            let list = triggers.get_mut(Team::Human);
            let mut tier = armed(TriggerWhen::TierReached { tier: 2 }, stop_intent(), Some(1.0));
            tier.name = name("tier");
            list.push(tier);
            let mut count = armed(
                TriggerWhen::UnitCount {
                    kind: "Footman".to_string(),
                    count: 2,
                },
                stop_intent(),
                Some(1.0),
            );
            count.name = name("count");
            list.push(count);
            let mut clock = armed(TriggerWhen::GameTime { at: 300.0 }, stop_intent(), Some(1.0));
            clock.name = name("clock");
            list.push(clock);
        }
        app.update();
        assert!(fired(&mut app).is_empty(), "T1, no army, second zero");

        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(Team::Human, TechTier::T2);
        for _ in 0..2 {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::default(),
                Health::new(100.0),
            ));
        }
        advance(&mut app, 301.0);
        app.update();
        let names: Vec<String> = fired(&mut app)
            .iter()
            .map(|s| s.trigger.unwrap().as_str().to_string())
            .collect();
        assert_eq!(names, vec!["tier", "count", "clock"]);
    }

    /// **`supply_capped` counts the queue, and refuses to fire on an empty
    /// board.** The predicate arena round 17 asked for, and the two things it
    /// has to get right to be worth arming.
    ///
    /// BLUE lost that match sitting at 28/28 with 2280 gold banked. The number
    /// was in every snapshot and nothing said it out loud; this is the rule
    /// that says it. Counting production is what makes it fire AT the stall
    /// rather than after it: four Footmen queued into two free supply is a
    /// team that has already stopped, because economy.rs will not pay for a
    /// front item whose supply does not fit.
    #[test]
    fn supply_capped_counts_the_queue_and_ignores_an_empty_board() {
        let mut app = trigger_app();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::SupplyCapped, stop_intent(), Some(1.0)));

        // Frame one: no supply buildings, so cap is 0 and headroom is 0 — and
        // the rule must NOT fire. "No base" is not "supply blocked", and a
        // predicate that could not tell them apart would fire before the match
        // began, every match, for everyone.
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "a zero cap is 'no economy yet', not 'capped'"
        );

        // A real economy with room to grow.
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let eco = economies.get_mut(Team::Human);
            eco.supply_cap = 12;
            eco.supply_used = 8;
        }
        advance(&mut app, 2.0);
        app.update();
        assert!(fired(&mut app).is_empty(), "4 free supply is not capped");

        // Four Footmen (2 supply each) queued into 4 free supply. The ledger
        // still reads 8/12 — nothing has been born — but the team is done
        // producing, and THAT is the moment worth telling a commander about.
        let barracks = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Human,
                Transform::from_xyz(0.0, 0.0, 0.0),
                Health::new(700.0),
                TrainingQueue::default(),
            ))
            .id();
        {
            let mut queue = app
                .world_mut()
                .entity_mut(barracks)
                .into_mut::<TrainingQueue>()
                .expect("just spawned with one");
            for _ in 0..2 {
                queue.queue.push_back(UnitKind::Footman);
            }
        }
        assert_eq!(
            unit_stats(UnitKind::Footman).supply * 2,
            4,
            "the arithmetic this test rests on"
        );
        advance(&mut app, 2.0);
        app.update();
        assert_eq!(
            fired(&mut app).len(),
            1,
            "queued supply fills the headroom, so the alarm goes off"
        );

        // The other team's queue is not our problem, and neither is their
        // ledger: each seat is asked about its own row.
        let claude_fired = {
            let mut app2 = trigger_app();
            app2.world_mut()
                .resource_mut::<Triggers>()
                .get_mut(Team::Claude)
                .push(armed(TriggerWhen::SupplyCapped, stop_intent(), Some(1.0)));
            {
                let mut economies = app2.world_mut().resource_mut::<Economies>();
                let human = economies.get_mut(Team::Human);
                human.supply_cap = 12;
                human.supply_used = 12;
                let claude = economies.get_mut(Team::Claude);
                claude.supply_cap = 40;
                claude.supply_used = 10;
            }
            app2.update();
            advance(&mut app2, 2.0);
            app2.update();
            fired(&mut app2).len()
        };
        assert_eq!(claude_fired, 0, "their cap is theirs; ours is ours");

        // And a farm going up unblocks it: raise the cap and the rule stops
        // holding, which is what makes it safe to repeat.
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            economies.get_mut(Team::Human).supply_cap = 24;
        }
        advance(&mut app, 2.0);
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "the farm finished, so we are not capped any more"
        );
    }

    #[test]
    fn mine_dry_means_a_dead_mine_next_to_one_of_our_halls() {
        let mut app = trigger_app();
        let mine = app
            .world_mut()
            .spawn((
                ResourceNode {
                    kind: ResourceKind::Gold,
                    remaining: 500,
                },
                Transform::from_xyz(-60.0, 0.0, -60.0),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::MineDry, stop_intent(), Some(1.0)));

        // No hall yet: a dry mine on the far side of the map is not our
        // problem, and there is nothing to measure "near" against.
        app.world_mut().entity_mut(mine).insert(ResourceNode {
            kind: ResourceKind::Gold,
            remaining: 0,
        });
        app.update();
        assert!(fired(&mut app).is_empty(), "no hall, no home mine");

        app.world_mut().spawn((
            Building { kind: BuildingKind::TownHall },
            Team::Human,
            Transform::from_xyz(-70.0, 0.0, -70.0),
            Health::new(1500.0),
        ));
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "our hall works that mine");
    }

    #[test]
    fn bounty_spawned_is_fog_honest_too() {
        let mut app = trigger_app();
        let spot = Vec3::new(0.0, 0.0, 0.0);
        app.world_mut().spawn((
            Bounty {
                gold: 200,
                expires_at: 999.0,
            },
            Transform::from_translation(spot),
        ));
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(TriggerWhen::BountySpawned, stop_intent(), Some(1.0)));
        app.update();
        assert!(fired(&mut app).is_empty(), "treasure you cannot see");

        app.insert_resource(FogGrids::test_revealed());
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1);
    }

    // -- the whole loop ----------------------------------------------------

    /// The headline: arm a home guard, hit the base, and the squad is recalled
    /// by the engine with nobody at either keyboard. Runs the real plugin set,
    /// so the intent goes through the real compiler in the same frame it fired.
    #[test]
    fn the_base_is_attacked_and_the_home_guard_recalls_the_squad() {
        let mut app = full_app();
        let hall = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::TownHall },
                Team::Human,
                Transform::from_xyz(-70.0, 0.0, -70.0),
                Health::new(1500.0),
            ))
            .id();
        // A squad-1 footman out in midfield, doing something else.
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_xyz(0.0, 0.0, 0.0),
                Health::new(100.0),
                SquadId(1),
                Order::Idle,
            ))
            .id();

        // Arm it exactly as a commander would: one intent, through the wire.
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: serde_json::from_str(
                r#"{"type":"trigger_set","name":"home-guard",
                    "when":{"type":"base_under_attack"},
                    "then":{"type":"posture","id":1,
                            "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":22.0}}}"#,
            )
            .expect("the recipe in COMMANDER_BRIEF.md parses"),
            trigger: None,
            plan: None,
        });
        app.update();
        assert_eq!(
            app.world().resource::<Triggers>().get(Team::Human).len(),
            1,
            "armed"
        );
        assert!(
            !app.world()
                .resource::<SquadOrders>()
                .0
                .contains_key(&(Team::Human, 1)),
            "and nothing has happened yet — a trigger is not its action"
        );

        // Now the raid.
        advance(&mut app, 10.0);
        let now = app.world().resource::<Time>().elapsed_secs();
        app.world_mut()
            .entity_mut(hall)
            .insert(LastDamaged { at: now });
        app.update();

        let posture = app
            .world()
            .resource::<SquadOrders>()
            .0
            .get(&(Team::Human, 1))
            .copied();
        assert!(
            matches!(posture, Some(SquadPosture::Defend { .. })),
            "the engine recalled squad 1 to the base, in the frame the rule fired"
        );
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .any(|e| e.message.starts_with("trigger home-guard fired:")),
            "and said so, in both renderers"
        );
        // The trigger is spent, not deleted: the seat can still see the rule it
        // set and that it went off.
        let triggers = app.world().resource::<Triggers>();
        assert_eq!(triggers.get(Team::Human)[0].status(now), "spent");
        // Sanity: the soldier is still ours and still in squad 1 — the posture
        // executor takes it from here.
        assert_eq!(*app.world().entity(soldier).get::<SquadId>().unwrap(), SquadId(1));
    }

    // -- enemy_in: the territorial predicate -------------------------------

    /// Put a region on the evaluator's world the way a commander does not —
    /// directly — because these tests are about the PREDICATE. The compiler's
    /// half (arm-time name validation) is tested in intent.rs.
    fn name_region(app: &mut App, team: Team, name: &str, center: Vec3, radius: f32) {
        app.world_mut()
            .resource_mut::<Regions>()
            .set(team, Region::new(name, center, radius))
            .expect("the test's own region is legal");
    }

    fn enemy_at(app: &mut App, spot: Vec3, kind: UnitKind) {
        app.world_mut().spawn((
            Unit { kind },
            Team::Claude,
            Transform::from_translation(spot),
            Health::new(100.0),
        ));
    }

    /// **Both filters, and neither is optional.** An enemy inside the circle
    /// but unseen does not count; an enemy seen but outside it does not count.
    /// The rule only fires on bodies that are in the place AND visible, which
    /// is what makes a region a piece of ground you are watching rather than a
    /// free sensor bolted to the map.
    #[test]
    fn enemy_in_counts_only_what_is_both_inside_and_seen() {
        let mut app = trigger_app();
        name_region(&mut app, Team::Human, "north-pass", Vec3::new(-60.0, 0.0, 60.0), 20.0);
        // One enemy inside the circle, one far outside it.
        enemy_at(&mut app, Vec3::new(-58.0, 0.0, 62.0), UnitKind::Footman);
        enemy_at(&mut app, Vec3::new(60.0, 0.0, -60.0), UnitKind::Footman);
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyIn {
                    region: "north-pass".to_string(),
                    class: None,
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));

        // Dark: the enemy is standing in the pass and the rule does not know.
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "unseen is unknown, for a territorial rule exactly as for any other"
        );

        // Light the map and the IDENTICAL world fires it. The only thing that
        // changed is what the arming team was shown.
        app.insert_resource(FogGrids::test_revealed());
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "seen and inside is known");

        // Two enemies are visible and only ONE is in the pass, so a threshold
        // of two must stay quiet — proof the circle is really filtering rather
        // than the fog doing all the work.
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemyIn {
            region: "north-pass".to_string(),
            class: None,
            count: 2,
        };
        advance(&mut app, 5.0);
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "the enemy across the map is not in the pass"
        );
    }

    /// The boundary is the circle's own `contains`, so a body exactly on the
    /// edge is in and one just past it is out. Asserted because "roughly near"
    /// is the failure mode a commander would never be able to debug.
    #[test]
    fn enemy_in_measures_the_circle_and_not_a_neighbourhood() {
        let mut app = trigger_app();
        app.insert_resource(FogGrids::test_revealed());
        name_region(&mut app, Team::Human, "ring", Vec3::ZERO, 10.0);
        // Exactly on the rim.
        enemy_at(&mut app, Vec3::new(10.0, 0.0, 0.0), UnitKind::Footman);
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyIn {
                    region: "ring".to_string(),
                    class: None,
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "the rim is inside");

        // A second world, a whisker further out.
        let mut app = trigger_app();
        app.insert_resource(FogGrids::test_revealed());
        name_region(&mut app, Team::Human, "ring", Vec3::ZERO, 10.0);
        enemy_at(&mut app, Vec3::new(10.5, 0.0, 0.0), UnitKind::Footman);
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyIn {
                    region: "ring".to_string(),
                    class: None,
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(fired(&mut app).is_empty(), "just outside is outside");
    }

    /// The class filter is the one `enemy_sighted` uses, so "5 siege in
    /// north-pass" means what it says.
    #[test]
    fn enemy_in_counts_only_the_named_class() {
        let mut app = trigger_app();
        app.insert_resource(FogGrids::test_revealed());
        name_region(&mut app, Team::Human, "ring", Vec3::ZERO, 20.0);
        for _ in 0..3 {
            enemy_at(&mut app, Vec3::new(2.0, 0.0, 2.0), UnitKind::Footman);
        }
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyIn {
                    region: "ring".to_string(),
                    class: Some("Siege".to_string()),
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert!(fired(&mut app).is_empty(), "three footmen are not a siege train");

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)[0]
            .when = TriggerWhen::EnemyIn {
            region: "ring".to_string(),
            class: Some("Footman".to_string()),
            count: 3,
        };
        advance(&mut app, 5.0);
        app.update();
        assert_eq!(fired(&mut app).len(), 1);
    }

    /// A region cleared out from under an armed rule makes it go QUIET rather
    /// than fall back to the whole map. An unresolvable name is no question,
    /// not a bigger one.
    #[test]
    fn a_rule_whose_region_was_forgotten_stops_asking() {
        let mut app = trigger_app();
        app.insert_resource(FogGrids::test_revealed());
        name_region(&mut app, Team::Human, "ring", Vec3::ZERO, 20.0);
        enemy_at(&mut app, Vec3::new(1.0, 0.0, 1.0), UnitKind::Footman);
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(armed(
                TriggerWhen::EnemyIn {
                    region: "ring".to_string(),
                    class: None,
                    count: 1,
                },
                stop_intent(),
                Some(1.0),
            ));
        app.update();
        assert_eq!(fired(&mut app).len(), 1, "armed and true");

        app.world_mut()
            .resource_mut::<Regions>()
            .clear(Team::Human, "ring");
        advance(&mut app, 5.0);
        app.update();
        assert!(
            fired(&mut app).is_empty(),
            "no place, no question — firing a defence of nowhere is worse than not firing"
        );
    }

    /// A region is per-team, so the two seats' identically-named circles are
    /// different ground and each rule reads its OWN.
    #[test]
    fn each_seat_reads_its_own_vocabulary() {
        let mut app = trigger_app();
        app.insert_resource(FogGrids::test_revealed());
        // Both teams name "the-spot", at opposite corners.
        name_region(&mut app, Team::Human, "the-spot", Vec3::new(-70.0, 0.0, -70.0), 15.0);
        name_region(&mut app, Team::Claude, "the-spot", Vec3::new(70.0, 0.0, 70.0), 15.0);
        // One Claude unit sitting in the HUMAN's spot.
        enemy_at(&mut app, Vec3::new(-70.0, 0.0, -70.0), UnitKind::Footman);
        for team in [Team::Human, Team::Claude] {
            app.world_mut()
                .resource_mut::<Triggers>()
                .get_mut(team)
                .push(armed(
                    TriggerWhen::EnemyIn {
                        region: "the-spot".to_string(),
                        class: None,
                        count: 1,
                    },
                    stop_intent(),
                    Some(1.0),
                ));
        }
        app.update();
        let fired = fired(&mut app);
        assert_eq!(fired.len(), 1, "exactly one seat's rule is true");
        assert_eq!(
            fired[0].team,
            Team::Human,
            "the human's spot is the one with an enemy standing in it"
        );
    }

    /// The predicate reads as English, with the place in it. This is the line
    /// that lands in the event feed and the snapshot when the rule fires.
    #[test]
    fn enemy_in_says_where() {
        assert_eq!(
            TriggerWhen::EnemyIn {
                region: "north-pass".to_string(),
                class: None,
                count: 5,
            }
            .phrase(),
            "5 or more enemies are seen in north-pass"
        );
        assert_eq!(
            TriggerWhen::EnemyIn {
                region: "mid".to_string(),
                class: Some("Siege".to_string()),
                count: 1,
            }
            .phrase(),
            "any enemy Siege are seen in mid"
        );
    }

    /// **The whole loop, territorially.** Name a pass, arm a watch on it, walk
    /// an army in, and the squad is defending it before anybody at either
    /// keyboard has read an event — through the real compiler, in the frame the
    /// rule fired, with a sentence a person can read.
    #[test]
    fn five_enemies_enter_the_pass_and_the_squad_is_already_there() {
        let mut app = full_app();
        app.insert_resource(FogGrids::test_revealed());
        let pass = Vec3::new(-60.0, 0.0, 60.0);

        // Squad 2, out in midfield doing something else.
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_xyz(0.0, 0.0, 0.0),
                Health::new(100.0),
                SquadId(2),
                Order::Idle,
            ))
            .id();

        // Both sentences through the wire, exactly as COMMANDER_BRIEF.md
        // spells them.
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: serde_json::from_str(
                r#"{"type":"region_set","name":"north-pass","x":-60.0,"z":60.0,"radius":20.0}"#,
            )
            .expect("the recipe parses"),
            trigger: None,
            plan: None,
        });
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 1".to_string(),
            intent: serde_json::from_str(
                r#"{"type":"trigger_set","name":"pass-watch",
                    "when":{"type":"enemy_in","region":"north-pass","count":5},
                    "then":{"type":"posture","id":2,
                            "posture":{"type":"defend","region":"north-pass"}},
                    "repeat":30.0}"#,
            )
            .expect("the recipe in COMMANDER_BRIEF.md parses"),
            trigger: None,
            plan: None,
        });
        app.update();
        assert_eq!(app.world().resource::<Triggers>().get(Team::Human).len(), 1, "armed");
        assert!(
            !app.world()
                .resource::<SquadOrders>()
                .0
                .contains_key(&(Team::Human, 2)),
            "and nothing has happened yet — a trigger is not its action"
        );

        // Four is not five: the rule is armed and the world is not yet its
        // condition.
        for i in 0..4 {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_translation(pass + Vec3::new(i as f32, 0.0, 0.0)),
                Health::new(100.0),
            ));
        }
        advance(&mut app, 5.0);
        app.update();
        assert!(
            !app.world()
                .resource::<SquadOrders>()
                .0
                .contains_key(&(Team::Human, 2)),
            "four in the pass is not the rule the commander wrote"
        );

        // The fifth arrives.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Footman },
            Team::Claude,
            Transform::from_translation(pass + Vec3::new(4.0, 0.0, 0.0)),
            Health::new(100.0),
        ));
        advance(&mut app, 5.0);
        app.update();

        match app.world().resource::<SquadOrders>().0.get(&(Team::Human, 2)) {
            Some(SquadPosture::Defend { pos, radius }) => {
                assert_eq!(*pos, pass, "the squad is defending the pass it was named for");
                assert_eq!(*radius, 20.0, "at the region's own radius");
            }
            other => panic!("expected squad 2 to be defending the pass, got {other:?}"),
        }
        // ...and it said so, in English, with the place named rather than
        // spelled in floats.
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .any(|e| e.message == "trigger pass-watch fired: squad 2 defends north-pass"),
            "feed was {:?}",
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(*app.world().entity(soldier).get::<SquadId>().unwrap(), SquadId(2));
    }
}
