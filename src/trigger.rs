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
//! systems in one set unordered unless something forces an edge. Nothing does
//! here, and nothing needs to: this system's only writes are `ResMut<Triggers>`
//! (nobody else touches it), `ResMut<GameEvents>` (no other writer lives in
//! `Think`) and `EventWriter<SubmitIntent>` (same). Everything it reads is
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
    tiers: Res<'w, TechTiers>,
    fog: Res<'w, FogGrids>,
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
/// Every arm is a fold over state the frame already has. Nothing here writes,
/// remembers, or subscribes: a predicate that needed its own bookkeeping would
/// be a predicate whose truth could drift from the world, and the whole value
/// of firing at machine speed is that the world is what fired it.
fn holds(when: &TriggerWhen, me: Team, now: f32, world: &TriggerWorld) -> bool {
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
fn evaluate_triggers(
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
            .init_resource::<TechTiers>()
            .init_resource::<GameEvents>()
            .add_event::<SubmitIntent>()
            .add_systems(Update, evaluate_triggers);
        // Pin the fog mode rather than inheriting `WC3_FOG`: two predicates
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
        Intent::Stop { units: vec![] }
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
}
