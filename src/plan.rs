//! plan.rs — `then` as a first-class word.
//!
//! doctrine.rs runs **continuous** standing policy; trigger.rs runs the
//! **contingent** half. Neither of them can say ORDER, and order is what a
//! build order is. This module runs the **sequenced** third: a named list of
//! steps the engine walks on the commander's behalf, one at a time, submitting
//! each step's intent through the ordinary compiler when its turn comes.
//!
//! ## Why this is the same argument as triggers, one rung along
//!
//! THESIS.md's tempo answer is "relocate fast work into the game itself".
//! Doctrine did it for the work that never stops; triggers did it for reaction.
//! What neither covered was the work that is *already decided* — "barracks,
//! then keep, then sanctum, then sorcerers" is a sequence a commander settles
//! before the match starts and then spends the first six minutes hand-feeding
//! to the engine one command per poll. For a language model that is ten to
//! fifteen seconds *per step* of a sequence with no decisions left in it. A
//! human at a keyboard pays a keystroke. That gap is not judgment either; it is
//! transcription, and this module deletes it for whichever player wrote the
//! plan down.
//!
//! ## What this module is allowed to do
//!
//! Exactly what trigger.rs is allowed to do: write [`SubmitIntent`]. It mints
//! no `Order`, spends nothing, and moves nothing. A plan step is an ordinary
//! intent through the ordinary compiler — same validation, same ownership
//! checks, same fog rule, same replay log, same error channel. The only things
//! that mark it out are [`SubmitIntent::plan`] and the tag `plan:<name>#k`.
//!
//! ## Where it sits in the frame
//!
//! `SimSet::Think`, after `FogSet`, at 4 Hz, `.before(trigger::evaluate_triggers)`.
//! The first three are trigger.rs's reasoning verbatim and for the same
//! reasons — after `Deaths` so a predicate cannot count a corpse, after `Fog` so
//! `enemy_sighted` is fog-honest, before `SimSet::Intent` so a step submitted
//! this tick is compiled this tick.
//!
//! The fourth is new and it is a **determinism edge, not a preference**. Both
//! this system and `evaluate_triggers` write `GameEvents` and `SubmitIntent`,
//! and Bevy leaves two systems in one set unordered unless something forces an
//! edge. Something has to, because `Order` is a component and last writer wins:
//! on a tick where a plan step and a trigger both name the same squad, whichever
//! submitted *later* is what the squad does.
//!
//! Plans go **first**, so a trigger lands **last** and wins. That is the right
//! ranking and it is the same one trigger.rs already argued for against
//! doctrine: a trigger is a rule written for the exact situation that just
//! occurred, and a plan is a sequence written before the match for the general
//! case. If your opening says "push mid" on the same tick your home guard says
//! "the base is burning, come home", the base is burning.
//!
//! ## Failure semantics: blocked, then halted, never skipped
//!
//! A step's intent is frozen at `plan_set` time and compiled when it runs, so
//! it can be refused — the gold is short, the building is still going up, the
//! worker died. The engine has three honest options and only one of them is
//! defensible:
//!
//!   * **Skip the step and carry on.** Refused. A plan that quietly drops the
//!     Blacksmith and goes on to research at it is worse than a plan that
//!     stopped, because the commander reads "running" and believes the sequence
//!     they wrote is the sequence that ran.
//!   * **Halt immediately.** Too brittle. Most refusals are *timing*: forty
//!     gold short, a worker mid-walk, a hall one tick from finishing. Halting
//!     on those would make plans useless for exactly the economic sequencing
//!     they exist for.
//!   * **Block, retry, then halt.** What this does. The plan stops advancing,
//!     its status becomes `blocked: <the compiler's own error>`, it re-submits
//!     the same step every [`PLAN_RETRY_S`], and if it is still refused after
//!     [`PLAN_BLOCK_GRACE_S`] it becomes `halted: <error>` and stops for good —
//!     on the step that failed, which is where a reader needs to find it.
//!
//! Both states carry the compiler's verbatim string, and both are announced on
//! the owner's event feed. Nothing is ever skipped, and a plan never lies about
//! where it is.
//!
//! **What counts as refused is narrower than "produced an error".** The
//! compiler routinely reports a dead id and orders the survivors anyway —
//! `own_units` is built that way on purpose — so `move [a,b]` with `b` a corpse
//! *moves a* and still pushes an error. Treating that as a refusal made a plan
//! block on the most ordinary event in the game, retry an order it had already
//! carried out, and then halt a sequence that was running correctly. So
//! `compile_intent` reports whether it **reached** anything, and only a step
//! that reached nothing blocks. The error still goes to every other channel;
//! what changes is whether the plan stops for it.
//!
//! ## Once through
//!
//! A plan does not loop. Repetition is a trigger's `repeat`, and a construct
//! with sequencing *and* iteration is a programming language with no debugger —
//! the exact thing the caps in shared.rs exist to refuse.

use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use std::time::Duration;

use crate::shared::*;
use crate::trigger::{holds, TriggerWorld};

/// Plan step cadence (~4 Hz). The same heartbeat as the trigger evaluator, and
/// deliberately so: a plan's `when` advance-conditions ARE trigger predicates,
/// and two rates would mean "we reached tier 2" became true at two different
/// moments depending on which construct asked.
const PLAN_MS: u64 = 250;

pub struct PlanPlugin;

impl Plugin for PlanPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            step_plans
                .run_if(on_timer(Duration::from_millis(PLAN_MS)))
                .in_set(SimSet::Think)
                .after(FogSet)
                // The determinism edge reasoned out in the module docs: plans
                // submit first so a trigger's answer to *this* situation lands
                // after a plan's answer to the general one, and wins.
                .before(crate::trigger::evaluate_triggers),
        );
    }
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

/// Walk both teams' plans one tick.
///
/// A plan is in exactly one of four situations on any sweep, and the body below
/// is those four in order: it has not submitted its current step yet; it is
/// blocked on a refusal; it is waiting for its advance-condition; or that
/// condition holds and it moves on.
fn step_plans(
    time: Res<Time>,
    mut plans: ResMut<Plans>,
    mut submissions: EventWriter<SubmitIntent>,
    mut feed: ResMut<GameEvents>,
    world: TriggerWorld,
) {
    let now = time.elapsed_secs();
    // Fixed team order and list order, for the reason `Triggers` is a `Vec`:
    // two plans can step on the same tick and the order they submit in has to
    // be the order they were written, identically on every run.
    for me in [Team::Human, Team::Claude] {
        // Decide first, mutate second — `holds` borrows the world immutably and
        // the body needs `&mut` on the resource. The index is stable because
        // nothing in this loop adds to or removes from the list.
        let count = plans.get(me).len();
        for index in 0..count {
            // Whether this plan's current advance-condition holds, computed
            // while the world borrow is alive.
            let ready = {
                let plan = &plans.get(me)[index];
                match (plan.live(), plan.applied, plan.current()) {
                    (true, true, Some(step)) => match &step.advance {
                        PlanAdvance::OnApplied => true,
                        PlanAdvance::When { when } => holds(when, me, now, &world),
                        PlanAdvance::AfterSeconds { secs } => now - plan.applied_at >= *secs,
                    },
                    _ => false,
                }
            };
            step_one(me, index, now, ready, &mut plans, &mut submissions, &mut feed);
        }
    }
}

/// One plan, one tick. Split out so the world borrow above ends before the
/// mutable one begins, and so the state machine reads as a list of cases
/// rather than as an indented block inside two loops.
fn step_one(
    me: Team,
    index: usize,
    now: f32,
    ready: bool,
    plans: &mut Plans,
    submissions: &mut EventWriter<SubmitIntent>,
    feed: &mut GameEvents,
) {
    let plan = &mut plans.get_mut(me)[index];
    if !plan.live() {
        return;
    }
    let of = plan.steps.len() as u8;
    let stamp = PlanStamp {
        name: plan.name,
        step: plan.step_no() as u8,
        of,
    };
    let Some(step) = plan.current().cloned() else {
        return;
    };

    // 1. The step has not gone out yet. Send it, and say so — both renderers,
    //    one producer, exactly as a fired trigger does. Without this line a
    //    plan would be the one thing in the game that changes the board every
    //    thirty seconds without ever saying what it just did.
    if !plan.submitted {
        submit(me, plan, stamp, step.intent, now, submissions, feed);
        return;
    }

    // 2. Refused. Retry on a slow clock inside the grace window, then give up
    //    ON THIS STEP — never past it.
    if let PlanState::Blocked(why) = plan.state.clone() {
        let since = *plan.blocked_since.get_or_insert(now);
        if !plan.told_blocked {
            plan.told_blocked = true;
            feed.push(
                me,
                now,
                format!("plan {stamp} blocked: {why}"),
                EventSeverity::Warning,
                None,
            );
        }
        if now - since >= PLAN_BLOCK_GRACE_S {
            plan.state = PlanState::Halted(why.clone());
            feed.push(
                me,
                now,
                format!("plan {stamp} halted: {why} — the rest of the plan will not run"),
                EventSeverity::Warning,
                None,
            );
            return;
        }
        if now - plan.last_try >= PLAN_RETRY_S {
            plan.last_try = now;
            submissions.write(SubmitIntent::plan_step(me, plan.source, stamp, step.intent));
        }
        return;
    }

    // 3. Out, accepted, and the advance-condition has not come true yet.
    if !ready {
        return;
    }

    // 4. Move on. Past the last step, the plan is finished — and stays in the
    //    list, `done`, for the reason a spent trigger does: "did my opening
    //    actually run?" has to be answerable from the snapshot.
    plan.at += 1;
    if plan.at >= plan.steps.len() {
        // PIN it to the last real index rather than leaving it one past the
        // end. `PlanRun::at` is public and documented as "the last index once
        // the plan is finished"; `step_no()` and `current()` clamp, but a
        // reader that indexed `steps[plan.at]` on a done plan would panic, and
        // an invariant that only holds because every reader remembers to clamp
        // is not an invariant.
        plan.at = plan.steps.len() - 1;
        plan.state = PlanState::Done;
        feed.push(
            me,
            now,
            format!("plan {} complete ({of} steps)", plan.name),
            EventSeverity::Info,
            None,
        );
        return;
    }
    plan.submitted = false;
    plan.applied = false;
    plan.applied_at = 0.0;
    plan.blocked_since = None;
    plan.told_blocked = false;
    // Send it NOW rather than on the next sweep. Waiting would charge a plan a
    // quarter-second of dead air per step for no reason - and worse, it would
    // make "then" mean something slightly different from what a commander who
    // sent the two commands by hand would have got.
    let next = PlanStamp {
        name: plan.name,
        step: plan.step_no() as u8,
        of,
    };
    let Some(step) = plan.current().cloned() else {
        return;
    };
    submit(me, plan, next, step.intent, now, submissions, feed);
}

/// Put one step's intent on the wire and tell its owner. The one place a plan
/// ever speaks, so the announcement can never drift from the submission.
fn submit(
    me: Team,
    plan: &mut PlanRun,
    stamp: PlanStamp,
    intent: Intent,
    now: f32,
    submissions: &mut EventWriter<SubmitIntent>,
    feed: &mut GameEvents,
) {
    plan.submitted = true;
    plan.last_try = now;
    feed.push(
        me,
        now,
        format!("plan {stamp}: {}", intent.sentence()),
        EventSeverity::Info,
        None,
    );
    submissions.write(SubmitIntent::plan_step(me, plan.source, stamp, intent));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandLatency, CommandNodes};
    use crate::intent::{IntentLog, IntentPlugin};

    /// The evaluator alone, with `Time` hand-driven and no `on_timer` — the
    /// idiom trigger.rs and doctrine.rs use, and for the same reason: the
    /// cadence is a tuning constant and a test that waited for it would be
    /// testing the clock.
    fn plan_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or the intent compiler (which reads it to gate
        // the `build` verb by roster) panics inside Bevy's worker pool and
        // the test HANGS or dies in `run_main` rather than failing cleanly.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<Plans>()
            // `TriggerWorld` reads it: a plan step may advance on `enemy_in`,
            // which asks about a named place.
            .init_resource::<Regions>()
            .init_resource::<TechTiers>()
            .init_resource::<GameEvents>()
            .add_event::<SubmitIntent>()
            .add_systems(Update, step_plans);
        app.insert_resource(FogGrids::test_dark());
        app
    }

    /// The evaluator and the real compiler in one app, so a test can set a plan
    /// the way a commander does and watch the engine walk it.
    fn full_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or the intent compiler (which reads it to gate
        // the `build` verb by roster) panics inside Bevy's worker pool and
        // the test HANGS or dies in `run_main` rather than failing cleanly.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<Triggers>()
            .init_resource::<Plans>()
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
            .add_systems(Update, step_plans.before(crate::intent::IntentApply));
        app.insert_resource(IntentLog::disabled());
        app.insert_resource(FogGrids::test_dark());
        app
    }

    fn name(s: &str) -> PlanName {
        PlanName::new(s).expect("test name is legal")
    }

    fn plan(steps: Vec<PlanStep>) -> PlanRun {
        PlanRun {
            name: name("p"),
            steps,
            source: IntentSource::Bridge,
            state: PlanState::Running,
            at: 0,
            submitted: false,
            applied: false,
            applied_at: 0.0,
            last_try: 0.0,
            blocked_since: None,
            told_blocked: false,
        }
    }

    fn step(intent: Intent, advance: PlanAdvance) -> PlanStep {
        PlanStep { intent, advance }
    }

    /// A step whose intent is trivially legal and names nothing.
    fn stop() -> Intent {
        Intent::Stop { units: vec![] }
    }

    fn sent(app: &mut App) -> Vec<SubmitIntent> {
        let mut events = app.world_mut().resource_mut::<Events<SubmitIntent>>();
        events.drain().collect()
    }

    fn advance_clock(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
    }

    /// Stand in for the compiler's verdict on whatever step each live plan has
    /// out right now. Deliberately reads the plans rather than the event queue,
    /// so a test may inspect the submissions AND answer them.
    fn verdict(app: &mut App, error: Option<&str>) {
        let now = app.world().resource::<Time>().elapsed_secs();
        let mut plans = app.world_mut().resource_mut::<Plans>();
        let stamps: Vec<PlanStamp> = plans
            .get(Team::Human)
            .iter()
            .filter(|p| p.live() && p.submitted)
            .map(|p| PlanStamp {
                name: p.name,
                step: p.step_no() as u8,
                of: p.steps.len() as u8,
            })
            .collect();
        for stamp in stamps {
            plans.report(Team::Human, stamp, error.map(str::to_string), now);
        }
    }

    fn accept(app: &mut App) {
        verdict(app, None);
    }

    fn refuse(app: &mut App, why: &str) {
        verdict(app, Some(why));
    }

    fn state(app: &App) -> PlanState {
        app.world().resource::<Plans>().get(Team::Human)[0]
            .state
            .clone()
    }

    fn at(app: &App) -> usize {
        app.world().resource::<Plans>().get(Team::Human)[0].at
    }

    // -- the three advance conditions -------------------------------------

    #[test]
    fn on_applied_advances_the_moment_the_step_is_accepted() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(stop(), PlanAdvance::OnApplied),
                step(stop(), PlanAdvance::OnApplied),
            ]));

        app.update();
        let first = sent(&mut app);
        assert_eq!(first.len(), 1, "the first sweep submits step 1");
        assert_eq!(first[0].plan.unwrap().step, 1);
        assert_eq!(at(&app), 0);

        // Re-run the sweep with no verdict: it must NOT advance on its own.
        advance_clock(&mut app, 1.0);
        app.update();
        assert_eq!(at(&app), 0, "an unanswered step is not an accepted one");
        assert!(sent(&mut app).is_empty(), "and nothing new goes out");

        accept(&mut app);
        advance_clock(&mut app, 1.0);
        app.update();
        assert_eq!(at(&app), 1, "accepted, so 'then' means now");
        let out = sent(&mut app);
        assert_eq!(out.len(), 1, "and step 2 went out on the same sweep");
        assert_eq!(out[0].plan.unwrap().step, 2);
    }

    #[test]
    fn a_when_step_waits_for_its_predicate_and_uses_the_trigger_vocabulary() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(
                    stop(),
                    PlanAdvance::When {
                        when: TriggerWhen::TierReached { tier: 2 },
                    },
                ),
                step(stop(), PlanAdvance::OnApplied),
            ]));

        app.update();
        sent(&mut app);
        accept(&mut app);
        for _ in 0..5 {
            advance_clock(&mut app, 10.0);
            app.update();
            assert_eq!(at(&app), 0, "still tier 1, so the plan waits");
            assert!(sent(&mut app).is_empty());
        }

        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(Team::Human, TechTier::T2);
        advance_clock(&mut app, 1.0);
        app.update();
        assert_eq!(at(&app), 1, "the keep is up, so the plan moves");
        assert_eq!(sent(&mut app).len(), 1);
    }

    #[test]
    fn an_after_step_waits_out_its_clock_from_acceptance() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(stop(), PlanAdvance::AfterSeconds { secs: 30.0 }),
                step(stop(), PlanAdvance::OnApplied),
            ]));

        app.update();
        // Ten seconds pass BEFORE the verdict lands, and they must not count:
        // the clock starts when the step was accepted, not when it was written.
        advance_clock(&mut app, 10.0);
        accept(&mut app);

        advance_clock(&mut app, 29.0);
        app.update();
        assert_eq!(at(&app), 0, "29s into a 30s wait");
        advance_clock(&mut app, 1.5);
        app.update();
        assert_eq!(at(&app), 1);
    }

    /// **The seam, proved on a predicate this file has never heard of.**
    ///
    /// `enemy_hero_down` arrived with the intel bead, after plans were written.
    /// plan.rs needed *no* change to accept it as an advance-condition — not a
    /// match arm, not a validation case, not a line. That is the whole point of
    /// `PlanAdvance::When` carrying a `TriggerWhen` rather than defining its own
    /// condition vocabulary: `trigger::holds` is the only thing that reads the
    /// enum and `intent::validate_predicate` the only thing that checks it, and
    /// neither lives here.
    ///
    /// This test exists to keep that true. If a future bead adds a predicate by
    /// special-casing it somewhere plans cannot see, this is what should fail.
    #[test]
    fn a_plan_advances_on_a_predicate_a_later_bead_added() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(
                    stop(),
                    PlanAdvance::When {
                        when: TriggerWhen::EnemyHeroDown { class: None },
                    },
                ),
                step(stop(), PlanAdvance::OnApplied),
            ]));

        app.update();
        accept(&mut app);
        sent(&mut app);
        for _ in 0..4 {
            advance_clock(&mut app, 10.0);
            app.update();
            assert_eq!(at(&app), 0, "their hero is alive, so the plan holds");
            assert!(sent(&mut app).is_empty());
        }

        // The intel ledger learns their hero is down — the same belief the
        // trigger evaluator reads, written the same way.
        let mut grids = FogGrids::test_dark();
        grids.test_hero_intel(
            Team::Human,
            UnitKind::Hero,
            HeroStatus::SeenDying,
            Vec3::new(20.0, 0.0, 20.0),
        );
        app.insert_resource(grids);

        advance_clock(&mut app, 1.0);
        app.update();
        assert_eq!(at(&app), 1, "and moves the moment the belief changes");
        assert_eq!(sent(&mut app).len(), 1, "step 2 went out");
    }

    /// **The territory seam.** A plan step advancing on `enemy_in` — a
    /// predicate a sibling bead added, over a vocabulary of PLACES a sibling
    /// bead added — with nothing in this file changed to allow it.
    ///
    /// That is the property worth a test rather than the feature: `PlanAdvance`
    /// carries a whole `TriggerWhen`, so every arm the predicate vocabulary
    /// grows becomes a way to sequence, for free and permanently. Here it buys
    /// something a commander actually wants and could not previously say:
    /// *hold, and when five of them are in the pass, commit* — one plan, no
    /// polling, and the pass named rather than spelled in floats.
    #[test]
    fn a_plan_advances_on_enemies_reaching_a_named_place() {
        let mut app = plan_app();
        // The place, named the way a commander names it.
        app.world_mut()
            .resource_mut::<Regions>()
            .set(
                Team::Human,
                Region::new("north-pass", Vec3::new(-60.0, 0.0, 60.0), 20.0),
            )
            .expect("the test's own region is legal");
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(
                    stop(),
                    PlanAdvance::When {
                        when: TriggerWhen::EnemyIn {
                            region: "north-pass".to_string(),
                            class: None,
                            count: 3,
                        },
                    },
                ),
                step(stop(), PlanAdvance::OnApplied),
            ]));

        app.update();
        accept(&mut app);
        sent(&mut app);

        // Two of them in the pass, and the map lit: fewer than the plan asked
        // for, so it holds. This also pins that the COUNT is doing work rather
        // than the region alone.
        app.insert_resource(FogGrids::test_revealed());
        for i in 0..2 {
            app.world_mut().spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_xyz(-60.0 + i as f32, 0.0, 60.0),
                Health::new(100.0),
            ));
        }
        for _ in 0..3 {
            advance_clock(&mut app, 5.0);
            app.update();
            assert_eq!(at(&app), 0, "two in the pass is not the three it waits on");
            assert!(sent(&mut app).is_empty());
        }

        // An enemy elsewhere on a fully-lit map must not count either — the
        // circle is the question, not the map.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Footman },
            Team::Claude,
            Transform::from_xyz(60.0, 0.0, -60.0),
            Health::new(100.0),
        ));
        advance_clock(&mut app, 5.0);
        app.update();
        assert_eq!(at(&app), 0, "seen, but not in the pass");
        assert!(sent(&mut app).is_empty());

        // The third one arrives IN the pass.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Footman },
            Team::Claude,
            Transform::from_xyz(-58.0, 0.0, 62.0),
            Health::new(100.0),
        ));
        advance_clock(&mut app, 5.0);
        app.update();
        assert_eq!(at(&app), 1, "and the plan moves the moment the pass is threatened");
        assert_eq!(sent(&mut app).len(), 1, "step 2 went out");
    }

    /// Fog honesty survives the seam. A plan waiting on `enemy_in` must not
    /// advance on an army its owner cannot see — otherwise sequencing would be
    /// a way to launder knowledge the predicate itself refuses to give.
    #[test]
    fn a_plan_waiting_on_a_place_is_as_fog_honest_as_the_predicate() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Regions>()
            .set(
                Team::Human,
                Region::new("north-pass", Vec3::new(-60.0, 0.0, 60.0), 20.0),
            )
            .expect("legal");
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(
                    stop(),
                    PlanAdvance::When {
                        when: TriggerWhen::EnemyIn {
                            region: "north-pass".to_string(),
                            class: None,
                            count: 1,
                        },
                    },
                ),
                step(stop(), PlanAdvance::OnApplied),
            ]));
        app.update();
        accept(&mut app);
        sent(&mut app);

        // Standing right there, in the dark (`plan_app` pins `test_dark`).
        app.world_mut().spawn((
            Unit { kind: UnitKind::Footman },
            Team::Claude,
            Transform::from_xyz(-60.0, 0.0, 60.0),
            Health::new(100.0),
        ));
        for _ in 0..3 {
            advance_clock(&mut app, 5.0);
            app.update();
            assert_eq!(at(&app), 0, "unseen is unknown, for a plan as for a rule");
        }

        // Light the map and the IDENTICAL world moves it on.
        app.insert_resource(FogGrids::test_revealed());
        advance_clock(&mut app, 5.0);
        app.update();
        assert_eq!(at(&app), 1, "seen is known");
    }

    /// The other half of the same seam, so both new arms are covered rather
    /// than one standing in for the pair.
    #[test]
    fn a_plan_can_also_wait_on_an_enemy_army_sighting() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(
                    stop(),
                    PlanAdvance::When {
                        when: TriggerWhen::EnemyArmySeen {
                            size: 4,
                            within_s: None,
                        },
                    },
                ),
                step(stop(), PlanAdvance::OnApplied),
            ]));
        app.update();
        accept(&mut app);
        sent(&mut app);
        advance_clock(&mut app, 5.0);
        app.update();
        assert_eq!(at(&app), 0, "nothing has been seen");

        let mut grids = FogGrids::test_dark();
        for i in 0..4 {
            grids.test_sight(
                Team::Human,
                Sighting {
                    id: 9000 + i,
                    team: Team::Claude,
                    kind: UnitKind::Footman,
                    pos: Vec3::new(20.0 + i as f32, 0.0, 20.0),
                    hp_frac: 1.0,
                    heading: None,
                    t_seen: 5.0,
                },
            );
        }
        app.insert_resource(grids);
        advance_clock(&mut app, 1.0);
        app.update();
        assert_eq!(at(&app), 1, "four of them together is the wait satisfied");
    }

    // -- failure: blocked, retried, halted, never skipped -------------------

    #[test]
    fn a_refused_step_blocks_the_plan_with_the_compilers_own_words() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(stop(), PlanAdvance::OnApplied),
                step(stop(), PlanAdvance::OnApplied),
            ]));
        app.update();
        refuse(&mut app, "not enough gold (need 160, have 120)");

        advance_clock(&mut app, 0.5);
        app.update();
        assert_eq!(
            state(&app),
            PlanState::Blocked("not enough gold (need 160, have 120)".to_string())
        );
        assert_eq!(at(&app), 0, "blocked ON the step, never past it");
        assert!(
            app.world()
                .resource::<Plans>()
                .get(Team::Human)[0]
                .status()
                .starts_with("blocked: not enough gold"),
            "and the status says why without anyone opening the error channel"
        );
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .any(|e| e.message.contains("blocked: not enough gold")),
            "the owner is told once, on their own feed"
        );
    }

    #[test]
    fn a_blocked_step_retries_on_a_slow_clock_and_recovers_if_it_can() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(stop(), PlanAdvance::OnApplied),
                step(stop(), PlanAdvance::OnApplied),
            ]));
        app.update();
        sent(&mut app);
        refuse(&mut app, "not enough gold");

        // Inside one retry period nothing goes out — a 4 Hz retry would put
        // forty copies of one refusal into the log.
        advance_clock(&mut app, 1.0);
        app.update();
        assert!(sent(&mut app).is_empty(), "too soon to try again");

        advance_clock(&mut app, PLAN_RETRY_S);
        app.update();
        let retry = sent(&mut app);
        assert_eq!(retry.len(), 1, "retried");
        assert_eq!(retry[0].plan.unwrap().step, 1, "the SAME step");

        // The gold arrives.
        let now = app.world().resource::<Time>().elapsed_secs();
        let stamp = PlanStamp {
            name: name("p"),
            step: 1,
            of: 2,
        };
        app.world_mut()
            .resource_mut::<Plans>()
            .report(Team::Human, stamp, None, now);
        advance_clock(&mut app, 0.5);
        app.update();
        assert_eq!(state(&app), PlanState::Running);
        assert_eq!(at(&app), 1, "and the plan carries on where it stopped");
    }

    #[test]
    fn a_step_refused_for_the_whole_grace_window_halts_the_plan_there() {
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(stop(), PlanAdvance::OnApplied),
                step(stop(), PlanAdvance::OnApplied),
            ]));
        app.update();
        refuse(&mut app, "you have no Sanctum");

        // Walk past the grace window, refusing every retry.
        for _ in 0..((PLAN_BLOCK_GRACE_S as usize) + 4) {
            advance_clock(&mut app, 1.0);
            app.update();
            refuse(&mut app, "you have no Sanctum");
        }
        assert_eq!(
            state(&app),
            PlanState::Halted("you have no Sanctum".to_string())
        );
        assert_eq!(at(&app), 0, "halted ON the failing step");
        assert!(
            sent(&mut app).is_empty(),
            "a halted plan submits nothing, ever again"
        );
        advance_clock(&mut app, 600.0);
        app.update();
        assert!(sent(&mut app).is_empty());
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .any(|e| e.message.contains("halted: you have no Sanctum")),
            "and it said so rather than going quiet"
        );
    }

    #[test]
    fn a_plan_never_skips_a_step() {
        // The property, stated on its own because it is the one thing that
        // would be worst to get wrong: step 2 must not run while step 1 is
        // being refused, however long that goes on.
        let mut app = plan_app();
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(plan(vec![
                step(
                    Intent::Train {
                        building: 1,
                        unit: "Footman".into(),
                    },
                    PlanAdvance::OnApplied,
                ),
                step(
                    Intent::Train {
                        building: 2,
                        unit: "Knight".into(),
                    },
                    PlanAdvance::OnApplied,
                ),
            ]));
        app.update();
        let mut steps_seen: Vec<u8> = sent(&mut app)
            .iter()
            .filter_map(|s| s.plan.map(|p| p.step))
            .collect();
        for _ in 0..30 {
            refuse(&mut app, "building 1 not found");
            advance_clock(&mut app, 1.0);
            app.update();
            steps_seen.extend(sent(&mut app).iter().filter_map(|s| s.plan.map(|p| p.step)));
        }
        assert!(
            steps_seen.iter().all(|k| *k == 1),
            "only step 1 was ever submitted, got {steps_seen:?}"
        );
    }

    // -- provenance and attribution ---------------------------------------

    #[test]
    fn a_plan_step_carries_its_seat_its_name_and_its_place_in_the_sequence() {
        let mut app = plan_app();
        let mut p = plan(vec![
            step(stop(), PlanAdvance::OnApplied),
            step(stop(), PlanAdvance::OnApplied),
            step(stop(), PlanAdvance::OnApplied),
        ]);
        p.name = name("opening");
        p.source = IntentSource::Ui;
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(p);
        app.update();
        sent(&mut app);
        accept(&mut app);
        advance_clock(&mut app, 0.5);
        app.update();

        let out = sent(&mut app);
        assert_eq!(out.len(), 1);
        let stamp = out[0].plan.expect("a plan step says so");
        assert_eq!(stamp.name.as_str(), "opening");
        assert_eq!((stamp.step, stamp.of), (2, 3));
        assert_eq!(out[0].source, IntentSource::Ui, "the AUTHOR, not the engine");
        assert_eq!(out[0].tag, "plan:opening#2");
        assert!(out[0].trigger.is_none());

        // The `why` a unit moved by this step answers.
        let why = IntentMark {
            source: IntentSource::Ui,
            at: 41.0,
            trigger: None,
            plan: Some(stamp),
        }
        .order("move")
        .why();
        assert_eq!(why, "plan:opening step 2/3 move by ui t=41");
    }

    #[test]
    fn a_stepping_plan_announces_itself_on_its_own_teams_feed_only() {
        let mut app = plan_app();
        let mut p = plan(vec![step(
            Intent::Train {
                building: 4294967298,
                unit: "Footman".into(),
            },
            PlanAdvance::OnApplied,
        )]);
        p.name = name("opening");
        app.world_mut()
            .resource_mut::<Plans>()
            .get_mut(Team::Human)
            .push(p);
        app.update();

        let feed = app.world().resource::<GameEvents>();
        assert!(
            feed.feed(Team::Human)
                .iter()
                .any(|e| e.message == "plan opening step 1/1: building 4294967298 trains Footman"),
            "the sentence a person reads, with the step number in it: {:?}",
            feed.feed(Team::Human).iter().map(|e| &e.message).collect::<Vec<_>>()
        );
        assert!(
            feed.feed(Team::Claude).is_empty(),
            "and the opponent is not — a plan is not intelligence"
        );
    }

    #[test]
    fn plans_step_in_the_order_they_were_set() {
        let mut app = plan_app();
        {
            let mut plans = app.world_mut().resource_mut::<Plans>();
            let list = plans.get_mut(Team::Human);
            for label in ["first", "second"] {
                let mut p = plan(vec![step(stop(), PlanAdvance::OnApplied)]);
                p.name = name(label);
                list.push(p);
            }
        }
        app.update();
        let order: Vec<String> = sent(&mut app)
            .iter()
            .map(|s| s.plan.unwrap().name.as_str().to_string())
            .collect();
        assert_eq!(order, vec!["first", "second"]);
    }

    // -- the caps ----------------------------------------------------------

    #[test]
    fn a_team_may_run_only_two_plans_at_once_but_may_always_replace_one() {
        let mut plans = Plans::default();
        for label in ["a", "b"] {
            let mut p = plan(vec![step(stop(), PlanAdvance::OnApplied)]);
            p.name = name(label);
            assert!(plans.set(Team::Human, p).is_ok());
        }
        let mut third = plan(vec![step(stop(), PlanAdvance::OnApplied)]);
        third.name = name("c");
        let err = plans
            .set(Team::Human, third.clone())
            .expect_err("the third is refused");
        assert!(err.contains(&format!("{MAX_PLANS_PER_TEAM} plans")), "{err}");
        assert!(err.contains('a') && err.contains('b'), "it names them: {err}");

        // Replacing by name is free and restarts the plan.
        let mut again = plan(vec![
            step(stop(), PlanAdvance::OnApplied),
            step(stop(), PlanAdvance::OnApplied),
        ]);
        again.name = name("a");
        assert!(plans.set(Team::Human, again).is_ok());
        assert_eq!(plans.get(Team::Human).len(), 2);
        assert_eq!(plans.get(Team::Human)[0].steps.len(), 2, "replaced in place");
        assert_eq!(plans.get(Team::Human)[0].at, 0, "and restarted");

        // A finished plan stops holding a slot: the cap is about how much is
        // running unattended, not about how much has ever run.
        plans.get_mut(Team::Human)[1].state = PlanState::Done;
        assert!(plans.set(Team::Human, third).is_ok());
        assert!(
            plans.get(Team::Human).iter().any(|p| p.name.as_str() == "b"),
            "and the finished one is still readable"
        );
    }

    // -- the whole loop, through the real compiler --------------------------

    /// **The headline.** A three-step plan set the way a commander sets one —
    /// one intent, through the wire — and walked to completion by the engine
    /// with nobody at either keyboard. Real plugin set, so every step goes
    /// through the real compiler in the frame it is submitted.
    #[test]
    fn a_three_step_plan_runs_itself_to_completion() {
        let mut app = full_app();
        // A squad-1 footman, so the postures below have something to move.
        app.world_mut().spawn((
            Unit {
                kind: UnitKind::Footman,
            },
            Team::Human,
            Transform::from_xyz(0.0, 0.0, 0.0),
            Health::new(100.0),
            SquadId(1),
            Order::Idle,
        ));

        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: serde_json::from_str(
                r#"{"type":"plan_set","name":"opening","steps":[
                     {"intent":{"type":"posture","id":1,
                                "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":22.0}},
                      "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}},
                     {"intent":{"type":"posture","id":1,
                                "posture":{"type":"push","x":0.0,"z":0.0}},
                      "advance":{"type":"after","secs":20}},
                     {"intent":{"type":"posture","id":1,
                                "posture":{"type":"push","x":70.0,"z":70.0}}}]}"#,
            )
            .expect("the recipe in COMMANDER_BRIEF.md parses"),
            trigger: None,
            plan: None,
        });
        app.update();
        assert_eq!(app.world().resource::<Plans>().get(Team::Human).len(), 1);

        fn posture(app: &App) -> Option<SquadPosture> {
            app.world()
                .resource::<SquadOrders>()
                .0
                .get(&(Team::Human, 1))
                .copied()
        }
        fn errs(app: &App) -> Vec<String> {
            app.world().resource::<IntentErrors>().get(Team::Human).clone()
        }

        // Step 1 goes out on the first sweep and is compiled in the SAME frame:
        // plan.rs sits upstream of the compiler exactly as trigger.rs does.
        advance_clock(&mut app, 0.5);
        app.update();
        assert!(
            matches!(posture(&app), Some(SquadPosture::Defend { .. })),
            "step 1 ran, got {:?}",
            posture(&app)
        );
        assert!(errs(&app).is_empty(), "and was accepted: {:?}", errs(&app));

        // Step 1's advance is `when tier 2`, so the plan holds there — it does
        // not move merely because the step it is on succeeded.
        for _ in 0..4 {
            advance_clock(&mut app, 5.0);
            app.update();
        }
        assert_eq!(at(&app), 0, "still on step 1, waiting for the keep");
        assert!(matches!(posture(&app), Some(SquadPosture::Defend { .. })));

        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(Team::Human, TechTier::T2);
        advance_clock(&mut app, 0.5);
        app.update();
        assert_eq!(at(&app), 1);
        assert!(
            matches!(posture(&app), Some(SquadPosture::Push { pos, .. }) if pos.x.abs() < 1.0),
            "step 2 ran on the tick the keep finished, got {:?}",
            posture(&app)
        );

        // Step 2's own `after` clock.
        advance_clock(&mut app, 10.0);
        app.update();
        assert_eq!(at(&app), 1, "10s into a 20s wait");
        advance_clock(&mut app, 11.0);
        app.update();
        assert_eq!(at(&app), 2);
        assert!(
            matches!(posture(&app), Some(SquadPosture::Push { pos, .. }) if pos.x > 60.0),
            "the last step ran"
        );

        // The last step's advance is the bare default, so the plan finishes on
        // the sweep after it lands.
        advance_clock(&mut app, 0.5);
        app.update();
        assert_eq!(state(&app), PlanState::Done);
        assert_eq!(
            app.world().resource::<Plans>().get(Team::Human)[0].status(),
            "done"
        );
        // `at` is PINNED to the last real index, not left one past the end:
        // it is a public field documented that way, and `steps[at]` on a
        // finished plan must not be a panic waiting for a future reader.
        let finished = &app.world().resource::<Plans>().get(Team::Human)[0];
        assert_eq!(finished.at, finished.steps.len() - 1);
        assert_eq!(finished.step_no(), 3, "and it still reads as step 3/3");
        assert!(finished.current().is_some());
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .iter()
                .any(|e| e.message == "plan opening complete (3 steps)"),
            "and said so"
        );
        assert!(errs(&app).is_empty(), "nothing was refused along the way");
    }

    /// **A step that lost one unit is a partial success, not a refusal.**
    ///
    /// The regression this pins is the worst one plans can have. `own_units`
    /// reports every dead id AND returns the survivors, so a step that says
    /// `move [alive, dead]` really does move `alive` — and still produces an
    /// error. Treating "any error" as "refused" made a plan block on the most
    /// ordinary event in the game (a squad member dying between `plan_set` and
    /// the step), re-issue the same order five times, and then halt a sequence
    /// that was in fact running correctly.
    #[test]
    fn a_step_that_reached_some_of_its_units_is_not_a_refusal() {
        let mut app = full_app();
        let alive = app
            .world_mut()
            .spawn((
                Unit {
                    kind: UnitKind::Footman,
                },
                Team::Human,
                Transform::from_xyz(0.0, 0.0, 0.0),
                Health::new(100.0),
                Order::Idle,
            ))
            .id();
        // An id that is well-formed and simply is not ours — exactly what a
        // dead squad member's id becomes.
        let ghost = 424242u64;

        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: serde_json::from_str(&format!(
                r#"{{"type":"plan_set","name":"opening","steps":[
                     {{"intent":{{"type":"move","units":[{},{ghost}],
                                  "x":-70.0,"z":-70.0}}}},
                     {{"intent":{{"type":"posture","id":1,
                                  "posture":{{"type":"push","x":70.0,"z":70.0}}}}}}]}}"#,
                alive.to_bits()
            ))
            .expect("parses"),
            trigger: None,
            plan: None,
        });
        app.update();
        advance_clock(&mut app, 0.5);
        app.update();

        // The survivor really was ordered...
        assert!(
            matches!(app.world().entity(alive).get::<Order>(), Some(Order::Move(_))),
            "the living unit was moved"
        );
        // ...the error was still reported, because every other channel wants it...
        assert!(
            app.world()
                .resource::<IntentErrors>()
                .get(Team::Human)
                .iter()
                .any(|e| e.contains("424242")),
            "the dead id is still named on the error channel"
        );
        // ...and the plan carried on rather than blocking.
        assert_eq!(
            state(&app),
            PlanState::Running,
            "a step that did what it could must not block the plan"
        );
        advance_clock(&mut app, 0.5);
        app.update();
        assert_eq!(at(&app), 1, "and it advanced");

        // The contrast: a step that reached NOBODY is a real refusal.
        let mut app = full_app();
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: serde_json::from_str(
                r#"{"type":"plan_set","name":"opening","steps":[
                     {"intent":{"type":"move","units":[424242],"x":-70.0,"z":-70.0}},
                     {"intent":{"type":"posture","id":1,
                                "posture":{"type":"push","x":70.0,"z":70.0}}}]}"#,
            )
            .expect("parses"),
            trigger: None,
            plan: None,
        });
        app.update();
        advance_clock(&mut app, 0.5);
        app.update();
        assert!(
            matches!(state(&app), PlanState::Blocked(_)),
            "reaching nobody is a refusal, got {:?}",
            state(&app)
        );
    }

    /// **A plan replaced mid-flight does not inherit the old one's verdict.**
    ///
    /// `plan_set` and a plan step's verdict are compiled in the same set, and a
    /// replacement arrives with the same name on the same step number — so name
    /// and step cannot tell the two apart. `submitted` can: a plan that has
    /// sent nothing cannot be the addressee of a verdict.
    #[test]
    fn a_replaced_plan_does_not_inherit_the_old_ones_verdict() {
        let mut plans = Plans::default();
        let mut first = plan(vec![
            step(stop(), PlanAdvance::OnApplied),
            step(stop(), PlanAdvance::OnApplied),
        ]);
        first.name = name("opening");
        first.submitted = true;
        plans.set(Team::Human, first).unwrap();

        let stamp = PlanStamp {
            name: name("opening"),
            step: 1,
            of: 2,
        };
        // The commander replaces it — same name, same length, so the stamp
        // still "matches" on everything but the one field that counts.
        let mut second = plan(vec![
            step(stop(), PlanAdvance::OnApplied),
            step(stop(), PlanAdvance::OnApplied),
        ]);
        second.name = name("opening");
        plans.set(Team::Human, second).unwrap();

        plans.report(Team::Human, stamp, Some("stale news".into()), 5.0);
        let now = &plans.get(Team::Human)[0];
        assert_eq!(
            now.state,
            PlanState::Running,
            "the fresh plan must not be blocked by the old plan's refusal"
        );
        assert!(!now.applied, "nor accepted by its acceptance");
        assert!(!now.submitted, "and it still has its own step 1 to send");
    }

    /// A step the compiler really refuses, through the real compiler: the plan
    /// blocks with the compiler's actual words rather than a paraphrase.
    #[test]
    fn a_step_the_real_compiler_refuses_blocks_the_real_plan() {
        let mut app = full_app();
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: serde_json::from_str(
                r#"{"type":"plan_set","name":"opening","steps":[
                     {"intent":{"type":"train","building":424242,"unit":"Footman"}},
                     {"intent":{"type":"posture","id":1,
                                "posture":{"type":"push","x":70.0,"z":70.0}}}]}"#,
            )
            .expect("parses"),
            trigger: None,
            plan: None,
        });
        app.update();
        advance_clock(&mut app, 0.5);
        app.update();

        let blocked = match state(&app) {
            PlanState::Blocked(why) => why,
            other => panic!("expected blocked, got {other:?}"),
        };
        assert!(
            blocked.contains("424242"),
            "the compiler's own words, verbatim: {blocked}"
        );
        assert!(
            !blocked.starts_with("plan:"),
            "with the channel tag stripped, not the error: {blocked}"
        );
        assert!(
            app.world()
                .resource::<SquadOrders>()
                .0
                .get(&(Team::Human, 1))
                .is_none(),
            "and step 2 did NOT run"
        );
    }
}
