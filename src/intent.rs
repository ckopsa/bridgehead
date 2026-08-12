//! intent.rs — the one place a player's intent becomes game state, and the
//! replay log that records it.
//!
//! `shared::Intent` is the vocabulary; this file is the grammar and the only
//! speaker. Both player-facing interfaces write `SubmitIntent` events and
//! nothing else:
//!
//!   * `ui.rs` compiles mouse gestures, hotkeys and command-card buttons into
//!     `Intent` values — a right-click on an enemy is an `Intent::Attack`, the
//!     `V` key is an `Intent::Retreat` with the parameters the gesture implies,
//!     `[U]` is an `Intent::Upgrade`, `[R]`/`[Y]`/`[D]` are `Intent::Cast` with
//!     an ability slot.
//!   * `bridge.rs` deserializes `commands.json` straight into `Intent` values;
//!     the wire format *is* the schema, so the protocol did not change when the
//!     compiler moved here.
//!   * `ai.rs`, the scripted commander, builds `Intent` values out of the
//!     decisions its think tick reaches — the third seat, and since
//!     wc3clone-jem no longer an exception to any of this.
//!
//! Everything downstream of this file is unchanged: the compiler writes the
//! same `Order` components, `TrainingQueue` pushes, `RallyPoint`s, doctrine
//! components, `SquadOrders` entries and `CastAbility`/`BuyItem`/`UseItem`/
//! `UpgradeBuilding`/`Surrender` events that `bridge.rs::apply_batch` and
//! `ui.rs`'s fifteen call sites used to write separately. units.rs, combat.rs,
//! economy.rs and doctrine.rs cannot tell the difference.
//!
//! ## The fairness invariant
//!
//! **No commander mutates game state except through intent submission.** No
//! footnote, no "except the script". That is what makes THESIS.md's structural
//! claim checkable rather than aspirational: the AI cannot act in ways the
//! human cannot, and — the half we had been failing — the human cannot be
//! denied a verb the AI has, because there is one list of verbs and one
//! compiler reading it. All three seats speak it: `ui`, `bridge`/`copilot`,
//! and `script`.
//!
//! One thing is deliberately *not* a commander and stays as it is:
//!
//!   * **Engine systems.** economy.rs's harvest follow-through and payments,
//!     combat.rs's chase, doctrine.rs's squad re-tasking and retreat triggers
//!     are the engine executing standing policy at machine speed. They write
//!     `Order`s directly and always will — that asymmetry *is* the tempo design
//!     (see docs/TEMPO.md §C4). The distinction that matters is not "human vs
//!     machine" but "deciding vs executing": ai.rs *decides*, so it speaks;
//!     doctrine.rs *executes what was already decided*, so it does not.
//!
//! ## Knowability
//!
//! The compiler is where fog of war stops being a rendering choice and becomes
//! a rule. `Intent::Attack` is refused against a target the issuing team cannot
//! see or remember (`FogGrid::knows_entity`), for *both* interfaces, because a
//! snapshot that will not show you an enemy must not accept orders against it
//! either. That check used to live in bridge.rs and therefore bound one seat;
//! it now binds whoever is speaking. See docs/INTENT.md for the one residual
//! gap (the human's right-click picker is strictly narrower than the rule).
//!
//! ## Ordering
//!
//! `apply_intents` runs in the `IntentApply` set, `.after(FogSet)` — the same
//! discipline every other fog consumer follows, so an intent is judged against
//! this frame's visibility and never the previous frame's. bridge.rs orders its
//! poll before the set and its snapshot after it, so a batch read this frame is
//! applied this frame and its validation errors ride out in the same snapshot.
//! ui.rs's whole input chain runs before the set.
//!
//! ## The log
//!
//! Every submitted intent — applied or rejected — is appended to a JSONL file
//! (`BH_INTENT_LOG`, default `bridge/intent_log.jsonl`) as a human-readable
//! sentence *and* its serialized form. The sentence does not record how the
//! intent was spelled, which is the point: a replay reads identically whether
//! the match was played with a mouse or with JSON.

use crate::command::{CommandLink, OrderIssuer};
use crate::shared::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::Serialize;
use std::io::Write as _;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Tuning knobs
// ---------------------------------------------------------------------------

/// Path of the per-match intent log. Empty or `0` turns logging off.
const INTENT_LOG_ENV: &str = "BH_INTENT_LOG";
const DEFAULT_INTENT_LOG: &str = "bridge/intent_log.jsonl";

/// Same formation grid both interfaces used before the merge.
const FORMATION_SPACING: f32 = 2.6;
/// Same training queue cap both interfaces enforced before the merge.
const MAX_QUEUE: usize = 7;
/// Hero inventory size, read off the shared component so it cannot drift.
const INVENTORY_SLOTS: usize = Inventory([None; 2]).0.len();

/// How many validation errors to keep per team. Bridge seats replace their
/// list every batch; this only bounds a long single-player session.
const MAX_ERRORS: usize = 64;

/// Channel tag on a rejection raised to the human's alert stack. The bridge's
/// errors are prefixed `cmd 3:` because a commander needs to know *which* of
/// the commands it just wrote was refused; a gesture is always the one the
/// player just made, so the human's prefix names the outcome instead. What
/// follows the colon is byte-identical to what the bridge is told.
const UI_NOTICE_PREFIX: &str = "order refused";

/// How long the same rejection stays quiet after the player has been told once
/// (game seconds). A right-click held down on an illegal target re-fails
/// identically every frame, and the player needs to hear that once.
const UI_NOTICE_REPEAT_S: f32 = 4.0;

/// How many distinct recent rejections the limiter remembers. A gesture can
/// fail several ways at once (three dead units in one selection is three
/// errors), and those errors then repeat *as a set* every frame — so a
/// one-slot memory would let them take turns evicting each other and flood
/// anyway. Comfortably larger than the alert stack, and bounded.
const UI_NOTICE_MEMORY: usize = 12;

/// Most rejection notices to raise in one frame, however many failed in it.
/// The alert stack is six rows and they are shared with the match's actual
/// news — a fumbled click must never push "hostiles near base" off the screen.
const UI_NOTICE_BURST: usize = 2;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct IntentPlugin;

/// The choke point, as a set other plugins can order against.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IntentApply;

impl Plugin for IntentPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SubmitIntent>()
            .init_resource::<IntentErrors>()
            // Registered here rather than in CorePlugin for the same reason
            // `IntentErrors` is: bridge.rs reads all three, but this file is
            // the only thing that ever writes them.
            .init_resource::<IntentApplied>()
            .init_resource::<IntentJournal>()
            // A fourth on the same reasoning, one layer up: `Triggers` is
            // read by trigger.rs, bridge.rs and ui.rs, and written by exactly
            // two verbs in this file. The writer owns the registration.
            .init_resource::<Triggers>()
            // And a fifth and sixth, on identical reasoning. `Plans` is read
            // by plan.rs, bridge.rs and ui.rs; `Regions` by trigger.rs,
            // bridge.rs and ui.rs. Each is written by exactly two verbs here.
            .init_resource::<Plans>()
            .init_resource::<Regions>()
            // A seventh, on identical reasoning. `SquadStances` is read by
            // bridge.rs's snapshot and ui.rs's doctrine card, and written by
            // exactly two verbs here (`stance` sets it, `posture` clears it).
            // Note that its partner `SquadOrders` is registered by `CorePlugin`
            // instead, and that asymmetry is historical rather than principled:
            // doctrine.rs cannot run without a posture map, so it predates this
            // rule. `init_resource` is idempotent, so both are safe.
            .init_resource::<SquadStances>()
            .init_resource::<UiNotices>()
            .insert_resource(IntentLog::from_env())
            // `IntentApply` lives INSIDE `SimSet::Intent`, declared once here
            // rather than restated per system: anything later tagged only
            // `.in_set(IntentApply)` then inherits the frame order instead of
            // silently floating outside it.
            .configure_sets(Update, IntentApply.in_set(SimSet::Intent))
            // `.after(FogSet)`: an intent is judged against the visibility its
            // issuer has right now, the same grid the snapshot and the HUD are
            // about to show them.
            .add_systems(Update, apply_intents.in_set(IntentApply).after(FogSet))
            // Inside the same set, and `.after(apply_intents)` so a `squad`
            // and a `stance` in one batch are both flushed before the joiner
            // looks: Bevy inserts the sync point for the ordering, and without
            // it the enrolment would be a frame ahead of the doctrine it
            // implies. See `stamp_stance_on_joiners` for why the component and
            // not any of its three writers is the choke point.
            .add_systems(
                Update,
                stamp_stance_on_joiners
                    .in_set(IntentApply)
                    .after(apply_intents),
            );
    }
}

// ---------------------------------------------------------------------------
// The world the compiler is allowed to touch
// ---------------------------------------------------------------------------

/// Entity first so a team's own hero can be *found*, not just checked — `buy`
/// and `use_item` name no unit and infer it from the team.
type IntentUnits<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Unit,
        &'static Team,
        &'static Transform,
        // Read-only: `autocast` edits ONE rule of a policy that may already
        // hold others, so the applier has to see the current one.
        Option<&'static AutoCastPolicy>,
        // Membership, for the one verb that names a SQUAD and then has to write
        // per-unit components: `stance`. Every other doctrine verb is handed an
        // explicit id list and never asks who is in what.
        Option<&'static SquadId>,
    ),
>;

/// `Entity` first for the same reason [`IntentUnits`] carries it: a building
/// selector has to *find* buildings, not merely check the one it was handed.
type IntentBuildings<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Building,
        &'static Team,
        Option<&'static UnderConstruction>,
        Option<&'static mut TrainingQueue>,
        Option<&'static Upgrading>,
    ),
>;

/// Anything that can be attacked: a live unit or building with a team. The
/// `Transform` is carried so an attack order can be checked against the
/// issuer's fog — an id a player could never have learned is not a legal
/// target, whichever interface names it.
type IntentTargets<'w, 's> = Query<
    'w,
    's,
    (
        &'static Team,
        Option<&'static Unit>,
        Option<&'static Building>,
        &'static Transform,
    ),
>;

/// Resource nodes, with the two extra columns late binding needs. `Entity` and
/// `Transform` are here rather than in a second query because "is this id a
/// node?" and "which node is nearest?" are one question asked from two ends,
/// and two queries over the same archetype is how they drift apart.
type IntentNodes<'w, 's> = Query<'w, 's, (Entity, &'static ResourceNode, &'static Transform)>;

/// Squad membership, for the `"select":"squad 2"` selector. A separate query
/// rather than a sixth column on [`IntentUnits`] for the same reason
/// [`IntentResearching`] is separate: widening the shared tuple would rewrite
/// every `units.get` destructure in this file for one caller's benefit.
type IntentSquads<'w, 's> = Query<'w, 's, &'static SquadId>;

/// Forges currently working. A separate query rather than a sixth column on
/// `IntentBuildings` on purpose: `research` is the only verb that asks, and
/// widening the shared tuple would rewrite every `buildings.get` destructure
/// in this file for one caller's benefit.
type IntentResearching<'w, 's> = Query<'w, 's, &'static Researching>;

/// The four queries late binding may read, as one system param.
///
/// Split out of [`IntentWorld`] rather than listed twice because the resolver
/// has **two** readers now. The compiler is one. The other is copilot.rs's
/// conflict preview, which has to expand a proposal's `"select"` phrases before
/// it can tell the human whose orders approving it would overwrite — and the
/// only honest way to do that is to run the same resolver over the same world.
/// Two statements of "what a selector may see" would drift; one cannot.
///
/// **Late binding reads all four read-only.** `buildings` carries
/// `&mut TrainingQueue` because the compiler's `train` and `cancel` arms push
/// and pop it, and it lives *here* rather than beside them because the building
/// selector family needs the same rows to answer "which barracks" — one query
/// over the buildings, iterated immutably by the resolver and mutably by the two
/// arms, instead of two queries that Bevy would refuse to schedule together.
#[derive(SystemParam)]
pub struct LateBindWorld<'w, 's> {
    units: IntentUnits<'w, 's>,
    nodes: IntentNodes<'w, 's>,
    squads: IntentSquads<'w, 's>,
    buildings: IntentBuildings<'w, 's>,
}

#[derive(SystemParam)]
pub struct IntentWorld<'w, 's> {
    bind: LateBindWorld<'w, 's>,
    targets: IntentTargets<'w, 's>,
    researching: IntentResearching<'w, 's>,
}

/// The read-only world knowledge a compile consults: money, hero records,
/// tiers, nav, research and fog. Bundled because `apply_intents` outgrew
/// Bevy's 16-param ceiling the moment three sibling features landed at once —
/// the same read/write split `CastLookup` uses in ui.rs.
#[derive(SystemParam)]
pub struct IntentTables<'w> {
    economies: Res<'w, Economies>,
    records: Res<'w, HeroRecords>,
    tiers: Res<'w, TechTiers>,
    /// Which roster each team is playing — the gate on the `build` verb. Beside
    /// `tiers` because it answers the same shape of question: what content is
    /// this team allowed to reach for.
    races: Res<'w, Races>,
    nav: Res<'w, NavGrid>,
    fog: Res<'w, FogGrids>,
    team_research: Res<'w, TeamResearch>,
}

/// The two halves of "what each squad is currently for": the posture doctrine.rs
/// executes, and the stance word that produced it.
///
/// Bundled because `apply_intents` sits exactly on Bevy's 16-parameter ceiling
/// (§6.6 of tools/BUILDER_BRIEF.md) and because the pairing is not arbitrary:
/// both are keyed by `(team, squad)`, both are written by this compiler and by
/// nothing else, and both are read by the snapshot and the HUD. Keeping them in
/// one param also keeps them honest — every write that sets a posture is next to
/// the write that names it, so the readout cannot drift from the doctrine.
#[derive(SystemParam)]
pub struct SquadPolicy<'w> {
    orders: ResMut<'w, SquadOrders>,
    stances: ResMut<'w, SquadStances>,
}

/// The two stores of **deferred** standing policy this compiler writes: armed
/// triggers and set plans.
///
/// Bundled for the reason every other `SystemParam` in this file is bundled —
/// `apply_intents` sits exactly on Bevy's 16-parameter ceiling — and, like
/// bridge.rs's `StandingOrders`, the pairing is not arbitrary. Both are written
/// by exactly two verbs here and by nothing else in the codebase; both are read
/// by their own evaluator in `SimSet::Think`, by the snapshot and by the HUD;
/// and both answer the one question "what has this commander told the engine to
/// do without them".
#[derive(SystemParam)]
pub struct DeferredPolicy<'w> {
    triggers: ResMut<'w, Triggers>,
    plans: ResMut<'w, Plans>,
    /// The third store on the same one-writer rule, and the one the other two
    /// READ: a trigger's `enemy_in` and a plan step's advance condition both
    /// resolve place names out of here, so it has to travel with them.
    regions: ResMut<'w, Regions>,
}

/// The events an intent can emit. ui.rs and bridge.rs each used to carry an
/// identical four-writer bundle; collapsing them into one is the duplication
/// this module exists to remove.
#[derive(SystemParam)]
pub struct IntentEvents<'w> {
    casts: EventWriter<'w, CastAbility>,
    buys: EventWriter<'w, BuyItem>,
    item_uses: EventWriter<'w, UseItem>,
    upgrades: EventWriter<'w, UpgradeBuilding>,
    research: EventWriter<'w, StartResearch>,
}

// ---------------------------------------------------------------------------
// Telling the human its gesture was refused
// ---------------------------------------------------------------------------

/// The rate limiter for rejections raised to the human's alert stack.
///
/// A bridge commander has always been told why a command was refused: the
/// errors ride back in the next snapshot's `errors` array, and reading them is
/// step 2 of the loop in tools/COMMANDER_BRIEF.md. The human at the keyboard
/// got nothing — the identical string was written to `IntentErrors`, sat in a
/// list only bridge.rs reads, and was overwritten. Same compiler, same
/// verdict, one seat told and one not, which is the fairness claim failing in
/// the *reverse* direction from the usual worry.
///
/// So the errors go to the human's existing news channel (`GameEvents`, which
/// the alert stack already renders and Space already focuses) as
/// `Warning`-severity notices. Rendering, not routing, is where the two seats
/// are allowed to differ: a file reader gets forty lines of history and all
/// the time in the world, a human gets six rows that fade.
///
/// The limiter exists because the two channels have different failure modes. A
/// bridge batch is a discrete document and its errors arrive once; a gesture
/// is a held mouse button that can re-fail at frame rate. Without this, one
/// stuck right-click would evict every real alert on screen.
#[derive(Resource, Default)]
struct UiNotices {
    /// Recent messages and the game time each was raised. Bounded by
    /// `UI_NOTICE_MEMORY`; entries expire after `UI_NOTICE_REPEAT_S`.
    recent: std::collections::VecDeque<(String, f32)>,
}

impl UiNotices {
    /// Raise up to `UI_NOTICE_BURST` of `errors` on `team`'s feed, skipping
    /// anything already said recently. `budget` is the frame's remaining
    /// allowance, so several failed gestures in one frame share one cap
    /// instead of getting one each.
    fn raise(
        &mut self,
        feed: &mut GameEvents,
        team: Team,
        now: f32,
        tag: &str,
        // The channel label — `order refused` for a gesture, `trigger <name>
        // refused` for a rule that fired and bounced.
        prefix: &str,
        errors: &[String],
        budget: &mut usize,
    ) {
        self.recent
            .retain(|(_, when)| now - *when < UI_NOTICE_REPEAT_S);
        for error in errors {
            if *budget == 0 {
                return;
            }
            // The tag is the channel label, not part of the error: the bridge
            // needs `cmd 3:` to find the command in its batch, the human needs
            // to know a gesture bounced. Everything after it is verbatim.
            let body = error
                .strip_prefix(&format!("{tag}: "))
                .unwrap_or(error)
                .to_string();
            if self.recent.iter().any(|(seen, _)| *seen == body) {
                continue;
            }
            let message = format!("{prefix}: {body}");
            self.recent.push_back((body, now));
            while self.recent.len() > UI_NOTICE_MEMORY {
                self.recent.pop_front();
            }
            *budget -= 1;
            // `pos: None` — a refusal happened at the cursor, not on the map,
            // and handing Space a camera jump to a place nothing occurred at
            // would be worse than skipping it (the focus walk skips
            // placeless alerts by design).
            feed.push(team, now, message, EventSeverity::Warning, None);
        }
    }
}

// ---------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------

/// Drain every submitted intent, validate it against the issuing team, apply
/// it, and log it. Intents apply in submission order, which for a bridge batch
/// is the order the commander wrote them.
#[allow(clippy::too_many_arguments)]
fn apply_intents(
    mut submissions: EventReader<SubmitIntent>,
    mut commands: Commands,
    time: Res<Time>,
    tables: IntentTables,
    mut squads: SquadPolicy,
    mut deferred: DeferredPolicy,
    mut ai_controlled: ResMut<AiControlled>,
    mut error_log: ResMut<IntentErrors>,
    // The positive half of the same channel: what each command cost to deliver.
    mut applied_log: ResMut<IntentApplied>,
    mut log: ResMut<IntentLog>,
    mut journal: ResMut<IntentJournal>,
    mut feed: ResMut<GameEvents>,
    mut notices: ResMut<UiNotices>,
    mut events: IntentEvents,
    mut world: IntentWorld,
    // Chain of Command (docs/TEMPO.md §3). Read-only: how far each unit is
    // from its team's nearest command node, and the curve that turns that into
    // seconds. Inert with BH_COMMAND_LATENCY unset.
    link: CommandLink,
) {
    // Owned copies: the compiler needs `&mut` on resources the reader borrows
    // from, and a batch is a handful of values.
    let batch: Vec<SubmitIntent> = submissions.read().cloned().collect();
    if batch.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    // One allowance for the whole frame, shared by every gesture in it.
    let mut notice_budget = UI_NOTICE_BURST;
    // Squad membership written by an earlier sentence in this frame's batch,
    // which no query can see yet. See `compile_intent`'s `batch_squads`.
    let mut batch_squads: std::collections::BTreeMap<Entity, Option<u8>> =
        std::collections::BTreeMap::new();
    for submission in batch {
        let mut errors: Vec<String> = Vec::new();
        // See `compile_intent`'s `reached` parameter. Per submission, because
        // "did THIS sentence do anything" is the question a plan step asks.
        let mut reached = false;
        // One issuer per sentence, so `max_delay` reports what THIS intent
        // cost — a group order spread across the map is logged with the worst
        // link any of its units pays.
        //
        // **A trigger-fired intent is exempt from the link**, and that is the
        // point of triggers rather than an exception to them. docs/TEMPO.md's
        // verb table exempts every doctrine verb on one rule — *standing orders
        // are local; direct orders travel* — because a unit under standing
        // policy already has its orders and does not need to ask. A trigger is
        // standing policy whose condition happened to come true just now: the
        // commander reached the unit when they ARMED it, and charging the link
        // again on firing would price the same reach twice. It also restores
        // the mechanism's own incentive at the contingent layer: pre-arming a
        // rule is strictly better than hand-answering an alarm at range, which
        // is C4 ("doctrine strictly better than micro at range") landing one
        // rung further out.
        //
        // **A plan step is exempt on the identical argument.** A plan is a
        // sequence of standing policy the engine executes unattended; its
        // author reached the units when they SET it, and the step firing four
        // minutes later is the engine doing what it was told, not a new order
        // travelling out from a commander. Charging the link per step would
        // also make a plan strictly worse than typing the same commands by
        // hand, which inverts C4.
        let mut issuer = if submission.trigger.is_some() || submission.plan.is_some() {
            link.exempt_issuer(now)
        } else {
            link.issuer(now)
        };
        compile_intent(
            submission.intent.clone(),
            submission.team,
            &submission.tag,
            // Who is speaking and when. Every order this call mints stamps
            // itself with this, so a unit can name the sentence that moved it.
            IntentMark {
                source: submission.source,
                at: now,
                trigger: submission.trigger,
                plan: submission.plan,
            },
            &mut errors,
            &mut ai_controlled,
            &tables.economies,
            &tables.records,
            // The issuing team's tech tier: what hero slots it has open.
            tables.tiers.get(submission.team),
            // ...and its roster: which buildings it may place at all.
            *tables.races,
            &tables.nav,
            &tables.team_research,
            // The issuer's own fog: what *they* can see decides what they may
            // order, and neither seat gets to borrow the other's eyes.
            tables.fog.get(submission.team),
            &mut squads.orders,
            &mut squads.stances,
            &mut deferred.triggers,
            &mut deferred.plans,
            &mut deferred.regions,
            &mut commands,
            &mut events,
            &mut world,
            &mut issuer,
            &mut reached,
            &mut batch_squads,
        );
        // **The plan's verdict, straight back to the plan.** In the same frame
        // it submitted, before its evaluator's next sweep — so a step that
        // bounced blocks the plan rather than being walked past. `errors` is
        // this step's errors and nothing else's, because the compiler is
        // per-submission.
        if let Some(stamp) = submission.plan {
            // **Refused, or merely partial?** A step that reached some of its
            // units did what it could and the plan carries on; the errors still
            // reach every other channel. Only a step that reached nothing is a
            // refusal, and only a refusal blocks. See `compile_intent`'s
            // `reached`.
            //
            // Tag stripped, exactly as `UiNotices::raise` strips it: the tag is
            // the CHANNEL (`plan:opening#2`), and a status line that read
            // `blocked: plan:opening#2: not enough gold` would say the plan's
            // name twice and the reason once.
            let why = errors.first().filter(|_| !reached).map(|e| {
                e.strip_prefix(&format!("{}: ", submission.tag))
                    .unwrap_or(e)
                    .to_string()
            });
            let verdict = deferred.plans.report(submission.team, stamp, why, now);
            // **Edge, not level** (arena/r17). A blocked step keeps retrying on
            // `PLAN_RETRY_S` — that part is right and unchanged, because the
            // refusal is usually timing and the retry is how a plan survives
            // it. What was wrong was letting each retry re-announce a verdict
            // the owner already has: twelve identical lines in the seat's
            // `errors` array across the grace window, twelve in the replay
            // log, and a `bridge_wait` that never got to sleep. The commander
            // who lost r17 chained waits to escape the noise and went a
            // hundred game-seconds without an order.
            //
            // So an identical re-refusal stops HERE, before any channel sees
            // it. Nothing is hidden: `plans[].status` in every snapshot still
            // reads `blocked: <why>` for as long as it is true, which is the
            // level-triggered rendering of the same fact and the right one for
            // a condition that persists. Transitions are announced; states are
            // displayed.
            //
            // A refusal with DIFFERENT words is a different problem and falls
            // through to every channel as before — `PlanVerdict::Blocked`.
            if verdict == PlanVerdict::BlockedAgain {
                continue;
            }
        }
        log.record(now, &submission, &errors, issuer.max_delay);
        // The same record, kept in memory as well as on disk. The file is the
        // match's, this is the seats': a co-commander reads its partner's
        // recent sentences out of its snapshot (`partner_log`) and would
        // otherwise be commanding next to someone it cannot hear.
        journal.push(
            submission.team,
            JournalEntry {
                t: (now * 10.0).round() / 10.0,
                source: submission.source,
                verb: submission.intent.verb(),
                sentence: submission.intent.sentence(),
                ok: errors.is_empty(),
            },
        );
        // The human's copy of the error channel. Source decides which renderer
        // is told, never whether the intent was legal — the verdict above was
        // reached without consulting it.
        if submission.source == IntentSource::Ui && !errors.is_empty() {
            // A trigger's refusal names the RULE, not the gesture: the player
            // made no gesture, and "order refused" would send them looking for
            // a click they never made. Same verdict, same words after the
            // colon — only the channel label differs, which is the one thing
            // `IntentSource` and this tag are allowed to decide.
            let prefix = match (submission.trigger, submission.plan) {
                (Some(name), _) => format!("trigger {name} refused"),
                // Names the STEP, not just the plan: with eight of them, "your
                // plan was refused" would send the player looking through the
                // whole sequence for the one that bounced.
                (None, Some(stamp)) => format!("plan {stamp} refused"),
                (None, None) => UI_NOTICE_PREFIX.to_string(),
            };
            notices.raise(
                &mut feed,
                submission.team,
                now,
                &submission.tag,
                &prefix,
                &errors,
                &mut notice_budget,
            );
        }
        // Where a refusal is DELIVERED — never whether it happened. The
        // scripted commander reads no snapshot and watches no alert stack, so
        // putting its errors in the team's channel would hand a seat sharing
        // that faction (autopilot handed back mid-match, a co-commander) a list
        // of failures it did not cause and cannot act on. It re-thinks every
        // second and simply tries again, so the useful audience for a script
        // rejection is whoever is reading a sim's `RUST_LOG=debug` trace.
        //
        // The verdict itself already went everywhere it goes: the intent log
        // has the sentence, its `ok: false` and the error strings verbatim.
        if submission.source == IntentSource::Script {
            for error in &errors {
                debug!("[script {:?}] refused: {error}", submission.team);
            }
        } else {
            let sink = error_log.get_mut(submission.team);
            sink.extend(errors);
            if sink.len() > MAX_ERRORS {
                let overflow = sink.len() - MAX_ERRORS;
                sink.drain(..overflow);
            }
        }

        // **The acknowledgement** (docs/TEMPO.md §4, issue 6). The human at the
        // keyboard learns the link from the HUD; a commander on the wire has no
        // HUD, so what it paid has to come back on the wire or it can only ever
        // infer the mechanic from things going wrong.
        //
        // Bridge-sourced only — a UI gesture's seat is a person looking at the
        // selection panel, and echoing their every right-click into the other
        // seat's snapshot would be noise for a reader that is not there.
        //
        // Silence when nothing was charged, on the same reasoning the intent
        // log omits its `link` field: an order that landed in the frame it was
        // spoken has nothing to acknowledge, and this keeps the whole channel
        // empty — and its wire key absent — whenever the feature is off.
        if submission.source == IntentSource::Bridge && issuer.max_delay > 0.0 {
            let sink = applied_log.get_mut(submission.team);
            sink.push(AppliedCommand {
                cmd: submission.tag.clone(),
                delay: issuer.max_delay,
            });
            // Bounded exactly like the error sink beside it: a snapshot is a
            // status report, not a transcript.
            if sink.len() > MAX_ERRORS {
                let overflow = sink.len() - MAX_ERRORS;
                sink.drain(..overflow);
            }
        }
    }
}

/// Apply one intent. `me` is the issuing team: every ownership check, economy
/// read, fog query and squad key below is taken against it, so the same code
/// runs for a human gesture and a bridge command without either being able to
/// touch the other faction.
///
/// Errors are appended rather than returned so that a partially-valid intent
/// (six live units and one corpse) still does what it can, exactly as the
/// bridge always did. `tag` prefixes them — `"cmd 3"` for a bridge batch,
/// `"ui"` for a gesture — which is what keeps the bridge's historical error
/// strings byte-identical.
// ---------------------------------------------------------------------------
// Late binding: the one place a name becomes a coordinate, and a role a roster
// ---------------------------------------------------------------------------

/// Everything late binding is allowed to look at.
///
/// A bundle rather than seven parameters because the resolver's inner helpers
/// all want most of it, and because the *shape* of this struct is the honest
/// statement of what a selector may know: this seat's own living units, their
/// squads, the neutral resource nodes, the nav grid (map geography, public per
/// docs/FOG.md) and the named places. **No enemy query, deliberately** — a
/// `"nearest enemy"` selector would be an intel question wearing a convenience
/// hat, and fog decides intel, not the resolver.
pub(crate) struct LateBind<'a, 'w, 's> {
    me: Team,
    regions: &'a Regions,
    units: &'a IntentUnits<'w, 's>,
    squads: &'a IntentSquads<'w, 's>,
    nodes: &'a IntentNodes<'w, 's>,
    /// This seat's own buildings, read-only. Same fog argument as `units`: your
    /// own structures are yours to know about, and there is deliberately no
    /// selector that reaches an enemy one.
    buildings: &'a IntentBuildings<'w, 's>,
    nav: &'a NavGrid,
}

impl<'a, 'w, 's> LateBind<'a, 'w, 's> {
    /// The binding a reader outside this file assembles, from the one param
    /// that says what late binding may see.
    ///
    /// The compiler builds its own inline (it already has the pieces
    /// unbundled); this exists for copilot.rs's conflict preview, which needs
    /// the identical view of the world so that "what would this proposal
    /// reach?" is answered by the resolver rather than by a second, narrower
    /// copy of the selector vocabulary.
    pub(crate) fn new(
        me: Team,
        regions: &'a Regions,
        nav: &'a NavGrid,
        world: &'a LateBindWorld<'w, 's>,
    ) -> Self {
        LateBind {
            me,
            regions,
            units: &world.units,
            squads: &world.squads,
            nodes: &world.nodes,
            buildings: &world.buildings,
            nav,
        }
    }
}

impl LateBind<'_, '_, '_> {
    /// Every living unit of this seat that a selector matches, as ids, **sorted
    /// by entity bits**.
    ///
    /// The sort is not cosmetic. `ground_order` hands out formation offsets by
    /// index, so an unsorted resolution would spread the same squad across the
    /// same ground in a different arrangement depending on Bevy's archetype
    /// order — a determinism hole of exactly the kind `SimSet` exists to close.
    fn units_matching(&self, sel: Selector) -> Vec<IntentId> {
        let mut out: Vec<Entity> = self
            .units
            .iter()
            .filter(|(entity, unit, team, ..)| {
                **team == self.me
                    && match sel {
                        Selector::Heroes => is_hero_kind(unit.kind),
                        Selector::Army => !is_worker_kind(unit.kind),
                        Selector::AllUnits => true,
                        Selector::Workers => is_worker_kind(unit.kind),
                        // The membership the squad has RIGHT NOW, which is the
                        // whole reason this selector exists.
                        Selector::Squad(n) => {
                            matches!(self.squads.get(*entity), Ok(SquadId(id)) if *id == n)
                        }
                        _ => false,
                    }
            })
            .map(|(entity, ..)| entity)
            .collect();
        out.sort_by_key(|e| e.to_bits());
        out.into_iter().map(intent_id).collect()
    }

    /// The nearest live node of `kind` to `from`, or `None` if the map has none
    /// left. Ties break on `(distance, x, z, id)` so the answer does not depend
    /// on query iteration order — the same rule
    /// `the_fingerprint_describes_the_world_not_the_visit_order` holds the
    /// fingerprint to.
    fn nearest_node(&self, kind: ResourceKind, from: Vec3) -> Option<IntentId> {
        // Written as `nearest_free_site` writes it — a running best compared as
        // a tuple — rather than with `min_by`, so the two "nearest" answers in
        // this file break ties by the same rule read the same way.
        let mut best: Option<(f32, f32, f32, u64)> = None;
        for (entity, node, tf) in self.nodes.iter() {
            if node.kind != kind || node.remaining == 0 {
                continue;
            }
            let p = tf.translation;
            let key = (from.distance(p), p.x, p.z, entity.to_bits());
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
        best.map(|(_, _, _, bits)| bits)
    }

    /// Every FINISHED building of this seat that a building selector matches,
    /// as ids, sorted by entity bits.
    ///
    /// Finished, always: a Barracks with scaffolding on it trains nothing, and
    /// a selector that resolved to one would turn a good sentence into the
    /// compiler's `under construction` refusal for reasons the commander never
    /// wrote. Sorted for the same reason `units_matching` sorts — the
    /// single-referent verbs take `first()`, and "the lowest id" has to mean the
    /// same thing on two runs of the same seed.
    fn buildings_matching(&self, sel: Selector) -> Vec<IntentId> {
        let Selector::Buildings { what, idle } = sel else {
            return Vec::new();
        };
        let mut out: Vec<Entity> = self
            .buildings
            .iter()
            .filter(|(_, building, team, under, queue, _)| {
                **team == self.me
                    && under.is_none()
                    && what.matches(building.kind)
                    // "Idle" is about the QUEUE, not about the progress bar: a
                    // building three seconds from finishing its last Footman is
                    // idle enough to take the next order, and one with four in
                    // the queue is not.
                    && (!idle || queue.as_ref().is_none_or(|q| q.queue.is_empty()))
            })
            .map(|(entity, ..)| entity)
            .collect();
        out.sort_by_key(|e| e.to_bits());
        out.into_iter().map(intent_id).collect()
    }

    /// What this seat has standing, as `Kind ×2, Kind` — the alternative half of
    /// a building selector's refusal.
    ///
    /// Own buildings only, so this leaks nothing (docs/FOG.md): it is the same
    /// list the seat's own snapshot already prints in full. Kinds in
    /// `ALL_BUILDING_KINDS` order rather than query order, because a refusal
    /// that reads differently on two runs of one seed is a determinism hole in
    /// the one place a commander is definitely reading.
    fn finished_building_roster(&self) -> String {
        let mut counts: Vec<(BuildingKind, usize)> = Vec::new();
        for kind in ALL_BUILDING_KINDS {
            let n = self
                .buildings
                .iter()
                .filter(|(_, b, team, under, _, _)| {
                    **team == self.me && under.is_none() && b.kind == kind
                })
                .count();
            if n > 0 {
                counts.push((kind, n));
            }
        }
        if counts.is_empty() {
            return "you have no finished buildings".to_string();
        }
        let list: Vec<String> = counts
            .into_iter()
            .map(|(k, n)| {
                if n == 1 {
                    building_name(k).to_string()
                } else {
                    format!("{} \u{d7}{n}", building_name(k))
                }
            })
            .collect();
        format!("you have: {}", list.join(", "))
    }

    /// Where a resolved unit list is standing, for the "nearest X" selectors.
    /// The FIRST living own unit in the list (they are sorted, so this is
    /// stable), or `None` if the list reaches nobody.
    fn anchor(&self, ids: &[IntentId]) -> Option<Vec3> {
        ids.iter()
            .find_map(|&id| own_unit(id, self.units, self.me).map(|(_, pos)| pos))
    }
}

/// Parse one selector phrase for a channel that takes units, refusing with a
/// sentence that names the fix.
fn unit_selector(verb: &str, raw: &str) -> Result<Selector, String> {
    let sel = parse_selector(raw).ok_or_else(|| unknown_selector(raw))?;
    if !sel.is_unit_selector() {
        return Err(format!(
            "{verb}: '{raw}' names {}, not units — unit selectors are: \
             {SELECTOR_UNIT_NAMES}",
            sel.channel_noun()
        ));
    }
    Ok(sel)
}

/// The same, for the four verbs whose `select` names a BUILDING.
///
/// A separate channel rather than a widening of the unit one, because
/// `{"type":"train","select":"my hero"}` is a real mistake with a helpful
/// answer, and the only way to give it is to know which list the phrase should
/// have come from.
fn building_selector(verb: &str, raw: &str) -> Result<Selector, String> {
    let sel = parse_selector(raw).ok_or_else(|| unknown_selector(raw))?;
    if !sel.is_building_selector() {
        return Err(format!(
            "{verb}: '{raw}' names {}, not a building — building selectors are: \
             {SELECTOR_BUILDING_NAMES}",
            sel.channel_noun()
        ));
    }
    Ok(sel)
}

/// Turn every `"region":"<name>"` and every `"select":"<phrase>"` in a
/// submitted intent into the coordinates and the ids they stand for, or refuse
/// with the list of names this seat may speak.
///
/// **This is the single resolution point, and that is the whole design.** The
/// alternative — every verb resolving its own place — is how you end up with
/// `defend` accepting a name that `push` does not, and with two spellings of
/// the unknown-name error. Here there is one function, one refusal, and every
/// arm below it sees plain floats and plain ids exactly as it did before
/// regions and selectors existed.
///
/// The role channel joined the place channel here rather than getting its own
/// pass for that reason and one more: **the two interact.** `harvest` with
/// `"select":"workers","target_select":"nearest tree"` has to resolve the
/// workers before it can say which tree is nearest, and a second resolver would
/// have had to either duplicate the first or run in a fixed order agreed by
/// comment. Inside one function the order is a line of code.
///
/// Three things it deliberately does NOT do:
///
///  * **It does not recurse into a trigger's `then`.** compile_intent's
///    `trigger_set` arm says why in full: the action is validated when it
///    FIRES, against the world that fired it. A region is on the same footing,
///    and so, now, is a selector — which is the entire point of the feature.
///    An armed rule keeps naming *the perimeter* and *my hero* rather than the
///    coordinates the perimeter had and the entity the hero was at arm time, so
///    moving a region re-aims every rule that names it, and reviving a hero
///    re-aims every rule that names the role.
///  * **It does not clamp or validate the geometry** — `clamp_to_map` and the
///    per-verb checks below still own that. A region's centre is already on the
///    map by construction, so there is nothing to add.
///  * **It does not keep a resolved coordinate or a resolved id anywhere.**
///    Resolution happens per submission, so a region moved between two
///    sentences moves both, and a unit killed between two firings of the same
///    trigger is gone from the second.
///
/// **An empty resolution is a refusal, not a quiet nothing.** A selector that
/// matches no units returns `Err`, which means the intent never reaches its
/// arm, `reached` stays false, and the seat is told in words. That is the rule
/// that makes r21's "move 0 units" inexpressible: the only way to order nobody
/// used to be to name nobody, and now naming a role that is currently empty
/// teaches instead of firing.
///
/// **Two callers, one of which is not applying anything.** The compiler calls
/// this to *execute* a sentence; copilot.rs's conflict preview calls it to
/// *read* one, so that the human reviewing `"select":"all army"` is told what
/// that reaches instead of being told it reaches nothing. The preview submits
/// nothing and writes nothing back — this function keeps no resolved id
/// anywhere, so a second call is only a second question asked of the same
/// world. The two calls are separated in time (preview at arrival, apply on
/// approval) and may legitimately disagree; saying so is the preview's job,
/// not this function's.
pub(crate) fn resolve_places(intent: Intent, bind: &LateBind) -> Result<Intent, String> {
    let me = bind.me;
    let regions = bind.regions;
    /// The unit channel. `select` outranks `units` on the same rule that makes
    /// a region outrank the coordinates beside it: naming a role is a stronger
    /// statement than listing ids, and silently unioning the two would make
    /// "all army plus this corpse" a sentence.
    fn crew(
        verb: &str,
        ids: Vec<IntentId>,
        select: &Option<String>,
        bind: &LateBind,
    ) -> Result<Vec<IntentId>, String> {
        let Some(raw) = select else {
            return Ok(ids);
        };
        let sel = unit_selector(verb, raw)?;
        let found = bind.units_matching(sel);
        if found.is_empty() {
            return Err(format!(
                "{verb}: '{raw}' matches none of your units right now — \
                 nothing was ordered"
            ));
        }
        Ok(found)
    }
    /// The single-referent form of `crew`: a build's worker, a cast's caster, a
    /// follow's leader. Narrowed to the lowest entity id among the matches,
    /// which is the same documented tie-break `own_hero` already uses for an
    /// omitted `hero` field — one rule for "which one did you mean", not two.
    fn soloist(
        verb: &str,
        id: Option<IntentId>,
        select: &Option<String>,
        bind: &LateBind,
    ) -> Result<Option<IntentId>, String> {
        let Some(raw) = select else {
            return Ok(id);
        };
        let sel = unit_selector(verb, raw)?;
        // Sorted by entity bits already, so `first` IS the lowest id.
        match bind.units_matching(sel).first() {
            Some(found) => Ok(Some(*found)),
            None => Err(format!(
                "{verb}: '{raw}' matches none of your units right now — \
                 nothing was ordered"
            )),
        }
    }
    /// The building channel: `train`'s producer, `template`'s stamper,
    /// `rally`'s source, `cancel`'s queue.
    ///
    /// Single-referent by the same documented tie-break as `soloist` above —
    /// the LOWEST entity id among the matches — because these four verbs act on
    /// exactly one building and picking "all of them" would turn one `train`
    /// into six Footmen. `idle barracks` is how you say "and pick a free one".
    ///
    /// Both channels empty is a refusal here rather than in the four arms,
    /// because `building` widened to `Option` to make room for the phrase and
    /// somebody has to notice when neither was given. The arms keep a defensive
    /// re-check, exactly as `resolved_point` does for the place channel.
    fn producer(
        verb: &str,
        id: Option<IntentId>,
        select: &Option<String>,
        bind: &LateBind,
    ) -> Result<Option<IntentId>, String> {
        let Some(raw) = select else {
            if id.is_none() {
                return Err(format!(
                    "{verb} needs a building id or a 'select' phrase — \
                     building selectors are: {SELECTOR_BUILDING_NAMES}"
                ));
            }
            return Ok(id);
        };
        let sel = building_selector(verb, raw)?;
        // Sorted by entity bits already, so `first` IS the lowest id.
        if let Some(found) = bind.buildings_matching(sel).first() {
            return Ok(Some(*found));
        }
        // The empty match teaches, and `idle` gets its own sentence: "you have
        // no barracks" and "both your barracks are busy" are different problems
        // with different fixes, and one wording for both would send a commander
        // to build a third barracks it did not need.
        if let Selector::Buildings { what, idle: true } = sel {
            let busy = bind
                .buildings_matching(Selector::Buildings { what, idle: false })
                .len();
            if busy > 0 {
                // Singular and plural read differently enough that one wording
                // for both is the sort of thing a commander notices instead of
                // the thing the sentence is about.
                let count = if busy == 1 {
                    format!("your only {} already has", what.word())
                } else {
                    format!("all {busy} of your {} already have", what.word())
                };
                return Err(format!(
                    "{verb}: '{raw}' matches none of your finished buildings — \
                     {count} something queued; drop 'idle' to queue behind it"
                ));
            }
        }
        Err(format!(
            "{verb}: '{raw}' matches none of your finished buildings — {}",
            bind.finished_building_roster()
        ))
    }
    /// `(x, z, radius_from_region)`. A region always supplies a radius; only
    /// the two verbs that have a radius to give away actually use it.
    fn shape(
        regions: &Regions,
        me: Team,
        x: Option<f32>,
        z: Option<f32>,
        region: &Option<String>,
    ) -> Result<(Option<f32>, Option<f32>, Option<f32>), String> {
        let Some(name) = region else {
            return Ok((x, z, None));
        };
        let found = regions
            .find(me, name)
            .ok_or_else(|| regions.unknown(me, name))?;
        Ok((
            Some(found.center.x),
            Some(found.center.z),
            Some(found.radius),
        ))
    }
    /// The refusal a place-taking verb earns when it named no ground at all.
    /// One wording, so `move` and `defend` teach the same lesson.
    fn needs(verb: &str) -> String {
        format!("{verb} needs x/z or a region name")
    }
    fn both(verb: &str, x: Option<f32>, z: Option<f32>) -> Result<(f32, f32), String> {
        match (x, z) {
            (Some(x), Some(z)) => Ok((x, z)),
            _ => Err(needs(verb)),
        }
    }

    Ok(match intent {
        Intent::Move {
            units,
            x,
            z,
            region,
            select,
        } => {
            let units = crew("move", units, &select, bind)?;
            let (x, z, _) = shape(regions, me, x, z, &region)?;
            let (px, pz) = both("move", x, z)?;
            Intent::Move {
                units,
                x: Some(px),
                z: Some(pz),
                region,
                select,
            }
        }
        Intent::AttackMove {
            units,
            x,
            z,
            region,
            select,
        } => {
            let units = crew("attackmove", units, &select, bind)?;
            let (x, z, _) = shape(regions, me, x, z, &region)?;
            let (px, pz) = both("attackmove", x, z)?;
            Intent::AttackMove {
                units,
                x: Some(px),
                z: Some(pz),
                region,
                select,
            }
        }
        Intent::Attack {
            units,
            target,
            select,
        } => Intent::Attack {
            units: crew("attack", units, &select, bind)?,
            target,
            select,
        },
        Intent::Harvest {
            units,
            target,
            select,
            target_select,
        } => {
            let units = crew("harvest", units, &select, bind)?;
            // The node is chosen from where the WORKERS are, which is why this
            // runs after the crew and not beside it.
            let target = match &target_select {
                None => target,
                Some(raw) => {
                    let sel = parse_selector(raw).ok_or_else(|| unknown_selector(raw))?;
                    let kind = match sel {
                        Selector::NearestTree => ResourceKind::Lumber,
                        Selector::NearestMine => ResourceKind::Gold,
                        _ => {
                            return Err(format!(
                                "harvest: '{raw}' does not name a resource node — \
                                 node selectors are: {SELECTOR_NODE_NAMES}"
                            ))
                        }
                    };
                    let Some(from) = bind.anchor(&units) else {
                        return Err("harvest: '(nearest)' needs at least one living worker to \
                             measure from — name units or a unit selector that matches"
                            .to_string());
                    };
                    let Some(found) = bind.nearest_node(kind, from) else {
                        return Err(format!(
                            "harvest: no {} left on the map — nothing was ordered",
                            sel.phrase().trim_start_matches("nearest ")
                        ));
                    };
                    Some(found)
                }
            };
            Intent::Harvest {
                units,
                target,
                select,
                target_select,
            }
        }
        Intent::Return { units, select } => Intent::Return {
            units: crew("return", units, &select, bind)?,
            select,
        },
        Intent::Follow {
            units,
            target,
            select,
            target_select,
        } => Intent::Follow {
            units: crew("follow", units, &select, bind)?,
            target: soloist("follow", target, &target_select, bind)?,
            select,
            target_select,
        },
        Intent::Stop { units, select } => Intent::Stop {
            units: crew("stop", units, &select, bind)?,
            select,
        },
        Intent::Build {
            worker,
            kind,
            x,
            z,
            region,
            select,
            site,
        } => {
            let worker = soloist("build", worker, &select, bind)?;
            let (x, z, _) = shape(regions, me, x, z, &region)?;
            let (px, pz) = both("build", x, z)?;
            // **The site selector, and the loop it exists to break.** The
            // rejection this verb already produces computes a legal alternative
            // and prints it (`blocked_site_error`'s `nearest legal: (x, z)`);
            // before this there was no way to say "yes, that one". Blue-r23
            // armed a farm trigger on fixed coordinates, watched it report
            // `site blocked` on every retry for the whole match, and never got
            // the farm. `"site":"nearest legal site"` accepts the hint in
            // advance, through the same `nearest_free_site` the hint comes
            // from — so what the selector picks is legal by construction rather
            // than by two functions agreeing.
            let (px, pz) = match &site {
                None => (px, pz),
                Some(raw) => {
                    let sel = parse_selector(raw).ok_or_else(|| unknown_selector(raw))?;
                    if sel != Selector::NearestLegalSite {
                        return Err(format!(
                            "build: '{raw}' does not name a site — the site selector is \
                             'nearest legal site'"
                        ));
                    }
                    // An unknown kind is the build arm's error to report, in its
                    // own words. Leave the point alone and let it.
                    match parse_building_kind(&kind) {
                        None => (px, pz),
                        Some(k) => {
                            let size = building_stats(k).size;
                            let want = snap_footprint(clamp_to_map(Vec3::new(px, 0.0, pz)), size);
                            match nearest_free_site(bind.nav, want, size, PLACEMENT_HINT_RADIUS) {
                                Some(p) => (p.x, p.z),
                                None => {
                                    return Err(format!(
                                        "build: no legal site for {} within \
                                         {PLACEMENT_HINT_RADIUS:.0} of ({px:.1}, {pz:.1}) — \
                                         name somewhere else",
                                        building_name(k)
                                    ))
                                }
                            }
                        }
                    }
                }
            };
            Intent::Build {
                worker,
                kind,
                x: Some(px),
                z: Some(pz),
                region,
                select,
                site,
            }
        }
        Intent::Cast {
            hero,
            ability,
            x,
            z,
            target,
            select,
        } => Intent::Cast {
            hero: soloist("cast", hero, &select, bind)?,
            ability,
            x,
            z,
            target,
            select,
        },
        Intent::Priority {
            units,
            classes,
            select,
        } => Intent::Priority {
            units: crew("priority", units, &select, bind)?,
            classes,
            select,
        },
        Intent::Autocast {
            units,
            min_enemies,
            ability,
            select,
        } => Intent::Autocast {
            units: crew("autocast", units, &select, bind)?,
            min_enemies,
            ability,
            select,
        },
        Intent::Squad { units, id, select } => Intent::Squad {
            units: crew("squad", units, &select, bind)?,
            id,
            select,
        },
        // Rally, retreat and leash each have a legal placeless form (a rally
        // onto a unit; the two doctrine verbs' CLEAR spelling), so an absent
        // place is not an error here — only an unresolvable name is.
        Intent::Rally {
            building,
            x,
            z,
            region,
            target,
            select,
        } => {
            let building = producer("rally", building, &select, bind)?;
            let (x, z, _) = shape(regions, me, x, z, &region)?;
            Intent::Rally {
                building,
                x,
                z,
                region,
                target,
                select,
            }
        }
        Intent::Train {
            building,
            unit,
            select,
        } => Intent::Train {
            building: producer("train", building, &select, bind)?,
            unit,
            select,
        },
        Intent::Cancel {
            building,
            index,
            select,
        } => Intent::Cancel {
            building: producer("cancel", building, &select, bind)?,
            index,
            select,
        },
        Intent::Template {
            building,
            squad,
            retreat,
            priority,
            autocast,
            select,
        } => Intent::Template {
            building: producer("template", building, &select, bind)?,
            squad,
            retreat,
            priority,
            autocast,
            select,
        },
        Intent::Retreat {
            units,
            below,
            x,
            z,
            region,
            select,
        } => {
            let units = crew("retreat", units, &select, bind)?;
            let (x, z, _) = shape(regions, me, x, z, &region)?;
            Intent::Retreat {
                units,
                below,
                x,
                z,
                region,
                select,
            }
        }
        Intent::Leash {
            units,
            x,
            z,
            region,
            radius,
            select,
        } => {
            let units = crew("leash", units, &select, bind)?;
            let (x, z, from_region) = shape(regions, me, x, z, &region)?;
            Intent::Leash {
                units,
                x,
                z,
                region,
                // An explicit radius still wins: naming a circle is a
                // convenience, never a ceiling on what you may say.
                radius: radius.or(from_region),
                select,
            }
        }
        Intent::Posture { id, posture } => {
            let posture = match posture {
                Some(PostureIntent::Defend { x, z, region, radius }) => {
                    let (x, z, from_region) = shape(regions, me, x, z, &region)?;
                    let (px, pz) = both("defend", x, z)?;
                    let radius = radius.or(from_region);
                    // The one place a missing radius is fatal: `defend` is a
                    // ring, and a ring with no size is not a posture.
                    let Some(radius) = radius else {
                        return Err(
                            "defend needs a radius, or a region whose own radius \
                             can be the ring"
                                .to_string(),
                        );
                    };
                    Some(PostureIntent::Defend {
                        x: Some(px),
                        z: Some(pz),
                        region,
                        radius: Some(radius),
                    })
                }
                Some(PostureIntent::Push { x, z, region }) => {
                    let (x, z, _) = shape(regions, me, x, z, &region)?;
                    let (px, pz) = both("push", x, z)?;
                    Some(PostureIntent::Push {
                        x: Some(px),
                        z: Some(pz),
                        region,
                    })
                }
                Some(PostureIntent::Forage { x, z, region }) => {
                    let (x, z, _) = shape(regions, me, x, z, &region)?;
                    let (px, pz) = both("forage", x, z)?;
                    Some(PostureIntent::Forage {
                        x: Some(px),
                        z: Some(pz),
                        region,
                    })
                }
                other => other,
            };
            Intent::Posture { id, posture }
        }
        // A stance's anchor resolves exactly like every other place, and the
        // region's own RADIUS is dropped on the floor — the third caller to do
        // so, alongside `push` and `forage`, and here for a sharper reason. A
        // stance is a fixed preset; if naming a wide region silently widened
        // the ring, two commanders saying `secure` would have two different
        // doctrines and the arena could not compare them. `posture defend` is
        // still there for anyone who wants the region to be the ring.
        //
        // A missing anchor is NOT an error here: the compiler defaults it to the
        // issuing team's own base, which is what `turtle` means with no
        // argument. See the `Stance` arm.
        Intent::Stance { squad, stance, x, z, region } => {
            let (x, z, _) = shape(regions, me, x, z, &region)?;
            Intent::Stance { squad, stance, x, z, region }
        }
        other => other,
    })
}

#[allow(clippy::too_many_arguments)]
fn compile_intent(
    intent: Intent,
    me: Team,
    tag: &str,
    mark: IntentMark,
    errors: &mut Vec<String>,
    ai_controlled: &mut AiControlled,
    economies: &Economies,
    records: &HeroRecords,
    tier: TechTier,
    races: Races,
    nav: &NavGrid,
    team_research: &TeamResearch,
    fog: &FogGrid,
    squad_orders: &mut SquadOrders,
    // The stance word behind each squad's posture, on the same one-writer rule
    // as `squad_orders` above it: two verbs here write it (`stance` sets it,
    // `posture` clears it, because a hand-tasked squad is no longer in one) and
    // the snapshot and the HUD read it.
    squad_stances: &mut SquadStances,
    // The armed triggers of every team. Written by two verbs here and read by
    // trigger.rs's evaluator, bridge.rs's snapshot and ui.rs's HUD — same
    // shape, and the same one-writer rule, as `SquadOrders` above it.
    triggers: &mut Triggers,
    // Every team's plans, on the same one-writer rule as `triggers` beside it:
    // two verbs here write them, plan.rs's evaluator and the two renderers
    // read them.
    plans: &mut Plans,
    // The named geography, third on the same rule. Written by the two
    // `region_*` verbs here; read by `resolve_places` at the top of this
    // function, by trigger.rs's `enemy_in`, by the plan-step validation below,
    // by bridge.rs's snapshot and by ui.rs's map.
    regions: &mut Regions,
    commands: &mut Commands,
    events: &mut IntentEvents,
    world: &mut IntentWorld,
    // docs/TEMPO.md §3. Every *direct* unit order below goes through this
    // instead of `commands.entity(e).try_insert(order)`, and that one
    // substitution is the whole of Chain of Command on the player path. Which
    // verbs are direct — and why the rest are not — is the table in
    // command.rs's module docs. Production, doctrine, casts and match-level
    // verbs keep writing straight through: standing orders are the fast path,
    // and that asymmetry IS the mechanism.
    issuer: &mut OrderIssuer,
    // **Did this intent reach anything at all?** Written only by the group-unit
    // paths, and read by exactly one caller: the plan evaluator's verdict.
    //
    // It exists because `errors` alone cannot answer "was this refused?".
    // `own_units` deliberately reports each dead id and returns the survivors,
    // so `move [a,b]` with `b` a corpse *moves a* and still pushes an error.
    // Every other channel wants that error; a plan wants to know whether to
    // carry on, and a plan that blocked — and eventually halted — because one
    // member of a squad had died would stop for the most ordinary event in the
    // game. Errors with `reached` are a partial success; errors without it are
    // a refusal.
    reached: &mut bool,
    // **Squad membership written EARLIER IN THIS BATCH**, entity → new squad
    // (`None` = removed from any squad).
    //
    // This exists because of the flush rule §6.3 of tools/BUILDER_BRIEF.md
    // states: `squad` writes `SquadId` through `Commands`, and Bevy does not
    // apply a command queue until the system ends — so within one
    // `apply_intents` the insert is invisible to every query. Nothing cared
    // until `stance`, which is the only verb here that finds its units *by
    // membership* rather than from an id list. Without this map, the perfectly
    // ordinary batch
    //
    //     [{"type":"squad","units":[1,2,3],"id":1},
    //      {"type":"stance","squad":1,"stance":"push","target":"mid"}]
    //
    // would set squad 1's posture (per-squad, so it lands) and silently skip
    // its leash, threshold and focus list (per-unit, so they find nobody) —
    // half a doctrine installed, with nothing said about the other half. Last
    // writer wins inside the batch, exactly as it does outside it.
    batch_squads: &mut std::collections::BTreeMap<Entity, Option<u8>>,
) {
    // Named locally so the arms below read exactly as they did when this was
    // one interface's private applier.
    let IntentWorld {
        bind,
        targets,
        researching,
    } = world;
    let LateBindWorld {
        units,
        nodes,
        squads,
        buildings,
    } = bind;
    // Names become coordinates and roles become rosters here and nowhere else.
    // Everything below this line sees the language it has always seen.
    let intent = match resolve_places(
        intent,
        &LateBind {
            me,
            regions,
            units,
            squads,
            nodes,
            buildings,
            nav,
        },
    ) {
        Ok(intent) => intent,
        Err(err) => {
            errors.push(format!("{tag}: {err}"));
            return;
        }
    };
    /// The ground a resolved intent names. `resolve_places` has already refused
    /// the placeless form of every verb that calls this, so `None` is
    /// unreachable — the re-check is defence against a future arm forgetting to
    /// go through the resolver, and it refuses rather than panicking.
    fn resolved_point(x: Option<f32>, z: Option<f32>) -> Option<Vec3> {
        match (x, z) {
            (Some(x), Some(z)) => Some(Vec3::new(x, 0.0, z)),
            _ => None,
        }
    }
    /// The same defence for the building channel: `resolve_places`'s `producer`
    /// has already refused an intent that names neither an id nor a phrase, so
    /// this is the arm's guard against a future path that skipped the resolver.
    fn needs_building(verb: &str) -> String {
        format!(
            "{verb} needs a building id or a 'select' phrase — \
             building selectors are: {SELECTOR_BUILDING_NAMES}"
        )
    }
    match intent {
        Intent::Move { units: ids, x, z, .. } => {
            let Some(target) = resolved_point(x, z) else {
                errors.push(format!("{tag}: move needs x/z or a region name"));
                return;
            };
            ground_order(
                commands,
                errors,
                tag,
                mark.order("move"),
                &ids,
                units,
                me,
                target,
                false,
                issuer,
                reached,
            );
        }
        Intent::AttackMove { units: ids, x, z, .. } => {
            let Some(target) = resolved_point(x, z) else {
                errors.push(format!("{tag}: attackmove needs x/z or a region name"));
                return;
            };
            ground_order(
                commands,
                errors,
                tag,
                mark.order("attackmove"),
                &ids,
                units,
                me,
                target,
                true,
                issuer,
                reached,
            );
        }
        Intent::Attack {
            units: ids, target, ..
        } => {
            let Some(target_entity) = intent_entity(target) else {
                errors.push(format!("{tag}: target {target} not found"));
                return;
            };
            match targets.get(target_entity) {
                Ok((team, unit, building, tf)) => {
                    // Only the seat's enemy is a legal attack target.
                    if *team != me.enemy() {
                        errors.push(format!("{tag}: target {target} is your own"));
                        return;
                    }
                    if unit.is_none() && building.is_none() {
                        errors.push(format!("{tag}: target {target} is not attackable"));
                        return;
                    }
                    // Fog cuts both ways: a snapshot that will not show you
                    // an enemy must not accept orders against it either,
                    // or the filtering is decoration. Visible now, or a
                    // structure we remember, is the whole legal set — the
                    // same set the player can click on.
                    if !fog.knows_entity(target, tf.translation) {
                        errors.push(format!("{tag}: target {target} is not visible"));
                        return;
                    }
                }
                Err(_) => {
                    errors.push(format!("{tag}: target {target} not found"));
                    return;
                }
            }
            // The link is measured from the unit being ordered, not from its
            // target: what is slow is reaching your own soldier, not reaching
            // the enemy. The reason travels with the order and is re-timed to
            // its arrival — see `command::dispatch_pending`.
            for (entity, pos) in own_units(&ids, units, me, tag, errors, reached) {
                issuer.issue(
                    commands,
                    me,
                    pos,
                    entity,
                    Order::Attack(target_entity),
                    mark.order("attack"),
                );
            }
        }
        Intent::Harvest {
            units: ids, target, ..
        } => {
            // `resolve_places` has already turned a `target_select` into an id,
            // so `None` here means the sentence named no node at all.
            let Some(target) = target else {
                errors.push(format!(
                    "{tag}: harvest needs target (a node id) or target_select \
                     (\"nearest tree\", \"nearest mine\")"
                ));
                return;
            };
            // Resource nodes are neutral: either seat may harvest any of
            // them.
            let node = match intent_entity(target).filter(|e| nodes.get(*e).is_ok()) {
                Some(node) => node,
                None => {
                    errors.push(format!("{tag}: resource node {target} not found"));
                    return;
                }
            };
            // A mined-out gold mine stays on the board as geography (economy.rs:
            // `mine_dry`, the income alarm and `mines[].remaining` all need a dry
            // mine they can look at), so its id still resolves. Say so here
            // rather than let it through: `harvest_loop` would silently
            // re-target the crew to the nearest live node, and a worker doing
            // something you did not ask for is worse than a refusal that names
            // the way to ask for it.
            if nodes.get(node).is_ok_and(|(_, n, _)| n.remaining == 0) {
                errors.push(format!(
                    "{tag}: resource node {target} is empty — use target_select \
                     \"nearest mine\" (or \"nearest tree\") for the closest one \
                     with anything left in it"
                ));
                return;
            }
            // `reached` is recomputed rather than inherited from `own_units`:
            // every survivor can still be skipped here for not being a worker,
            // and a `harvest` that ordered nobody is a refusal, not a partial.
            let mut sent = 0usize;
            for (entity, pos) in own_units(&ids, units, me, tag, errors, &mut false) {
                // Only workers can gather; anyone else would just stand there.
                if !is_worker(units, entity) {
                    errors.push(format!(
                        "{tag}: unit {} is not a Worker",
                        entity.to_bits()
                    ));
                    continue;
                }
                sent += 1;
                issuer.issue(
                    commands,
                    me,
                    pos,
                    entity,
                    Order::Harvest(node),
                    mark.order("harvest"),
                );
            }
            *reached |= sent > 0;
        }
        Intent::Return { units: ids, .. } => {
            for (entity, pos) in own_units(&ids, units, me, tag, errors, reached) {
                issuer.issue(
                    commands,
                    me,
                    pos,
                    entity,
                    Order::ReturnResources,
                    mark.order("return"),
                );
            }
        }
        Intent::Follow {
            units: ids, target, ..
        } => {
            let Some(target) = target else {
                errors.push(format!(
                    "{tag}: follow needs target (a unit id) or target_select \
                     (e.g. \"my hero\")"
                ));
                return;
            };
            let leader = match intent_entity(target) {
                Some(e) => match units.get(e) {
                    Ok((_, _, team, ..)) if *team == me => e,
                    _ => {
                        errors.push(format!("{tag}: unit {target} not found/not yours"));
                        return;
                    }
                },
                None => {
                    errors.push(format!("{tag}: unit {target} not found/not yours"));
                    return;
                }
            };
            for (entity, pos) in own_units(&ids, units, me, tag, errors, reached) {
                if entity == leader {
                    continue; // a unit following itself would deadlock its own order
                }
                issuer.issue(
                    commands,
                    me,
                    pos,
                    entity,
                    Order::Follow(leader),
                    mark.order("follow"),
                );
            }
        }
        Intent::Stop { units: ids, .. } => {
            // The established Stop: re-issue a Move to the unit's own spot,
            // which halts it and clears any attack target. It is a direct
            // order like any other — "halt" travels down the same wire as
            // "advance", which is what stops latency from being escapable by
            // spamming stop.
            for (entity, pos) in own_units(&ids, units, me, tag, errors, reached) {
                issuer.issue(commands, me, pos, entity, Order::Move(pos), mark.order("stop"));
            }
        }
        Intent::Build {
            worker,
            kind,
            x,
            z,
            ..
        } => {
            let Some(site) = resolved_point(x, z) else {
                errors.push(format!("{tag}: build needs x/z or a region name"));
                return;
            };
            let Some(building_kind) = parse_building_kind(&kind) else {
                errors.push(format!("{tag}: unknown building kind '{kind}'"));
                return;
            };
            // `resolve_places` has already turned a `select` into an id.
            let Some(worker) = worker else {
                errors.push(format!(
                    "{tag}: build needs worker (a unit id) or select (e.g. \"workers\")"
                ));
                return;
            };
            let Some((entity, _)) = own_unit(worker, units, me) else {
                errors.push(format!("{tag}: unit {worker} not found/not yours"));
                return;
            };
            if !is_worker(units, entity) {
                errors.push(format!("{tag}: unit {worker} is not a Worker"));
                return;
            }
            // The ROSTER gate, ahead of the tech gate: a building this team's
            // race does not have is not a missing requirement, it is a
            // different game, and saying so plainly is more use to a commander
            // than a list of prerequisites it can never meet. economy.rs
            // re-checks this at the pay-point; this is the error string.
            if !race_has_building(races.get(me), building_kind) {
                errors.push(format!(
                    "{tag}: {} is not in the {} roster",
                    building_name(building_kind),
                    races.get(me).name()
                ));
                return;
            }
            // Same tech gate economy.rs applies at placement — reported
            // here so the commander learns why instead of watching a
            // worker walk out and come back empty-handed.
            if let Some(err) = requirement_error(
                tag,
                building_name(building_kind),
                building_requires(building_kind),
                &completed_kinds(buildings, me),
            ) {
                errors.push(err);
                return;
            }
            let stats = building_stats(building_kind);
            // Snap to nav-cell boundaries exactly like the placement ghost.
            let pos = snap_footprint(clamp_to_map(site), stats.size);
            if !nav.rect_is_free(pos, stats.size) {
                errors.push(format!(
                    "{tag}: {}",
                    blocked_site_error(nav, pos, building_kind)
                ));
                return;
            }
            if !economies
                .get(me)
                .can_afford(stats.cost_gold, stats.cost_lumber)
            {
                errors.push(format!(
                    "{tag}: cannot afford {kind} ({}g {}l)",
                    stats.cost_gold, stats.cost_lumber
                ));
                return;
            }
            // economy.rs pays when the worker reaches the site, same as the UI.
            //
            // EXEMPT from link latency, per docs/TEMPO.md §4's open question,
            // answered as it recommends: the worker has to walk to the site
            // anyway, so the delay would be invisible at the point of contact
            // and would show up only as a slower economy. Taxing the build
            // order taxes macro, and macro is not the thing reaction speed was
            // winning. It still stamps its reason like every other verb.
            issuer.issue_instant(
                commands,
                entity,
                Order::Build {
                    kind: building_kind,
                    pos,
                },
                mark.order("build"),
            );
        }
        Intent::Upgrade { building } => {
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, b, team, under, _, upgrading)) = buildings.get(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            if under.is_some() {
                errors.push(format!("{tag}: building {building} is under construction"));
                return;
            }
            if upgrading.is_some() {
                errors.push(format!("{tag}: building {building} is already upgrading"));
                return;
            }
            let name = building_name(b.kind);
            let Some((cost_gold, cost_lumber, _)) = upgrade_cost(b.kind) else {
                errors.push(format!("{tag}: {name} has no upgrade"));
                return;
            };
            if !economies.get(me).can_afford(cost_gold, cost_lumber) {
                let to = building_name(
                    building_upgrades_to(b.kind).expect("a cost implies a next tier"),
                );
                errors.push(format!(
                    "{tag}: cannot afford {to} ({cost_gold}g {cost_lumber}l)"
                ));
                return;
            }
            // economy.rs takes the money and starts the conversion — the
            // same single owner of every payment the UI goes through.
            events.upgrades.write(UpgradeBuilding { building: entity });
        }
        Intent::Train { building, unit, .. } => {
            let Some(building) = building else {
                errors.push(format!("{tag}: {}", needs_building("train")));
                return;
            };
            let Some(kind) = parse_unit_kind(&unit) else {
                errors.push(format!("{tag}: unknown unit kind '{unit}'"));
                return;
            };
            // Read the tech state before taking the mutable borrow of the
            // producing building below.
            let completed = completed_kinds(buildings, me);
            // NOT the generic `requirement_error`: a training gate is refused
            // at the very building that will train the unit once the gate is
            // met, and `X requires Y` read there sends the commander looking
            // for a different building. See `shared::train_gate_error`.
            if let Some(err) = train_gate_error(kind, &completed) {
                errors.push(format!("{tag}: {err}"));
                return;
            }
            // Hero slots. economy.rs is the authoritative gate (it enforces
            // at the pay-point, where the money and the race conditions are);
            // this is the same rule stated early so a seat gets an error
            // string back instead of watching the item vanish unpaid off the
            // front of its queue three seconds later.
            //
            // The count is living heroes PLUS every hero already sitting in
            // any of this team's queues — the edge case that makes this worth
            // writing at all: two halls each queuing a Priestess, or one hall
            // queuing three Champions, are both "in flight" and neither is
            // alive yet.
            //
            // The same list prices the hero below: a first hero already in a
            // queue has no record yet, so `held` is the only thing that knows
            // the team has spent its one free hero (`shared::hero_train_cost`).
            let mut held: Vec<UnitKind> = Vec::new();
            if is_hero_kind(kind) {
                held.extend(
                    units
                        .iter()
                        .filter(|(_, u, t, ..)| **t == me && is_hero_kind(u.kind))
                        .map(|(_, u, ..)| u.kind),
                );
                for (_, _, b_team, _, b_queue, _) in buildings.iter() {
                    if *b_team != me {
                        continue;
                    }
                    let Some(b_queue) = b_queue else { continue };
                    held.extend(b_queue.queue.iter().copied().filter(|k| is_hero_kind(*k)));
                }
                match hero_slot_check(&held, kind, tier) {
                    HeroSlotVerdict::Ok => {}
                    HeroSlotVerdict::DuplicateClass => {
                        errors.push(format!(
                            "{tag}: {} already fielded or queued (heroes are one per class)",
                            kind_name(kind)
                        ));
                        return;
                    }
                    HeroSlotVerdict::NoSlot { used, slots } => {
                        errors.push(format!(
                            "{tag}: hero slots full ({used}/{slots} at tier {}) — \
                             upgrade a hall for another",
                            tier.level()
                        ));
                        return;
                    }
                }
            }
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, b, team, under, queue, _)) = buildings.get_mut(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            if under.is_some() {
                errors.push(format!("{tag}: building {building} is under construction"));
                return;
            }
            if !trainable(b.kind).contains(&kind) {
                errors.push(format!(
                    "{tag}: {}",
                    wrong_trainer_error(b.kind, kind, &completed)
                ));
                return;
            }
            let Some(mut queue) = queue else {
                errors.push(format!("{tag}: building {building} has no training queue"));
                return;
            };
            if queue.queue.len() >= MAX_QUEUE {
                errors.push(format!("{tag}: training queue full ({MAX_QUEUE})"));
                return;
            }
            // Hero classes are priced by `hero_train_cost` (free once, then
            // full fare) — every hero kind, not just the Champion: pricing
            // the Priestess off her raw stats let a seat buy a revival at
            // full price (or worse, a first hero cheaply) depending on the
            // record. `is_hero_kind` is the same test economy.rs charges by,
            // and `held` is the same list economy.rs prices with.
            let (cost_gold, cost_lumber) = if is_hero_kind(kind) {
                let (g, l, _) = hero_train_cost(records, me, kind, &held);
                (g, l)
            } else {
                let s = unit_stats(kind);
                (s.cost_gold, s.cost_lumber)
            };
            if !economies.get(me).can_afford(cost_gold, cost_lumber) {
                // A hero that costs anything at all is a hero this team is not
                // fielding for the first time, and a commander who has read
                // "heroes are free" needs to be told which rule just charged
                // them rather than left to re-read the brief.
                let why = if is_hero_kind(kind) {
                    if records.get(me, kind).is_some() {
                        " — reviving a class you have lost"
                    } else {
                        " — only your FIRST hero is free"
                    }
                } else {
                    ""
                };
                errors.push(format!(
                    "{tag}: cannot afford {unit} ({cost_gold}g {cost_lumber}l){why}"
                ));
                return;
            }
            // Gate only — economy.rs deducts when training starts.
            queue.queue.push_back(kind);
        }
        Intent::Cancel { building, index, .. } => {
            let Some(building) = building else {
                errors.push(format!("{tag}: {}", needs_building("cancel")));
                return;
            };
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, _, team, _, queue, _)) = buildings.get_mut(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            let Some(mut queue) = queue else {
                errors.push(format!("{tag}: building {building} has no training queue"));
                return;
            };
            if index >= queue.queue.len() {
                errors.push(format!("{tag}: queue index {index} out of range"));
                return;
            }
            queue.queue.remove(index);
            if index == 0 {
                queue.progress = 0.0;
            }
        }
        Intent::Research { building, upgrade } => {
            let Some(kind) = parse_research_kind(&upgrade) else {
                errors.push(format!("{tag}: unknown research '{upgrade}'"));
                return;
            };
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, b, team, under, _, upgrading)) = buildings.get(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            // Ownership first, and phrased identically to every other building
            // verb: a seat must not be able to tell "not yours" from "does not
            // exist", or the error message becomes a scouting tool.
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            if !building_researches(b.kind).contains(&kind) {
                errors.push(format!(
                    "{tag}: {} cannot research {}",
                    building_name(b.kind),
                    kind.id()
                ));
                return;
            }
            if under.is_some() {
                errors.push(format!("{tag}: building {building} is under construction"));
                return;
            }
            // A forge converting into something else is not a forge right now.
            // Unreachable today (no ladder runs through a Blacksmith) and
            // checked anyway, because `Upgrading` freezes training for the same
            // reason and the two ought to agree.
            if upgrading.is_some() {
                errors.push(format!("{tag}: building {building} is already upgrading"));
                return;
            }
            // One job per forge, rejected rather than queued — see `Researching`.
            if let Ok(active) = researching.get(entity) {
                errors.push(format!(
                    "{tag}: building {building} is already researching {} ({:.0}s left)",
                    active.kind.id(),
                    active.remaining.max(0.0)
                ));
                return;
            }
            let level = team_research.get(me).level(kind);
            let Some(step) = research_step(kind, level + 1) else {
                errors.push(format!(
                    "{tag}: {} is already at max level ({RESEARCH_MAX_LEVEL})",
                    kind.id()
                ));
                return;
            };
            if !economies.get(me).can_afford(step.cost_gold, step.cost_lumber) {
                errors.push(format!(
                    "{tag}: cannot afford {} {} ({}g {}l)",
                    kind.id(),
                    step.level,
                    step.cost_gold,
                    step.cost_lumber
                ));
                return;
            }
            // economy.rs takes the money and starts the clock — the same single
            // owner of every payment `upgrade` and `build` go through.
            events.research.write(StartResearch {
                building: entity,
                kind,
            });
        }
        Intent::Rally {
            building,
            x,
            z,
            target,
            ..
        } => {
            let Some(building) = building else {
                errors.push(format!("{tag}: {}", needs_building("rally")));
                return;
            };
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, b, team, _, _, _)) = buildings.get(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            if trainable(b.kind).is_empty() {
                errors.push(format!(
                    "{tag}: {} produces no units",
                    building_name(b.kind)
                ));
                return;
            }
            let rally = match (x, z, target) {
                (Some(x), Some(z), _) => {
                    Some(RallyTarget::Ground(clamp_to_map(Vec3::new(x, 0.0, z))))
                }
                (_, _, Some(id)) => match intent_entity(id) {
                    // A resource node (neutral, so either seat may name
                    // one) makes new workers start gathering; one of our
                    // own units makes new units follow it.
                    Some(e) if nodes.get(e).is_ok() => Some(RallyTarget::Node(e)),
                    Some(e) => match units.get(e) {
                        Ok((_, _, team, ..)) if *team == me => Some(RallyTarget::Unit(e)),
                        _ => None,
                    },
                    None => None,
                },
                _ => None,
            };
            match rally {
                Some(target) => {
                    commands.entity(entity).try_insert(RallyPoint { target });
                }
                None => errors.push(format!(
                    "{tag}: rally needs x/z or a valid node/own-unit target"
                )),
            }
        }
        Intent::Cast {
            hero,
            ability,
            x,
            z,
            target,
            ..
        } => {
            // `resolve_places` has already turned a `select` into an id, so a
            // missing one means the sentence named no caster at all. Named
            // rather than defaulted: a cast that quietly picked *some* hero is
            // the bug `own_hero`'s doc comment describes at length.
            let Some(hero) = hero else {
                errors.push(format!(
                    "{tag}: cast needs hero/caster (an id) or select (e.g. \"my hero\")"
                ));
                return;
            };
            let Some(entity) = intent_entity(hero) else {
                errors.push(format!("{tag}: caster {hero} not found/not yours"));
                return;
            };
            // A caster is either one of our heroes (any class — the Hero
            // component and the unit ability table agree on which kinds
            // have one) or one of our finished buildings with an ability.
            // combat.rs owns the unlock/mana/cooldown verdict either way,
            // exactly as it does for the R and C hotkeys.
            //
            // WHAT THE ID NAMES, resolved once into owned facts. Two reasons
            // it is shaped this way rather than as a chain of fallthroughs:
            // the borrow ends here, so each failure below can re-ask nothing;
            // and there are FOUR distinct ways to fail, which the old single
            // fallthrough flattened into one sentence.
            //
            // `wc3clone-d4y`, round-10 AAR: a commander cast Call to Arms at
            // their expansion TownHall and read
            // `caster N is not a hero or an own ability building`. Every word
            // of that points at the tech tree, so they checked the catalog,
            // found TownHall listed as a Call to Arms caster, and filed a bug
            // against the roster. The roster was right and the compiler was
            // right — *every* hall is a caster and a second one resolves
            // exactly like the first (`a_second_hall_casts_call_to_arms_like_
            // the_first` pins that). What the id named was something the
            // buildings query could not find at all: a dead entity, or one
            // never in it. The engine knew which; the string refused to say.
            let unit_hit = units
                .get(entity)
                .ok()
                .map(|(_, u, team, tf, ..)| (u.kind, *team, tf.translation));
            let building_hit = buildings
                .get(entity)
                .ok()
                .map(|(_, b, team, under, _, _)| (b.kind, *team, under.is_some()));

            // Where the caster is standing, when it is a unit. `None` means a
            // building caster, which needs no position: `abilities_of_building`
            // is `is_hall`-only and a hall IS a command node, so its link is
            // provably zero (`every_building_caster_is_a_command_node`).
            let mut caster_pos: Option<Vec3> = None;
            let list = match (unit_hit, building_hit) {
                (Some((kind, team, pos)), _) if team == me => {
                    let list = abilities_of_unit(kind);
                    if list.is_empty() {
                        // Previously fell through to the building lookup and
                        // came back as "not a hero or an own ability
                        // building" — true of a Footman, and no help at all.
                        errors.push(format!("{tag}: {} has no ability", kind_name(kind)));
                        return;
                    }
                    caster_pos = Some(pos);
                    list
                }
                (_, Some((kind, team, under))) if team == me => {
                    if under {
                        errors.push(format!("{tag}: building {hero} is under construction"));
                        return;
                    }
                    let list = abilities_of_building(kind);
                    if list.is_empty() {
                        errors.push(format!("{tag}: {} has no ability", building_name(kind)));
                        return;
                    }
                    list
                }
                // It exists and it is the enemy's. Say so: "not yours" is a
                // fact about ownership, and sending the reader to the tech
                // tree for it costs a minute of the wrong investigation.
                (Some(_), _) | (_, Some(_)) => {
                    errors.push(format!("{tag}: caster {hero} is not yours"));
                    return;
                }
                // Nothing has that id. The overwhelmingly likely cause is a
                // snapshot that has aged out from under the batch, which is
                // the one explanation the reader can act on.
                (None, None) => {
                    errors.push(format!(
                        "{tag}: caster {hero} not found — no unit or building has that id \
                         (it may have died since the snapshot you read)"
                    ));
                    return;
                }
            };
            // A named slot is checked for EXISTENCE here so a typo is an
            // error instead of a silent no-op; whether it is unlocked and
            // off cooldown stays combat.rs's call.
            let selector = match &ability {
                None => None,
                Some(reference) => {
                    if ability_slot(reference, list).is_none() {
                        errors.push(format!(
                            "{tag}: caster {hero} has no ability {reference:?} (has {})",
                            list.iter()
                                .map(|d| d.name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                        return;
                    }
                    Some(reference.clone())
                }
            };

            // --- geometry ------------------------------------------------
            //
            // WHICH DEF is being aimed. An explicit selector names it outright;
            // with no selector, a caster with a single ability still has only
            // one answer (every shipping targeted caster — the Sorcerer — is
            // that case, and so is every ability building). A caster with
            // several abilities and no selector is genuinely ambiguous here,
            // because "first UNLOCKED" is combat.rs's question and needs a
            // hero level this query does not carry; those fall through
            // un-checked and combat.rs fizzles them if the aim is bad.
            let aimed_def = ability_slot_of(&selector, list)
                .or(if list.len() == 1 { Some(0) } else { None })
                .map(|i| list[i]);

            // Where the caster stands. Buildings answer through `targets`,
            // which carries a `Transform` for anything with a `Team` — a
            // building caster needed no position before this bead, because a
            // hall is a command node and its link is provably zero.
            let origin = caster_pos.or_else(|| targets.get(entity).ok().map(|(_, _, _, tf)| tf.translation));

            let requested = match (x, z, target) {
                (Some(x), Some(z), None) => Some(CastTarget::Point(Vec3::new(x, 0.0, z))),
                (None, None, Some(id)) => {
                    let Some(victim) = intent_entity(id) else {
                        errors.push(format!("{tag}: cast target {id} not found"));
                        return;
                    };
                    Some(CastTarget::Unit(victim))
                }
                (None, None, None) => None,
                // Half a point, or a point AND a unit: two aims are not an
                // aim. Said plainly rather than by picking one.
                _ => {
                    errors.push(format!(
                        "{tag}: cast wants either x AND z, or target, or neither — not a mixture"
                    ));
                    return;
                }
            };

            // A payload is only meaningful if the ability takes one, and only
            // legal if it is inside the ability's reach. Both are refused
            // rather than clamped or walked into: see the note on
            // `AbilityTarget` — a caster that closes the distance by itself is
            // a caster back in the front line, which is the exact failure
            // targeted casting exists to end.
            if let (Some(def), Some(want)) = (aimed_def, requested) {
                let Some(range) = def.target.range() else {
                    errors.push(format!(
                        "{tag}: {} is cast on the caster and takes no target — send just \
                         the caster and the ability",
                        def.name
                    ));
                    return;
                };
                let aim_pos = match want {
                    CastTarget::Point(p) => Some(p),
                    CastTarget::Unit(victim) => {
                        if !def.target.wants_unit() {
                            errors.push(format!(
                                "{tag}: {} is cast at a POINT — send x and z, not target",
                                def.name
                            ));
                            return;
                        }
                        match targets.get(victim) {
                            Ok((_, unit, _, tf)) => {
                                if unit.is_none() {
                                    // Named by the id the commander sent, not
                                    // by an `Entity` debug string they have
                                    // never seen and cannot look up.
                                    errors.push(format!(
                                        "{tag}: {} is cast on a unit; {} is a building",
                                        def.name,
                                        intent_id(victim)
                                    ));
                                    return;
                                }
                                Some(tf.translation)
                            }
                            Err(_) => {
                                errors.push(format!("{tag}: cast target not found"));
                                return;
                            }
                        }
                    }
                };
                if let (Some(from), Some(to)) = (origin, aim_pos) {
                    let reach = Vec2::new(from.x - to.x, from.z - to.z).length();
                    if reach > range {
                        // Both numbers, because "out of range" without them
                        // sends the reader to the catalog to find one of them
                        // and to the snapshot to compute the other.
                        errors.push(format!(
                            "{tag}: {} reaches {range:.0} and that point is {reach:.1} away \
                             — move the caster closer or aim nearer (the caster is not \
                             walked into range: casting from behind your line is the point)",
                            def.name
                        ));
                        return;
                    }
                }
            }

            // A DIRECT ORDER, and priced like one (docs/TEMPO.md §7). For a
            // hero this computes zero — a hero is a command node, so hero
            // micro is exactly as fast as it always was — and for a Sorcerer
            // standing in the middle of a fight it costs what reaching that
            // far costs. The player who would rather not pay it has the
            // `autocast` verb, which is instant, and that is the whole design.
            match caster_pos {
                Some(pos) => issuer.issue_cast(
                    commands,
                    &mut events.casts,
                    me,
                    pos,
                    entity,
                    selector,
                    requested,
                ),
                None => {
                    events.casts.write(CastAbility {
                        caster: entity,
                        ability: selector,
                        target: requested,
                    });
                }
            }
        }
        Intent::Buy { shop, item, hero } => {
            let Some(item) = parse_item(&item) else {
                errors.push(format!("{tag}: unknown item '{item}'"));
                return;
            };
            let Some(entity) = intent_entity(shop) else {
                errors.push(format!("{tag}: building {shop} not found/not yours"));
                return;
            };
            let Ok((_, b, team, under, _, _)) = buildings.get(entity) else {
                errors.push(format!("{tag}: building {shop} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {shop} not found/not yours"));
                return;
            }
            if b.kind != BuildingKind::Shop {
                errors.push(format!(
                    "{tag}: {} does not sell items",
                    building_name(b.kind)
                ));
                return;
            }
            if under.is_some() {
                errors.push(format!("{tag}: building {shop} is under construction"));
                return;
            }
            // The shelf is tiered. Derived from our standing completed
            // buildings by the same function that feeds `TechTiers`, so this
            // needs no extra resource and cannot disagree with economy.rs's
            // authoritative check — it only turns a silent race-log into a
            // sentence a commander can act on.
            let def = item_def(item);
            let tier = tech_tier_for(
                buildings
                    .iter()
                    .filter(|(_, _, team, under, _, _)| **team == me && under.is_none())
                    .map(|(_, b, _, _, _, _)| b.kind),
            );
            if !item_unlocked(item, tier) {
                errors.push(format!(
                    "{tag}: {} requires tier {} (you are {})",
                    def.name,
                    def.tier.name(),
                    tier.name()
                ));
                return;
            }
            // Which hero is buying: the one named, or the lowest-id living
            // hero. A named hero that does not resolve has already logged its
            // own error — do not silently sell to somebody else.
            let named = hero;
            let Some(hero) = own_hero(units, me, named, tag, errors) else {
                if named.is_none() {
                    errors.push(format!("{tag}: no living hero to buy for"));
                }
                return;
            };
            // economy.rs re-validates and pays (gold, free slot, distance-
            // free just like the UI's Shop card).
            events.buys.write(BuyItem {
                shop: entity,
                hero,
                item,
            });
        }
        Intent::UseItem {
            slot,
            hero,
            destination,
        } => {
            if slot >= INVENTORY_SLOTS {
                errors.push(format!(
                    "{tag}: item slot {slot} out of range (0..{})",
                    INVENTORY_SLOTS - 1
                ));
                return;
            }
            let named = hero;
            let Some(hero) = own_hero(units, me, named, tag, errors) else {
                if named.is_none() {
                    errors.push(format!("{tag}: no living hero to use an item"));
                }
                return;
            };
            // WHERE the scroll lands, when the caller cared. Validated here
            // and not in combat.rs because this is the layer that can still
            // say NO out loud: a destination that is not one of your standing
            // halls is refused with a sentence, rather than silently becoming
            // "nearest" — which is precisely the outcome the field exists to
            // stop. One message for every way of getting it wrong (unknown
            // id, enemy building, a Farm, a hall still going up), because
            // "your standing hall" already names all four conditions and a
            // finer-grained answer would leak the enemy's building ids.
            let destination = match destination {
                None => None,
                Some(id) => {
                    let hall = intent_entity(id)
                        .and_then(|e| buildings.get(e).ok().map(|b| (e, b)))
                        .filter(|(_, (_, b, team, under, _, _))| {
                            **team == me && under.is_none() && is_hall(b.kind)
                        })
                        .map(|(e, _)| e);
                    if hall.is_none() {
                        errors
                            .push(format!("{tag}: destination {id} is not your standing hall"));
                        return;
                    }
                    hall
                }
            };
            // combat.rs checks the slot is actually filled.
            events.item_uses.write(UseItem {
                hero,
                slot,
                destination,
            });
        }
        Intent::Autopilot { on } => {
            // Only ever this seat's own faction.
            set_autopilot(ai_controlled, me, on);
            info!(
                "bridge: autopilot {} for {:?} — scripted AI {} the macro game",
                if on { "ON" } else { "OFF" },
                me,
                if on { "takes over" } else { "releases" }
            );
        }
        Intent::Surrender => {
            info!("bridge: {:?} seat surrenders", me);
            commands.send_event(Surrender { team: me });
        }
        Intent::Ready => {
            // Deliberately unconditional and deliberately silent about
            // whether it changed anything: `ready` is idempotent, and a seat
            // that says it twice — or says it after the clock has started —
            // gets the same answer as one that says it once. `ready_gate`
            // (shared.rs) owns the decision and does the announcing; this arm
            // only carries the statement across the compiler, the same way
            // `surrender` does.
            commands.send_event(MatchReady { team: me });
        }
        Intent::Priority {
            units: ids,
            classes,
            ..
        } => {
            // One bad class name invalidates the whole list rather than
            // silently installing a priority order the commander didn't ask
            // for.
            let parsed = match parse_target_classes(&classes) {
                Ok(parsed) => parsed,
                Err(name) => {
                    errors.push(format!("{tag}: unknown target class '{name}'"));
                    return;
                }
            };
            for (entity, _) in own_units(&ids, units, me, tag, errors, reached) {
                let mut ec = commands.entity(entity);
                if parsed.is_empty() {
                    ec.try_remove::<TargetPriority>();
                } else {
                    ec.try_insert(TargetPriority(parsed.clone()));
                }
            }
        }
        Intent::Retreat {
            units: ids,
            below,
            x,
            z,
            ..
        } => {
            let below_frac = below.unwrap_or(0.0);
            let clear = below_frac == 0.0;
            if !clear && !(below_frac > 0.0 && below_frac < 1.0) {
                errors.push(format!(
                    "{tag}: retreat 'below' must be a fraction in (0,1), got {below_frac}"
                ));
                return;
            }
            let rally = match (x, z) {
                (Some(x), Some(z)) => Some(clamp_to_map(Vec3::new(x, 0.0, z))),
                _ => None,
            };
            if !clear && rally.is_none() {
                errors.push(format!("{tag}: retreat needs a rally x/z"));
                return;
            }
            for (entity, _) in own_units(&ids, units, me, tag, errors, reached) {
                let mut ec = commands.entity(entity);
                match rally {
                    Some(rally) if !clear => {
                        ec.try_insert(RetreatPolicy { below_frac, rally });
                    }
                    _ => {
                        ec.try_remove::<RetreatPolicy>();
                    }
                }
            }
        }
        Intent::Leash {
            units: ids,
            x,
            z,
            radius,
            ..
        } => {
            let radius = radius.unwrap_or(0.0);
            let clear = !(radius > 0.0);
            let anchor = match (x, z) {
                (Some(x), Some(z)) => Some(clamp_to_map(Vec3::new(x, 0.0, z))),
                _ => None,
            };
            if !clear && anchor.is_none() {
                errors.push(format!("{tag}: leash needs an anchor x/z"));
                return;
            }
            for (entity, _) in own_units(&ids, units, me, tag, errors, reached) {
                let mut ec = commands.entity(entity);
                match anchor {
                    Some(anchor) if !clear => {
                        ec.try_insert(LeashPolicy { anchor, radius });
                    }
                    _ => {
                        ec.try_remove::<LeashPolicy>();
                    }
                }
            }
        }
        Intent::Autocast {
            units: ids,
            min_enemies,
            ability,
            ..
        } => {
            let min_enemies = min_enemies.unwrap_or(0);
            // Recomputed for the same reason `harvest` recomputes it: a
            // selection of non-casters reaches real units and still does
            // nothing, which is a refusal rather than a partial success.
            let mut set = 0usize;
            for (entity, _) in own_units(&ids, units, me, tag, errors, &mut false) {
                // Any CASTER can auto-cast — heroes were merely the only ones
                // that existed when this verb was written. The gate is "does
                // this kind have an ability list", which is the same question
                // `Intent::Cast` already asks.
                let Ok((_, unit, _, _, policy, _)) = units.get(entity) else {
                    continue;
                };
                let list = abilities_of_unit(unit.kind);
                if list.is_empty() {
                    errors.push(format!(
                        "{tag}: unit {} has no abilities",
                        entity.to_bits()
                    ));
                    continue;
                }
                let slot = match &ability {
                    None => 0,
                    Some(reference) => match ability_slot(reference, list) {
                        Some(slot) => slot,
                        None => {
                            errors.push(format!(
                                "{tag}: unit {} has no ability {reference:?}",
                                entity.to_bits()
                            ));
                            continue;
                        }
                    },
                };
                // Edit ONE rule and keep the rest: a hero told to auto-heal
                // does not thereby stop auto-slamming.
                let mut next = policy.cloned().unwrap_or_default();
                if min_enemies == 0 {
                    next.clear_ability(slot);
                } else {
                    next.set(slot, min_enemies);
                }
                let mut ec = commands.entity(entity);
                if next.is_empty() {
                    ec.try_remove::<AutoCastPolicy>();
                } else {
                    ec.try_insert(next);
                }
                set += 1;
            }
            *reached |= set > 0;
        }
        Intent::Squad { units: ids, id, .. } => {
            for (entity, _) in own_units(&ids, units, me, tag, errors, reached) {
                let mut ec = commands.entity(entity);
                match id {
                    Some(id) => {
                        ec.try_insert(SquadId(id));
                    }
                    None => {
                        ec.try_remove::<SquadId>();
                    }
                }
                // ...and the same fact where a later sentence in this batch can
                // see it, since the insert above will not be visible to any
                // query until the system ends. See `batch_squads`.
                batch_squads.insert(entity, id);
            }
        }
        Intent::Posture { id, posture } => {
            // **A hand-set posture takes the squad out of its stance.** Not
            // because the stance stops working — the leash and the retreat
            // threshold it installed are still on the members, exactly as they
            // would be if a commander had typed the four verbs — but because
            // the WORD is no longer true, and `squads[].stance` is a readout a
            // commander steers by. A snapshot that still said "push" about a
            // squad the same commander had just told to defend would be the one
            // failure this feature cannot afford. Cleared before the early
            // return below, so the clear-posture form clears it too.
            squad_stances.0.remove(&(me, id));
            // Squad ids are per-team, so red's squad 1 and blue's squad 1
            // are different squads.
            let posture = match posture {
                None => {
                    // Clearing a posture leaves membership intact: the squad
                    // simply stops being re-tasked.
                    squad_orders.0.remove(&(me, id));
                    return;
                }
                // `resolve_places` has already turned any region into these
                // three numbers — a named region's own radius becoming the
                // ring is a mapping stated on `PostureIntent`, applied there,
                // and invisible here on purpose.
                Some(PostureIntent::Defend { x, z, radius, .. }) => {
                    let radius = radius.unwrap_or(0.0);
                    if !(radius > 0.0) {
                        errors.push(format!(
                            "{tag}: defend radius must be > 0, got {radius}"
                        ));
                        return;
                    }
                    let Some(pos) = resolved_point(x, z) else {
                        errors.push(format!("{tag}: defend needs x/z or a region name"));
                        return;
                    };
                    SquadPosture::Defend {
                        pos: clamp_to_map(pos),
                        radius,
                    }
                }
                Some(PostureIntent::Push { x, z, .. }) => {
                    let Some(pos) = resolved_point(x, z) else {
                        errors.push(format!("{tag}: push needs x/z or a region name"));
                        return;
                    };
                    SquadPosture::Push {
                        pos: clamp_to_map(pos),
                    }
                }
                Some(PostureIntent::Forage { x, z, .. }) => {
                    let Some(muster) = resolved_point(x, z) else {
                        errors.push(format!("{tag}: forage needs x/z or a region name"));
                        return;
                    };
                    SquadPosture::Forage {
                        muster: clamp_to_map(muster),
                    }
                }
                Some(PostureIntent::Escort { unit }) => {
                    let Some((target, _)) = own_unit(unit, units, me) else {
                        errors
                            .push(format!("{tag}: unit {unit} not found/not yours"));
                        return;
                    };
                    SquadPosture::Escort { unit: target }
                }
            };
            squad_orders.0.insert((me, id), posture);
        }
        // -------------------------------------------------------------------
        // Stances. Five words, each a fixed bundle of the four doctrine verbs
        // above. See shared.rs's `StanceKind` for the design and
        // assets/data/stances.ron for the numbers.
        //
        // **There is no stance machinery downstream of here.** This arm writes
        // exactly what `posture`, `leash`, `retreat` and `priority` write — the
        // same `SquadOrders` entry and the same three components, through the
        // same `try_insert`/`try_remove` — so doctrine.rs and combat.rs cannot
        // tell a stanced squad from a hand-tuned one, and a stance can never
        // acquire a behaviour the individual verbs do not have. What the word
        // buys is that all five land in one submission (no half-applied
        // doctrine if a batch is cut short) and that the engine remembers which
        // word it was.
        // -------------------------------------------------------------------
        Intent::Stance {
            squad,
            stance,
            x,
            z,
            ..
        } => {
            let Some(kind) = parse_stance(&stance) else {
                errors.push(format!(
                    "{tag}: no stance called '{stance}' - the five are: {}",
                    stance_words()
                ));
                return;
            };
            let def = kind.def();

            // The anchor. Omitted means the team's own base, which is what
            // `turtle` means with no argument and a sane floor for the rest —
            // `resolve_places` has already turned any region name into these
            // two numbers, or refused with the list of names this seat knows.
            let anchor = clamp_to_map(match resolved_point(x, z) {
                Some(p) => p,
                None => me.base_pos(),
            });
            let rally = match def.rally {
                StanceRally::Anchor => anchor,
                StanceRally::Base => clamp_to_map(me.base_pos()),
            };

            // 1. The posture, into the same map the `posture` verb writes.
            squad_orders.0.insert(
                (me, squad),
                match def.posture {
                    StancePosture::Defend => SquadPosture::Defend {
                        pos: anchor,
                        radius: def.radius,
                    },
                    StancePosture::Push => SquadPosture::Push { pos: anchor },
                    StancePosture::Forage => SquadPosture::Forage { muster: anchor },
                },
            );

            // 2. The per-unit half, onto this squad's CURRENT members — which
            //    is precisely the set `leash`/`retreat`/`priority` would have
            //    reached had the commander listed them by id. Members who join
            //    LATER get it too, from `stamp_stance_on_joiners` below, which
            //    replays this same applier over the same recorded word: a
            //    commander that reinforces a stanced squad no longer gets
            //    bodies carrying half the doctrine (wc3clone-bol).
            //
            //    Absent pieces REMOVE rather than leave alone. A stance is
            //    absolute: switching from `turtle` to `push` must not leave the
            //    turtle's leash on, or the push walks twenty metres and gets
            //    recalled by a policy nobody can see. This is the "replaces the
            //    bundle atomically" half of the design.
            let mut members = 0usize;
            for (entity, _, team, _, _, squad_id) in units.iter() {
                // Membership as of THIS SENTENCE: a `squad` earlier in the same
                // batch outranks the component, which has not been flushed yet.
                let member_of = match batch_squads.get(&entity) {
                    Some(assigned) => *assigned,
                    None => squad_id.map(|s| s.0),
                };
                if *team != me || member_of != Some(squad) {
                    continue;
                }
                members += 1;
                stamp_stance(&mut commands.entity(entity), def, anchor, rally);
            }

            // 3. The word, so the snapshot can echo it and silence can continue
            //    it. Recorded even for an EMPTY squad, and deliberately: a
            //    commander who stances squad 2 before training into it has said
            //    something true about squad 2, and the posture is already
            //    waiting in `SquadOrders` for the first member to arrive.
            squad_stances.0.insert((me, squad), kind);

            // An empty squad is not a refusal — see above — but it is worth
            // saying out loud, because "I set a stance and nothing moved" is
            // otherwise a silent five seconds a commander has to diagnose.
            //
            // The caveat this message used to carry is gone with wc3clone-bol:
            // it warned that the leash, retreat and focus "reach only the
            // members present when you send it", which was true and is no
            // longer. It now says what remains true — nothing is standing
            // there yet — and what the commander should expect instead.
            if members == 0 {
                errors.push(format!(
                    "{tag}: squad {squad} has no members - the {} stance is set and waiting; \
                     its posture, leash, retreat and focus all land on whoever joins next",
                    kind.word()
                ));
            }
            // Reached: the squad's standing order changed whether or not a body
            // was standing in it. A stance on an empty squad is a real,
            // executed policy, so a plan step that sets one must not block.
            *reached = true;
        }
        Intent::Template {
            building,
            squad,
            retreat,
            priority,
            autocast,
            ..
        } => {
            let Some(building) = building else {
                errors.push(format!("{tag}: {}", needs_building("template")));
                return;
            };
            // Only our own, finished, unit-producing buildings can carry a
            // template — anywhere else it would never be read.
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, b, team, under, queue, _)) = buildings.get(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            if under.is_some() {
                errors.push(format!("{tag}: building {building} is under construction"));
                return;
            }
            if queue.is_none() {
                errors.push(format!(
                    "{tag}: {} has no training queue",
                    building_name(b.kind)
                ));
                return;
            }
            // Same class parsing (and same all-or-nothing rule) as the
            // `priority` command; an empty list means "no priority piece".
            let priority = match priority {
                Some(names) => match parse_target_classes(&names) {
                    Ok(parsed) => (!parsed.is_empty()).then_some(parsed),
                    Err(name) => {
                        errors.push(format!("{tag}: unknown target class '{name}'"));
                        return;
                    }
                },
                None => None,
            };
            let retreat = match retreat {
                Some(r) => {
                    if !(r.below > 0.0 && r.below < 1.0) {
                        errors.push(format!(
                            "{tag}: template retreat 'below' must be a fraction in (0,1), \
                             got {}",
                            r.below
                        ));
                        return;
                    }
                    Some(RetreatPolicy {
                        below_frac: r.below,
                        rally: clamp_to_map(Vec3::new(r.x, 0.0, r.z)),
                    })
                }
                None => None,
            };
            // 0 reads as "off" here exactly as it does in `autocast`.
            let autocast = autocast.filter(|n| *n > 0);

            let template = DoctrineTemplate {
                squad,
                retreat,
                priority,
                autocast,
            };
            let empty = template.squad.is_none()
                && template.retreat.is_none()
                && template.priority.is_none()
                && template.autocast.is_none();
            let mut ec = commands.entity(entity);
            if empty {
                ec.try_remove::<DoctrineTemplate>();
            } else {
                ec.try_insert(template);
            }
        }

        // -------------------------------------------------------------------
        // Triggers. See docs/INTENT.md § "Triggers" and trigger.rs.
        // -------------------------------------------------------------------
        Intent::TriggerSet {
            name,
            when,
            then,
            repeat,
        } => {
            let Some(name) = TriggerName::new(&name) else {
                errors.push(format!(
                    "{tag}: '{name}' is not a usable trigger name — 1..{TRIGGER_NAME_MAX} \
                     printable ASCII characters"
                ));
                return;
            };
            // A trigger may not arm a trigger. This is the line between
            // doctrine and programming, and it is also what makes
            // MAX_TRIGGERS_PER_TEAM an actual bound rather than a starting
            // balance.
            // The same refusal now covers plans AND the place vocabulary, and
            // it has to. A trigger whose `then` set a plan whose step armed a
            // trigger would be a cycle, and the two caps would stop bounding
            // anything. The rule the whole v3 vocabulary keeps is one sentence
            // — *a deferred action may not defer another action* — with the
            // single, bounded exception that a plan STEP may arm a trigger,
            // because a trigger cannot defer anything further.
            //
            // `region_set` is on the list for a related but distinct reason,
            // one step further out: it is not deferral, it is EDITING THE
            // VOCABULARY the other rules are written in. A rule that renamed
            // ground while the match ran would make every other rule's meaning
            // depend on firing order, and "what does north-pass mean right
            // now?" would stop being answerable by reading the snapshot.
            // Territory is something a commander says, not something a rule
            // does.
            if matches!(
                *then,
                Intent::TriggerSet { .. }
                    | Intent::TriggerClear { .. }
                    | Intent::PlanSet { .. }
                    | Intent::PlanClear { .. }
                    | Intent::RegionSet { .. }
                    | Intent::RegionClear { .. }
            ) {
                errors.push(format!(
                    "{tag}: a trigger cannot arm or clear another trigger or a plan, or \
                     name or forget ground — triggers are doctrine, not a scripting language"
                ));
                return;
            }
            if let Some(secs) = repeat {
                if !(secs > 0.0) {
                    errors.push(format!(
                        "{tag}: trigger {name} repeat cooldown must be > 0 seconds, got {secs} \
                         (omit it entirely for a trigger that fires once)"
                    ));
                    return;
                }
            }
            // The predicate's parameters are constants the commander typed, so
            // they CAN be judged now — including `enemy_in`'s region, which is
            // vocabulary this seat either has or does not. Refusing a
            // misspelled place at arm time is the difference between learning
            // it immediately and learning it at 3 a.m. when the rule failed to
            // fire.
            if let Err(err) = validate_predicate(&when, me, regions) {
                errors.push(format!("{tag}: trigger {name}: {err}"));
                return;
            }
            // NOTE what is deliberately NOT checked here: the ACTION. A
            // trigger's whole point is that the world at fire time is
            // different from the world at arm time — the units it names may
            // not be trained yet, the enemy it attacks may not be visible yet.
            // Validating `then` now would refuse exactly the sentences worth
            // arming. It is validated in full when it fires, by this same
            // compiler, and the refusal reaches the arming seat's own error
            // channel tagged `trigger:<name>`.
            let trigger = TriggerRule {
                name,
                when,
                then: *then,
                repeat,
                // The AUTHOR, not the executor. A trigger armed from the wire
                // stays a bridge intent when it fires, and a preset the human
                // pressed stays a `ui` one — which is what routes its refusals
                // to the alert stack rather than into a file nobody is reading.
                source: mark.source,
                armed: true,
                last_fired: None,
            };
            if let Err(err) = triggers.set(me, trigger) {
                errors.push(format!("{tag}: {err}"));
            }
        }
        // -------------------------------------------------------------------
        // Plans. See docs/INTENT.md § "Plans" and plan.rs.
        // -------------------------------------------------------------------
        Intent::PlanSet { name, steps } => {
            let Some(name) = PlanName::new(&name) else {
                errors.push(format!(
                    "{tag}: '{name}' is not a usable plan name — 1..{TRIGGER_NAME_MAX} \
                     printable ASCII characters"
                ));
                return;
            };
            if steps.is_empty() {
                errors.push(format!(
                    "{tag}: plan {name} has no steps — a plan is a sequence, and \
                     an empty one is a plan_clear spelled the long way"
                ));
                return;
            }
            if steps.len() > MAX_PLAN_STEPS {
                errors.push(format!(
                    "{tag}: plan {name} has {} steps — the most is {MAX_PLAN_STEPS}. \
                     A longer sequence is two plans, or a plan and some triggers",
                    steps.len()
                ));
                return;
            }
            for (i, step) in steps.iter().enumerate() {
                let k = i + 1;
                // A plan may not set or clear a plan. Same line, same reason as
                // a trigger's: this is where doctrine would turn into a
                // programming language, and it is what makes MAX_PLANS_PER_TEAM
                // an actual bound rather than a starting balance.
                //
                // Note what IS allowed: a step may `trigger_set`. "Build the
                // barracks, then arm the home guard" is a real sentence, and it
                // stays bounded because a trigger's own `then` may not be a
                // plan or a trigger — so the whole graph is two rungs deep with
                // a cap on each.
                if matches!(step.intent, Intent::PlanSet { .. } | Intent::PlanClear { .. }) {
                    errors.push(format!(
                        "{tag}: plan {name} step {k} sets or clears a plan — plans are \
                         doctrine, not a scripting language (a step MAY arm a trigger)"
                    ));
                    return;
                }
                match &step.advance {
                    PlanAdvance::OnApplied => {}
                    PlanAdvance::When { when } => {
                        // The same arm-time judgement a trigger gets, for the
                        // same reason and now including territory: a plan step
                        // that advances on `enemy_in` names a PLACE, and a
                        // misspelled place should be refused with the menu here
                        // rather than silently stall the sequence at step k.
                        if let Err(err) = validate_predicate(when, me, regions) {
                            errors.push(format!("{tag}: plan {name} step {k}: {err}"));
                            return;
                        }
                    }
                    PlanAdvance::AfterSeconds { secs } => {
                        if !(*secs > 0.0) {
                            errors.push(format!(
                                "{tag}: plan {name} step {k}: 'after' must be > 0 seconds, \
                                 got {secs} (omit advance entirely for 'as soon as it lands')"
                            ));
                            return;
                        }
                    }
                }
                // **Per-step readiness: teaching, and never a gate.**
                //
                // A chain — a plan whose steps are stances — is written to be
                // armed before the world it names exists: "turtle until the
                // hero is healed, then secure the northwest mine" is set at
                // leisure, and the northwest mine may be a region this seat has
                // not scouted, let alone named, yet. Refusing that plan would
                // refuse exactly the sentence the feature is for
                // (docs/AFFORDANCES.md § Chains), so nothing below this comment
                // returns.
                //
                // What a commander IS owed is to be told which steps cannot be
                // resolved *yet*, at the moment they arm, rather than at 3 a.m.
                // when the sequence stopped at step 2. So we dry-run the ONE
                // resolver against the world as it stands — no second notion of
                // what "resolvable" means, and the refusal is in the resolver's
                // own words, which is the same string the step will block with
                // if the name is still unknown when its turn comes.
                //
                // Some of what this reports is permanent rather than pending (a
                // step with no x/z at all will never resolve). That is fine and
                // deliberately not sorted into two messages here: the resolver's
                // sentence already says which of the two it is, and a second
                // classifier would be a second opinion about the first one.
                if let Err(err) = resolve_places(
                    step.intent.clone(),
                    &LateBind {
                        me,
                        regions,
                        units,
                        squads,
                        nodes,
                        buildings,
                        nav,
                    },
                ) {
                    errors.push(format!(
                        "{tag}: chain holds at step {k}: {err} — plan {name} is armed \
                         anyway; the step resolves when its turn comes, and blocks \
                         there if it still cannot"
                    ));
                }
            }
            // NOT checked, for exactly the reason a trigger's `then` is not:
            // every step but the first describes a world that does not exist
            // yet. The building step 3 trains from is the one step 1 puts up.
            // Each step is validated in full by this same compiler at the
            // moment it runs, and a refusal comes back through `Plans::report`
            // and blocks the plan where a person can see it.
            let plan = PlanRun {
                name,
                steps,
                source: mark.source,
                state: PlanState::Running,
                at: 0,
                submitted: false,
                applied: false,
                applied_at: 0.0,
                last_try: 0.0,
                blocked_since: None,
                told_blocked: false,
            };
            if let Err(err) = plans.set(me, plan) {
                errors.push(format!("{tag}: {err}"));
            }
        }
        Intent::PlanClear { name } => match name {
            Some(name) => {
                if !plans.clear(me, name.trim()) {
                    errors.push(format!("{tag}: you have no plan named '{name}'"));
                }
            }
            None => {
                plans.clear_all(me);
            }
        },
        Intent::TriggerClear { name } => match name {
            Some(name) => {
                if !triggers.clear(me, name.trim()) {
                    // Named rather than silent: "I cleared it" and "there was
                    // nothing by that name" call for opposite next moves, and
                    // a commander that cannot tell them apart will spend a poll
                    // wondering why its rule keeps firing.
                    errors.push(format!("{tag}: you have no trigger named '{name}'"));
                }
            }
            None => {
                triggers.clear_all(me);
            }
        },

        // --- territory ---
        Intent::RegionSet {
            name,
            x,
            z,
            radius,
        } => {
            let name = match validate_region_name(&name) {
                Ok(name) => name,
                Err(err) => {
                    errors.push(format!("{tag}: {err}"));
                    return;
                }
            };
            if !(REGION_RADIUS_MIN..=REGION_RADIUS_MAX).contains(&radius) {
                errors.push(format!(
                    "{tag}: region '{name}' radius must be between \
                     {REGION_RADIUS_MIN:.0} and {REGION_RADIUS_MAX:.0}, got {radius}"
                ));
                return;
            }
            // Clamped like every other coordinate this compiler accepts: a
            // region centred off the board would name ground nothing can stand
            // on, and silently is how `move` has always treated the same
            // mistake.
            let center = clamp_to_map(Vec3::new(x, 0.0, z));
            if let Err(err) = regions.set(me, Region::new(name, center, radius)) {
                errors.push(format!("{tag}: {err}"));
            }
        }
        Intent::RegionClear { name } => match name {
            Some(name) => {
                // Named rather than silent, exactly as `trigger_clear` is: "I
                // forgot it" and "you never named that" call for opposite next
                // moves. A built-in gets its own answer, because "you have no
                // region called our base" would be a lie — you do, the map
                // gave it to you, and it is not yours to forget.
                if !regions.clear(me, &name) {
                    if builtin_places(me)
                        .iter()
                        .any(|b| normalize_place(&b.name) == normalize_place(&name))
                    {
                        errors.push(format!(
                            "{tag}: '{name}' is a built-in place on this map — \
                             it cannot be cleared"
                        ));
                    } else {
                        errors.push(format!("{tag}: you have no region named '{name}'"));
                    }
                }
            }
            None => {
                regions.clear_all(me);
            }
        },
    }
}

/// Is this predicate expressible? Checked at arm time, because a predicate is
/// the one half of a trigger that CAN be judged before the world moves — every
/// parameter in it is a constant the commander typed.
fn validate_predicate(when: &TriggerWhen, me: Team, regions: &Regions) -> Result<(), String> {
    /// The class check `enemy_sighted` and `enemy_in` share. One matcher, one
    /// wording — the two predicates ask the same question about the same word.
    fn class_ok(class: &Option<String>) -> Result<(), String> {
        match class {
            // The same words `priority` takes, matched by the same
            // function — there is one name matcher in this language.
            Some(name) if parse_target_class(name).is_none() => Err(format!(
                "unknown target class '{name}' (one of {})",
                ALL_TARGET_CLASSES
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            _ => Ok(()),
        }
    }
    fn frac(label: &str, value: f32) -> Result<(), String> {
        if value > 0.0 && value <= 1.0 {
            Ok(())
        } else {
            Err(format!(
                "{label} must be a health fraction in (0,1], got {value}"
            ))
        }
    }
    match when {
        TriggerWhen::HeroBelow { frac: f } => frac("hero_below", *f),
        TriggerWhen::HeroAbove { frac: f } => frac("hero_above", *f),
        TriggerWhen::SquadBelow { frac: f, .. } => frac("squad_below", *f),
        TriggerWhen::EnemySighted { class, count } => {
            if *count == 0 {
                return Err("enemy_sighted count must be at least 1".to_string());
            }
            class_ok(class)
        }
        TriggerWhen::EnemyIn {
            region,
            class,
            count,
        } => {
            if *count == 0 {
                return Err("enemy_in count must be at least 1".to_string());
            }
            if regions.find(me, region).is_none() {
                return Err(regions.unknown(me, region));
            }
            class_ok(class)
        }
        TriggerWhen::TierReached { tier } => {
            if (1..=3).contains(tier) {
                Ok(())
            } else {
                Err(format!("tier must be 1, 2 or 3, got {tier}"))
            }
        }
        TriggerWhen::UnitCount { kind, count } => {
            if *count == 0 {
                return Err("unit_count count must be at least 1".to_string());
            }
            if parse_unit_kind(kind).is_none() {
                return Err(format!("unknown unit kind '{kind}'"));
            }
            Ok(())
        }
        TriggerWhen::GameTime { at } => {
            if *at >= 0.0 {
                Ok(())
            } else {
                Err(format!("game_time must not be negative, got {at}"))
            }
        }
        TriggerWhen::EnemyArmySeen { size, within_s } => {
            if *size == 0 {
                return Err("enemy_army_seen size must be at least 1".to_string());
            }
            if within_s.is_some_and(|w| w <= 0.0) {
                return Err("enemy_army_seen within_s must be positive".to_string());
            }
            Ok(())
        }
        TriggerWhen::EnemyHeroDown { class } => match class {
            // Refused at ARM time rather than silently never firing. A
            // predicate naming "Footman" as a hero class is a typo, and the
            // seat that typed it is owed the word — a rule that is armed,
            // listed, and structurally incapable of coming true is the worst
            // available outcome.
            Some(name) => match parse_unit_kind(name) {
                Some(kind) if is_hero_kind(kind) => Ok(()),
                Some(_) => Err(format!(
                    "'{name}' is not a hero class (one of {})",
                    HERO_CLASSES
                        .iter()
                        .map(|k| kind_name(*k))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                None => Err(format!("unknown unit kind '{name}'")),
            },
            None => Ok(()),
        },
        // The unparameterised predicates: nothing to get wrong, so nothing to
        // check. `supply_capped` joins them — it takes no argument precisely
        // because "capped" is the engine's own production gate, not a
        // threshold a commander gets to pick and then mis-tune.
        TriggerWhen::BaseUnderAttack
        | TriggerWhen::BountySpawned
        | TriggerWhen::MineDry
        | TriggerWhen::SupplyCapped => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Stances: one applier, and the joiner that replays it
// ---------------------------------------------------------------------------

/// The per-unit half of a stance, on one member.
///
/// Three components, and **absent pieces REMOVE rather than leave alone** — a
/// stance is absolute, so a body that walks into a `push` squad carrying a
/// `turtle`'s leash has that leash taken off it, exactly as a member present at
/// the moment the stance was set would.
///
/// Extracted from the `stance` arm for wc3clone-bol so that the arm and
/// [`stamp_stance_on_joiners`] are one applier and not two. Two copies of this
/// would be a divergence with a schedule: the next field added to `StanceDef`
/// would land in one of them, and the difference between a founding member and
/// a reinforcement would be invisible until an arena round paid for it.
fn stamp_stance(ec: &mut EntityCommands, def: &StanceDef, anchor: Vec3, rally: Vec3) {
    if def.retreat_below > 0.0 {
        ec.try_insert(RetreatPolicy {
            below_frac: def.retreat_below,
            rally,
        });
    } else {
        ec.try_remove::<RetreatPolicy>();
    }
    if def.leash > 0.0 {
        ec.try_insert(LeashPolicy {
            anchor,
            radius: def.leash,
        });
    } else {
        ec.try_remove::<LeashPolicy>();
    }
    if def.priority.is_empty() {
        ec.try_remove::<TargetPriority>();
    } else {
        ec.try_insert(TargetPriority(def.priority.clone()));
    }
}

/// Where a stance's bundle is pinned, **recovered from the posture it
/// installed** rather than remembered separately.
///
/// A stance writes its anchor into `SquadOrders` as the posture's own point,
/// and `SquadStances` records the word; between them the whole bundle is
/// reconstructible, so nothing new has to be stored and nothing new can drift.
/// That matters more than the saved bytes: a second copy of the anchor would be
/// a second answer to "where is this squad's doctrine pinned", and the first
/// time a commander re-aimed a stance the two would disagree.
///
/// `None` when the squad's posture is not one a stance can install — which in
/// practice means `Escort`, and cannot co-exist with a recorded word (the
/// `posture` verb clears the word on its way past). Defensive, not expected.
fn stance_bundle(
    team: Team,
    kind: StanceKind,
    posture: &SquadPosture,
) -> Option<(&'static StanceDef, Vec3, Vec3)> {
    let anchor = match posture {
        SquadPosture::Defend { pos, .. } => *pos,
        SquadPosture::Push { pos } => *pos,
        SquadPosture::Forage { muster } => *muster,
        SquadPosture::Escort { .. } => return None,
    };
    let def = kind.def();
    let rally = match def.rally {
        StanceRally::Anchor => anchor,
        StanceRally::Base => clamp_to_map(team.base_pos()),
    };
    Some((def, anchor, rally))
}

/// **A unit that enters a stanced squad inherits the whole stance, not just the
/// posture** (wc3clone-bol).
///
/// The 0uu.2 stance arm stamped the leash, retreat threshold and focus list on
/// the members standing in the squad at the moment the word was sent. The
/// posture is per-squad and covered everyone; those three did not. A commander
/// that stanced squad 1 and then reinforced it — which is the ordinary shape of
/// a match — ended up fielding one squad wearing two different doctrines, and
/// the only way to see it was to notice that half the army did not break off.
///
/// **The choke point is the component, not any of its writers.** Three places
/// enrol a unit today — the `squad` verb above, `DoctrineTemplate` at spawn in
/// units.rs, and doctrine.rs's auto-enrolment into `DEFAULT_SQUAD` — and there
/// is no fourth path into a squad that does not go through writing `SquadId`.
/// So this watches `Changed<SquadId>`, which is every one of them and anything
/// added later, instead of asking each writer to remember to call an applier.
/// It also means doctrine.rs keeps its module contract: it enrols by writing
/// `SquadId`, exactly as it always did, and the doctrine components stay
/// written by this file alone.
///
/// `Changed` rather than `Added` deliberately: moving from squad 2 to squad 3
/// is joining squad 3, and the new squad's bundle replaces the old one's
/// wholesale — the same "switching replaces the bundle atomically" rule the
/// `stance` verb follows.
///
/// **Only stanced squads.** A squad holding a hand-set `posture` and no word
/// stamps nothing, because there is nothing per-squad to stamp: `leash`,
/// `retreat` and `priority` take a unit selector, not a squad, and a selection
/// may span squads or contain unsquadded units. Making those uniform would mean
/// inventing a per-squad record their own vocabulary does not have. The stance
/// is the one doctrine this engine holds per squad, so the stance is the one
/// doctrine a joiner can inherit. See docs/INTENT.md and the `stance` arm.
fn stamp_stance_on_joiners(
    mut commands: Commands,
    stances: Res<SquadStances>,
    orders: Res<SquadOrders>,
    joiners: Query<(Entity, &Team, &SquadId), (With<Unit>, Changed<SquadId>)>,
) {
    if stances.0.is_empty() {
        return;
    }
    for (entity, team, squad) in &joiners {
        let key = (*team, squad.0);
        let Some(kind) = stances.0.get(&key).copied() else {
            continue;
        };
        let Some(posture) = orders.0.get(&key) else {
            continue;
        };
        let Some((def, anchor, rally)) = stance_bundle(*team, kind, posture) else {
            continue;
        };
        stamp_stance(&mut commands.entity(entity), def, anchor, rally);
    }
}

// ---------------------------------------------------------------------------
// The replay log
// ---------------------------------------------------------------------------

/// One line of `intent_log.jsonl`: what was meant, in English and in JSON.
#[derive(Serialize)]
struct IntentRecord<'a> {
    /// Wall-clock milliseconds since the Unix epoch — the real time a human or
    /// a commander decided this.
    wall_ms: u64,
    /// Game-time seconds, which is what the rest of the match is stamped in.
    t: f32,
    team: &'a str,
    /// Which interface spelled it. Recorded, never consulted for authority.
    source: &'a str,
    tag: &'a str,
    verb: &'a str,
    /// The half a person reads. Carries a `(+N.Ns link)` suffix when Chain of
    /// Command delayed the order — a world fact about how far the unit was
    /// from its command structure, not a fact about how the intent was
    /// spelled, so it belongs in the sentence rather than beside it.
    sentence: String,
    /// Worst link latency this intent paid, in seconds. Absent (rather than
    /// `0.0`) whenever nothing was delayed, so a `BH_COMMAND_LATENCY`-off log
    /// line is character-for-character a v1 log line.
    #[serde(skip_serializing_if = "no_link")]
    link: f32,
    /// The provenance string this intent stamps on the units it moves — the
    /// join key between this log and a snapshot's `units[].why`. Grep a unit's
    /// answer here and you land on the sentence that caused it. Absent for the
    /// verbs that install policy rather than behaviour (their reason shows up
    /// later, as `policy:…`, on the frame doctrine.rs acts on it).
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
    /// False when validation rejected some or all of it.
    ok: bool,
    #[serde(skip_serializing_if = "no_errors")]
    errors: &'a [String],
    /// The half a machine replays.
    intent: &'a Intent,
}

fn no_errors(errs: &&[String]) -> bool {
    errs.is_empty()
}

fn no_link(link: &f32) -> bool {
    *link <= 0.0
}

/// Header written once when a match opens its log.
#[derive(Serialize)]
struct SessionRecord {
    wall_ms: u64,
    session: &'static str,
    note: &'static str,
}

/// The append-only match log. Opened lazily on the first intent, so a run in
/// which nobody says anything leaves no file behind.
#[derive(Resource, Default)]
pub struct IntentLog {
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
    /// Latched after the first IO failure so a broken path cannot spam the
    /// console once per order for a whole match.
    broken: bool,
}

impl IntentLog {
    fn from_env() -> Self {
        let raw = std::env::var(INTENT_LOG_ENV).unwrap_or_else(|_| DEFAULT_INTENT_LOG.to_string());
        let raw = raw.trim();
        let path = if raw.is_empty() || raw == "0" {
            None
        } else {
            Some(PathBuf::from(raw))
        };
        IntentLog {
            path,
            file: None,
            broken: false,
        }
    }

    /// A log that writes nothing. Tests take this so they depend on neither
    /// `BH_INTENT_LOG` nor the filesystem, and leave no file behind.
    #[cfg(test)]
    pub fn disabled() -> Self {
        IntentLog {
            path: None,
            file: None,
            broken: false,
        }
    }

    fn record(&mut self, now: f32, submission: &SubmitIntent, errors: &[String], raw_link: f32) {
        if self.path.is_none() || self.broken {
            return;
        }
        let link = (raw_link * 10.0).round() / 10.0;
        let record = IntentRecord {
            wall_ms: wall_ms(),
            t: (now * 10.0).round() / 10.0,
            team: team_name(submission.team),
            source: submission.source.name(),
            tag: &submission.tag,
            verb: submission.intent.verb(),
            sentence: link_sentence(&submission.intent, link),
            link,
            why: submission.intent.provenance_verb().map(|verb| {
                IntentMark {
                    // ARRIVAL, not speech. This string is the join key
                    // between this log and a snapshot's `units[].why`, so it
                    // has to be character-for-character what the unit will
                    // answer — and what the unit answers is stamped when the
                    // order LANDS (`command::dispatch_pending`). The moment it
                    // was spoken is not lost: it is this record's own `t`, and
                    // `link` is the gap between the two.
                    //
                    // Raw rather than the rounded `link` above, so the two
                    // renderings round identically instead of drifting by a
                    // tenth.
                    at: now + raw_link,
                    source: submission.source,
                    // So a trigger-fired order's log line and the unit's own
                    // `why` are the same string: `trigger:home-guard move by
                    // bridge t=41`. The join key has to match on both rungs or
                    // it stops being a join.
                    trigger: submission.trigger,
                    // Same join, one rung along, for a plan step.
                    plan: submission.plan,
                }
                .order(verb)
                .why()
            }),
            ok: errors.is_empty(),
            errors,
            intent: &submission.intent,
        };
        let line = match serde_json::to_string(&record) {
            Ok(line) => line,
            Err(err) => {
                warn!("intent log: could not serialize record ({err})");
                return;
            }
        };
        if let Err(err) = self.append(&line) {
            warn!("intent log: disabled after write failure ({err})");
            self.broken = true;
        }
    }

    fn append(&mut self, line: &str) -> std::io::Result<()> {
        if self.file.is_none() {
            let path = self.path.clone().expect("checked by caller");
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            // Truncating gives one file per match, which is what a replay is.
            // Point BH_INTENT_LOG somewhere unique to keep a series.
            let mut file = std::fs::File::create(&path)?;
            let header = SessionRecord {
                wall_ms: wall_ms(),
                session: "wc3clone-intent-log-v1",
                note: "every player-issued intent, from either interface, in submission order",
            };
            if let Ok(line) = serde_json::to_string(&header) {
                writeln!(file, "{line}")?;
            }
            info!("intent log: {}", path.display());
            self.file = Some(file);
        }
        let file = self.file.as_mut().expect("opened above");
        writeln!(file, "{line}")?;
        file.flush()
    }
}

/// The sentence, plus what the chain of command charged to deliver it:
///
/// ```text
/// [ 91.6s] Human/ui: 4 units attack-move to (12.0, -30.0) (+1.8s link)
/// ```
///
/// A replay of a match played with `BH_COMMAND_LATENCY` off is unannotated
/// and therefore identical to a v1 replay.
fn link_sentence(intent: &Intent, link: f32) -> String {
    let sentence = intent.sentence();
    if link <= 0.0 {
        return sentence;
    }
    format!("{sentence} (+{link:.1}s link)")
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn team_name(team: Team) -> &'static str {
    match team {
        Team::Human => "Human",
        Team::Claude => "Claude",
    }
}

// ---------------------------------------------------------------------------
// Helpers — the validation vocabulary shared by every intent
// ---------------------------------------------------------------------------

/// Resolve a selector to a slot of `list`, unlocked or not — `autocast` writes
/// rules for abilities a hero has not levelled into yet, and that is fine.
/// Whether a slot is unlocked, funded and off cooldown stays combat.rs's call.
fn ability_slot(sel: &AbilitySelector, list: &[AbilityDef]) -> Option<usize> {
    match sel {
        AbilitySelector::Index(i) => (*i < list.len()).then_some(*i),
        AbilitySelector::Id(id) => ability_index_by_id(list, id),
    }
}

/// The slot an OPTIONAL selector names, if it names one at all.
fn ability_slot_of(sel: &Option<AbilitySelector>, list: &[AbilityDef]) -> Option<usize> {
    ability_slot(sel.as_ref()?, list)
}

/// Resolve one id to a living unit of the seat's own team.
fn own_unit(id: IntentId, units: &IntentUnits, me: Team) -> Option<(Entity, Vec3)> {
    let entity = intent_entity(id)?;
    match units.get(entity) {
        Ok((_, _, team, tf, ..)) if *team == me => Some((entity, tf.translation)),
        _ => None,
    }
}

/// Which of the seat's living heroes an item verb is about.
///
/// `named` is the intent's optional `hero` field. Given one, it must resolve to
/// a living hero of this team — anything else is an error rather than a silent
/// fall-back, because "the potion went to the wrong hero" is exactly the bug
/// this parameter exists to prevent, and quietly substituting a different hero
/// would reintroduce it.
///
/// Omitted, the tie-break is **the living hero with the lowest entity id**, and
/// it is sorted rather than left to query order so it is stable frame to frame
/// and identical for both seats. With one hero on the field — every call site
/// that predates hero slots — it picks that hero, so omitting the field is
/// exactly the old behaviour.
fn own_hero(
    units: &IntentUnits,
    me: Team,
    named: Option<IntentId>,
    tag: &str,
    errors: &mut Vec<String>,
) -> Option<Entity> {
    let heroes: Vec<Entity> = units
        .iter()
        .filter(|(_, u, team, ..)| **team == me && is_hero_kind(u.kind))
        .map(|(entity, ..)| entity)
        .collect();
    match named {
        // Naming a hero is a claim about a SPECIFIC entity. If the id does not
        // resolve to a live entity at all, or resolves to something that is not
        // one of this team's living heroes, that is an error — never a quiet
        // fall-back to the default, which would hand the item to precisely the
        // hero the caller was steering away from. (The first version of this
        // function mapped an unresolvable id to `None` and then let the
        // no-name branch pick the default; the live bridge check caught it.)
        Some(id) => {
            let picked = intent_entity(id).and_then(|e| pick_item_hero(&heroes, Some(e)));
            if picked.is_none() {
                errors.push(format!("{tag}: hero {id} not found/not yours"));
            }
            picked
        }
        None => pick_item_hero(&heroes, None),
    }
}

/// The choice itself, as a pure function so it can be tested without a World:
/// a NAMED hero must be one of this team's living heroes (no silent
/// substitution — sending the potion to somebody else is the bug), and an
/// unnamed one resolves to the lowest entity id.
fn pick_item_hero(heroes: &[Entity], named: Option<Entity>) -> Option<Entity> {
    match named {
        Some(hero) => heroes.contains(&hero).then_some(hero),
        None => heroes.iter().copied().min(),
    }
}

/// Resolve a list of ids to living units of the seat's own team, recording one
/// error per id that doesn't qualify (an enemy's unit included).
fn own_units(
    ids: &[IntentId],
    units: &IntentUnits,
    me: Team,
    tag: &str,
    errors: &mut Vec<String>,
    // Set when this call reached at least one real unit. See `Reached` — a
    // group order that lost one member to a corpse still ORDERED the rest, and
    // the difference matters to exactly one caller.
    reached: &mut bool,
) -> Vec<(Entity, Vec3)> {
    if ids.is_empty() {
        errors.push(format!("{tag}: no units given"));
        return Vec::new();
    }
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        match own_unit(id, units, me) {
            Some(found) => out.push(found),
            None => errors.push(format!("{tag}: unit {id} not found/not yours")),
        }
    }
    *reached |= !out.is_empty();
    out
}

/// The seat's completed (not under construction) buildings — the input to
/// every requirement check on the command path.
fn completed_kinds(buildings: &IntentBuildings, me: Team) -> Vec<BuildingKind> {
    buildings
        .iter()
        .filter(|(_, _, team, under, _, _)| **team == me && under.is_none())
        .map(|(_, building, ..)| building.kind)
        .collect()
}

fn is_worker(units: &IntentUnits, entity: Entity) -> bool {
    matches!(units.get(entity), Ok((_, u, ..)) if is_worker_kind(u.kind))
}

/// Move / AttackMove for a group, spread over the UI's formation grid.
#[allow(clippy::too_many_arguments)]
fn ground_order(
    commands: &mut Commands,
    errors: &mut Vec<String>,
    tag: &str,
    why: Provenance,
    ids: &[IntentId],
    units: &IntentUnits,
    me: Team,
    ground: Vec3,
    attack_move: bool,
    issuer: &mut OrderIssuer,
    reached: &mut bool,
) {
    let group = own_units(ids, units, me, tag, errors, reached);
    let count = group.len();
    for (i, (entity, pos)) in group.into_iter().enumerate() {
        let p = clamp_to_map(ground + formation_offset(i, count));
        let order = if attack_move {
            Order::AttackMove(p)
        } else {
            Order::Move(p)
        };
        // Each member pays its OWN link, measured where it is standing now —
        // a group half in the base and half at the front arrives in two
        // waves, which is the mechanic being honest rather than a bug. (The
        // log's `why` names the WORST of them, so it joins against the last
        // unit to receive the order; the rest answer with their own, earlier
        // arrival.)
        issuer.issue(commands, me, pos, entity, order, why);
    }
}

/// Deterministic grid offsets so a group doesn't pile onto one spot.
pub fn formation_offset(index: usize, count: usize) -> Vec3 {
    if count <= 1 {
        return Vec3::ZERO;
    }
    let cols = (count as f32).sqrt().ceil().max(1.0) as usize;
    let rows = count.div_ceil(cols);
    let col = index % cols;
    let row = index / cols;
    Vec3::new(
        (col as f32 - (cols as f32 - 1.0) * 0.5) * FORMATION_SPACING,
        0.0,
        (row as f32 - (rows as f32 - 1.0) * 0.5) * FORMATION_SPACING,
    )
}

pub fn clamp_to_map(p: Vec3) -> Vec3 {
    Vec3::new(
        p.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
        0.0,
        p.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
    )
}

/// Snap a footprint centre so its edges land on nav-cell boundaries.
pub fn snap_footprint(p: Vec3, size: f32) -> Vec3 {
    let half = size * 0.5;
    Vec3::new(
        ((p.x - half) / CELL).round() * CELL + half,
        0.0,
        ((p.z - half) / CELL).round() * CELL + half,
    )
}

/// How far a blocked-placement rejection looks for somewhere that *would*
/// work. Round-9 AAR (`wc3clone-vjy`): both commanders spent 20s+ guessing
/// after `site (56.0, -56.0) is blocked for TownHall`, because the string named
/// no rule and no alternative. 15 world units is a little under two TownHall
/// footprints — far enough to clear a gold mine's 6x6 block plus your own
/// half-footprint, near enough that the answer is still the base you meant.
pub const PLACEMENT_HINT_RADIUS: f32 = 15.0;

/// The nearest site within `radius` of `around` where a `size`-edge footprint
/// would actually fit, or `None` if the whole neighbourhood is taken.
///
/// Candidates are generated on the nav lattice and put through the *same*
/// `snap_footprint` + `rect_is_free` pair the rejection above just applied, so
/// a hint is legal by construction rather than by two functions agreeing —
/// which is the property `a_blocked_placement_hint_is_itself_legal` asserts by
/// feeding the hint straight back to the validator.
///
/// Ties break on (distance, x, z) so two seats reading the same board are
/// given the same advice. There is no fog consideration on purpose: the nav
/// grid is map furniture (terrain, trees, mines) plus buildings, and
/// docs/FOG.md already holds that map geography is public. A hint can point at
/// a cell an enemy building has since taken, and the commander then gets the
/// ordinary rejection there — the same thing that happens to the human's ghost.
pub fn nearest_free_site(nav: &NavGrid, around: Vec3, size: f32, radius: f32) -> Option<Vec3> {
    let steps = (radius / CELL).ceil() as i32;
    let mut best: Option<(f32, Vec3)> = None;
    for dz in -steps..=steps {
        for dx in -steps..=steps {
            let candidate = snap_footprint(
                clamp_to_map(Vec3::new(
                    around.x + dx as f32 * CELL,
                    0.0,
                    around.z + dz as f32 * CELL,
                )),
                size,
            );
            let d = (candidate.x - around.x).hypot(candidate.z - around.z);
            if d > radius || !nav.rect_is_free(candidate, size) {
                continue;
            }
            let better = match best {
                None => true,
                Some((bd, bp)) => (d, candidate.x, candidate.z) < (bd, bp.x, bp.z),
            };
            if better {
                best = Some((d, candidate));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// The whole blocked-site rejection, hint included. One function because the
/// bridge's `errors` array and the human's alert stack must read identically —
/// docs/INTENT.md's "the text after the channel tag is byte-identical".
pub fn blocked_site_error(nav: &NavGrid, pos: Vec3, kind: BuildingKind) -> String {
    let size = building_stats(kind).size;
    let hint = match nearest_free_site(nav, pos, size, PLACEMENT_HINT_RADIUS) {
        Some(p) => format!("nearest legal: ({:.1}, {:.1})", p.x, p.z),
        None => format!("no legal site within {PLACEMENT_HINT_RADIUS:.0}"),
    };
    format!(
        "site ({:.1}, {:.1}) is blocked for {} — needs {size:.0}x{size:.0} clear \
         (mines block 6x6, trees 2x2, buildings their own footprint); {hint}",
        pos.x,
        pos.z,
        building_name(kind),
    )
}

/// `None` when `reqs` are satisfied, otherwise the error line to report, e.g.
/// `"cmd 3: Tower requires Barracks"`.
fn requirement_error(
    tag: &str,
    what: &'static str,
    reqs: &[BuildingKind],
    completed: &[BuildingKind],
) -> Option<String> {
    if requirements_met(reqs, completed.iter().copied()) {
        return None;
    }
    // Tier-aware, exactly like `requirements_met`: a Castle covers a "requires
    // Keep", so it must not be listed as the thing you are missing.
    let missing: Vec<&str> = reqs
        .iter()
        .filter(|r| !completed.iter().any(|owned| building_satisfies(*owned, **r)))
        .map(|r| building_name(*r))
        .collect();
    Some(format!("{tag}: {what} requires {}", missing.join(" + ")))
}

/// Parse a whole class list, all-or-nothing: `Err(name)` names the first
/// unknown class so the caller can reject the command outright rather than
/// install a focus-fire order nobody asked for.
pub fn parse_target_classes(names: &[String]) -> Result<Vec<TargetClass>, String> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        match parse_target_class(name) {
            Some(class) => out.push(class),
            None => return Err(name.clone()),
        }
    }
    Ok(out)
}

pub fn parse_target_class(name: &str) -> Option<TargetClass> {
    let wanted = normalize_name(name);
    ALL_TARGET_CLASSES
        .iter()
        .copied()
        .find(|c| normalize_name(c.name()) == wanted)
}

// `normalize_name` is `shared::normalize_name` — the one name matcher, moved
// next to the catalog it folds the names of so that abilities (resolved in
// shared.rs) and everything else (resolved here) cannot drift apart again.

/// Both parsers match against the catalog's own ids (`shared::kind_name` /
/// `building_name`), so a kind added to the shared enums is orderable through
/// the bridge the moment it exists — no table here to fall out of date.
pub fn parse_unit_kind(name: &str) -> Option<UnitKind> {
    let wanted = normalize_name(name);
    ALL_UNIT_KINDS
        .into_iter()
        .find(|k| normalize_name(kind_name(*k)) == wanted)
}

pub fn parse_building_kind(name: &str) -> Option<BuildingKind> {
    let wanted = normalize_name(name);
    ALL_BUILDING_KINDS
        .into_iter()
        .find(|k| normalize_name(building_name(*k)) == wanted)
}

/// Research ladders parse off the catalog's own ids, and off their display
/// names as well: a commander reading `catalog.research` sees both `"attack"`
/// and `"Weapon Smithing"` on the entry, and either ought to work. The same
/// `normalize_name` as everything else, so `"weapon_smithing"` lands too.
pub fn parse_research_kind(name: &str) -> Option<ResearchKind> {
    let wanted = normalize_name(name);
    ALL_RESEARCH_KINDS.into_iter().find(|k| {
        normalize_name(k.id()) == wanted || normalize_name(k.label()) == wanted
    })
}

/// Items parse off the catalog's own ids too (`item_def(..).name`), so
/// `"town_portal"`, `"Town Portal"` and `"TownPortal"` are one item.
pub fn parse_item(name: &str) -> Option<ItemId> {
    let wanted = normalize_name(name);
    ALL_ITEMS
        .into_iter()
        .find(|id| normalize_name(item_def(*id).name) == wanted)
}

/// Hand a faction to (or take it from) the scripted macro AI.
pub fn set_autopilot(ai_controlled: &mut AiControlled, team: Team, on: bool) {
    match team {
        Team::Claude => ai_controlled.claude = on,
        Team::Human => ai_controlled.human = on,
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandLatency, CommandNodes, PendingOrder, DEFAULT_HALL_RADIUS};

    /// An app running the real compiler against a real (if bare) world.
    ///
    /// Everything `apply_intents` reads, defaulted, plus the five event
    /// channels it writes. `IntentLog` is replaced with a disabled one after
    /// the plugin installs it: a unit test must not depend on `BH_INTENT_LOG`
    /// and must not leave a file behind.
    fn compiler_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<Economies>()
            .init_resource::<HeroRecords>()
            .init_resource::<TechTiers>()
            .init_resource::<NavGrid>()
            .init_resource::<TeamResearch>()
            .init_resource::<SquadOrders>()
            .init_resource::<AiControlled>()
            .init_resource::<GameEvents>()
            .add_event::<CastAbility>()
            .add_event::<MatchReady>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            // Chain of Command's two resources (docs/TEMPO.md §3). Defaulted
            // rather than plugged in: the default is `on: false`, so the
            // compiler behaves exactly as it did before latency existed and
            // every assertion below is about compilation, not propagation.
            // The tests that DO exercise propagation turn it on explicitly.
            .init_resource::<CommandNodes>()
            .init_resource::<CommandLatency>()
            .add_plugins(IntentPlugin);
        app.insert_resource(IntentLog::disabled());
        // Pin the fog mode: the ambient `BH_FOG` must not decide an outcome.
        app.insert_resource(FogGrids::test_dark());
        app
    }

    /// **The new direct-order path pays, and nobody had to remember to make it
    /// pay.** The ghost right-click (bead/polish) taught the human a gesture
    /// that produces `Intent::Attack` against a remembered building. It landed
    /// after Chain of Command was written, was never considered by it, and is
    /// priced correctly anyway — because there is exactly one `Attack` arm and
    /// exactly one place an order becomes real.
    ///
    /// This is the choke point (docs/INTENT.md) paying for itself: a new way of
    /// speaking cannot accidentally arrive at a privileged speed.
    #[test]
    fn a_ghost_attack_pays_the_link_like_any_other_direct_order() {
        let mut app = compiler_app();
        // Latency on, and this team owns no command nodes — the severed-arm
        // case, so any positive charge proves the path was priced at all.
        app.insert_resource(CommandLatency { on: true, ..Default::default() })
            .insert_resource(CommandNodes { nodes: Vec::new(), ready: true });

        let barracks_at = Vec3::new(60.0, 0.0, 60.0);
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        let barracks = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Claude,
                Transform::from_translation(barracks_at),
                Health::new(700.0),
            ))
            .id();
        remember(&mut app, Team::Human, barracks, BuildingKind::Barracks, barracks_at);

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Attack {
                units: vec![soldier.to_bits()],
                target: barracks.to_bits(),
                select: None,
            },
        ));
        app.update();

        // Still legal, and still validated in the frame it was spoken: the
        // compiler's verdict is never what gets deferred.
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "the ghost attack must stay legal"
        );
        let pending = app
            .world()
            .entity(soldier)
            .get::<PendingOrder>()
            .expect("a direct attack from outside the chain of command must travel");
        assert!(matches!(pending.order, Order::Attack(t) if t == barracks));
        assert!(pending.link() > 0.0);
        // ...and the soldier has not started marching yet.
        assert!(
            matches!(app.world().entity(soldier).get::<Order>(), Some(Order::Idle)),
            "an in-transit attack must not disturb the unit yet"
        );
    }

    /// **The acknowledgement** (docs/TEMPO.md §4, issue 6). A commander on the
    /// wire has no HUD: if reaching a unit cost it two seconds, the only way it
    /// can find that out is if the wire says so. `applied` is that answer, keyed
    /// by the same `cmd N` handle the error channel already uses, so a batch's
    /// refusals and its costs read as one verdict.
    ///
    /// The three cases in one test, because it is the *contrast* that is the
    /// contract: a bridge command that paid is reported; the human's own gesture
    /// is not (their acknowledgement is the selection panel, and echoing it onto
    /// the wire would be noise for a reader who is not there); and a command
    /// that cost nothing says nothing, which is what keeps the channel — and its
    /// wire key — empty whenever the feature is off.
    #[test]
    fn a_bridge_command_is_told_what_reaching_its_units_cost() {
        let mut app = compiler_app();
        // The severed-arm case again: no nodes, so every direct order pays.
        app.insert_resource(CommandLatency { on: true, ..Default::default() })
            .insert_resource(CommandNodes { nodes: Vec::new(), ready: true });

        let far = Vec3::new(60.0, 0.0, 60.0);
        let spawn_soldier = |app: &mut App| {
            app.world_mut()
                .spawn((
                    Unit { kind: UnitKind::Footman },
                    Team::Human,
                    Transform::from_translation(far),
                    Order::Idle,
                ))
                .id()
        };
        let commanded = spawn_soldier(&mut app);
        let clicked = spawn_soldier(&mut app);

        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 3".to_string(),
            intent: Intent::Move {
                units: vec![commanded.to_bits()],
                x: Some(0.0),
                z: Some(0.0),
                region: None,
                select: None,
            },
            trigger: None,
            plan: None,
        });
        // The same sentence, from the seat with a screen.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Move {
                units: vec![clicked.to_bits()],
                x: Some(0.0),
                z: Some(0.0),
                region: None,
                select: None,
            },
        ));
        app.update();

        let charged = app
            .world()
            .entity(commanded)
            .get::<PendingOrder>()
            .expect("a direct order from outside the chain of command travels")
            .link();
        assert!(charged > 0.0);

        let applied = app.world().resource::<IntentApplied>().get(Team::Human).clone();
        assert_eq!(
            applied.len(),
            1,
            "expected exactly the bridge command to be acknowledged, got {applied:?}"
        );
        assert_eq!(
            applied[0].cmd, "cmd 3",
            "the acknowledgement must name the command with the same handle the \
             error channel uses, or the two cannot be joined"
        );
        assert!(
            (applied[0].delay - charged).abs() < 1e-5,
            "acknowledged {:.3}s but actually charged {charged:.3}s",
            applied[0].delay
        );

        // A command that costs nothing is not worth a line: the same batch
        // spoken from inside a command node's radius acknowledges silence.
        app.insert_resource(CommandNodes {
            nodes: vec![(Team::Human, far, DEFAULT_HALL_RADIUS)],
            ready: true,
        });
        app.world_mut().resource_mut::<IntentApplied>().human.clear();
        let inside = spawn_soldier(&mut app);
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: Intent::Move {
                units: vec![inside.to_bits()],
                x: Some(61.0),
                z: Some(61.0),
                region: None,
                select: None,
            },
            trigger: None,
            plan: None,
        });
        app.update();

        assert!(
            app.world().entity(inside).get::<PendingOrder>().is_none(),
            "an order given inside a node's radius must land at once"
        );
        assert!(
            app.world().resource::<IntentApplied>().get(Team::Human).is_empty(),
            "silence means instant — an order that paid nothing must not be \
             acknowledged, or the wire gains a key with the feature off"
        );
    }

    /// The half of co-command's legibility that runs human -> AI.
    ///
    /// The human already sees their partner's directives — they arrive as
    /// proposals with a note. Without a journal the partner could not see
    /// theirs, and would be commanding next to someone it cannot hear. So
    /// every intent the compiler handles is also remembered in memory, tagged
    /// with which author spelled it, and a copilot seat serializes its team's
    /// tail as `partner_log`.
    ///
    /// The point of the assertion is that this needed **no new vocabulary**:
    /// what a co-commander reads is `Intent::sentence()` — the same string the
    /// replay log writes and the same one the human's proposal card shows.
    #[test]
    fn every_authors_sentences_are_remembered_source_tagged() {
        let mut app = compiler_app();
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();

        // The human right-clicks; then their co-commander installs doctrine.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Move {
                units: vec![soldier.to_bits()],
                x: Some(12.0),
                z: Some(-4.0),
                region: None,
                select: None,
            },
        ));
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Copilot,
            tag: "cmd 0".to_string(),
            intent: Intent::Squad {
                units: vec![soldier.to_bits()],
                id: Some(1),
                select: None,
            },
            trigger: None,
            plan: None,
        });
        app.update();

        let journal = app.world().resource::<IntentJournal>();
        let human: Vec<String> = journal
            .get(Team::Human)
            .iter()
            .map(|e| format!("{} | {}", e.source.name(), e.sentence))
            .collect();
        assert_eq!(
            human,
            vec![
                format!("ui | move unit {} to (12.0, -4.0)", soldier.to_bits()),
                format!("copilot | unit {} join squad 1", soldier.to_bits()),
            ]
        );
        // One team's authors, one journal. The opponent's is untouched — a
        // co-commander reads its partner, never the enemy's plan.
        assert!(journal.get(Team::Claude).is_empty());
    }

    /// The journal is a tail, not a transcript: an hour-long match must not
    /// grow a snapshot field without bound.
    #[test]
    fn the_journal_keeps_only_a_tail() {
        let mut app = compiler_app();
        for i in 0..JOURNAL_MAX + 5 {
            app.world_mut().send_event(SubmitIntent::ui(
                Team::Human,
                Intent::Move {
                    units: vec![i as u64],
                    x: Some(0.0),
                    z: Some(0.0),
                    region: None,
                    select: None,
                },
            ));
        }
        app.update();
        let journal = app.world().resource::<IntentJournal>();
        assert_eq!(journal.get(Team::Human).len(), JOURNAL_MAX);
        // Oldest dropped, newest kept — the tail a partner actually wants.
        assert!(journal
            .get(Team::Human)
            .back()
            .is_some_and(|e| e.sentence.contains(&(JOURNAL_MAX + 4).to_string())));
        // Rejections are kept too (these units do not exist): a partner
        // learning that your last four clicks bounced is a partner who stops
        // proposing around a plan you never actually issued.
        assert!(journal.get(Team::Human).iter().all(|e| !e.ok));
    }

    /// Remember `building` (an enemy structure) in `team`'s grid, exactly as
    /// `update_fog` does, without needing a scout to walk there and back.
    fn remember(app: &mut App, team: Team, building: Entity, kind: BuildingKind, pos: Vec3) {
        app.world_mut().resource_mut::<FogGrids>().test_remember(
            team,
            RememberedBuilding {
                id: building.to_bits(),
                team: team.enemy(),
                kind,
                pos,
                hp: 700.0,
                max_hp: 700.0,
                done: true,
                last_seen: 0.0,
            },
        );
    }

    /// The gap docs/INTENT.md called "the one residual asymmetry", closed.
    ///
    /// The compiler's rule has always been `knows_entity` — visible now OR a
    /// remembered structure — while the human's right-click picker only
    /// offered what `fog_sees` allowed. A scouted barracks standing in the fog
    /// was therefore a legal target for a bridge commander and un-clickable
    /// for the human: the AI could express something the human could not,
    /// which is the one direction THESIS.md's fairness claim cannot survive.
    ///
    /// `ui.rs::right_mouse` now picks against `FogGrid::ghosts()` — the same
    /// iterator that draws the boxes on screen — and hands the record's `id`
    /// straight to `Intent::Attack`. That id is the real entity's `to_bits()`,
    /// so this test asserts the two halves that makes the gesture work: the
    /// id round-trips to the live entity, and the intent built from it is the
    /// *same value* a commander types and compiles to a real `Order::Attack`.
    #[test]
    fn a_remembered_building_is_attackable_by_id() {
        let barracks_at = Vec3::new(40.0, 0.0, 40.0);
        let mut app = compiler_app();

        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();
        let barracks = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Claude,
                Transform::from_translation(barracks_at),
                Health::new(700.0),
            ))
            .id();
        remember(&mut app, Team::Human, barracks, BuildingKind::Barracks, barracks_at);

        // The picker's premise: the record is a ghost (not currently seen),
        // and its id resolves back to the entity the compiler will look up.
        {
            let grid = app.world().resource::<FogGrids>().get(Team::Human);
            let ghost = grid.ghosts().next().expect("the barracks is remembered");
            assert!(!grid.sees(ghost.pos), "a ghost is by definition unseen");
            assert_eq!(Entity::try_from_bits(ghost.id), Ok(barracks));
            assert!(grid.knows_entity(ghost.id, ghost.pos));
        }

        // The gesture. `intent_id` is `Entity::to_bits`, which is what the
        // ghost record stores — so this is character-for-character the JSON a
        // commander sends, and the assertion below says so.
        let gesture = Intent::Attack {
            units: vec![soldier.to_bits()],
            target: barracks.to_bits(),
            select: None,
        };
        let typed: Intent = serde_json::from_str(&format!(
            r#"{{"type":"attack","units":[{}],"target":{}}}"#,
            soldier.to_bits(),
            barracks.to_bits()
        ))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&gesture).unwrap(),
            serde_json::to_value(&typed).unwrap()
        );

        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, gesture));
        app.update();

        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "attacking a remembered structure is legal — it is the rule the \
             compiler has always had, and now the human can reach it"
        );
        assert!(
            matches!(
                app.world().entity(soldier).get::<Order>(),
                Some(Order::Attack(hit)) if *hit == barracks
            ),
            "the same verb the bridge uses, against the same entity"
        );
    }

    /// A ghost can be a lie, and the interface must let the player find that
    /// out the same way a commander does — by ordering the attack and being
    /// told. The building is razed while nobody is watching, so the memory
    /// survives (docs/FOG.md: "the correct amount of wrong"), the click still
    /// lands, and the refusal is the same string the bridge would receive.
    ///
    /// This is also the end-to-end check on the error channel: a `ui`-source
    /// rejection reaches the human's `GameEvents` feed, which is the buffer
    /// the alert stack renders.
    #[test]
    fn a_refused_gesture_reaches_the_humans_alert_stack() {
        let barracks_at = Vec3::new(40.0, 0.0, 40.0);
        let mut app = compiler_app();

        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();
        let barracks = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Claude,
                Transform::from_translation(barracks_at),
                Health::new(700.0),
            ))
            .id();
        remember(&mut app, Team::Human, barracks, BuildingKind::Barracks, barracks_at);
        // Razed behind our back. The ghost stays — and so does the gesture.
        app.world_mut().entity_mut(barracks).despawn();

        let stale = barracks.to_bits();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Attack {
                units: vec![soldier.to_bits()],
                target: stale,
                select: None,
            },
        ));
        app.update();

        let expected = format!("target {stale} not found");
        assert_eq!(
            app.world().resource::<IntentErrors>().get(Team::Human),
            &[format!("ui: {expected}")],
            "the bridge's error string, unchanged, in the bridge's channel"
        );
        let feed = app.world().resource::<GameEvents>();
        let notices: Vec<&GameEvent> = feed
            .feed(Team::Human)
            .iter()
            .filter(|e| e.message.starts_with(UI_NOTICE_PREFIX))
            .collect();
        assert_eq!(notices.len(), 1, "the player is told exactly once");
        assert_eq!(notices[0].message, format!("{UI_NOTICE_PREFIX}: {expected}"));
        assert_eq!(notices[0].severity, EventSeverity::Warning);
        // Never the opponent's business that we fumbled an order.
        assert!(feed.feed(Team::Claude).is_empty());
    }

    /// A held-down right-click on an illegal target re-submits the identical
    /// gesture every frame. The alert stack is six rows and shares them with
    /// the match's real news, so one stuck mouse button must not be able to
    /// evict "hostiles near base". Told once, then silent.
    #[test]
    fn a_held_click_cannot_flood_the_alert_stack() {
        let mut app = compiler_app();
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();

        for _ in 0..60 {
            app.world_mut().send_event(SubmitIntent::ui(
                Team::Human,
                Intent::Attack {
                    units: vec![soldier.to_bits()],
                    target: 999_999,
                    select: None,
                },
            ));
            app.update();
        }

        let notices = app
            .world()
            .resource::<GameEvents>()
            .feed(Team::Human)
            .iter()
            .filter(|e| e.message.starts_with(UI_NOTICE_PREFIX))
            .count();
        assert_eq!(notices, 1, "sixty identical refusals are one piece of news");

        // A DIFFERENT refusal still gets through — the limiter suppresses
        // repetition, not the channel.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Build {
                worker: Some(soldier.to_bits()),
                kind: "Nonsense".to_string(),
                x: Some(0.0),
                z: Some(0.0),
                region: None,
                select: None,
                site: None,
            },
        ));
        app.update();
        let notices = app
            .world()
            .resource::<GameEvents>()
            .feed(Team::Human)
            .iter()
            .filter(|e| e.message.starts_with(UI_NOTICE_PREFIX))
            .count();
        assert_eq!(notices, 2, "a new problem is still news");
    }

    /// A bridge seat already reads its errors out of the snapshot; raising
    /// them on the event feed as well would double-report them and put one
    /// seat's mistakes into an artifact the other seat's renderer reads from.
    /// Source picks the channel, and that is the whole of what source decides.
    #[test]
    fn a_bridge_rejection_does_not_touch_the_event_feed() {
        let mut app = compiler_app();
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();

        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: Intent::Attack {
                units: vec![soldier.to_bits()],
                target: 999_999,
                select: None,
            },
            trigger: None,
            plan: None,
        });
        app.update();

        assert_eq!(
            app.world().resource::<IntentErrors>().get(Team::Human),
            &["cmd 0: target 999999 not found".to_string()],
            "the bridge's own channel is unchanged"
        );
        assert!(
            app.world()
                .resource::<GameEvents>()
                .feed(Team::Human)
                .is_empty(),
            "and nothing leaked into the human renderer's feed"
        );
    }

    /// The wire format is the schema: every historical bridge command must
    /// still deserialize into an `Intent`, and the tags must be unchanged.
    #[test]
    fn legacy_wire_commands_parse() {
        let cases = [
            r#"{"type":"move","units":[1],"x":1.0,"z":2.0}"#,
            r#"{"type":"attackmove","units":[1,2],"x":-3.5,"z":4.0}"#,
            r#"{"type":"attack","units":[1],"target":9}"#,
            r#"{"type":"harvest","units":[1],"target":9}"#,
            r#"{"type":"return","units":[1]}"#,
            r#"{"type":"follow","units":[1],"target":2}"#,
            r#"{"type":"stop","units":[1]}"#,
            r#"{"type":"build","worker":1,"kind":"Farm","x":0.0,"z":0.0}"#,
            r#"{"type":"train","building":1,"unit":"Footman"}"#,
            r#"{"type":"upgrade","building":1}"#,
            r#"{"type":"cancel","building":1,"index":0}"#,
            r#"{"type":"research","building":1,"upgrade":"attack"}"#,
            r#"{"type":"research","building":1,"upgrade":"armor"}"#,
            r#"{"type":"rally","building":1,"x":1.0,"z":2.0}"#,
            r#"{"type":"rally","building":1,"target":7}"#,
            r#"{"type":"cast","hero":1}"#,
            r#"{"type":"cast","caster":1}"#,
            r#"{"type":"cast","hero":1,"ability":2}"#,
            r#"{"type":"cast","hero":1,"ability":"Slam"}"#,
            // v3 geometry: a ground point, and a named unit. Both OPTIONAL —
            // the four forms above are still legal and still mean what they
            // always meant, which is the whole back-compat claim.
            r#"{"type":"cast","caster":1,"ability":"Slow","x":12.0,"z":-4.0}"#,
            r#"{"type":"cast","caster":1,"ability":"Slow","target":9}"#,
            r#"{"type":"buy","shop":1,"item":"HealingPotion"}"#,
            r#"{"type":"use_item","slot":0}"#,
            r#"{"type":"autopilot","on":true}"#,
            r#"{"type":"surrender"}"#,
            // The third match-level statement (wc3clone-t0d). A unit variant
            // like `surrender`, so the whole wire shape is the tag.
            r#"{"type":"ready"}"#,
            r#"{"type":"priority","units":[1],"classes":["Hero","Siege"]}"#,
            r#"{"type":"priority","units":[1]}"#,
            r#"{"type":"retreat","units":[1],"below":0.35,"x":1.0,"z":2.0}"#,
            r#"{"type":"leash","units":[1],"x":1.0,"z":2.0,"radius":20.0}"#,
            r#"{"type":"autocast","units":[1],"min_enemies":3}"#,
            r#"{"type":"autocast","units":[1],"min_enemies":3,"ability":1}"#,
            r#"{"type":"autocast","units":[1],"min_enemies":3,"ability":"Heal"}"#,
            r#"{"type":"squad","units":[1],"id":1}"#,
            r#"{"type":"squad","units":[1]}"#,
            r#"{"type":"posture","id":1,"posture":{"type":"defend","x":1.0,"z":2.0,"radius":18.0}}"#,
            r#"{"type":"posture","id":1,"posture":{"type":"push","x":1.0,"z":2.0}}"#,
            r#"{"type":"posture","id":1,"posture":{"type":"escort","unit":4}}"#,
            r#"{"type":"posture","id":1,"posture":{"type":"forage","x":0.0,"z":0.0}}"#,
            r#"{"type":"posture","id":1}"#,
            r#"{"type":"template","building":1,"squad":1,"retreat":{"below":0.35,"x":1.0,"z":2.0},"priority":["Hero"],"autocast":3}"#,
            r#"{"type":"template","building":1}"#,
            // v4 stances. Every spelling of the anchor, because all four are
            // documented in tools/COMMANDER_BRIEF.md and a commander that
            // learned one must never find another rejected: coordinates, the
            // `target` name the brief leads with, the `region` alias every
            // other place-taking verb uses, and the bare form that means "at
            // my base". Plus `id` for `squad`, so a commander who reached for
            // `posture`'s key gets the sentence it obviously meant.
            r#"{"type":"stance","squad":1,"stance":"turtle"}"#,
            r#"{"type":"stance","squad":1,"stance":"push","x":1.0,"z":2.0}"#,
            r#"{"type":"stance","squad":2,"stance":"secure","target":"north-pass"}"#,
            r#"{"type":"stance","squad":2,"stance":"harass","region":"north-pass"}"#,
            r#"{"type":"stance","id":3,"stance":"stage","x":0.0,"z":0.0}"#,
            // v3 triggers. Every predicate, both cadences, both clear-forms.
            r#"{"type":"trigger_set","name":"home-guard","when":{"type":"base_under_attack"},"then":{"type":"posture","id":1,"posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":22.0}}}"#,
            r#"{"type":"trigger_set","name":"hero-save","when":{"type":"hero_below","frac":0.35},"then":{"type":"move","units":[1],"x":-70.0,"z":-70.0},"repeat":60.0}"#,
            r#"{"type":"trigger_set","name":"sq","when":{"type":"squad_below","id":1,"frac":0.5},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"eyes","when":{"type":"enemy_sighted"},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"eyes2","when":{"type":"enemy_sighted","class":"Siege","count":3},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"gold","when":{"type":"bounty_spawned"},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"dry","when":{"type":"mine_dry"},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"tech","when":{"type":"tier_reached","tier":2},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"army","when":{"type":"unit_count","kind":"Footman","count":6},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_set","name":"clock","when":{"type":"game_time","at":360.0},"then":{"type":"stop","units":[1]}}"#,
            r#"{"type":"trigger_clear","name":"home-guard"}"#,
            r#"{"type":"trigger_clear"}"#,
            // v3 plans. All three advance forms, the terse step, both clears.
            r#"{"type":"plan_set","name":"opening","steps":[{"intent":{"type":"stop","units":[1]}}]}"#,
            r#"{"type":"plan_set","name":"boom","steps":[
                {"intent":{"type":"build","worker":1,"kind":"Barracks","x":-60.0,"z":-60.0},
                 "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}},
                {"intent":{"type":"train","building":2,"unit":"Sorcerer"},
                 "advance":{"type":"after","secs":30.0}},
                {"intent":{"type":"posture","id":1,"posture":{"type":"push","x":70.0,"z":70.0}},
                 "advance":{"type":"on_applied"}}]}"#,
            r#"{"type":"plan_clear","name":"opening"}"#,
            r#"{"type":"plan_clear"}"#,
        ];
        for case in cases {
            let parsed: Intent = serde_json::from_str(case)
                .unwrap_or_else(|e| panic!("{case} failed to parse: {e}"));
            // Every intent renders a sentence and re-serializes with its verb
            // as the `type` tag — the property the replay log depends on.
            assert!(!parsed.sentence().is_empty(), "{case} had no sentence");
            let round: serde_json::Value = serde_json::to_value(&parsed).unwrap();
            assert_eq!(
                round.get("type").and_then(|v| v.as_str()),
                Some(parsed.verb()),
                "{case} re-serialized under a different tag"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Triggers (docs/INTENT.md § Triggers)
    // -----------------------------------------------------------------------

    fn arm(app: &mut App, team: Team, json: &str) {
        app.world_mut().send_event(from_the_wire(team, json));
        app.update();
    }

    fn trigger_names(app: &App, team: Team) -> Vec<String> {
        app.world()
            .resource::<Triggers>()
            .get(team)
            .iter()
            .map(|t| t.name.as_str().to_string())
            .collect()
    }

    /// **`supply_capped` arrives from the wire under the name the tooling
    /// writes.** The one thing a hand-checked `holds` test cannot prove: that
    /// the string `tools/intent_compile.py` emits, that `COMMANDER_BRIEF.md`
    /// prints, and that a commander types are all the same string the enum
    /// deserializes from — and that `validate_predicate` lets it through
    /// rather than refusing an arm it has never heard of.
    #[test]
    fn supply_capped_arms_from_the_wire_spelling_the_brief_prints() {
        let mut app = compiler_app();
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"supply-capped","repeat":45,
                "when":{"type":"supply_capped"},"then":{"type":"stop","units":[]}}"#,
        );
        let refused = app.world().resource::<IntentErrors>().get(Team::Human).clone();
        assert!(
            refused.is_empty(),
            "the brief's recipe 7 must arm cleanly: {refused:?}"
        );
        let armed = &app.world().resource::<Triggers>().get(Team::Human)[0];
        assert_eq!(armed.when, TriggerWhen::SupplyCapped);
        assert_eq!(
            armed.when.phrase(),
            "we are supply capped",
            "and it reads back as English in `sentence` and the feed"
        );
    }

    /// **The cap is eight, and re-using a name is free.**
    ///
    /// The bound is what makes triggers doctrine rather than programming, so it
    /// has to be a bound rather than a suggestion — and it has to be a bound on
    /// *distinct rules*, or a commander tuning one number every cycle would
    /// spend their whole allowance on a rule they already had.
    #[test]
    fn a_team_may_arm_eight_triggers_and_replacing_one_costs_nothing() {
        let mut app = compiler_app();
        for i in 0..MAX_TRIGGERS_PER_TEAM {
            arm(
                &mut app,
                Team::Human,
                &format!(
                    r#"{{"type":"trigger_set","name":"rule-{i}",
                        "when":{{"type":"game_time","at":{i}.0}},
                        "then":{{"type":"stop","units":[]}}}}"#
                ),
            );
        }
        assert_eq!(trigger_names(&app, Team::Human).len(), MAX_TRIGGERS_PER_TEAM);
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "eight must fit"
        );

        // The ninth is refused, and the refusal names what is already there so
        // the commander can pick one to drop without another round trip.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"one-too-many",
                "when":{"type":"mine_dry"},"then":{"type":"stop","units":[]}}"#,
        );
        let errors = app.world().resource::<IntentErrors>().get(Team::Human).clone();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("you already have 8 triggers"), "{}", errors[0]);
        assert!(errors[0].contains("rule-0"), "the refusal lists them: {}", errors[0]);
        assert_eq!(trigger_names(&app, Team::Human).len(), MAX_TRIGGERS_PER_TEAM);

        // Re-stating rule-3 replaces it IN PLACE — same slot, same order, no
        // cap spent. Order matters because it is firing order.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"rule-3",
                "when":{"type":"mine_dry"},"then":{"type":"stop","units":[]}}"#,
        );
        let names = trigger_names(&app, Team::Human);
        assert_eq!(names.len(), MAX_TRIGGERS_PER_TEAM);
        assert_eq!(names[3], "rule-3", "replaced in place, not moved to the end");
        assert_eq!(
            app.world().resource::<Triggers>().get(Team::Human)[3].when,
            TriggerWhen::MineDry,
            "and it really is the new rule"
        );
    }

    /// The cap is per team, like every other thing in this game that is per
    /// team. A commander filling their own eight must not be able to starve
    /// their opponent's.
    #[test]
    fn the_cap_is_per_team() {
        let mut app = compiler_app();
        for i in 0..MAX_TRIGGERS_PER_TEAM {
            arm(
                &mut app,
                Team::Human,
                &format!(
                    r#"{{"type":"trigger_set","name":"h{i}",
                        "when":{{"type":"mine_dry"}},"then":{{"type":"stop","units":[]}}}}"#
                ),
            );
        }
        arm(
            &mut app,
            Team::Claude,
            r#"{"type":"trigger_set","name":"c0","when":{"type":"mine_dry"},
                "then":{"type":"stop","units":[]}}"#,
        );
        assert_eq!(trigger_names(&app, Team::Claude), vec!["c0"]);
        assert!(app.world().resource::<IntentErrors>().get(Team::Claude).is_empty());
    }

    /// **A trigger cannot arm a trigger.** The line between doctrine and a
    /// scripting language, and also the thing that makes the cap an actual
    /// bound: without it, one trigger could re-arm seven others forever.
    #[test]
    fn a_trigger_may_not_arm_or_clear_another_trigger() {
        let mut app = compiler_app();
        for nested in [
            r#"{"type":"trigger_set","name":"outer","when":{"type":"mine_dry"},
                "then":{"type":"trigger_set","name":"inner","when":{"type":"mine_dry"},
                        "then":{"type":"stop","units":[]}}}"#,
            r#"{"type":"trigger_set","name":"outer","when":{"type":"mine_dry"},
                "then":{"type":"trigger_clear","name":"whatever"}}"#,
        ] {
            arm(&mut app, Team::Human, nested);
            let errors = app.world().resource::<IntentErrors>().get(Team::Human).clone();
            assert!(
                errors.iter().any(|e| e.contains("cannot arm or clear another trigger")),
                "{errors:?}"
            );
            assert!(trigger_names(&app, Team::Human).is_empty(), "nothing was armed");
            app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();
        }
    }

    /// Bad predicate parameters are refused at ARM time, because a predicate is
    /// the one half of a trigger whose every parameter is a constant the
    /// commander typed. The action is deliberately not checked here — see the
    /// note in `compile_intent`.
    #[test]
    fn a_predicate_is_validated_when_it_is_armed() {
        let mut app = compiler_app();
        for (json, expect) in [
            (
                r#"{"type":"trigger_set","name":"a","when":{"type":"hero_below","frac":1.5},
                    "then":{"type":"stop","units":[]}}"#,
                "health fraction in (0,1]",
            ),
            (
                r#"{"type":"trigger_set","name":"a","when":{"type":"enemy_sighted","class":"Wizard"},
                    "then":{"type":"stop","units":[]}}"#,
                "unknown target class 'Wizard'",
            ),
            (
                r#"{"type":"trigger_set","name":"a","when":{"type":"unit_count","kind":"Dragon","count":2},
                    "then":{"type":"stop","units":[]}}"#,
                "unknown unit kind 'Dragon'",
            ),
            (
                r#"{"type":"trigger_set","name":"a","when":{"type":"tier_reached","tier":7},
                    "then":{"type":"stop","units":[]}}"#,
                "tier must be 1, 2 or 3",
            ),
            (
                r#"{"type":"trigger_set","name":"a","when":{"type":"mine_dry"},
                    "then":{"type":"stop","units":[]},"repeat":0.0}"#,
                "cooldown must be > 0",
            ),
            (
                r#"{"type":"trigger_set","name":"   ","when":{"type":"mine_dry"},
                    "then":{"type":"stop","units":[]}}"#,
                "is not a usable trigger name",
            ),
        ] {
            arm(&mut app, Team::Human, json);
            let errors = app.world().resource::<IntentErrors>().get(Team::Human).clone();
            assert!(
                errors.iter().any(|e| e.contains(expect)),
                "wanted {expect:?} in {errors:?}"
            );
            assert!(trigger_names(&app, Team::Human).is_empty());
            app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();
        }
    }

    /// Clearing: one by name, or the whole slate. A name that is not there is
    /// an ERROR rather than a silent no-op — "I cleared it" and "there was
    /// nothing by that name" call for opposite next moves.
    #[test]
    fn clearing_names_one_rule_or_all_of_them() {
        let mut app = compiler_app();
        for name in ["a", "b"] {
            arm(
                &mut app,
                Team::Human,
                &format!(
                    r#"{{"type":"trigger_set","name":"{name}","when":{{"type":"mine_dry"}},
                        "then":{{"type":"stop","units":[]}}}}"#
                ),
            );
        }
        arm(&mut app, Team::Human, r#"{"type":"trigger_clear","name":"a"}"#);
        assert_eq!(trigger_names(&app, Team::Human), vec!["b"]);
        assert!(app.world().resource::<IntentErrors>().get(Team::Human).is_empty());

        arm(&mut app, Team::Human, r#"{"type":"trigger_clear","name":"a"}"#);
        let errors = app.world().resource::<IntentErrors>().get(Team::Human).clone();
        assert!(
            errors.iter().any(|e| e.contains("you have no trigger named 'a'")),
            "{errors:?}"
        );

        arm(&mut app, Team::Human, r#"{"type":"trigger_clear"}"#);
        assert!(trigger_names(&app, Team::Human).is_empty(), "the whole slate");
    }

    // -----------------------------------------------------------------------
    // Plans (docs/INTENT.md § Plans)
    // -----------------------------------------------------------------------

    fn plan_names(app: &App, team: Team) -> Vec<String> {
        app.world()
            .resource::<Plans>()
            .get(team)
            .iter()
            .map(|p| p.name.as_str().to_string())
            .collect()
    }

    fn first_error(app: &App, team: Team) -> String {
        app.world()
            .resource::<IntentErrors>()
            .get(team)
            .first()
            .cloned()
            .unwrap_or_default()
    }

    /// **The caps are two plans of eight steps, and re-using a name is free.**
    ///
    /// Same argument as the trigger cap and the same shape of test: a bound has
    /// to be a bound, and it has to be a bound on *distinct plans*, or a
    /// commander iterating on one opening would spend the whole allowance on a
    /// plan they already had.
    #[test]
    fn a_team_may_run_two_plans_of_eight_steps_and_replacing_one_is_free() {
        let mut app = compiler_app();
        let step = r#"{"intent":{"type":"stop","units":[]}}"#;
        let eight = vec![step; MAX_PLAN_STEPS].join(",");
        for name in ["opening", "follow-up"] {
            arm(
                &mut app,
                Team::Human,
                &format!(r#"{{"type":"plan_set","name":"{name}","steps":[{eight}]}}"#),
            );
        }
        assert_eq!(plan_names(&app, Team::Human), vec!["opening", "follow-up"]);
        assert!(app.world().resource::<IntentErrors>().get(Team::Human).is_empty());

        arm(
            &mut app,
            Team::Human,
            &format!(r#"{{"type":"plan_set","name":"third","steps":[{step}]}}"#),
        );
        let err = first_error(&app, Team::Human);
        assert!(err.contains(&format!("{MAX_PLANS_PER_TEAM} plans")), "{err}");
        assert!(err.contains("opening") && err.contains("follow-up"), "it names them: {err}");
        assert_eq!(plan_names(&app, Team::Human).len(), MAX_PLANS_PER_TEAM);
        app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();

        // Nine steps is one too many, and the refusal says so with the number.
        let nine = vec![step; MAX_PLAN_STEPS + 1].join(",");
        arm(
            &mut app,
            Team::Human,
            &format!(r#"{{"type":"plan_set","name":"opening","steps":[{nine}]}}"#),
        );
        let err = first_error(&app, Team::Human);
        assert!(err.contains("9 steps") && err.contains("the most is 8"), "{err}");
        assert_eq!(
            app.world().resource::<Plans>().get(Team::Human)[0].steps.len(),
            MAX_PLAN_STEPS,
            "and the plan it would have replaced is untouched"
        );
        app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();

        // Replacing by name costs no slot and restarts the plan.
        app.world_mut().resource_mut::<Plans>().get_mut(Team::Human)[0].at = 4;
        arm(
            &mut app,
            Team::Human,
            &format!(r#"{{"type":"plan_set","name":"opening","steps":[{step}]}}"#),
        );
        assert!(app.world().resource::<IntentErrors>().get(Team::Human).is_empty());
        assert_eq!(plan_names(&app, Team::Human), vec!["opening", "follow-up"]);
        let p = &app.world().resource::<Plans>().get(Team::Human)[0];
        assert_eq!((p.steps.len(), p.at), (1, 0), "replaced in place and restarted");
    }

    /// Plans are per team, like everything else in this compiler.
    #[test]
    fn one_teams_plans_are_not_the_others() {
        let mut app = compiler_app();
        let step = r#"{"intent":{"type":"stop","units":[]}}"#;
        for name in ["h1", "h2"] {
            arm(
                &mut app,
                Team::Human,
                &format!(r#"{{"type":"plan_set","name":"{name}","steps":[{step}]}}"#),
            );
        }
        arm(
            &mut app,
            Team::Claude,
            &format!(r#"{{"type":"plan_set","name":"c0","steps":[{step}]}}"#),
        );
        assert_eq!(plan_names(&app, Team::Claude), vec!["c0"]);
        assert!(app.world().resource::<IntentErrors>().get(Team::Claude).is_empty());
    }

    /// **A plan cannot set a plan, and a trigger cannot set one either** — but
    /// a plan step MAY arm a trigger.
    ///
    /// That asymmetry is the whole bound and it is worth pinning: the graph is
    /// exactly two rungs deep (plan -> trigger -> ordinary intent), each rung
    /// is capped, and no edge points back up. Remove the second refusal and a
    /// trigger could set a plan whose step re-armed the trigger, forever.
    #[test]
    fn the_deferral_graph_is_two_rungs_deep_and_never_points_back_up() {
        let mut app = compiler_app();
        for nested in [
            r#"{"type":"plan_set","name":"outer","steps":[
                {"intent":{"type":"plan_set","name":"inner","steps":[
                    {"intent":{"type":"stop","units":[]}}]}}]}"#,
            r#"{"type":"plan_set","name":"outer","steps":[
                {"intent":{"type":"plan_clear","name":"whatever"}}]}"#,
        ] {
            arm(&mut app, Team::Human, nested);
            let err = first_error(&app, Team::Human);
            assert!(err.contains("sets or clears a plan"), "{err}");
            assert!(plan_names(&app, Team::Human).is_empty(), "nothing was set");
            app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();
        }

        // A TRIGGER may not set a plan either.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"t","when":{"type":"mine_dry"},
                "then":{"type":"plan_set","name":"p","steps":[
                    {"intent":{"type":"stop","units":[]}}]}}"#,
        );
        let err = first_error(&app, Team::Human);
        assert!(err.contains("cannot arm or clear another trigger or a plan"), "{err}");
        assert!(app.world().resource::<Triggers>().get(Team::Human).is_empty());
        app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();

        // But a plan step arming a trigger is a real idiom and is accepted.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"plan_set","name":"opening","steps":[
                {"intent":{"type":"trigger_set","name":"home-guard",
                           "when":{"type":"base_under_attack"},
                           "then":{"type":"stop","units":[]}}}]}"#,
        );
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "{:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );
        assert_eq!(plan_names(&app, Team::Human), vec!["opening"]);
    }

    /// Advance conditions are validated when the plan is SET, because — exactly
    /// like a trigger's predicate — every parameter in one is a constant the
    /// commander typed. The step INTENTS are deliberately not checked; that
    /// note lives on the arm in `compile_intent`.
    #[test]
    fn an_advance_condition_is_validated_when_the_plan_is_set() {
        let mut app = compiler_app();
        for (json, expect) in [
            (
                r#"{"type":"plan_set","name":"p","steps":[
                    {"intent":{"type":"stop","units":[]},
                     "advance":{"type":"when","when":{"type":"tier_reached","tier":9}}}]}"#,
                "tier must be 1, 2 or 3",
            ),
            (
                r#"{"type":"plan_set","name":"p","steps":[
                    {"intent":{"type":"stop","units":[]}},
                    {"intent":{"type":"stop","units":[]},
                     "advance":{"type":"after","secs":0.0}}]}"#,
                "'after' must be > 0 seconds",
            ),
            (
                r#"{"type":"plan_set","name":"   ","steps":[
                    {"intent":{"type":"stop","units":[]}}]}"#,
                "is not a usable plan name",
            ),
            (
                r#"{"type":"plan_set","name":"p","steps":[]}"#,
                "has no steps",
            ),
        ] {
            arm(&mut app, Team::Human, json);
            let err = first_error(&app, Team::Human);
            assert!(err.contains(expect), "wanted {expect:?} in {err:?}");
            assert!(plan_names(&app, Team::Human).is_empty());
            app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();
        }

        // A step's INTENT naming things that do not exist yet is accepted —
        // that is the entire point of writing a sequence in advance.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"plan_set","name":"p","steps":[
                {"intent":{"type":"train","building":999999,"unit":"Footman"}},
                {"intent":{"type":"research","building":999998,"upgrade":"attack"}}]}"#,
        );
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "a plan may name a world that does not exist yet: {:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );
    }

    /// **A chain is a plan whose steps are stances, and nothing here had to
    /// learn a new verb to say so.** The compiler takes the sentence
    /// `docs/AFFORDANCES.md` § Chains describes — turtle until the hero is
    /// healed, then secure the northwest mine — with no `stance_plan`, no
    /// second plan machinery, and no arm-time refusal.
    #[test]
    fn a_chain_is_a_plan_whose_steps_are_stances() {
        let mut app = compiler_app();
        // `northwest mine` is the MAP's own word, live from second zero with
        // nothing named first — so this chain is armable on the opening poll.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"plan_set","name":"hold","steps":[
                {"intent":{"type":"stance","squad":1,"stance":"turtle"},
                 "advance":{"type":"when","when":{"type":"hero_above","frac":0.8}}},
                {"intent":{"type":"stance","squad":1,"stance":"secure",
                           "target":"northwest mine"}}]}"#,
        );
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "a chain over ground this seat can name is unremarkable: {:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );
        assert_eq!(plan_names(&app, Team::Human), vec!["hold"]);
        let sentence = Intent::PlanSet {
            name: "hold".to_string(),
            steps: app.world().resource::<Plans>().get(Team::Human)[0].steps.clone(),
        }
        .sentence();
        assert!(
            sentence.contains("squad 1 takes the turtle stance")
                && sentence.contains("when every living hero is back above 80% health")
                && sentence.contains("squad 1 takes the secure stance"),
            "the chain reads as one English sentence: {sentence}"
        );
    }

    /// **Teaching-only validation of a late-bound target.** A chain step whose
    /// ground has not been named yet ARMS — that is the whole point of a policy
    /// decided at leisure — and the seat is told, at arm time, which step is
    /// holding and why. docs/AFFORDANCES.md § Chains: *"it arms and reports
    /// 'chain holds at step 1: target unresolvable until scouted'"*.
    #[test]
    fn a_chain_step_that_cannot_resolve_yet_arms_and_says_which_step_holds() {
        let mut app = compiler_app();
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"plan_set","name":"hold","steps":[
                {"intent":{"type":"stance","squad":1,"stance":"turtle"},
                 "advance":{"type":"when","when":{"type":"hero_above","frac":0.8}}},
                {"intent":{"type":"stance","squad":1,"stance":"secure",
                           "target":"their-expansion"}}]}"#,
        );
        assert_eq!(
            plan_names(&app, Team::Human),
            vec!["hold"],
            "armed. An unscouted target must never cost a commander the plan"
        );
        let errors = app.world().resource::<IntentErrors>().get(Team::Human).clone();
        assert_eq!(errors.len(), 1, "one line, about the one step: {errors:?}");
        assert!(
            errors[0].contains("chain holds at step 2")
                && errors[0].contains("no region named 'their-expansion'")
                && errors[0].contains("known places")
                && errors[0].contains("armed"),
            "it names the step, the resolver's own reason, the menu of places, \
             and the fact that it armed anyway: {}",
            errors[0]
        );
        app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();

        // The same notice for the OTHER late-bound channel, so the rule is
        // about resolvability rather than about regions.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"plan_set","name":"push","steps":[
                {"intent":{"type":"attackmove","select":"all army","x":0.0,"z":0.0}}]}"#,
        );
        let errors = app.world().resource::<IntentErrors>().get(Team::Human).clone();
        assert!(
            errors[0].contains("chain holds at step 1")
                && errors[0].contains("matches none of your units right now"),
            "{errors:?}"
        );
        assert!(plan_names(&app, Team::Human).contains(&"push".to_string()));
    }

    /// The wait-condition half. `hero_above` is judged at arm time like every
    /// other predicate parameter, and it reaches plans through the one seam —
    /// `PlanAdvance::When` carries a whole `TriggerWhen`, so a predicate added
    /// for chains is a trigger predicate too, for free and in both directions.
    #[test]
    fn hero_above_is_a_predicate_like_any_other_at_arm_time() {
        let mut app = compiler_app();
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"plan_set","name":"p","steps":[
                {"intent":{"type":"stop","units":[]},
                 "advance":{"type":"when","when":{"type":"hero_above","frac":0.0}}}]}"#,
        );
        let err = first_error(&app, Team::Human);
        assert!(
            err.contains("hero_above must be a health fraction in (0,1]"),
            "{err}"
        );
        assert!(plan_names(&app, Team::Human).is_empty());
        app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();

        // And as a trigger, with no work anywhere for it to be one.
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"rejoin","when":{"type":"hero_above","frac":0.9},
                "then":{"type":"stance","squad":1,"stance":"push","target":"mid"}}"#,
        );
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "{:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );
        assert_eq!(trigger_names(&app, Team::Human), vec!["rejoin"]);
    }

    /// Clearing: one by name, or the whole slate, with the same "there was
    /// nothing by that name" error triggers give.
    #[test]
    fn clearing_names_one_plan_or_all_of_them() {
        let mut app = compiler_app();
        let step = r#"{"intent":{"type":"stop","units":[]}}"#;
        for name in ["a", "b"] {
            arm(
                &mut app,
                Team::Human,
                &format!(r#"{{"type":"plan_set","name":"{name}","steps":[{step}]}}"#),
            );
        }
        arm(&mut app, Team::Human, r#"{"type":"plan_clear","name":"a"}"#);
        assert_eq!(plan_names(&app, Team::Human), vec!["b"]);
        assert!(app.world().resource::<IntentErrors>().get(Team::Human).is_empty());

        arm(&mut app, Team::Human, r#"{"type":"plan_clear","name":"a"}"#);
        assert!(
            first_error(&app, Team::Human).contains("you have no plan named 'a'"),
            "{}",
            first_error(&app, Team::Human)
        );
        app.world_mut().resource_mut::<IntentErrors>().get_mut(Team::Human).clear();

        arm(&mut app, Team::Human, r#"{"type":"plan_clear"}"#);
        assert!(plan_names(&app, Team::Human).is_empty());
        assert!(app.world().resource::<IntentErrors>().get(Team::Human).is_empty());
    }

    /// **A plan's sentence is the whole sequence**, joined by the word the verb
    /// is named after. It is what a co-commander's proposal is reviewed on, so
    /// a step count would not do — the human answering `[Enter]` has to see
    /// what they are agreeing to on the line they are answering.
    #[test]
    fn a_plans_sentence_reads_as_one_english_sequence() {
        let plan: Intent = serde_json::from_str(
            r#"{"type":"plan_set","name":"boomer","steps":[
                {"intent":{"type":"build","worker":7,"kind":"Barracks","x":-60.0,"z":-60.0},
                 "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}},
                {"intent":{"type":"train","building":9,"unit":"Sorcerer"},
                 "advance":{"type":"after","secs":30.0}},
                {"intent":{"type":"posture","id":2,"posture":{"type":"push","x":70.0,"z":70.0}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            plan.sentence(),
            "plan boomer (3 steps): worker 7 builds Barracks at (-60.0, -60.0), \
             then when we reach tier 2: building 9 trains Sorcerer, \
             then after 30s: squad 2 pushes to (70.0, 70.0)"
        );
        // The last step's advance decides when the plan reports itself done, so
        // it reads as a trailing clause rather than introducing a step that is
        // not there.
        let trailing: Intent = serde_json::from_str(
            r#"{"type":"plan_set","name":"p","steps":[
                {"intent":{"type":"stop","units":[1]},
                 "advance":{"type":"when","when":{"type":"unit_count","kind":"Footman","count":6}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            trailing.sentence(),
            "plan p (1 steps): unit 1 hold position (done when we field 6 or more Footman)"
        );
    }

    /// **A plan step is link-exempt**, on the identical argument that exempts a
    /// trigger: it is engine-executed standing policy whose author paid the
    /// reach when they wrote it down (docs/TEMPO.md verb table).
    #[test]
    fn a_plan_step_pays_no_command_link() {
        let mut app = compiler_app();
        // The severed-arm case the other link tests use: no command nodes, so
        // every DIRECT order to a unit pays the full curve.
        app.insert_resource(CommandLatency { on: true, ..Default::default() })
            .insert_resource(CommandNodes { nodes: Vec::new(), ready: true });

        let soldier = |app: &mut App| {
            app.world_mut()
                .spawn((
                    Unit { kind: UnitKind::Footman },
                    Team::Human,
                    Transform::from_xyz(60.0, 0.0, 60.0),
                    Health::new(100.0),
                    Order::Idle,
                ))
                .id()
        };

        // Spoken by hand: it travels.
        let by_hand = soldier(&mut app);
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Move {
                units: vec![by_hand.to_bits()],
                x: Some(-70.0),
                z: Some(-70.0),
                region: None,
                select: None,
            },
        ));
        app.update();
        let pending = app
            .world()
            .entity(by_hand)
            .get::<PendingOrder>()
            .expect("a hand order from outside the chain of command travels");
        assert!(pending.link() > 0.0);

        // The identical order as a plan step: it lands in the frame it was
        // submitted, exactly as a fired trigger's does.
        let by_plan = soldier(&mut app);
        app.world_mut().send_event(SubmitIntent::plan_step(
            Team::Human,
            IntentSource::Bridge,
            PlanStamp {
                name: PlanName::new("opening").unwrap(),
                step: 2,
                of: 5,
            },
            Intent::Move {
                units: vec![by_plan.to_bits()],
                x: Some(-70.0),
                z: Some(-70.0),
                region: None,
                select: None,
            },
        ));
        app.update();
        assert!(
            app.world().entity(by_plan).get::<PendingOrder>().is_none(),
            "a plan step is engine-executed standing policy and pays nothing"
        );
        assert!(
            matches!(app.world().entity(by_plan).get::<Order>(), Some(Order::Move(_))),
            "it is already moving"
        );
        // And the unit can say which step moved it.
        let why = app
            .world()
            .entity(by_plan)
            .get::<Provenance>()
            .expect("a plan step stamps its targets")
            .why();
        assert!(why.starts_with("plan:opening step 2/5 move by bridge"), "{why}");
    }

    /// **The sentence**, which is what a person reads in `intent_log.jsonl` and
    /// in the event feed. It carries BOTH halves — the condition and the action
    /// it defers — because a line naming only the condition leaves the reader
    /// unable to tell what is about to happen to their army.
    #[test]
    fn a_trigger_reads_as_one_english_sentence() {
        let armed: Intent = serde_json::from_str(
            r#"{"type":"trigger_set","name":"home-guard",
                "when":{"type":"base_under_attack"},
                "then":{"type":"posture","id":1,
                        "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":22.0}}}"#,
        )
        .unwrap();
        assert_eq!(
            armed.sentence(),
            "when the base is attacked: squad 1 defends (-70.0, -70.0) within 22 \
             (trigger: home-guard)"
        );

        let repeating: Intent = serde_json::from_str(
            r#"{"type":"trigger_set","name":"hero-save",
                "when":{"type":"hero_below","frac":0.35},
                "then":{"type":"move","units":[41],"x":-70.0,"z":-70.0},
                "repeat":60.0}"#,
        )
        .unwrap();
        assert_eq!(
            repeating.sentence(),
            "when a hero drops below 35% health: move unit 41 to (-70.0, -70.0) \
             (trigger: hero-save, repeating every 60s)"
        );

        assert_eq!(
            Intent::TriggerClear { name: Some("home-guard".into()) }.sentence(),
            "clear trigger home-guard"
        );
        assert_eq!(
            Intent::TriggerClear { name: None }.sentence(),
            "clear every trigger"
        );
    }

    /// **A trigger-fired order is exempt from the command link, and says so.**
    ///
    /// docs/TEMPO.md exempts every doctrine verb on one rule — *standing orders
    /// are local; direct orders travel*. A trigger is standing policy whose
    /// condition came true, so its author paid the reach when they armed it.
    /// The contrast is the contract: the identical `move`, from the identical
    /// place, at the identical severed-arm latency, pays when a player speaks
    /// it now and pays nothing when a rule fires it.
    #[test]
    fn a_trigger_fired_order_pays_no_link_where_a_spoken_one_would() {
        let mut app = compiler_app();
        app.insert_resource(CommandLatency { on: true, ..Default::default() })
            .insert_resource(CommandNodes { nodes: Vec::new(), ready: true });
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        let go = Intent::Move {
            units: vec![soldier.to_bits()],
            x: Some(40.0),
            z: Some(40.0),
            region: None,
            select: None,
        };

        // Spoken now: it travels.
        app.world_mut().send_event(SubmitIntent::ui(Team::Human, go.clone()));
        app.update();
        assert!(
            app.world().entity(soldier).get::<PendingOrder>().is_some(),
            "a direct order from outside the chain of command must travel"
        );
        app.world_mut().entity_mut(soldier).remove::<PendingOrder>();
        app.world_mut().entity_mut(soldier).insert(Order::Idle);

        // Fired by a rule armed earlier: it lands now.
        let name = TriggerName::new("home-guard").unwrap();
        app.world_mut()
            .send_event(SubmitIntent::fired(Team::Human, IntentSource::Bridge, name, go));
        app.update();
        assert!(
            app.world().entity(soldier).get::<PendingOrder>().is_none(),
            "engine-executed standing policy must not pay the link twice"
        );
        assert!(
            matches!(app.world().entity(soldier).get::<Order>(), Some(Order::Move(_))),
            "and the unit is actually moving"
        );
    }

    /// **"Why are you doing that?" has a trigger rung.** A trigger-fired order
    /// that answered `order:move by bridge` would be claiming somebody decided
    /// to move this unit just now, which is exactly what did not happen. The
    /// seat is still named, because a trigger has an author and the engine is
    /// only its executor.
    #[test]
    fn a_trigger_fired_order_says_which_rule_moved_the_unit() {
        let mut app = compiler_app();
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        let name = TriggerName::new("home-guard").unwrap();
        app.world_mut().send_event(SubmitIntent::fired(
            Team::Human,
            IntentSource::Ui,
            name,
            Intent::Move {
                units: vec![soldier.to_bits()],
                x: Some(1.0),
                z: Some(2.0),
                region: None,
                select: None,
            },
        ));
        app.update();
        let why = app
            .world()
            .entity(soldier)
            .get::<Provenance>()
            .expect("a fired order stamps its reason")
            .why();
        assert_eq!(why, "trigger:home-guard move by ui t=0");
    }

    /// **A trigger is in the match record twice**: once as the sentence that
    /// armed it, once as the sentence it fired. `IntentJournal` is the
    /// in-memory tail of `intent_log.jsonl` — same four fields — so asserting
    /// on it is asserting on what the replay file says.
    ///
    /// The fired entry is attributed to the seat that ARMED the rule, not to
    /// the engine, and the `tag` names which rule spoke. Both matter to a
    /// reader: a co-commander's `partner_log` would otherwise show its partner
    /// giving an order they were not at the keyboard for.
    #[test]
    fn arming_a_trigger_and_firing_it_both_reach_the_replay_record() {
        let mut app = compiler_app();
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"home-guard","repeat":30.0,
                "when":{"type":"base_under_attack"},
                "then":{"type":"posture","id":1,
                        "posture":{"type":"defend","x":-70.0,"z":-70.0,"radius":26.0}}}"#,
        );
        // Now the fire, exactly as trigger.rs submits it.
        let name = TriggerName::new("home-guard").unwrap();
        app.world_mut().send_event(SubmitIntent::fired(
            Team::Human,
            IntentSource::Bridge,
            name,
            Intent::Posture {
                id: 1,
                posture: Some(PostureIntent::Defend {
                    x: Some(-70.0),
                    z: Some(-70.0),
                    region: None,
                    radius: Some(26.0),
                }),
            },
        ));
        app.update();

        let journal = app.world().resource::<IntentJournal>();
        let lines: Vec<(&str, &str, bool)> = journal
            .get(Team::Human)
            .iter()
            .map(|e| (e.verb, e.sentence.as_str(), e.ok))
            .collect();
        assert_eq!(
            lines,
            vec![
                (
                    "trigger_set",
                    "when the base is attacked: squad 1 defends (-70.0, -70.0) within 26 \
                     (trigger: home-guard, repeating every 30s)",
                    true
                ),
                ("posture", "squad 1 defends (-70.0, -70.0) within 26", true),
            ]
        );
        // The action's sentence is a SUBSTRING of the rule's, because the rule
        // renders `then.sentence()` verbatim. That is what lets a reader join
        // "what fired" to "what I armed" by eye.
        assert!(lines[0].1.contains(lines[1].1));
        assert!(
            journal
                .get(Team::Human)
                .iter()
                .all(|e| e.source == IntentSource::Bridge),
            "the author, not the engine"
        );
    }

    /// A rule armed from the keyboard that bounces must tell the human, and
    /// must name the RULE rather than a gesture they never made.
    #[test]
    fn a_failing_trigger_tells_the_human_which_rule_failed() {
        let mut app = compiler_app();
        let name = TriggerName::new("home-guard").unwrap();
        app.world_mut().send_event(SubmitIntent::fired(
            Team::Human,
            IntentSource::Ui,
            name,
            Intent::Move {
                units: vec![999_999],
                x: Some(1.0),
                z: Some(2.0),
                region: None,
                select: None,
            },
        ));
        app.update();
        let feed = app.world().resource::<GameEvents>();
        let notices: Vec<&str> = feed
            .feed(Team::Human)
            .iter()
            .map(|e| e.message.as_str())
            .collect();
        assert!(
            notices.iter().any(|m| m.starts_with("trigger home-guard refused:")),
            "{notices:?}"
        );
        assert!(
            !notices.iter().any(|m| m.starts_with("order refused")),
            "the player made no gesture: {notices:?}"
        );
    }

    /// **Every hall is a caster, not just the first one** — `wc3clone-d4y`,
    /// round-10 AAR. `cast CallToArms` at an expansion TownHall came back
    /// `caster N is not a hero or an own ability building`, twice, while the
    /// identical command worked at the team's Keep.
    #[test]
    fn a_second_hall_casts_call_to_arms_like_the_first() {
        let mut app = compiler_app();
        let hall = |app: &mut App, kind: BuildingKind, at: Vec3| {
            app.world_mut()
                .spawn((
                    Building { kind },
                    Team::Human,
                    Transform::from_translation(at),
                ))
                .id()
        };
        let keep = hall(&mut app, BuildingKind::Keep, Vec3::new(-70.0, 0.0, -70.0));
        let expansion = hall(&mut app, BuildingKind::TownHall, Vec3::new(20.0, 0.0, 20.0));

        for (label, caster) in [("keep", keep), ("expansion", expansion)] {
            app.world_mut()
                .resource_mut::<IntentErrors>()
                .get_mut(Team::Human)
                .clear();
            app.world_mut().send_event(SubmitIntent {
                team: Team::Human,
                source: IntentSource::Bridge,
                tag: "cmd 0".to_string(),
                intent: Intent::Cast {
                    x: None,
                    z: None,
                    target: None,
                    hero: Some(caster.to_bits()),
                    ability: Some(AbilitySelector::Id("CallToArms".to_string())),
                    select: None,
                },
                trigger: None,
                plan: None,
            });
            app.update();
            assert!(
                app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
                "{label} hall refused the cast: {:?}",
                app.world().resource::<IntentErrors>().get(Team::Human)
            );
        }

        // So the roster and the compiler were both right, and the string was
        // the liar: ONE sentence stood for four different failures, and the
        // one it sounded like was the only one that was never happening.
        // Each now says which.
        let cast_error = |app: &mut App, caster: u64| -> String {
            app.world_mut()
                .resource_mut::<IntentErrors>()
                .get_mut(Team::Human)
                .clear();
            app.world_mut().send_event(SubmitIntent {
                team: Team::Human,
                source: IntentSource::Bridge,
                tag: "cmd 0".to_string(),
                intent: Intent::Cast {
                    x: None,
                    z: None,
                    target: None,
                    hero: Some(caster),
                    ability: None,
                    select: None,
                },
                trigger: None,
                plan: None,
            });
            app.update();
            app.world()
                .resource::<IntentErrors>()
                .get(Team::Human)
                .first()
                .cloned()
                .unwrap_or_default()
        };

        // 1. A dead or never-existent id — the round-10 case. The old string
        //    sent the reader to the catalog; this one names the real suspect.
        app.world_mut().entity_mut(expansion).despawn();
        let msg = cast_error(&mut app, expansion.to_bits());
        assert!(msg.contains("not found"), "stale id: {msg}");
        assert!(msg.contains("may have died"), "stale id gives no cause: {msg}");
        assert!(
            !msg.contains("ability building"),
            "a dead hall must not be reported as a tech-tree problem: {msg}"
        );

        // 2. Someone else's hall. Ownership, not tech.
        let theirs = hall(&mut app, BuildingKind::TownHall, Vec3::new(70.0, 0.0, 70.0));
        app.world_mut().entity_mut(theirs).insert(Team::Claude);
        assert_eq!(
            cast_error(&mut app, theirs.to_bits()),
            format!("cmd 0: caster {} is not yours", theirs.to_bits())
        );

        // 3. Our own building, standing, that genuinely has no ability. This
        //    one was always right and stays untouched.
        let farm = hall(&mut app, BuildingKind::Farm, Vec3::new(-60.0, 0.0, -60.0));
        assert_eq!(cast_error(&mut app, farm.to_bits()), "cmd 0: Farm has no ability");

        // 4. Our own UNIT with no abilities. This used to fall through to the
        //    building lookup and come back as "not a hero or an own ability
        //    building" — technically true of a Footman, and useless.
        let footman = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();
        assert_eq!(
            cast_error(&mut app, footman.to_bits()),
            "cmd 0: Footman has no ability"
        );

        // 5. And an own hall still going up is refused for the reason it is
        //    actually refused for, in the same words every other verb uses.
        let raising = hall(&mut app, BuildingKind::TownHall, Vec3::new(-20.0, 0.0, -20.0));
        app.world_mut()
            .entity_mut(raising)
            .insert(UnderConstruction { remaining: 12.0 });
        let msg = cast_error(&mut app, raising.to_bits());
        assert!(msg.contains("is under construction"), "{msg}");
    }

    /// **A blocked site names one that works, and the name is good** —
    /// `wc3clone-vjy`, round-9 AAR.
    ///
    /// `site (56.0, -56.0) is blocked for TownHall` named no rule and no
    /// alternative, and both commanders spent 20s+ guessing at 2-unit
    /// increments. The obvious failure mode of a fix is a hint that is itself
    /// illegal, so this does not eyeball the string: it takes the coordinates
    /// out of the rejection and feeds them back through the identical
    /// compiler, and demands the second order be accepted.
    #[test]
    fn a_blocked_placement_hint_is_itself_legal() {
        let mut app = compiler_app();
        // A gold mine's footprint, exactly as terrain.rs lays it down (6x6),
        // sitting where the commander wants their expansion hall. A TownHall
        // is 8x8, so its centre has to clear the mine's by 7 on an axis —
        // there is no separate "keep away from mines" rule, just two
        // footprints that cannot overlap, and that is the whole reason the
        // site the eye picks is the site that fails.
        let mine = Vec3::new(56.0, 0.0, -56.0);
        app.world_mut()
            .resource_mut::<NavGrid>()
            .set_blocked_rect(mine, 6.0, true);
        let eco = app.world_mut().resource_mut::<Economies>().get_mut(Team::Human).gold;
        assert!(eco > 0, "the default economy is what pays for the retry");
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let e = economies.get_mut(Team::Human);
            e.gold = 2000;
            e.lumber = 2000;
        }
        let worker = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Worker },
                Team::Human,
                Transform::from_translation(Vec3::new(50.0, 0.0, -50.0)),
            ))
            .id();

        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: Intent::Build {
                worker: Some(worker.to_bits()),
                kind: "TownHall".to_string(),
                x: Some(mine.x),
                z: Some(mine.z),
                region: None,
                select: None,
                site: None,
            },
            trigger: None,
            plan: None,
        });
        app.update();

        let errors = app.world().resource::<IntentErrors>().get(Team::Human).to_vec();
        assert_eq!(errors.len(), 1, "expected exactly one rejection: {errors:?}");
        let msg = &errors[0];
        // The rule, so a commander can predict the next one instead of probing.
        assert!(msg.contains("8x8 clear"), "no clearance rule in '{msg}'");
        assert!(msg.contains("mines block 6x6"), "no mine rule in '{msg}'");

        // The alternative, parsed the way a commander would read it.
        let hint = msg
            .split("nearest legal: (")
            .nth(1)
            .unwrap_or_else(|| panic!("no hint in '{msg}'"));
        let hint = hint.split(')').next().unwrap();
        let (hx, hz) = hint.split_once(", ").expect("hint is an x, z pair");
        let (hx, hz) = (hx.parse::<f32>().unwrap(), hz.parse::<f32>().unwrap());

        // Within the promised radius of the site actually asked for — which is
        // the SNAPPED site the error printed, not the raw request.
        let asked = snap_footprint(clamp_to_map(mine), building_stats(BuildingKind::TownHall).size);
        let d = (hx - asked.x).hypot(hz - asked.z);
        assert!(d <= PLACEMENT_HINT_RADIUS, "hint is {d} away, past the promise");

        // ...and legal, asserted by the validator rather than by inspection.
        app.world_mut()
            .resource_mut::<IntentErrors>()
            .get_mut(Team::Human)
            .clear();
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: Intent::Build {
                worker: Some(worker.to_bits()),
                kind: "TownHall".to_string(),
                x: Some(hx),
                z: Some(hz),
                region: None,
                select: None,
                site: None,
            },
            trigger: None,
            plan: None,
        });
        app.update();
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "the hint was refused: {:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );

        // The other half of the promise: when there really is nowhere, say so
        // rather than pointing at something far away.
        {
            let mut nav = app.world_mut().resource_mut::<NavGrid>();
            nav.set_blocked_rect(mine, PLACEMENT_HINT_RADIUS * 2.0 + 20.0, true);
        }
        let nav = app.world().resource::<NavGrid>();
        assert!(nearest_free_site(nav, mine, 8.0, PLACEMENT_HINT_RADIUS).is_none());
        assert!(blocked_site_error(nav, mine, BuildingKind::TownHall)
            .contains("no legal site within 15"));
    }

    /// The ability selector is untagged on the wire: a bare number is a slot
    /// index, a bare string is an ability id. Both must survive a round trip,
    /// because the log is what a replay reads back.
    #[test]
    fn ability_selectors_round_trip_untagged() {
        let by_index: Intent =
            serde_json::from_str(r#"{"type":"cast","hero":5,"ability":2}"#).unwrap();
        let by_id: Intent =
            serde_json::from_str(r#"{"type":"cast","hero":5,"ability":"Slam"}"#).unwrap();
        let bare: Intent = serde_json::from_str(r#"{"type":"cast","hero":5}"#).unwrap();
        assert_eq!(by_index.sentence(), "5 casts ability slot 2");
        assert_eq!(by_id.sentence(), "5 casts Slam");
        assert_eq!(bare.sentence(), "5 casts its ability");

        // Untagged means the JSON carries the bare value, not a wrapper.
        let v = serde_json::to_value(&by_id).unwrap();
        assert_eq!(v.get("ability").and_then(|a| a.as_str()), Some("Slam"));
        let v = serde_json::to_value(&by_index).unwrap();
        assert_eq!(v.get("ability").and_then(|a| a.as_u64()), Some(2));
        // Omitted stays omitted, so a v1 log line is still a v1 log line.
        let v = serde_json::to_value(&bare).unwrap();
        assert!(v.get("ability").is_none());
        // ...and geometry it never carried is absent too, so an old log line
        // round-trips byte for byte.
        assert!(v.get("x").is_none() && v.get("z").is_none() && v.get("target").is_none());
    }

    // -----------------------------------------------------------------------
    // v3: targeted-cast geometry on the wire
    // -----------------------------------------------------------------------

    /// A Sorcerer of the seat's own team, standing where you put it.
    fn spawn_sorcerer(app: &mut App, team: Team, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Sorcerer },
                team,
                Transform::from_translation(at),
                Health::new(unit_stats(UnitKind::Sorcerer).hp),
                Order::Idle,
            ))
            .id()
    }

    /// A command off the wire, tagged the way a bridge batch tags one — so a
    /// refusal below lands in the same `errors` array a commander reads.
    fn from_the_wire(team: Team, json: &str) -> SubmitIntent {
        SubmitIntent {
            team,
            source: IntentSource::Bridge,
            tag: "cmd 1".to_string(),
            intent: serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}")),
            trigger: None,
            plan: None,
        }
    }

    fn casts_of(app: &mut App) -> Vec<CastAbility> {
        app.world_mut()
            .resource_mut::<Events<CastAbility>>()
            .drain()
            .collect()
    }

    /// **`ready` travels from the wire to the gate** — `wc3clone-t0d`. The one
    /// verb that is legal before the match exists still goes the ordinary way:
    /// JSON, `Intent`, compiler, event. Nothing about the handshake is a side
    /// channel, which is why it gets a sentence and a journal line like every
    /// other statement a seat makes.
    #[test]
    fn the_ready_verb_travels_from_the_wire_to_the_gate() {
        let mut app = compiler_app();

        app.world_mut()
            .send_event(from_the_wire(Team::Human, r#"{"type":"ready"}"#));
        app.update();

        let readies: Vec<MatchReady> = app
            .world_mut()
            .resource_mut::<Events<MatchReady>>()
            .drain()
            .collect();
        assert_eq!(readies.len(), 1, "one statement, one event");
        assert_eq!(readies[0].team, Team::Human);
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "`ready` is never refused"
        );

        // Idempotent on the wire: saying it twice is two events, and the gate
        // (shared.rs `ready_gate`) is what makes the second one a no-op. The
        // compiler deliberately does not know whether the match has started —
        // that is exactly the knowledge that would make this arm need a
        // resource it has no business holding.
        app.world_mut()
            .send_event(from_the_wire(Team::Human, r#"{"type":"ready"}"#));
        app.world_mut()
            .send_event(from_the_wire(Team::Claude, r#"{"type":"ready"}"#));
        app.update();
        let readies: Vec<MatchReady> = app
            .world_mut()
            .resource_mut::<Events<MatchReady>>()
            .drain()
            .collect();
        assert_eq!(readies.len(), 2);
        // A seat only ever speaks for itself — the team on the event is the
        // team on the submission, never the one it names.
        assert_eq!(readies[1].team, Team::Claude);

        // The half a person reads.
        assert_eq!(Intent::Ready.verb(), "ready");
        assert_eq!(Intent::Ready.sentence(), "declare ready to begin");
    }

    /// **The wire carries the aim.** A commander's coordinates survive the
    /// compiler and arrive on the event combat.rs reads — the point of the
    /// whole verb.
    #[test]
    fn a_point_cast_carries_its_coordinates_to_the_executor() {
        let mut app = compiler_app();
        let sorcerer = spawn_sorcerer(&mut app, Team::Human, Vec3::ZERO);

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            &format!(
                r#"{{"type":"cast","caster":{},"ability":"Slow","x":6.0,"z":0.0}}"#,
                sorcerer.to_bits()
            ),
        ));
        app.update();

        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "{:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );
        let fired = casts_of(&mut app);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].caster, sorcerer);
        match fired[0].target {
            Some(CastTarget::Point(p)) => {
                assert_eq!((p.x, p.z), (6.0, 0.0));
            }
            other => panic!("expected the commander's point, got {other:?}"),
        }
    }

    /// **Out of range is refused, with both numbers.** The decision (see
    /// `AbilityTarget`) is instant rejection rather than walking the caster
    /// into range: a Sorcerer that closes the distance by itself is a Sorcerer
    /// back in the front rank, which is the failure targeted casting exists to
    /// end. So the compiler has to *teach* instead — and the same string
    /// reaches the bridge's `errors` array and the human's alert stack,
    /// because both seats read one channel.
    #[test]
    fn a_cast_beyond_its_range_is_refused_with_both_numbers() {
        let mut app = compiler_app();
        let sorcerer = spawn_sorcerer(&mut app, Team::Human, Vec3::ZERO);

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            &format!(
                r#"{{"type":"cast","caster":{},"ability":"Slow","x":40.0,"z":0.0}}"#,
                sorcerer.to_bits()
            ),
        ));
        app.update();

        let errors = app.world().resource::<IntentErrors>().get(Team::Human).to_vec();
        assert_eq!(errors.len(), 1, "{errors:?}");
        let msg = &errors[0];
        assert!(msg.contains("Slow"), "names the ability: {msg}");
        assert!(msg.contains('9'), "names the ability's reach: {msg}");
        assert!(msg.contains("40"), "names the distance asked for: {msg}");
        // And nothing was cast: a refusal is a refusal, not a partial one.
        assert!(casts_of(&mut app).is_empty());
        // The caster was not re-tasked towards the point either.
        assert!(matches!(
            app.world().entity(sorcerer).get::<Order>(),
            Some(Order::Idle)
        ));
    }

    /// A point handed to an ability that has nowhere to put it is a mistake
    /// worth naming, not a payload to silently drop.
    #[test]
    fn a_caster_centred_ability_refuses_a_target_it_cannot_use() {
        let mut app = compiler_app();
        let tf = Transform::from_translation(Vec3::ZERO);
        let champion = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                Team::Human,
                Hero::from_record(None),
                Health::new(unit_stats(UnitKind::Hero).hp),
                Order::Idle,
                tf,
            ))
            .id();

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            &format!(
                r#"{{"type":"cast","caster":{},"ability":"Slam","x":3.0,"z":0.0}}"#,
                champion.to_bits()
            ),
        ));
        app.update();

        let errors = app.world().resource::<IntentErrors>().get(Team::Human).to_vec();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("takes no target"), "{}", errors[0]);
    }

    /// **Back-compat at the compiler.** The four v2 spellings still compile to
    /// a cast with no aim, which is exactly what they used to compile to — and
    /// what the engine now reads as "aim it for me".
    #[test]
    fn the_old_cast_forms_still_compile_to_an_unaimed_cast() {
        for form in [
            r#"{"type":"cast","hero":ID}"#,
            r#"{"type":"cast","caster":ID}"#,
            r#"{"type":"cast","hero":ID,"ability":0}"#,
            r#"{"type":"cast","hero":ID,"ability":"Slow"}"#,
        ] {
            let mut app = compiler_app();
            let sorcerer = spawn_sorcerer(&mut app, Team::Human, Vec3::ZERO);
            let json = form.replace("ID", &sorcerer.to_bits().to_string());
            app.world_mut().send_event(from_the_wire(Team::Human, &json));
            app.update();

            assert!(
                app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
                "{json} was refused: {:?}",
                app.world().resource::<IntentErrors>().get(Team::Human)
            );
            let fired = casts_of(&mut app);
            assert_eq!(fired.len(), 1, "{json} produced no cast");
            assert!(
                fired[0].target.is_none(),
                "{json} must carry no aim — the engine picks"
            );
        }
    }

    /// Half a point is not a point, and a point plus a unit is two aims. Both
    /// are named rather than half-obeyed.
    #[test]
    fn a_malformed_aim_is_refused_rather_than_guessed() {
        for bad in [
            r#"{"type":"cast","caster":ID,"ability":"Slow","x":3.0}"#,
            r#"{"type":"cast","caster":ID,"ability":"Slow","z":3.0}"#,
            r#"{"type":"cast","caster":ID,"ability":"Slow","x":1.0,"z":2.0,"target":7}"#,
        ] {
            let mut app = compiler_app();
            let sorcerer = spawn_sorcerer(&mut app, Team::Human, Vec3::ZERO);
            let json = bad.replace("ID", &sorcerer.to_bits().to_string());
            app.world_mut().send_event(from_the_wire(Team::Human, &json));
            app.update();

            let errors = app.world().resource::<IntentErrors>().get(Team::Human).to_vec();
            assert_eq!(errors.len(), 1, "{json} should be refused, got {errors:?}");
            assert!(casts_of(&mut app).is_empty());
        }
    }

    #[test]
    fn upgrade_is_one_verb_for_both_seats() {
        let typed: Intent = serde_json::from_str(r#"{"type":"upgrade","building":77}"#).unwrap();
        let gesture = Intent::Upgrade { building: 77 };
        assert_eq!(
            serde_json::to_value(&gesture).unwrap(),
            serde_json::to_value(&typed).unwrap()
        );
        assert_eq!(
            gesture.sentence(),
            "building 77 upgrades to its next tier"
        );
    }

    /// A round trip through JSON must not change what an intent means — this
    /// is what makes the log a replay spine rather than a diary.
    #[test]
    fn intents_round_trip() {
        let original = Intent::AttackMove {
            units: vec![7, 8, 9],
            x: Some(12.5),
            z: Some(-30.5),
            region: None,
            select: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(original.sentence(), back.sentence());
        assert_eq!(
            back.sentence(),
            "attack-move 3 units to (12.5, -30.5)".to_string()
        );
    }

    /// **Two heroes, one potion.** The whole reason `buy`/`use_item` grew a
    /// `hero` field: hero slots scale with the hall ladder now, so a Keep team
    /// fields a Champion AND a Priestess and "the team's hero" stopped being a
    /// well-defined phrase. A named hero must win, and a wrong name must be
    /// refused rather than quietly redirected — silently selling to the other
    /// hero is precisely the bug this parameter exists to prevent.
    #[test]
    fn buy_targets_the_named_hero_when_a_team_fields_two() {
        let champion = Entity::from_raw(11);
        let priestess = Entity::from_raw(42);
        let heroes = [champion, priestess];

        // Named: the item goes where it was addressed, in either direction.
        assert_eq!(pick_item_hero(&heroes, Some(priestess)), Some(priestess));
        assert_eq!(pick_item_hero(&heroes, Some(champion)), Some(champion));

        // Unnamed: the documented, stable tie-break — lowest entity id. It is
        // deliberately not query order, so the two seats and successive frames
        // all resolve the same hero.
        assert_eq!(pick_item_hero(&heroes, None), Some(champion));
        let reversed = [priestess, champion];
        assert_eq!(
            pick_item_hero(&reversed, None),
            Some(champion),
            "the default may not depend on iteration order",
        );

        // Back-compatible: with one hero, omitting the field picks that hero,
        // which is exactly what every pre-slots call site already got.
        assert_eq!(pick_item_hero(&[priestess], None), Some(priestess));

        // A name that is not one of this team's living heroes is refused. The
        // caller turns this `None` into an error string; what matters here is
        // that it never falls through to somebody else's inventory.
        let stranger = Entity::from_raw(99);
        assert_eq!(pick_item_hero(&heroes, Some(stranger)), None);
        assert_eq!(pick_item_hero(&[], Some(champion)), None);
        assert_eq!(pick_item_hero(&[], None), None);

        // The regression a live bridge run caught: "named a hero" and "named
        // nothing" must never collapse into each other. An id that resolves to
        // no entity at all is a NAMED request that failed — refusing it is the
        // whole point — whereas `None` means "you pick". Written as the two
        // distinct calls the caller makes, so the day someone flattens the
        // unresolvable case back into `None` this fails instead of silently
        // posting the potion to the wrong hero.
        assert_eq!(
            pick_item_hero(&heroes, Some(stranger)),
            None,
            "an id that is not one of my heroes must not resolve to my default",
        );
        assert_ne!(
            pick_item_hero(&heroes, Some(stranger)),
            pick_item_hero(&heroes, None),
            "a failed name and an omitted name must not agree",
        );
    }

    /// The field is optional on the wire and names the hero in the log, so an
    /// old one-hero command still parses and a new one reads back as English.
    #[test]
    fn the_item_verbs_carry_an_optional_hero_on_the_wire() {
        // Historical form: no `hero` key at all.
        let legacy: Intent =
            serde_json::from_str(r#"{"type":"buy","shop":1,"item":"HealingPotion"}"#).unwrap();
        assert_eq!(legacy.sentence(), "buy HealingPotion at shop 1");
        let legacy_use: Intent =
            serde_json::from_str(r#"{"type":"use_item","slot":1}"#).unwrap();
        assert_eq!(legacy_use.sentence(), "hero uses item in slot 1");

        // Addressed form: round-trips, and the sentence says who.
        let addressed = Intent::Buy {
            shop: 1,
            item: "HealingPotion".to_string(),
            hero: Some(42),
        };
        let json = serde_json::to_string(&addressed).unwrap();
        assert!(json.contains("\"hero\":42"), "the field must survive: {json}");
        let back: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sentence(), "hero 42 buys HealingPotion at shop 1");

        let use_addressed = Intent::UseItem {
            slot: 0,
            hero: Some(42),
            destination: None,
        };
        let back: Intent =
            serde_json::from_str(&serde_json::to_string(&use_addressed).unwrap()).unwrap();
        assert_eq!(back.sentence(), "hero 42 uses item in slot 0");

        // Omitting it must not serialize a null — the wire shape is unchanged
        // for every command that does not care.
        let plain = Intent::UseItem {
            slot: 0,
            hero: None,
            destination: None,
        };
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"type":"use_item","slot":0}"#,
        );
    }

    /// The research verb, from both ends. The Blacksmith card's [Q] and a
    /// commander's JSON are the same intent and the same log sentence — the
    /// claim docs/INTENT.md exists to keep checkable, applied to the newest
    /// verb rather than only the old ones.
    #[test]
    fn a_research_gesture_and_a_research_command_are_the_same_intent() {
        // [Q] on a selected Blacksmith. ui.rs spells the ladder with the
        // catalog id, which is exactly what a commander types.
        let gesture = Intent::Research {
            building: 77,
            upgrade: ResearchKind::Attack.id().to_string(),
        };
        let typed: Intent =
            serde_json::from_str(r#"{"type":"research","building":77,"upgrade":"attack"}"#)
                .unwrap();
        assert_eq!(
            serde_json::to_value(&gesture).unwrap(),
            serde_json::to_value(&typed).unwrap()
        );
        assert_eq!(gesture.sentence(), typed.sentence());
        assert_eq!(gesture.sentence(), "building 77 researches attack");
        assert_eq!(gesture.verb(), "research");
    }

    /// Ladder names parse the same loose way every other name on the wire does,
    /// by id or by display name, and nothing else gets through.
    #[test]
    fn research_names_parse_by_id_or_label() {
        assert_eq!(parse_research_kind("attack"), Some(ResearchKind::Attack));
        assert_eq!(parse_research_kind("Attack"), Some(ResearchKind::Attack));
        assert_eq!(parse_research_kind("armor"), Some(ResearchKind::Armor));
        assert_eq!(parse_research_kind("ARMOR"), Some(ResearchKind::Armor));
        // The catalog's display name, in every spelling `normalize_name` folds.
        assert_eq!(
            parse_research_kind("Weapon Smithing"),
            Some(ResearchKind::Attack)
        );
        assert_eq!(
            parse_research_kind("weapon_smithing"),
            Some(ResearchKind::Attack)
        );
        assert_eq!(
            parse_research_kind("armor-plating"),
            Some(ResearchKind::Armor)
        );
        // ...and nothing else. A typo is a rejected command with a sentence in
        // the log, not a silently mis-bought upgrade.
        assert_eq!(parse_research_kind("armour"), None);
        assert_eq!(parse_research_kind("damage"), None);
        assert_eq!(parse_research_kind(""), None);
    }

    /// Ability ids parse the same loose way every other name on the wire does.
    ///
    /// This was the last name in the language matched by
    /// `eq_ignore_ascii_case` rather than `normalize_name`, which meant
    /// `"CallToArms"` and `"calltoarms"` worked while `"Call to Arms"` — the
    /// spelling a person types, and the one a reader mentally inserts a space
    /// into when reading `catalog.abilities` — was an unknown ability. A
    /// vocabulary with two matching rules is two vocabularies.
    #[test]
    fn ability_ids_parse_like_every_other_name() {
        let hall = abilities_of_building(BuildingKind::TownHall);
        // Every old form still resolves: normalising is strictly looser than
        // case folding, so nothing that parsed before stopped parsing.
        assert_eq!(ability_index_by_id(hall, "CallToArms"), Some(0));
        assert_eq!(ability_index_by_id(hall, "calltoarms"), Some(0));
        assert_eq!(ability_index_by_id(hall, "CALLTOARMS"), Some(0));
        // ...and the spellings that used to be rejected now land.
        assert_eq!(ability_index_by_id(hall, "Call to Arms"), Some(0));
        assert_eq!(ability_index_by_id(hall, "call_to_arms"), Some(0));
        assert_eq!(ability_index_by_id(hall, "call-to-arms"), Some(0));

        // The hero kit, including a second slot, so this is not a one-row
        // coincidence.
        let champion = abilities_of_unit(UnitKind::Hero);
        assert_eq!(ability_index_by_id(champion, "Slam"), Some(0));
        assert_eq!(ability_index_by_id(champion, " war cry "), Some(1));

        // A wrong name is still a rejected cast, not a silent slot 0.
        assert_eq!(ability_index_by_id(hall, "CallToArm"), None);
        assert_eq!(ability_index_by_id(hall, ""), None);

        // The selector reached through the wire form agrees, because it is the
        // same function underneath — this is the path a `cast` command takes.
        let selector: AbilitySelector = serde_json::from_str(r#""Call to Arms""#).unwrap();
        assert!(matches!(selector, AbilitySelector::Id(_)));

        // Target classes are the other holdout the unification swept up.
        assert_eq!(parse_target_class("hero"), Some(TargetClass::Hero));
        assert_eq!(parse_target_class("Hero"), Some(TargetClass::Hero));
        assert_eq!(parse_target_class("wizard"), None);
    }

    /// The claim the whole module exists to make: a human gesture and a bridge
    /// command are not *translated* into each other, they are the same value.
    ///
    /// The left-hand side of each pair is built the way ui.rs builds it from a
    /// gesture; the right-hand side is the JSON a commander writes by hand.
    /// They must be indistinguishable — same serialized form, same sentence —
    /// because a replay must not be able to tell who was playing.
    #[test]
    fn a_gesture_and_a_command_are_the_same_intent() {
        // [G] Guard on two selected units standing around (12.0, -8.0), which
        // ui.rs compiles to an anchor + the card's fixed 18-unit radius.
        let gesture = Intent::Leash {
            units: vec![41, 42],
            x: Some(12.0),
            z: Some(-8.0),
            region: None,
            radius: Some(18.0),
            select: None,
        };
        let typed: Intent = serde_json::from_str(
            r#"{"type":"leash","units":[41,42],"x":12.0,"z":-8.0,"radius":18.0}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&gesture).unwrap(),
            serde_json::to_value(&typed).unwrap()
        );
        assert_eq!(gesture.sentence(), typed.sentence());
        assert_eq!(gesture.sentence(), "2 units hold within 18 of (12.0, -8.0)");

        // [Y] — the hero's second ability. The UI is index-native because a
        // hotkey IS a slot; a commander may say the same thing by name. Both
        // spellings are the same verb on the same caster.
        let gesture = Intent::Cast {
            x: None,
            z: None,
            target: None,
            hero: Some(5),
            ability: Some(AbilitySelector::Index(1)),
            select: None,
        };
        let typed: Intent =
            serde_json::from_str(r#"{"type":"cast","hero":5,"ability":1}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&gesture).unwrap(),
            serde_json::to_value(&typed).unwrap()
        );
        assert_eq!(gesture.sentence(), typed.sentence());

        // Right-click on an enemy with three units selected.
        let gesture = Intent::Attack {
            units: vec![1, 2, 3],
            target: 77,
            select: None,
        };
        let typed: Intent =
            serde_json::from_str(r#"{"type":"attack","units":[1,2,3],"target":77}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&gesture).unwrap(),
            serde_json::to_value(&typed).unwrap()
        );
        assert_eq!(gesture.sentence(), typed.sentence());
    }

    /// The player-facing surface is a projection of the catalog, not a list
    /// maintained alongside it: a kind added to `ALL_UNIT_KINDS` and
    /// `trainable()` becomes orderable with no change here at all. This is the
    /// property that lets both kinds of player — the one reading a command
    /// card and the one reading `state.json` — discover new content the same
    /// way, so it is worth a test rather than a comment. (Moved here from
    /// bridge.rs with the parser: it is the vocabulary's property now, not one
    /// interface's.)
    #[test]
    fn every_unit_kind_is_orderable_by_name() {
        for kind in ALL_UNIT_KINDS {
            assert_eq!(
                parse_unit_kind(kind_name(kind)),
                Some(kind),
                "{} is in the catalog but not orderable",
                kind_name(kind)
            );
        }
        // Players type what they like; names are normalized, not matched raw.
        assert_eq!(parse_unit_kind("spearman"), Some(UnitKind::Spearman));
        assert_eq!(parse_unit_kind("Spear Man"), Some(UnitKind::Spearman));
        assert_eq!(parse_unit_kind("pikeman"), None);
    }

    #[test]
    fn names_parse_loosely() {
        assert_eq!(
            parse_building_kind("town_hall"),
            Some(BuildingKind::TownHall)
        );
        assert_eq!(
            parse_building_kind("Town Hall"),
            Some(BuildingKind::TownHall)
        );
        assert_eq!(parse_unit_kind("footman"), Some(UnitKind::Footman));
        assert_eq!(parse_item("town portal"), Some(ItemId::TownPortal));
        assert_eq!(parse_target_class("siege"), Some(TargetClass::Siege));
        assert!(parse_building_kind("nonsense").is_none());
    }

    // -----------------------------------------------------------------------
    // use_item: WHICH hall
    // -----------------------------------------------------------------------

    fn item_uses_of(app: &mut App) -> Vec<UseItem> {
        app.world_mut()
            .resource_mut::<Events<UseItem>>()
            .drain()
            .collect()
    }

    fn spawn_hero(app: &mut App, team: Team, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Hero },
                team,
                Transform::from_translation(at),
                Order::Idle,
                Health::new(600.0),
                Inventory([Some(ItemId::ScrollOfMassTeleport), None]),
            ))
            .id()
    }

    /// A hall, finished unless `under` says otherwise.
    fn spawn_hall_at(app: &mut App, kind: BuildingKind, team: Team, at: Vec3, under: bool) -> Entity {
        let mut e = app.world_mut().spawn((
            Building { kind },
            team,
            Transform::from_translation(at),
            Health::new(building_stats(kind).hp),
        ));
        if under {
            e.insert(UnderConstruction { remaining: 5.0 });
        }
        e.id()
    }

    /// **A refusal has to name the rule that refused.** The compiler's only
    /// job on price is the message (economy.rs is what actually charges), and
    /// the message a commander gets for their second hero has to distinguish
    /// three states that all read "cannot afford Priestess" otherwise: the
    /// waiver already spent, a class being bought back, and an ordinary unit
    /// that costs what the catalog says it costs.
    ///
    /// The specific way this goes wrong without the suffix is documented in
    /// the arena ledger: a commander reads "your first hero is free", queues
    /// one, comes back for the second, is told it cannot afford a thing the
    /// brief called free, and concludes the game is broken rather than that
    /// the rule has a boundary.
    #[test]
    fn a_hero_refusal_names_which_price_rule_charged_it() {
        let mut app = compiler_app();
        let hall = spawn_hall_at(
            &mut app,
            BuildingKind::Keep,
            Team::Human,
            Vec3::new(60.0, 0.0, 60.0),
            false,
        );
        app.world_mut()
            .entity_mut(hall)
            .insert(TrainingQueue::default());
        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(Team::Human, TechTier::T2);
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let e = economies.get_mut(Team::Human);
            e.gold = 0;
            e.lumber = 0;
            e.supply_cap = 100;
        }
        let train = |app: &mut App, kind: UnitKind| {
            app.world_mut().send_event(SubmitIntent::ui(
                Team::Human,
                Intent::Train {
                    building: Some(intent_id(hall)),
                    unit: kind_name(kind).to_string(),
                    select: None,
                },
            ));
            app.update();
            drain_errors(app, Team::Human)
        };

        // Broke, and the first hero goes through anyway: free is free.
        assert!(
            train(&mut app, UnitKind::Hero).is_empty(),
            "a team with no hero and no gold can still field its first"
        );

        // ...and the second, with the Champion now in flight, is refused with
        // the rule spelled out.
        let errs = train(&mut app, UnitKind::Priestess);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].contains("cannot afford Priestess (400g 100l)")
                && errs[0].contains("only your FIRST hero is free"),
            "the refusal must name the boundary it hit: {errs:?}"
        );

        // A class with a record is refused in the other vocabulary — the money
        // is the same, the reason a commander must act on is not.
        app.world_mut().resource_mut::<HeroRecords>().set(
            Team::Human,
            HeroRecord { level: 3, xp: 0.0, kind: UnitKind::Priestess },
        );
        let errs = train(&mut app, UnitKind::Priestess);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].contains("reviving a class you have lost"),
            "a revival is not a first hero and must not be described as one: {errs:?}"
        );

        // Nothing changes for anything that is not a hero: the bare shape that
        // plan.rs's `blocked:` status and every arena AAR already parse.
        let errs = train(&mut app, UnitKind::Worker);
        assert_eq!(errs.len(), 1, "{errs:?}");
        let worker = unit_stats(UnitKind::Worker);
        assert!(
            errs[0].ends_with(&format!(
                "cannot afford Worker ({}g {}l)",
                worker.cost_gold, worker.cost_lumber
            )),
            "the plain shape every plan-blocked reader already parses: {errs:?}"
        );
    }

    /// **The happy path, and the reason the field exists.** A named hall that
    /// IS one of your standing halls survives the compiler and arrives on the
    /// event combat.rs reads — with the entity, not a coordinate, so the
    /// executor re-checks it against a world that may have moved on.
    #[test]
    fn a_named_hall_reaches_the_executor() {
        let mut app = compiler_app();
        let hero = spawn_hero(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        spawn_hall_at(&mut app, BuildingKind::TownHall, Team::Human, Vec3::new(60.0, 0.0, 60.0), false);
        let far = spawn_hall_at(&mut app, BuildingKind::Keep, Team::Human, Vec3::new(-70.0, 0.0, -70.0), false);

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::UseItem {
                slot: 0,
                hero: Some(hero.to_bits()),
                destination: Some(far.to_bits()),
            },
        ));
        app.update();

        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "naming your own standing hall is legal: {:?}",
            app.world().resource::<IntentErrors>().get(Team::Human)
        );
        let uses = item_uses_of(&mut app);
        assert_eq!(uses.len(), 1, "one use_item event: {uses:?}");
        assert_eq!(uses[0].destination, Some(far), "the chosen hall, not the near one");
    }

    /// **Omitted is the old behaviour, spelled as `None`.** The compiler does
    /// not substitute a hall of its own choosing — it passes the absence
    /// through, and combat.rs's nearest-hall rule (which predates this field)
    /// is what answers. Back-compatible by construction, not by coincidence.
    #[test]
    fn an_omitted_destination_stays_omitted_all_the_way_to_the_executor() {
        let mut app = compiler_app();
        let hero = spawn_hero(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        spawn_hall_at(&mut app, BuildingKind::TownHall, Team::Human, Vec3::new(60.0, 0.0, 60.0), false);
        spawn_hall_at(&mut app, BuildingKind::Keep, Team::Human, Vec3::new(-70.0, 0.0, -70.0), false);

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::UseItem { slot: 0, hero: None, destination: None },
        ));
        app.update();

        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "the historic call shape is still legal"
        );
        let uses = item_uses_of(&mut app);
        assert_eq!(uses.len(), 1, "one use_item event: {uses:?}");
        assert_eq!(uses[0].hero, hero, "and it still finds the team's only hero");
        assert_eq!(uses[0].destination, None, "no hall was chosen, so none is invented");
    }

    /// **Every way of getting it wrong, and the one sentence that teaches all
    /// four.** A destination that is not your own standing hall is REFUSED,
    /// never quietly downgraded to "nearest" — a scroll that silently went
    /// somewhere else is exactly the failure the field exists to prevent, and
    /// a fall-back would reintroduce it while looking like it worked.
    ///
    /// One message for all four because "your standing hall" already names
    /// every condition, and a finer-grained answer ("that is the enemy's
    /// Keep") would hand a seat a building id it could not otherwise have.
    #[test]
    fn a_destination_that_is_not_your_standing_hall_is_refused() {
        let mine = Vec3::new(60.0, 0.0, 60.0);
        let cases: Vec<(&str, Box<dyn Fn(&mut App) -> u64>)> = vec![
            (
                "an enemy hall",
                Box::new(|app: &mut App| {
                    spawn_hall_at(app, BuildingKind::TownHall, Team::Claude, Vec3::new(-70.0, 0.0, -70.0), false)
                        .to_bits()
                }),
            ),
            (
                "a hall still going up",
                Box::new(|app: &mut App| {
                    spawn_hall_at(app, BuildingKind::TownHall, Team::Human, Vec3::new(-70.0, 0.0, -70.0), true)
                        .to_bits()
                }),
            ),
            (
                "a building of ours that is not a hall",
                Box::new(|app: &mut App| {
                    spawn_hall_at(app, BuildingKind::Barracks, Team::Human, Vec3::new(-70.0, 0.0, -70.0), false)
                        .to_bits()
                }),
            ),
            // A bare number nobody ever minted.
            ("an id that names nothing", Box::new(|_: &mut App| 987_654_321_u64)),
        ];

        for (what, make) in cases {
            let mut app = compiler_app();
            let hero = spawn_hero(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
            spawn_hall_at(&mut app, BuildingKind::TownHall, Team::Human, mine, false);
            let bad = make(&mut app);

            // Sent as a commander would send it, so the assertion below is on
            // the exact string that rides back in the snapshot's `errors`.
            app.world_mut().send_event(SubmitIntent {
                team: Team::Human,
                source: IntentSource::Bridge,
                tag: "cmd 0".to_string(),
                trigger: None,
                plan: None,
                intent: Intent::UseItem {
                    slot: 0,
                    hero: Some(hero.to_bits()),
                    destination: Some(bad),
                },
            });
            app.update();

            let errors = app.world().resource::<IntentErrors>().get(Team::Human).to_vec();
            assert_eq!(errors.len(), 1, "{what}: expected one rejection, got {errors:?}");
            assert_eq!(
                errors[0],
                format!("cmd 0: destination {bad} is not your standing hall"),
                "{what}: the message has to teach the rule"
            );
            assert!(
                item_uses_of(&mut app).is_empty(),
                "{what}: a refused destination must not fire the item at all — \
                 falling back to the nearest hall IS the bug"
            );
        }
    }

    /// The sentence carries the choice. Two `use_item` commands that differ
    /// only in where the army lands must not read identically in the log a
    /// person reviews the match from.
    #[test]
    fn the_sentence_names_the_hall_that_was_chosen() {
        let aimed = Intent::UseItem { slot: 0, hero: Some(7), destination: Some(34) };
        assert_eq!(aimed.sentence(), "hero 7 uses item in slot 0, bound for hall 34");
        let plain = Intent::UseItem { slot: 0, hero: Some(7), destination: None };
        assert_eq!(plain.sentence(), "hero 7 uses item in slot 0");
        assert_ne!(aimed.sentence(), plain.sentence(), "the choice is visible in the log");
    }

    /// The wire, both directions. The new field round-trips under its own
    /// name, and omitting it serializes to the byte-identical historic shape —
    /// so a commander written before this bead keeps working unchanged.
    #[test]
    fn the_destination_rides_the_wire_without_disturbing_the_old_shape() {
        let aimed: Intent =
            serde_json::from_str(r#"{"type":"use_item","slot":1,"hero":7,"destination":34}"#).unwrap();
        assert!(
            matches!(aimed, Intent::UseItem { slot: 1, hero: Some(7), destination: Some(34) }),
            "the field parses under its own name"
        );
        assert_eq!(
            serde_json::to_string(&aimed).unwrap(),
            r#"{"type":"use_item","slot":1,"hero":7,"destination":34}"#
        );
        // The historic form still parses, and still means "nearest".
        let old: Intent = serde_json::from_str(r#"{"type":"use_item","slot":0}"#).unwrap();
        assert!(
            matches!(old, Intent::UseItem { slot: 0, hero: None, destination: None }),
            "a command written before this bead means what it always meant"
        );
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"type":"use_item","slot":0}"#,
            "an unused field must not appear on the wire"
        );
    }

    /// **The latency row is unchanged** (docs/TEMPO.md §4). `use_item` is
    /// exempt because a hero is a command node, and choosing a destination
    /// does not move the hero — the item is still spent from the same bag, at
    /// the same place, by the same speaker. A destination that started
    /// charging for the privilege would be a silent tax on the one verb this
    /// bead exists to make more expressive.
    #[test]
    fn choosing_a_destination_does_not_start_charging_for_the_item() {
        let mut app = compiler_app();
        // Latency ON and no command nodes at all — the severed-arm case, where
        // anything that pays pays visibly.
        app.insert_resource(CommandLatency { on: true, ..Default::default() })
            .insert_resource(CommandNodes { nodes: Vec::new(), ready: true });
        let hero = spawn_hero(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        let far = spawn_hall_at(&mut app, BuildingKind::Keep, Team::Human, Vec3::new(-70.0, 0.0, -70.0), false);

        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            trigger: None,
            plan: None,
            intent: Intent::UseItem {
                slot: 0,
                hero: Some(hero.to_bits()),
                destination: Some(far.to_bits()),
            },
        });
        app.update();

        // Fired this frame, not queued for later — the exemption in one
        // observation.
        let uses = item_uses_of(&mut app);
        assert_eq!(uses.len(), 1, "the item fires on the frame it was asked for: {uses:?}");
        assert!(
            app.world().resource::<IntentApplied>().get(Team::Human).is_empty(),
            "an exempt verb reports no cost, however its destination is spelled"
        );
    }

    // -----------------------------------------------------------------------
    // Territory: named places and regions
    // -----------------------------------------------------------------------

    /// Every helper below speaks through the wire, never through `Regions`
    /// directly — a test that reached into the resource would be testing a
    /// setter rather than the language.
    fn region_set(app: &mut App, team: Team, name: &str, x: f32, z: f32, radius: f32) {
        app.world_mut().send_event(SubmitIntent::ui(
            team,
            Intent::RegionSet {
                name: name.to_string(),
                x,
                z,
                radius,
            },
        ));
        app.update();
    }

    fn errors_of(app: &App, team: Team) -> Vec<String> {
        app.world().resource::<IntentErrors>().get(team).to_vec()
    }

    fn drain_errors(app: &mut App, team: Team) -> Vec<String> {
        let out = errors_of(app, team);
        app.world_mut().resource_mut::<IntentErrors>().get_mut(team).clear();
        out
    }

    fn region_named(app: &App, team: Team, name: &str) -> Option<Region> {
        app.world().resource::<Regions>().find(team, name)
    }

    // -- CRUD, caps and refusals -------------------------------------------

    #[test]
    fn a_region_is_named_replaced_by_name_and_forgotten() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "north-pass", -60.0, 60.0, 18.0);
        let first = region_named(&app, Team::Human, "north-pass").expect("named");
        assert_eq!(first.center, Vec3::new(-60.0, 0.0, 60.0));
        assert_eq!(first.radius, 18.0);
        assert!(errors_of(&app, Team::Human).is_empty());

        // Re-stating the name MOVES the circle rather than minting a second
        // one — the trigger rule, applied to geography, and the reason a
        // commander can re-aim a region without spending a slot.
        region_set(&mut app, Team::Human, "north-pass", -40.0, 40.0, 24.0);
        assert_eq!(
            app.world().resource::<Regions>().get(Team::Human).len(),
            1,
            "replace by name, in place"
        );
        let moved = region_named(&app, Team::Human, "north-pass").expect("still named");
        assert_eq!(moved.center, Vec3::new(-40.0, 0.0, 40.0));
        assert_eq!(moved.radius, 24.0);

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::RegionClear {
                name: Some("north-pass".to_string()),
            },
        ));
        app.update();
        assert!(region_named(&app, Team::Human, "north-pass").is_none());
    }

    /// Case, dashes and underscores are noise; a possessive is not.
    #[test]
    fn a_region_name_folds_punctuation_but_not_meaning() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "The Perimeter", 0.0, 0.0, 20.0);
        for spelling in ["the perimeter", "THE-PERIMETER", "the_perimeter", "  The   Perimeter "] {
            assert!(
                region_named(&app, Team::Human, spelling).is_some(),
                "'{spelling}' must find the same circle"
            );
        }
        // ...and the label keeps the capitals the commander typed.
        assert_eq!(
            region_named(&app, Team::Human, "the perimeter").unwrap().name,
            "The Perimeter"
        );
        // The two built-in aliases differ by a possessive and must NOT fold
        // together — this is the case a naive stop-word list gets wrong.
        let ours = region_named(&app, Team::Human, "our base").expect("built-in");
        let theirs = region_named(&app, Team::Human, "their base").expect("built-in");
        assert_ne!(ours.center, theirs.center);
    }

    #[test]
    fn the_region_cap_is_eight_and_says_which_eight() {
        let mut app = compiler_app();
        for i in 0..MAX_REGIONS_PER_TEAM {
            region_set(&mut app, Team::Human, &format!("r{i}"), i as f32, 0.0, 10.0);
        }
        assert!(drain_errors(&mut app, Team::Human).is_empty(), "eight is legal");

        region_set(&mut app, Team::Human, "one-too-many", 50.0, 50.0, 10.0);
        let errs = drain_errors(&mut app, Team::Human);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("8 regions"), "names the cap: {}", errs[0]);
        assert!(errs[0].contains("r0, r1"), "lists them so one can be picked: {}", errs[0]);
        assert!(region_named(&app, Team::Human, "one-too-many").is_none());

        // Replacing by name is free even at the cap — otherwise the cap would
        // punish tuning a rule rather than owning too many.
        region_set(&mut app, Team::Human, "r3", -80.0, -80.0, 12.0);
        assert!(drain_errors(&mut app, Team::Human).is_empty());
        assert_eq!(
            region_named(&app, Team::Human, "r3").unwrap().center,
            Vec3::new(-80.0, 0.0, -80.0)
        );
    }

    #[test]
    fn a_region_may_not_steal_a_built_in_name() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "mid", 80.0, 80.0, 10.0);
        let errs = drain_errors(&mut app, Team::Human);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("built-in"), "{}", errs[0]);
        // And `mid` still means the middle of the map for BOTH seats, which is
        // the whole property the refusal protects.
        assert_eq!(
            region_named(&app, Team::Human, "mid").unwrap().center,
            Vec3::ZERO
        );
        assert_eq!(
            region_named(&app, Team::Claude, "mid").unwrap().center,
            Vec3::ZERO
        );

        // Clearing one is refused in its own words rather than with "you have
        // no region called that", which would be a lie.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::RegionClear {
                name: Some("our base".to_string()),
            },
        ));
        app.update();
        let errs = drain_errors(&mut app, Team::Human);
        assert!(errs[0].contains("cannot be cleared"), "{}", errs[0]);
    }

    #[test]
    fn a_region_radius_is_bounded_at_both_ends() {
        let mut app = compiler_app();
        for bad in [0.0, REGION_RADIUS_MIN - 0.1, REGION_RADIUS_MAX + 0.1, 500.0] {
            region_set(&mut app, Team::Human, "too", 0.0, 0.0, bad);
            let errs = drain_errors(&mut app, Team::Human);
            assert_eq!(errs.len(), 1, "radius {bad} must be refused");
            assert!(errs[0].contains("radius must be between"), "{}", errs[0]);
        }
        assert!(region_named(&app, Team::Human, "too").is_none());
    }

    /// A region is DOCTRINE. The two teams' lists are independent, and one
    /// seat's name is unspeakable at the other.
    #[test]
    fn regions_are_per_team_and_invisible_across_the_line() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "my-secret", -50.0, -50.0, 20.0);
        assert!(region_named(&app, Team::Human, "my-secret").is_some());
        assert!(
            region_named(&app, Team::Claude, "my-secret").is_none(),
            "naming ground tells the enemy nothing"
        );
        assert!(
            !app.world()
                .resource::<Regions>()
                .known_names(Team::Claude)
                .iter()
                .any(|n| n == "my-secret")
        );
        // Both seats may hold the SAME name for different ground.
        region_set(&mut app, Team::Claude, "my-secret", 50.0, 50.0, 20.0);
        assert_eq!(
            region_named(&app, Team::Human, "my-secret").unwrap().center,
            Vec3::new(-50.0, 0.0, -50.0)
        );
        assert_eq!(
            region_named(&app, Team::Claude, "my-secret").unwrap().center,
            Vec3::new(50.0, 0.0, 50.0)
        );
    }

    // -- built-in derivation -----------------------------------------------

    /// The map's own vocabulary exists before anybody arms anything, is the
    /// same for both seats except the two per-seat aliases, and names the
    /// mines the way intent_compile.py's `pick_mine` already does.
    #[test]
    fn the_built_in_places_are_map_facts_available_from_second_zero() {
        let names: Vec<String> = builtin_places(Team::Human)
            .into_iter()
            .map(|r| r.name)
            .collect();
        for want in [
            "our base",
            "their base",
            "mid",
            "southwest mine",
            "northeast mine",
            "northwest mine",
            "southeast mine",
        ] {
            assert!(names.iter().any(|n| n == want), "missing built-in '{want}' in {names:?}");
        }
        // Every name is unique — a vocabulary with two `north mine`s in it
        // would make `region` resolution a coin flip.
        let mut folded: Vec<String> = names.iter().map(|n| normalize_place(n)).collect();
        let before = folded.len();
        folded.sort();
        folded.dedup();
        assert_eq!(before, folded.len(), "built-in names must be distinct");
    }

    /// The mine names are the INVERSE of the tool that resolves them: the mine
    /// this calls `northwest mine` is the mine nearest intent_compile.py's
    /// `northwest` compass anchor, so the two vocabularies cannot drift.
    #[test]
    fn each_mine_is_named_for_the_compass_anchor_it_is_nearest() {
        let places = builtin_places(Team::Human);
        let anchors = [
            ("northwest mine", Vec3::new(-60.0, 0.0, 60.0)),
            ("northeast mine", Vec3::new(60.0, 0.0, 60.0)),
            ("southwest mine", Vec3::new(-60.0, 0.0, -60.0)),
            ("southeast mine", Vec3::new(60.0, 0.0, -60.0)),
        ];
        for (name, anchor) in anchors {
            let region = places.iter().find(|r| r.name == name).expect(name);
            // The mine this name resolves to must be the one closest to the
            // anchor the same word means in the compass table.
            let nearest = GOLD_MINE_POSITIONS
                .iter()
                .min_by(|a, b| a.distance(anchor).total_cmp(&b.distance(anchor)))
                .unwrap();
            assert_eq!(
                region.center, *nearest,
                "'{name}' must be the mine at the {name} anchor"
            );
        }
    }

    /// The two per-seat aliases resolve to the RIGHT corner for each seat, and
    /// everything else is identical between them.
    #[test]
    fn our_base_and_their_base_are_the_only_seat_relative_names() {
        let human = builtin_places(Team::Human);
        let claude = builtin_places(Team::Claude);
        assert_eq!(human.len(), claude.len());
        for (h, c) in human.iter().zip(claude.iter()) {
            assert_eq!(h.name, c.name, "both seats speak the same words");
            if h.name == "our base" || h.name == "their base" {
                assert_ne!(h.center, c.center, "'{}' must be seat-relative", h.name);
            } else {
                assert_eq!(h.center, c.center, "'{}' is neutral ground", h.name);
            }
        }
        let ours = human.iter().find(|r| r.name == "our base").unwrap();
        assert_eq!(ours.center, HUMAN_BASE);
        let theirs = human.iter().find(|r| r.name == "their base").unwrap();
        assert_eq!(theirs.center, CLAUDE_BASE);
        // ...and the Claude seat reads the identical two words the other way.
        assert_eq!(
            claude.iter().find(|r| r.name == "our base").unwrap().center,
            CLAUDE_BASE
        );
    }

    /// Every ford the map declares is a place you can name, at the ford's own
    /// opening width. `open` declares none and therefore offers none — the
    /// list is derived from the map rather than hardcoded per map.
    #[test]
    fn every_chokepoint_the_map_declares_becomes_a_named_place() {
        let places = builtin_places(Team::Human);
        let chokes = crate::terrain::active_map().chokepoints();
        for choke in &chokes {
            let region = places
                .iter()
                .find(|r| r.name == choke.name)
                .unwrap_or_else(|| panic!("ford '{}' must be nameable", choke.name));
            assert_eq!(region.center, choke.pos);
            assert_eq!(
                region.radius,
                (choke.width * 0.5).max(REGION_RADIUS_MIN),
                "a ford's region is its own opening"
            );
        }
        assert_eq!(
            places.len(),
            3 + GOLD_MINE_POSITIONS.len() + chokes.len(),
            "the built-in list is exactly bases + mid + mines + fords"
        );
    }

    // -- compiler resolution ------------------------------------------------

    /// The headline: a name in place of coordinates, resolved once, at submit
    /// time, for every verb that takes ground.
    #[test]
    fn a_region_name_stands_in_for_coordinates_anywhere_ground_is_named() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "the-perimeter", -40.0, 30.0, 26.0);
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Move {
                units: vec![soldier.to_bits()],
                x: None,
                z: None,
                region: Some("the-perimeter".to_string()),
                select: None,
            },
        ));
        app.update();
        assert!(errors_of(&app, Team::Human).is_empty(), "{:?}", errors_of(&app, Team::Human));
        assert!(
            matches!(
                app.world().entity(soldier).get::<Order>(),
                Some(Order::Move(p)) if (p.x + 40.0).abs() < 1e-3 && (p.z - 30.0).abs() < 1e-3
            ),
            "the order landed on the region's centre, got {:?}",
            app.world().entity(soldier).get::<Order>()
        );
    }

    /// Explicit coordinates still work, and a region WINS over them when both
    /// are given — one precedence, stated once, so no verb can disagree.
    #[test]
    fn a_region_outranks_coordinates_given_alongside_it() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "here", 10.0, -10.0, 12.0);
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Move {
                units: vec![soldier.to_bits()],
                x: Some(90.0),
                z: Some(90.0),
                region: Some("here".to_string()),
                select: None,
            },
        ));
        app.update();
        assert!(
            matches!(
                app.world().entity(soldier).get::<Order>(),
                Some(Order::Move(p)) if (p.x - 10.0).abs() < 1e-3
            ),
            "the name is the decision; the numbers alongside it are not"
        );
    }

    /// An unknown name is refused **with the menu attached**. This is the
    /// teaching error: a commander that mistyped gets the vocabulary back, not
    /// a "no".
    #[test]
    fn an_unknown_region_name_is_refused_with_the_list_of_known_places() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "the-perimeter", -40.0, 30.0, 26.0);
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Move {
                units: vec![soldier.to_bits()],
                x: None,
                z: None,
                region: Some("the-perimiter".to_string()),
                select: None,
            },
        ));
        app.update();
        let errs = drain_errors(&mut app, Team::Human);
        assert_eq!(errs.len(), 1, "one refusal, not one per unit");
        assert!(errs[0].contains("no region named 'the-perimiter'"), "{}", errs[0]);
        assert!(errs[0].contains("the-perimeter"), "offers the near miss: {}", errs[0]);
        assert!(errs[0].contains("center ford") || errs[0].contains("mid"), "offers the map's own: {}", errs[0]);
        assert!(
            matches!(app.world().entity(soldier).get::<Order>(), Some(Order::Idle)),
            "a refused sentence moves nothing"
        );
    }

    /// A sentence that names no ground at all earns one wording, whichever
    /// verb said it — that is what "one resolution point" buys.
    #[test]
    fn a_place_taking_verb_with_no_place_is_refused_in_one_voice() {
        let mut app = compiler_app();
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        let cases: Vec<(Intent, &str)> = vec![
            (
                Intent::Move {
                    units: vec![soldier.to_bits()],
                    x: None,
                    z: None,
                    region: None,
                    select: None,
                },
                "move needs x/z or a region name",
            ),
            (
                Intent::AttackMove {
                    units: vec![soldier.to_bits()],
                    x: None,
                    z: None,
                    region: None,
                    select: None,
                },
                "attackmove needs x/z or a region name",
            ),
            (
                Intent::Posture {
                    id: 1,
                    posture: Some(PostureIntent::Push {
                        x: None,
                        z: None,
                        region: None,
                    }),
                },
                "push needs x/z or a region name",
            ),
        ];
        for (intent, want) in cases {
            app.world_mut().send_event(SubmitIntent::ui(Team::Human, intent));
            app.update();
            let errs = drain_errors(&mut app, Team::Human);
            assert_eq!(errs.len(), 1, "expected exactly one refusal for {want}");
            assert!(errs[0].ends_with(want), "got {}", errs[0]);
        }
    }

    // -- posture-by-region mappings ----------------------------------------

    /// `defend` takes the region's own radius as its ring, `push` takes only
    /// the centre, `forage` musters at the centre. Each mapping asserted
    /// against the posture doctrine.rs will actually execute.
    #[test]
    fn each_posture_maps_a_region_onto_its_own_shape() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "north-pass", -60.0, 60.0, 19.0);

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 1,
                posture: Some(PostureIntent::Defend {
                    x: None,
                    z: None,
                    region: Some("north-pass".to_string()),
                    radius: None,
                }),
            },
        ));
        app.update();
        assert!(errors_of(&app, Team::Human).is_empty(), "{:?}", errors_of(&app, Team::Human));
        match app.world().resource::<SquadOrders>().0.get(&(Team::Human, 1)) {
            Some(SquadPosture::Defend { pos, radius }) => {
                assert_eq!(*pos, Vec3::new(-60.0, 0.0, 60.0));
                assert_eq!(*radius, 19.0, "the region IS the ring");
            }
            other => panic!("expected a defend posture, got {other:?}"),
        }

        // An explicit radius still wins: naming a circle is a convenience,
        // never a ceiling on what may be said.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 1,
                posture: Some(PostureIntent::Defend {
                    x: None,
                    z: None,
                    region: Some("north-pass".to_string()),
                    radius: Some(30.0),
                }),
            },
        ));
        app.update();
        assert!(matches!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Human, 1)),
            Some(SquadPosture::Defend { radius, .. }) if *radius == 30.0
        ));

        // Push: centre only, radius deliberately dropped.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 2,
                posture: Some(PostureIntent::Push {
                    x: None,
                    z: None,
                    region: Some("north-pass".to_string()),
                }),
            },
        ));
        app.update();
        assert!(matches!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Human, 2)),
            Some(SquadPosture::Push { pos }) if *pos == Vec3::new(-60.0, 0.0, 60.0)
        ));

        // Forage: the centre is the muster point.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 3,
                posture: Some(PostureIntent::Forage {
                    x: None,
                    z: None,
                    region: Some("north-pass".to_string()),
                }),
            },
        ));
        app.update();
        assert!(matches!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Human, 3)),
            Some(SquadPosture::Forage { muster }) if *muster == Vec3::new(-60.0, 0.0, 60.0)
        ));
    }

    // -----------------------------------------------------------------
    // Stances (wc3clone-0uu.2). Five words, each a fixed bundle of the four
    // doctrine verbs. docs/AFFORDANCES.md § Stances is the argument.
    // -----------------------------------------------------------------

    /// A footman of `team`, in `squad`, standing at the origin.
    fn squad_member(app: &mut App, team: Team, squad: u8) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                team,
                Transform::from_translation(Vec3::ZERO),
                Health::new(100.0),
                Order::Idle,
                SquadId(squad),
            ))
            .id()
    }

    fn stance(squad: u8, word: &str, at: Option<Vec3>) -> Intent {
        Intent::Stance {
            squad,
            stance: word.to_string(),
            x: at.map(|p| p.x),
            z: at.map(|p| p.z),
            region: None,
        }
    }

    /// **The bundle.** One word installs the posture, the leash, the retreat
    /// threshold and the focus list that `stances.ron` says it does — through
    /// the same components the four individual verbs write, which is what makes
    /// doctrine.rs unable to tell the difference.
    #[test]
    fn a_stance_compiles_to_the_bundle_its_row_describes() {
        let mut app = compiler_app();
        let member = squad_member(&mut app, Team::Human, 1);
        let anchor = Vec3::new(20.0, 0.0, -30.0);

        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "secure", Some(anchor))));
        app.update();

        let def = StanceKind::Secure.def();
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "a stance on a squad with a member is not a refusal"
        );

        // 1. The posture, in the same map `posture` writes.
        match app.world().resource::<SquadOrders>().0.get(&(Team::Human, 1)) {
            Some(SquadPosture::Defend { pos, radius }) => {
                assert_eq!(*pos, anchor);
                assert_eq!(*radius, def.radius);
            }
            other => panic!("secure must install a Defend ring, got {other:?}"),
        }

        // 2. The three per-unit policies, on the squad's members.
        let world = app.world();
        let leash = world.entity(member).get::<LeashPolicy>().expect("secure leashes");
        assert_eq!(leash.anchor, anchor);
        assert_eq!(leash.radius, def.leash);
        let retreat = world
            .entity(member)
            .get::<RetreatPolicy>()
            .expect("secure sets a retreat threshold");
        assert_eq!(retreat.below_frac, def.retreat_below);
        // `rally: Anchor` — a defensive stance pulls its wounded INTO the ring.
        assert_eq!(retreat.rally, anchor);
        let prio = world
            .entity(member)
            .get::<TargetPriority>()
            .expect("secure sets a focus list");
        assert_eq!(prio.0, def.priority);

        // 3. The word, for the snapshot to echo.
        assert_eq!(
            world.resource::<SquadStances>().0.get(&(Team::Human, 1)),
            Some(&StanceKind::Secure)
        );
    }

    /// **The default is persistence.** The whole point of the feature: a
    /// commander that says nothing for as many polls as you like still has the
    /// stance it set, and the engine is still executing it. r21 lost a match to
    /// the opposite — 98 seconds in which nothing continued the last decision.
    #[test]
    fn a_stance_survives_any_number_of_silent_polls() {
        let mut app = compiler_app();
        let member = squad_member(&mut app, Team::Human, 1);
        let anchor = Vec3::new(-40.0, 0.0, 10.0);

        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "harass", Some(anchor))));
        app.update();

        // Twenty frames in which the commander says nothing at all.
        for _ in 0..20 {
            app.update();
        }

        assert_eq!(
            app.world().resource::<SquadStances>().0.get(&(Team::Human, 1)),
            Some(&StanceKind::Harass),
            "silence must never dissolve a stance"
        );
        assert!(matches!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Human, 1)),
            Some(SquadPosture::Push { pos }) if *pos == anchor
        ));
        assert!(
            app.world().entity(member).get::<LeashPolicy>().is_some(),
            "the bundle is still installed after twenty silent frames"
        );
    }

    /// **Switching replaces the bundle whole.** The failure this guards is
    /// specific and would be invisible: `turtle` installs a 20-unit leash, and
    /// if `push` merely failed to mention leashes, the pushing squad would walk
    /// twenty metres and be hauled back by a policy nobody can see and nobody
    /// set. Absent pieces REMOVE.
    #[test]
    fn switching_stance_replaces_the_whole_bundle() {
        let mut app = compiler_app();
        let member = squad_member(&mut app, Team::Human, 1);

        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "turtle", None)));
        app.update();
        assert!(
            app.world().entity(member).get::<LeashPolicy>().is_some(),
            "turtle leashes its members"
        );

        let objective = Vec3::new(70.0, 0.0, 70.0);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "push", Some(objective))));
        app.update();

        let world = app.world();
        assert!(
            world.entity(member).get::<LeashPolicy>().is_none(),
            "push has no leash, so switching to it must REMOVE turtle's"
        );
        // ...and the pieces push does have are push's, not turtle's.
        let retreat = world.entity(member).get::<RetreatPolicy>().unwrap();
        assert_eq!(retreat.below_frac, StanceKind::Push.def().retreat_below);
        // `rally: Base` — an offensive stance sends its wounded out of the
        // fight, not back to the objective it is attacking.
        assert_eq!(retreat.rally, clamp_to_map(Team::Human.base_pos()));
        assert_eq!(
            world.resource::<SquadStances>().0.get(&(Team::Human, 1)),
            Some(&StanceKind::Push)
        );
    }

    /// A hand-set posture takes the squad OUT of its stance, because the word
    /// is no longer true. The readout is what a commander steers by; a
    /// `squads[].stance` that lied would be worse than no field at all.
    #[test]
    fn a_hand_set_posture_clears_the_stance_word() {
        let mut app = compiler_app();
        squad_member(&mut app, Team::Human, 1);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "turtle", None)));
        app.update();

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 1,
                posture: Some(PostureIntent::Push {
                    x: Some(10.0),
                    z: Some(10.0),
                    region: None,
                }),
            },
        ));
        app.update();

        assert!(
            app.world().resource::<SquadStances>().0.get(&(Team::Human, 1)).is_none(),
            "a squad tasked by hand is in no stance"
        );
        // Stand-down clears it too — same arm, before the early return.
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "stage", None)));
        app.update();
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, Intent::Posture { id: 1, posture: None }));
        app.update();
        assert!(app.world().resource::<SquadStances>().0.get(&(Team::Human, 1)).is_none());
    }

    /// **`squad` then `stance`, in ONE batch.** The obvious opening a commander
    /// writes, and the one shape that could silently half-work: `squad` inserts
    /// `SquadId` through `Commands`, Bevy does not flush a command queue until
    /// the system ends, so the stance's per-unit half would find an empty squad
    /// and install nothing while its posture — which is per-squad — landed
    /// fine. Half a doctrine, no error, and a leash the commander believes is
    /// on. `batch_squads` is what makes the two sentences agree.
    #[test]
    fn a_squad_and_a_stance_in_one_batch_reach_the_same_units() {
        let mut app = compiler_app();
        // Deliberately NOT pre-enrolled: this is a bare unit, exactly as it is
        // the moment it walks out of a barracks.
        let unit = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Health::new(100.0),
                Order::Idle,
            ))
            .id();

        // Both sentences in the same frame, in the order a commander writes
        // them. `apply_intents` drains them together.
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 0".to_string(),
            intent: Intent::Squad {
                units: vec![unit.to_bits()],
                id: Some(1),
                select: None,
            },
            trigger: None,
            plan: None,
        });
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Bridge,
            tag: "cmd 1".to_string(),
            intent: stance(1, "turtle", None),
            trigger: None,
            plan: None,
        });
        app.update();

        assert!(
            app.world().entity(unit).get::<LeashPolicy>().is_some(),
            "the stance must reach a unit the sentence before it enrolled"
        );
        assert!(app.world().entity(unit).get::<RetreatPolicy>().is_some());
        assert!(
            app.world().resource::<IntentErrors>().get(Team::Human).is_empty(),
            "and must not claim the squad was empty"
        );
    }

    // -----------------------------------------------------------------
    // Late joiners (wc3clone-bol). A unit that walks into a stanced squad
    // wears the whole stance, not just the posture.
    // -----------------------------------------------------------------

    /// **The bead.** A commander stances squad 1, then reinforces it — the
    /// ordinary shape of a match. Before this, the newcomer inherited the
    /// posture (per-squad) and none of the leash, threshold or focus list, so
    /// the squad fielded two different doctrines and the only symptom was that
    /// half of it did not break off.
    #[test]
    fn a_unit_that_joins_a_stanced_squad_inherits_the_whole_stance() {
        let mut app = compiler_app();
        let founder = squad_member(&mut app, Team::Human, 1);
        let anchor = Vec3::new(20.0, 0.0, -30.0);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "secure", Some(anchor))));
        app.update();

        // A body trained after the word was sent, enrolled by a later `squad`.
        let recruit = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Health::new(100.0),
                Order::Idle,
            ))
            .id();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Squad {
                units: vec![recruit.to_bits()],
                id: Some(1),
                select: None,
            },
        ));
        app.update();

        let def = StanceKind::Secure.def();
        let world = app.world();
        for (who, entity) in [("the founder", founder), ("the recruit", recruit)] {
            let leash = world
                .entity(entity)
                .get::<LeashPolicy>()
                .unwrap_or_else(|| panic!("{who} must carry secure's leash"));
            assert_eq!(leash.anchor, anchor, "{who}'s leash is pinned elsewhere");
            assert_eq!(leash.radius, def.leash);
            let retreat = world
                .entity(entity)
                .get::<RetreatPolicy>()
                .unwrap_or_else(|| panic!("{who} must carry secure's threshold"));
            assert_eq!(retreat.below_frac, def.retreat_below);
            assert_eq!(retreat.rally, anchor);
            let prio = world
                .entity(entity)
                .get::<TargetPriority>()
                .unwrap_or_else(|| panic!("{who} must carry secure's focus list"));
            assert_eq!(prio.0, def.priority);
        }
    }

    /// **The choke point is the component, not the `squad` verb.** units.rs
    /// stamps a `DoctrineTemplate`'s squad at spawn and doctrine.rs enrols
    /// strays into `DEFAULT_SQUAD`; neither goes anywhere near this compiler.
    /// Writing `SquadId` from outside is how this test says so — if the
    /// inheritance were wired into the verb instead, this would fail.
    #[test]
    fn a_unit_enrolled_by_another_module_inherits_the_stance_too() {
        let mut app = compiler_app();
        squad_member(&mut app, Team::Human, DEFAULT_SQUAD);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(DEFAULT_SQUAD, "turtle", None)));
        app.update();

        // No intent, no batch, no compiler: just the component, exactly as
        // `default_squad_autonomy` and the production template write it.
        let stray = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Health::new(100.0),
                Order::Idle,
                SquadId(DEFAULT_SQUAD),
            ))
            .id();
        app.update();

        assert!(
            app.world().entity(stray).get::<LeashPolicy>().is_some(),
            "a unit enrolled outside the compiler still joined a turtling squad"
        );
        assert_eq!(
            app.world().entity(stray).get::<RetreatPolicy>().unwrap().below_frac,
            StanceKind::Turtle.def().retreat_below
        );
    }

    /// Changing squads is joining one, and the new squad's bundle replaces the
    /// old one's whole — the same rule `switching_stance_replaces_the_whole_bundle`
    /// pins for a squad that switches word. A body walked out of a `turtle` and
    /// into a `push` must not keep the turtle's leash, or the push it just
    /// joined gets hauled back by a policy nobody set.
    #[test]
    fn a_unit_that_changes_squad_swaps_bundles_whole() {
        let mut app = compiler_app();
        let unit = squad_member(&mut app, Team::Human, 1);
        squad_member(&mut app, Team::Human, 2);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "turtle", None)));
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            stance(2, "push", Some(Vec3::new(70.0, 0.0, 70.0))),
        ));
        app.update();
        assert!(
            app.world().entity(unit).get::<LeashPolicy>().is_some(),
            "control: turtle leashed it"
        );

        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Squad {
                units: vec![unit.to_bits()],
                id: Some(2),
                select: None,
            },
        ));
        app.update();

        let world = app.world();
        assert!(
            world.entity(unit).get::<LeashPolicy>().is_none(),
            "push has no leash, so joining a pushing squad must REMOVE turtle's"
        );
        assert_eq!(
            world.entity(unit).get::<RetreatPolicy>().unwrap().rally,
            clamp_to_map(Team::Human.base_pos()),
            "push rallies to base; the turtle's anchor rally must be gone"
        );
    }

    /// **Only stanced squads.** A squad holding a hand-set `posture` has no
    /// per-squad doctrine to hand down — `leash`, `retreat` and `priority` take
    /// a unit selector, not a squad — so a joiner is left exactly as it was.
    /// This is the documented limit of the feature, and the reason the word
    /// rather than the posture is what a joiner inherits.
    #[test]
    fn a_joiner_of_an_unstanced_squad_is_left_alone() {
        let mut app = compiler_app();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 1,
                posture: Some(PostureIntent::Push {
                    x: Some(10.0),
                    z: Some(10.0),
                    region: None,
                }),
            },
        ));
        app.update();

        let recruit = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Health::new(100.0),
                Order::Idle,
                SquadId(1),
            ))
            .id();
        app.update();

        let world = app.world();
        assert!(world.entity(recruit).get::<LeashPolicy>().is_none());
        assert!(world.entity(recruit).get::<RetreatPolicy>().is_none());
        assert!(world.entity(recruit).get::<TargetPriority>().is_none());
    }

    /// A stance reaches only ITS OWN squad, and only its own team. The same
    /// ownership rule every other verb obeys, asked of the one verb here that
    /// finds its units by membership rather than by an id list.
    #[test]
    fn a_stance_touches_only_its_own_squad_and_team() {
        let mut app = compiler_app();
        let mine = squad_member(&mut app, Team::Human, 1);
        let other_squad = squad_member(&mut app, Team::Human, 2);
        let enemy = squad_member(&mut app, Team::Claude, 1);

        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "turtle", None)));
        app.update();

        let world = app.world();
        assert!(world.entity(mine).get::<LeashPolicy>().is_some());
        assert!(
            world.entity(other_squad).get::<LeashPolicy>().is_none(),
            "squad 2 was not spoken to"
        );
        assert!(
            world.entity(enemy).get::<LeashPolicy>().is_none(),
            "red's squad 1 is not blue's squad 1"
        );
        assert!(world.resource::<SquadStances>().0.get(&(Team::Claude, 1)).is_none());
    }

    /// **The refusal teaches.** An unknown word names all five rather than
    /// leaving a commander to guess, and installs nothing.
    #[test]
    fn an_unknown_stance_word_lists_the_five() {
        let mut app = compiler_app();
        squad_member(&mut app, Team::Human, 1);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Human, stance(1, "aggressive", None)));
        app.update();

        let errors = app.world().resource::<IntentErrors>().get(Team::Human).to_vec();
        let line = errors.first().expect("an unknown stance is refused");
        for word in ["turtle", "stage", "push", "secure", "harass"] {
            assert!(line.contains(word), "the refusal must name '{word}': {line}");
        }
        assert!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Human, 1)).is_none(),
            "a refused stance installs nothing"
        );
    }

    /// Every stance word is spellable however a commander types it, and every
    /// one of the five actually resolves to a row. The cheap guard against a
    /// word being added to the enum and forgotten in the data file — which the
    /// loader would catch at startup, but a test catches at `cargo test`.
    #[test]
    fn every_stance_word_round_trips_and_has_a_row() {
        for kind in ALL_STANCES {
            assert_eq!(parse_stance(kind.word()), Some(kind));
            assert_eq!(parse_stance(&kind.word().to_uppercase()), Some(kind));
            let def = kind.def();
            assert_eq!(def.kind, kind);
            assert!(!def.label.is_empty());
        }
        // And the cycle the human's tile walks visits all five and returns.
        let mut seen = vec![ALL_STANCES[0]];
        let mut at = ALL_STANCES[0];
        for _ in 1..ALL_STANCES.len() {
            at = at.next();
            seen.push(at);
        }
        assert_eq!(seen, ALL_STANCES.to_vec());
        assert_eq!(at.next(), ALL_STANCES[0], "the cycle wraps");
    }

    /// A stance with no anchor means the team's own base — which is what
    /// `turtle` means with no argument, and the reason the verb has a legal
    /// one-word form at all.
    #[test]
    fn a_stance_with_no_anchor_holds_the_teams_own_base() {
        let mut app = compiler_app();
        squad_member(&mut app, Team::Claude, 4);
        app.world_mut()
            .send_event(SubmitIntent::ui(Team::Claude, stance(4, "turtle", None)));
        app.update();

        assert!(matches!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Claude, 4)),
            Some(SquadPosture::Defend { pos, .. })
                if *pos == clamp_to_map(Team::Claude.base_pos())
        ));
    }

    /// A built-in needs no arming: "defend the center ford" is a legal
    /// sentence in the first second of a match on a map that has one.
    #[test]
    fn a_posture_may_name_a_built_in_place_with_nothing_armed() {
        let mut app = compiler_app();
        let place = builtin_places(Team::Human)
            .into_iter()
            .find(|r| r.name == "mid")
            .expect("every map has a middle");
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Posture {
                id: 1,
                posture: Some(PostureIntent::Defend {
                    x: None,
                    z: None,
                    region: Some("mid".to_string()),
                    radius: None,
                }),
            },
        ));
        app.update();
        assert!(errors_of(&app, Team::Human).is_empty());
        assert!(matches!(
            app.world().resource::<SquadOrders>().0.get(&(Team::Human, 1)),
            Some(SquadPosture::Defend { pos, radius })
                if *pos == place.center && *radius == place.radius
        ));
    }

    /// `leash` borrows the region's radius the same way `defend` does — the
    /// two are the same shape, so they had better agree.
    #[test]
    fn a_leash_named_by_region_borrows_the_regions_radius() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "the-ring", 20.0, -20.0, 16.0);
        let soldier = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                Order::Idle,
            ))
            .id();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::Leash {
                units: vec![soldier.to_bits()],
                x: None,
                z: None,
                region: Some("the-ring".to_string()),
                radius: None,
                select: None,
            },
        ));
        app.update();
        let policy = app
            .world()
            .entity(soldier)
            .get::<LeashPolicy>()
            .expect("leashed");
        assert_eq!(policy.anchor, Vec3::new(20.0, 0.0, -20.0));
        assert_eq!(policy.radius, 16.0);
    }

    // -- sentences ----------------------------------------------------------

    /// A named place is SPOKEN as its name. This is most of why regions exist:
    /// the replay line for a defended ford reads "defends north-pass", not
    /// "defends (-60.0, 60.0)".
    #[test]
    fn a_sentence_naming_a_region_reads_as_the_name() {
        assert_eq!(
            Intent::Posture {
                id: 2,
                posture: Some(PostureIntent::Defend {
                    x: Some(-60.0),
                    z: Some(60.0),
                    region: Some("north-pass".to_string()),
                    radius: None,
                }),
            }
            .sentence(),
            "squad 2 defends north-pass"
        );
        assert_eq!(
            Intent::Move {
                units: vec![7],
                x: None,
                z: None,
                region: Some("the-perimeter".to_string()),
                select: None,
            }
            .sentence(),
            "move unit 7 to the-perimeter"
        );
        // Without a name, nothing changed: the old sentence, byte for byte.
        assert_eq!(
            Intent::Move {
                units: vec![7],
                x: Some(1.0),
                z: Some(2.0),
                region: None,
                select: None,
            }
            .sentence(),
            "move unit 7 to (1.0, 2.0)"
        );
        assert_eq!(
            Intent::RegionSet {
                name: "north-pass".to_string(),
                x: -60.0,
                z: 60.0,
                radius: 18.0,
            }
            .sentence(),
            "'north-pass' is the ground within 18 of (-60.0, 60.0)"
        );
    }

    // -- triggers -----------------------------------------------------------

    /// `enemy_in`'s region is a constant the commander typed, so it is judged
    /// at ARM time — with the menu attached, like every other unknown name.
    #[test]
    fn arming_enemy_in_with_an_unknown_region_is_refused_immediately() {
        let mut app = compiler_app();
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::TriggerSet {
                name: "pass-watch".to_string(),
                when: TriggerWhen::EnemyIn {
                    region: "nowhere".to_string(),
                    class: None,
                    count: 5,
                },
                then: Box::new(Intent::Stop {
                    units: vec![],
                    select: None,
                }),
                repeat: None,
            },
        ));
        app.update();
        let errs = drain_errors(&mut app, Team::Human);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("no region named 'nowhere'"), "{}", errs[0]);
        assert!(
            app.world().resource::<Triggers>().get(Team::Human).is_empty(),
            "a rule whose predicate cannot be spelled must not be armed"
        );

        // The same rule against a BUILT-IN arms with nothing named first.
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::TriggerSet {
                name: "pass-watch".to_string(),
                when: TriggerWhen::EnemyIn {
                    region: "mid".to_string(),
                    class: None,
                    count: 5,
                },
                then: Box::new(Intent::Stop {
                    units: vec![],
                    select: None,
                }),
                repeat: None,
            },
        ));
        app.update();
        assert!(drain_errors(&mut app, Team::Human).is_empty());
        assert_eq!(app.world().resource::<Triggers>().get(Team::Human).len(), 1);
    }

    /// A trigger's ACTION is not resolved at arm time — the codebase's existing
    /// rule, extended to territory rather than excepted from it. The rule keeps
    /// naming *the perimeter*, so moving the region re-aims the rule.
    #[test]
    fn an_armed_rule_keeps_the_name_rather_than_the_coordinates() {
        let mut app = compiler_app();
        region_set(&mut app, Team::Human, "the-perimeter", -40.0, 30.0, 20.0);
        app.world_mut().send_event(SubmitIntent::ui(
            Team::Human,
            Intent::TriggerSet {
                name: "hold".to_string(),
                when: TriggerWhen::GameTime { at: 1.0 },
                then: Box::new(Intent::Posture {
                    id: 1,
                    posture: Some(PostureIntent::Defend {
                        x: None,
                        z: None,
                        region: Some("the-perimeter".to_string()),
                        radius: None,
                    }),
                }),
                repeat: None,
            },
        ));
        app.update();
        let stored = app.world().resource::<Triggers>().get(Team::Human)[0].then.clone();
        assert!(
            matches!(
                &stored,
                Intent::Posture {
                    posture: Some(PostureIntent::Defend { x: None, region: Some(name), .. }),
                    ..
                } if name == "the-perimeter"
            ),
            "the stored action must still be a NAME, got {stored:?}"
        );
        assert_eq!(stored.sentence(), "squad 1 defends the-perimeter");
    }

    #[test]
    fn a_trigger_cannot_name_or_forget_ground() {
        let mut app = compiler_app();
        for then in [
            Intent::RegionSet {
                name: "sneaky".to_string(),
                x: 0.0,
                z: 0.0,
                radius: 10.0,
            },
            Intent::RegionClear { name: None },
        ] {
            app.world_mut().send_event(SubmitIntent::ui(
                Team::Human,
                Intent::TriggerSet {
                    name: "t".to_string(),
                    when: TriggerWhen::GameTime { at: 0.0 },
                    then: Box::new(then),
                    repeat: None,
                },
            ));
            app.update();
            let errs = drain_errors(&mut app, Team::Human);
            assert_eq!(errs.len(), 1);
            assert!(errs[0].contains("doctrine, not a scripting language"), "{}", errs[0]);
        }
        assert!(app.world().resource::<Triggers>().get(Team::Human).is_empty());
    }

    // -----------------------------------------------------------------------
    // Late-bound selectors (docs/AFFORDANCES.md § Chains; arena r21–r23)
    // -----------------------------------------------------------------------

    /// Fire a stored trigger's action exactly the way `trigger.rs` does: pull
    /// the `then` out of `Triggers` and submit it. Nothing here re-derives the
    /// intent, so a test that fires twice is genuinely firing the SAME stored
    /// sentence twice.
    fn fire(app: &mut App, team: Team, name: &str) {
        let then = app
            .world()
            .resource::<Triggers>()
            .get(team)
            .iter()
            .find(|t| t.name.as_str() == name)
            .unwrap_or_else(|| panic!("no trigger named {name}"))
            .then
            .clone();
        let stamp = TriggerName::new(name).unwrap();
        app.world_mut()
            .send_event(SubmitIntent::fired(team, IntentSource::Bridge, stamp, then));
        app.update();
    }

    fn order_of(app: &App, entity: Entity) -> Order {
        app.world()
            .entity(entity)
            .get::<Order>()
            .cloned()
            .unwrap_or_default()
    }

    fn spawn_worker(app: &mut App, team: Team, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit {
                    kind: UnitKind::Worker,
                },
                team,
                Transform::from_translation(at),
                Order::Idle,
            ))
            .id()
    }

    fn spawn_footman(app: &mut App, team: Team, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit {
                    kind: UnitKind::Footman,
                },
                team,
                Transform::from_translation(at),
                Order::Idle,
            ))
            .id()
    }

    fn spawn_node(app: &mut App, kind: ResourceKind, at: Vec3, remaining: u32) -> Entity {
        app.world_mut()
            .spawn((
                ResourceNode { kind, remaining },
                Transform::from_translation(at),
            ))
            .id()
    }

    /// **The bead, in one test.** Arm a hero-save rule against the ROLE, kill
    /// the hero the role pointed at when the rule was armed, train another, and
    /// fire the same stored sentence again. The new hero moves.
    ///
    /// This is r21's `"units":[]` corpse and red-r23's dead hero ids in a single
    /// scenario, and the thing that fixes both is that the stored `then` still
    /// says `"select":"my hero"` on the second firing — the resolution happened
    /// in `resolve_places`, at the top of the compiler, and was thrown away
    /// afterwards exactly like a region's coordinates.
    #[test]
    fn a_selector_binds_to_the_hero_that_exists_when_the_rule_fires() {
        let mut app = compiler_app();
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"hero-save","repeat":5,
                "when":{"type":"base_under_attack"},
                "then":{"type":"move","select":"my hero","x":-70.0,"z":-70.0}}"#,
        );
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "arming against a role the team does not have yet must not refuse"
        );

        let first = spawn_hero(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));
        fire(&mut app, Team::Human, "hero-save");
        assert!(
            matches!(order_of(&app, first), Order::Move(p) if p.x == -70.0 && p.z == -70.0),
            "the hero alive at fire time is the one that moves"
        );

        // The hero dies and the team trains another. A frozen id would now name
        // a corpse; the role names whoever holds it.
        app.world_mut().despawn(first);
        let second = spawn_hero(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        app.update();
        drain_errors(&mut app, Team::Human);

        fire(&mut app, Team::Human, "hero-save");
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "the re-fire must not report a dead id"
        );
        assert!(
            matches!(order_of(&app, second), Order::Move(p) if p.x == -70.0 && p.z == -70.0),
            "the hero trained AFTER the rule was armed is the one that moves"
        );

        // And the stored sentence is still the role, not a resolved roster.
        let then = app
            .world()
            .resource::<Triggers>()
            .get(Team::Human)
            .iter()
            .find(|t| t.name.as_str() == "hero-save")
            .unwrap()
            .then
            .clone();
        match then {
            Intent::Move { units, select, .. } => {
                assert!(units.is_empty(), "resolution must not be written back");
                assert_eq!(select.as_deref(), Some("my hero"));
            }
            other => panic!("stored then changed shape: {other:?}"),
        }
    }

    /// **"Move 0 units" is inexpressible.** r21's hero-save fired with
    /// `"units":[]`, was rejected as "no units given", and the hero died three
    /// seconds later. A selector that currently matches nobody is the same
    /// situation said in the new vocabulary, and it must teach rather than
    /// fire: nothing is ordered, and the seat is told which phrase found
    /// nobody.
    #[test]
    fn an_empty_selector_teaches_instead_of_ordering_nobody() {
        let mut app = compiler_app();
        // A worker exists, so the team is not empty — only the ARMY is.
        let worker = spawn_worker(&mut app, Team::Human, Vec3::ZERO);
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"move","select":"all army","x":5.0,"z":5.0}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'all army' matches none of your units right now")
                && errors[0].contains("nothing was ordered"),
            "{}",
            errors[0]
        );
        assert!(
            matches!(order_of(&app, worker), Order::Idle),
            "an empty resolution must not spill onto somebody else"
        );
    }

    /// A plan step whose selector matches nobody is a REFUSAL, not a partial
    /// success — so the plan blocks and says why instead of walking past a step
    /// that did nothing. (`reached` is what tells the two apart; an empty
    /// resolution never reaches the verb's arm at all.)
    #[test]
    fn an_empty_selector_blocks_the_plan_step_that_used_it() {
        let mut app = compiler_app();
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"plan_set","name":"opening","steps":[
                {"intent":{"type":"attackmove","select":"all army","x":0.0,"z":0.0}}]}"#,
        ));
        app.update();
        drain_errors(&mut app, Team::Human);

        let stamp = PlanStamp {
            name: PlanName::new("opening").unwrap(),
            step: 1,
            of: 1,
        };
        let step = app.world().resource::<Plans>().get(Team::Human)[0].steps[0]
            .intent
            .clone();
        // What plan.rs's evaluator sets on the way out; without it `report`
        // treats the verdict as addressed to a plan that has sent nothing.
        app.world_mut().resource_mut::<Plans>().get_mut(Team::Human)[0].submitted = true;
        app.world_mut().send_event(SubmitIntent::plan_step(
            Team::Human,
            IntentSource::Bridge,
            stamp,
            step,
        ));
        app.update();
        let state = app.world().resource::<Plans>().get(Team::Human)[0]
            .state
            .clone();
        assert!(
            matches!(state, PlanState::Blocked(ref why) if why.contains("all army")),
            "{state:?}"
        );
    }

    /// **A selector outranks the ids beside it**, exactly as a region outranks
    /// the coordinates beside it. Red-r23's stale rosters were lists that had
    /// been right when they were written; a sentence carrying both a stale list
    /// and a role must mean the role, and must not report the stale list's
    /// corpses as errors it is about to ignore anyway.
    #[test]
    fn a_selector_outranks_the_ids_beside_it() {
        let mut app = compiler_app();
        let corpse = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        app.world_mut().despawn(corpse);
        let alive = spawn_footman(&mut app, Team::Human, Vec3::new(3.0, 0.0, 0.0));
        app.update();

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            &format!(
                r#"{{"type":"move","units":[{}],"select":"all army","x":8.0,"z":8.0}}"#,
                corpse.to_bits()
            ),
        ));
        app.update();
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "the overruled list must not be reported"
        );
        assert!(matches!(order_of(&app, alive), Order::Move(p) if p.x == 8.0 && p.z == 8.0));
    }

    /// `squad N` means the members it has NOW. A unit enrolled after the rule
    /// was armed is in it; a unit that never joined is not.
    #[test]
    fn a_squad_selector_names_the_members_the_squad_has_now() {
        let mut app = compiler_app();
        let veteran = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        app.world_mut().entity_mut(veteran).insert(SquadId(2));
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"push","repeat":5,
                "when":{"type":"game_time","at":0.0},
                "then":{"type":"attackmove","select":"squad 2","x":40.0,"z":40.0}}"#,
        );
        drain_errors(&mut app, Team::Human);

        // A recruit joins the squad AFTER the rule was written.
        let recruit = spawn_footman(&mut app, Team::Human, Vec3::new(1.0, 0.0, 0.0));
        app.world_mut().entity_mut(recruit).insert(SquadId(2));
        // And an outsider that must not be swept up.
        let outsider = spawn_footman(&mut app, Team::Human, Vec3::new(2.0, 0.0, 0.0));
        app.update();

        fire(&mut app, Team::Human, "push");
        assert!(matches!(order_of(&app, veteran), Order::AttackMove(_)));
        assert!(matches!(order_of(&app, recruit), Order::AttackMove(_)));
        assert!(matches!(order_of(&app, outsider), Order::Idle));
    }

    /// **The tree is chosen when the order compiles.** Red-r23 memorized a tree
    /// id and had it chopped out from under a repeating harvest order. The same
    /// rule written against `"nearest tree"` re-answers the question every time
    /// it fires.
    #[test]
    fn the_nearest_tree_is_chosen_at_fire_time_not_arm_time() {
        let mut app = compiler_app();
        let worker = spawn_worker(&mut app, Team::Human, Vec3::ZERO);
        let near = spawn_node(
            &mut app,
            ResourceKind::Lumber,
            Vec3::new(5.0, 0.0, 0.0),
            100,
        );
        let far = spawn_node(
            &mut app,
            ResourceKind::Lumber,
            Vec3::new(40.0, 0.0, 0.0),
            100,
        );
        // A mine sitting nearer than either tree, to prove the kind is honoured.
        spawn_node(&mut app, ResourceKind::Gold, Vec3::new(1.0, 0.0, 0.0), 100);
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"chop","repeat":5,
                "when":{"type":"game_time","at":0.0},
                "then":{"type":"harvest","select":"workers","target_select":"nearest tree"}}"#,
        );
        drain_errors(&mut app, Team::Human);

        fire(&mut app, Team::Human, "chop");
        assert!(matches!(order_of(&app, worker), Order::Harvest(e) if e == near));

        // The near tree is felled. The stored rule still says "nearest tree".
        app.world_mut().despawn(near);
        app.update();
        fire(&mut app, Team::Human, "chop");
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "a felled tree must not become an error"
        );
        assert!(matches!(order_of(&app, worker), Order::Harvest(e) if e == far));
    }

    /// An exhausted map teaches instead of firing, on the same rule as an empty
    /// unit selector.
    #[test]
    fn a_nearest_node_selector_with_nothing_left_teaches() {
        let mut app = compiler_app();
        spawn_worker(&mut app, Team::Human, Vec3::ZERO);
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"harvest","select":"workers","target_select":"nearest mine"}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("no mine left on the map"),
            "{}",
            errors[0]
        );
    }

    /// **A dry mine is a place, not a job.** economy.rs keeps a mined-out gold
    /// mine on the board — `mine_dry`, the income alarm and `mines[].remaining`
    /// all need a dry mine they can look at, and blue-r23's expand trigger never
    /// fired because the node was despawned in the same statement that emptied
    /// it. The cost of keeping it is that its id still resolves, so `harvest`
    /// owes the commander a refusal that names the way to ask again: without one
    /// the harvest loop would silently re-aim the crew at a node nobody named.
    #[test]
    fn harvesting_a_dry_mine_is_refused_and_names_the_selector() {
        let mut app = compiler_app();
        let worker = spawn_worker(&mut app, Team::Human, Vec3::ZERO);
        let dry = spawn_node(&mut app, ResourceKind::Gold, Vec3::new(5.0, 0.0, 0.0), 0);
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            &format!(
                r#"{{"type":"harvest","units":[{}],"target":{}}}"#,
                worker.to_bits(),
                dry.to_bits()
            ),
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("is empty"), "{}", errors[0]);
        assert!(errors[0].contains("nearest mine"), "{}", errors[0]);
        assert!(
            matches!(order_of(&app, worker), Order::Idle),
            "a refused harvest orders nobody"
        );
    }

    /// **Blue-r23's loop, closed.** A fixed-coordinate farm trigger reported
    /// `site blocked` on every retry all match; the rejection was already
    /// computing a legal alternative and printing it, and there was no way to
    /// say "yes, that one". `"site":"nearest legal site"` says it.
    #[test]
    fn a_site_selector_accepts_the_nearest_legal_footprint() {
        let mut app = compiler_app();
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let e = economies.get_mut(Team::Human);
            e.gold = 2000;
            e.lumber = 2000;
        }
        let wanted = Vec3::new(20.0, 0.0, 20.0);
        app.world_mut()
            .resource_mut::<NavGrid>()
            .set_blocked_rect(wanted, 6.0, true);
        spawn_worker(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));

        // Without the selector: the historical refusal, unchanged.
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"build","select":"workers","kind":"Farm","x":20.0,"z":20.0}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("is blocked for"), "{}", errors[0]);

        // With it: the engine takes its own advice.
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"build","select":"workers","kind":"Farm","x":20.0,"z":20.0,
                "site":"nearest legal site"}"#,
        ));
        app.update();
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "the nearest legal site must itself be legal"
        );
        let placed = app
            .world_mut()
            .query::<&Order>()
            .iter(app.world())
            .find_map(|o| match o {
                Order::Build { kind, pos } => Some((*kind, *pos)),
                _ => None,
            })
            .expect("a build order was issued");
        let (kind, pos) = placed;
        assert_eq!(kind, BuildingKind::Farm);
        let size = building_stats(BuildingKind::Farm).size;
        assert!(
            app.world().resource::<NavGrid>().rect_is_free(pos, size),
            "the chosen site must be free"
        );
        assert!(
            (pos.x - wanted.x).hypot(pos.z - wanted.z) <= PLACEMENT_HINT_RADIUS,
            "and near where the commander asked: {pos:?}"
        );
    }

    /// A misspelled phrase earns the list of phrases that exist — the
    /// `Regions::unknown` rule applied to roles.
    #[test]
    fn an_unknown_selector_names_the_ones_that_exist() {
        let mut app = compiler_app();
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"move","select":"my hreo","x":1.0,"z":1.0}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("unknown selector 'my hreo'"),
            "{}",
            errors[0]
        );
        assert!(errors[0].contains("all army"), "{}", errors[0]);
        assert!(errors[0].contains("squad <n>"), "{}", errors[0]);
    }

    /// The channels are typed, and saying so is the teaching. A tree is not an
    /// army; the refusal names the phrases that ARE.
    #[test]
    fn a_node_phrase_in_the_unit_channel_says_which_phrases_belong_there() {
        let mut app = compiler_app();
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"move","select":"nearest tree","x":1.0,"z":1.0}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("names a resource node, not units"),
            "{}",
            errors[0]
        );
        assert!(errors[0].contains(SELECTOR_UNIT_NAMES), "{}", errors[0]);
    }

    /// A selector never reaches across the line. `all units` is *my* units, and
    /// the enemy's stay where they are.
    #[test]
    fn a_selector_is_bounded_by_the_seat_that_speaks_it() {
        let mut app = compiler_app();
        let mine = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        let theirs = spawn_footman(&mut app, Team::Claude, Vec3::new(1.0, 0.0, 0.0));
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"move","select":"all units","x":9.0,"z":9.0}"#,
        ));
        app.update();
        assert!(matches!(order_of(&app, mine), Order::Move(_)));
        assert!(matches!(order_of(&app, theirs), Order::Idle));
    }

    /// The resolved list is sorted by entity id, not by whatever order the
    /// archetypes happen to be walked in — `ground_order` hands out formation
    /// offsets by index, so an unsorted resolution would arrange the same squad
    /// differently in two runs of the same binary.
    #[test]
    fn a_selector_resolves_in_a_deterministic_order() {
        let mut app = compiler_app();
        let a = spawn_footman(&mut app, Team::Human, Vec3::ZERO);
        let b = spawn_footman(&mut app, Team::Human, Vec3::new(1.0, 0.0, 0.0));
        let c = spawn_footman(&mut app, Team::Human, Vec3::new(2.0, 0.0, 0.0));
        app.update();
        let mut expected = [a, b, c];
        expected.sort_by_key(|e| e.to_bits());

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"move","select":"all army","x":0.0,"z":0.0}"#,
        ));
        app.update();
        let places: Vec<Vec3> = expected
            .iter()
            .map(|e| match order_of(&app, *e) {
                Order::Move(p) => p,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(places.len(), 3);
        assert!(places[0] != places[1] && places[1] != places[2]);
        assert_eq!(
            places[0],
            clamp_to_map(Vec3::ZERO + formation_offset(0, 3)),
            "the lowest entity id holds slot 0"
        );
    }

    /// **The wire did not move.** Every historical spelling still parses
    /// (`legacy_wire_commands_parse` pins that), and — the half a parse test
    /// cannot show — a sentence that uses none of the new keys still
    /// SERIALIZES to exactly the keys it always did. `bridge.rs` echoes armed
    /// triggers and set plans back into `state.json` by re-serializing the
    /// stored `Intent`, so a stray `"select":null` would appear in every
    /// snapshot of every match that never used the feature.
    #[test]
    fn the_selector_keys_are_absent_from_a_sentence_that_does_not_use_them() {
        let cases: Vec<(Intent, serde_json::Value)> = vec![
            (
                Intent::Move {
                    units: vec![7],
                    x: Some(1.0),
                    z: Some(2.0),
                    region: None,
                    select: None,
                },
                serde_json::json!({"type":"move","units":[7],"x":1.0,"z":2.0}),
            ),
            (
                Intent::Harvest {
                    units: vec![7],
                    target: Some(9),
                    select: None,
                    target_select: None,
                },
                serde_json::json!({"type":"harvest","units":[7],"target":9}),
            ),
            (
                Intent::Follow {
                    units: vec![7],
                    target: Some(9),
                    select: None,
                    target_select: None,
                },
                serde_json::json!({"type":"follow","units":[7],"target":9}),
            ),
            (
                Intent::Build {
                    worker: Some(7),
                    kind: "Farm".to_string(),
                    x: Some(1.0),
                    z: Some(2.0),
                    region: None,
                    select: None,
                    site: None,
                },
                serde_json::json!({"type":"build","worker":7,"kind":"Farm","x":1.0,"z":2.0}),
            ),
            (
                Intent::Cast {
                    hero: Some(7),
                    ability: None,
                    x: None,
                    z: None,
                    target: None,
                    select: None,
                },
                serde_json::json!({"type":"cast","hero":7}),
            ),
            (
                Intent::Squad {
                    units: vec![7],
                    id: Some(2),
                    select: None,
                },
                serde_json::json!({"type":"squad","units":[7],"id":2}),
            ),
            (
                Intent::Priority {
                    units: vec![7],
                    classes: vec!["ranged".to_string()],
                    select: None,
                },
                serde_json::json!({"type":"priority","units":[7],"classes":["ranged"]}),
            ),
            // The four building verbs, whose `building` widened to `Option` to
            // make room for the phrase. A historical command always carries it,
            // so it serializes exactly as it always did — this is the pin.
            (
                Intent::Train {
                    building: Some(7),
                    unit: "Footman".to_string(),
                    select: None,
                },
                serde_json::json!({"type":"train","building":7,"unit":"Footman"}),
            ),
            (
                Intent::Cancel {
                    building: Some(7),
                    index: 0,
                    select: None,
                },
                serde_json::json!({"type":"cancel","building":7,"index":0}),
            ),
            (
                Intent::Rally {
                    building: Some(7),
                    x: Some(1.0),
                    z: Some(2.0),
                    region: None,
                    target: None,
                    select: None,
                },
                serde_json::json!({"type":"rally","building":7,"x":1.0,"z":2.0}),
            ),
            (
                Intent::Template {
                    building: Some(7),
                    squad: Some(2),
                    retreat: None,
                    priority: None,
                    autocast: None,
                    select: None,
                },
                serde_json::json!({"type":"template","building":7,"squad":2}),
            ),
        ];
        for (intent, expected) in cases {
            let got = serde_json::to_value(&intent).unwrap();
            assert_eq!(got, expected, "serialized shape moved for {intent:?}");
            // And it round-trips: what the snapshot prints, the wire re-reads.
            let back: Intent = serde_json::from_value(got).unwrap();
            assert_eq!(
                serde_json::to_value(&back).unwrap(),
                serde_json::to_value(&intent).unwrap()
            );
        }
    }

    /// Every new spelling, parsed from the wire in the shape
    /// `tools/COMMANDER_BRIEF.md` prints it. The companion to
    /// `legacy_wire_commands_parse`, which pins the old ones.
    #[test]
    fn the_selector_forms_parse_from_the_wire() {
        let forms = [
            r#"{"type":"move","select":"my hero","x":1.0,"z":2.0}"#,
            r#"{"type":"attackmove","select":"all army","region":"north"}"#,
            r#"{"type":"attack","select":"squad 1","target":9}"#,
            r#"{"type":"harvest","select":"workers","target_select":"nearest tree"}"#,
            r#"{"type":"return","select":"workers"}"#,
            r#"{"type":"follow","select":"all army","target_select":"my hero"}"#,
            r#"{"type":"stop","select":"all units"}"#,
            r#"{"type":"build","select":"workers","kind":"Farm","x":1.0,"z":2.0,
                "site":"nearest legal site"}"#,
            r#"{"type":"cast","select":"my hero","ability":"Slam"}"#,
            r#"{"type":"priority","select":"all army","classes":["ranged"]}"#,
            r#"{"type":"retreat","select":"squad 2","below":0.35,"region":"home"}"#,
            r#"{"type":"leash","select":"squad 2","region":"home"}"#,
            r#"{"type":"autocast","select":"my hero","min_enemies":3}"#,
            r#"{"type":"squad","select":"all army","id":1}"#,
            // The building channel (`wc3clone-3ji`). Four verbs, every one of
            // which used to demand an entity id.
            r#"{"type":"train","select":"my barracks","unit":"Footman"}"#,
            r#"{"type":"train","select":"idle Barracks","unit":"Footman"}"#,
            r#"{"type":"train","select":"my hall","unit":"Worker"}"#,
            r#"{"type":"rally","select":"my barracks","x":1.0,"z":2.0}"#,
            r#"{"type":"template","select":"my barracks","squad":2}"#,
            r#"{"type":"cancel","select":"my barracks","index":0}"#,
        ];
        for form in forms {
            let parsed: Intent =
                serde_json::from_str(form).unwrap_or_else(|e| panic!("{form}: {e}"));
            // A selector-bearing sentence reads as its phrase, not as a count
            // of the ids it did not carry.
            let sentence = parsed.sentence();
            assert!(
                !sentence.contains("0 units"),
                "{form} -> {sentence}: a selector must never read as a count"
            );
        }
    }

    /// Every phrase the vocabulary advertises actually parses, in every
    /// spelling the wire tolerates. A phrase printed in a refusal that the
    /// parser then rejects is a refusal that lies.
    #[test]
    fn every_advertised_selector_phrase_parses() {
        for phrase in [
            "my hero",
            "My Hero",
            "my_hero",
            "hero",
            "all army",
            "army",
            "all units",
            "workers",
            "squad 0",
            "squad 7",
            "squad-3",
            "nearest tree",
            "nearest mine",
            "nearest legal site",
            // The building family: possessive, bare, plural, idle, and the
            // hall ladder. Every one of these is a spelling a commander writes.
            "my barracks",
            "barracks",
            "My Barracks",
            "my_barracks",
            "the barracks",
            "idle barracks",
            "my idle barracks",
            "my farms",
            "farm",
            "my hall",
            "hall",
            "idle hall",
            "my town hall",
            "townhall",
            "war mill",
        ] {
            assert!(
                parse_selector(phrase).is_some(),
                "advertised phrase does not parse: {phrase}"
            );
        }
        assert_eq!(parse_selector("squad 3"), Some(Selector::Squad(3)));
        assert_eq!(parse_selector("squad 300"), None, "a squad id is a u8");
        assert_eq!(parse_selector(""), None);
        assert_eq!(parse_selector("north-pass"), None, "a place is not a role");
    }

    // -----------------------------------------------------------------------
    // The building channel (wc3clone-3ji)
    // -----------------------------------------------------------------------

    /// The grammar, spelled out: what each phrase means, and what the open set
    /// must NOT swallow.
    #[test]
    fn the_building_grammar_folds_case_articles_and_plurals() {
        let barracks = Selector::Buildings {
            what: BuildingRef::Kind(BuildingKind::Barracks),
            idle: false,
        };
        for phrase in ["my barracks", "Barracks", "the barracks", "our BARRACKS", "a barracks"] {
            assert_eq!(parse_selector(phrase), Some(barracks), "{phrase}");
        }
        assert_eq!(
            parse_selector("my farms"),
            Some(Selector::Buildings {
                what: BuildingRef::Kind(BuildingKind::Farm),
                idle: false
            }),
            "a plural is the same building"
        );
        let idle_barracks = Selector::Buildings {
            what: BuildingRef::Kind(BuildingKind::Barracks),
            idle: true,
        };
        for phrase in ["idle barracks", "my idle barracks", "idle my barracks"] {
            assert_eq!(parse_selector(phrase), Some(idle_barracks), "{phrase}");
        }
        assert_eq!(
            parse_selector("my hall"),
            Some(Selector::Buildings { what: BuildingRef::Hall, idle: false })
        );

        // The open set is LAST, so it cannot shadow a fixed phrase. These are
        // the collisions that would hurt if it ran first.
        assert_eq!(parse_selector("mine"), Some(Selector::NearestMine));
        assert_eq!(parse_selector("hero"), Some(Selector::Heroes));
        assert_eq!(parse_selector("workers"), Some(Selector::Workers));
        assert_eq!(parse_selector("north-pass"), None);
        assert_eq!(parse_selector("nonsense"), None);

        // And the channels know each other apart, so a refusal can name the
        // right list.
        assert!(barracks.is_building_selector() && !barracks.is_unit_selector());
        assert!(!Selector::Heroes.is_building_selector());
        assert_eq!(barracks.phrase(), "my Barracks");
        assert_eq!(idle_barracks.phrase(), "idle Barracks");
    }

    /// A barracks, finished, with an empty training queue.
    fn spawn_barracks(app: &mut App, team: Team, at: Vec3) -> Entity {
        let e = spawn_hall_at(app, BuildingKind::Barracks, team, at, false);
        app.world_mut().entity_mut(e).insert(TrainingQueue::default());
        e
    }

    fn queue_of(app: &App, entity: Entity) -> Vec<UnitKind> {
        app.world()
            .entity(entity)
            .get::<TrainingQueue>()
            .map(|q| q.queue.iter().copied().collect())
            .unwrap_or_default()
    }

    fn rich(app: &mut App, team: Team) {
        let mut economies = app.world_mut().resource_mut::<Economies>();
        let e = economies.get_mut(team);
        e.gold = 5000;
        e.lumber = 5000;
        e.supply_cap = 200;
    }

    /// **The bead, in one test.** A repeating `train` rule armed against the
    /// ROLE keeps producing after the barracks it would have named is rubble.
    ///
    /// This is the production half of `a_selector_binds_to_the_hero_that_
    /// exists_when_the_rule_fires`, and it is the r23-class failure both
    /// commanders described: every cycle spent re-reading a building id out of
    /// `buildings[]`, and a rule that stopped working the moment the building
    /// died.
    #[test]
    fn a_building_selector_finds_the_producer_that_exists_when_the_rule_fires() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        arm(
            &mut app,
            Team::Human,
            r#"{"type":"trigger_set","name":"steady","repeat":5,
                "when":{"type":"base_under_attack"},
                "then":{"type":"train","select":"my barracks","unit":"Footman"}}"#,
        );
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "arming against a building the team has not built yet must not refuse"
        );

        let first = spawn_barracks(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));
        fire(&mut app, Team::Human, "steady");
        assert_eq!(queue_of(&app, first), vec![UnitKind::Footman]);

        // The barracks is razed and another goes up. A frozen id names rubble.
        app.world_mut().despawn(first);
        let second = spawn_barracks(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        app.update();
        drain_errors(&mut app, Team::Human);

        fire(&mut app, Team::Human, "steady");
        assert!(
            drain_errors(&mut app, Team::Human).is_empty(),
            "the re-fire must not report a dead building"
        );
        assert_eq!(
            queue_of(&app, second),
            vec![UnitKind::Footman],
            "the barracks built AFTER the rule was armed is the one that trains"
        );

        // And nothing was written back: the stored sentence is still the role.
        let then = app
            .world()
            .resource::<Triggers>()
            .get(Team::Human)
            .iter()
            .find(|t| t.name.as_str() == "steady")
            .unwrap()
            .then
            .clone();
        match then {
            Intent::Train { building, select, .. } => {
                assert_eq!(building, None, "resolution must not be written back");
                assert_eq!(select.as_deref(), Some("my barracks"));
            }
            other => panic!("stored then changed shape: {other:?}"),
        }
    }

    /// The documented tie-break: lowest entity id, the same rule `build`'s
    /// worker and `cast`'s caster already use. One `train`, one unit — a
    /// selector that queued at every match would turn one sentence into four.
    #[test]
    fn a_single_referent_building_selector_takes_the_lowest_id() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        let first = spawn_barracks(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));
        let second = spawn_barracks(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        assert!(first.to_bits() < second.to_bits(), "spawn order is id order here");

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"my barracks","unit":"Footman"}"#,
        ));
        app.update();
        assert!(drain_errors(&mut app, Team::Human).is_empty());
        assert_eq!(queue_of(&app, first), vec![UnitKind::Footman]);
        assert!(queue_of(&app, second).is_empty(), "one sentence, one unit");
    }

    /// `idle` is the phrase that wins games: it walks past a producer that is
    /// already working, and when they are all working it says so in words
    /// naming the fix rather than queueing six deep on one building.
    #[test]
    fn idle_walks_past_a_busy_producer_and_teaches_when_they_are_all_busy() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        let busy = spawn_barracks(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));
        let free = spawn_barracks(&mut app, Team::Human, Vec3::new(20.0, 0.0, 20.0));
        app.world_mut()
            .entity_mut(busy)
            .get_mut::<TrainingQueue>()
            .unwrap()
            .queue
            .push_back(UnitKind::Footman);

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"idle barracks","unit":"Archer"}"#,
        ));
        app.update();
        assert!(drain_errors(&mut app, Team::Human).is_empty());
        assert_eq!(
            queue_of(&app, free),
            vec![UnitKind::Archer],
            "the lowest-id match is the lowest-id IDLE match"
        );

        // Now both are busy. The refusal names the count and the way out.
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"idle barracks","unit":"Archer"}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'idle barracks' matches none of your finished buildings")
                && errors[0].contains("all 2 of your Barracks already have something queued")
                && errors[0].contains("drop 'idle'"),
            "{}",
            errors[0]
        );
    }

    /// An empty match teaches by naming what the seat DOES have — the
    /// `Regions::unknown` rule applied to buildings. Own buildings only, so it
    /// leaks nothing the snapshot did not already print.
    #[test]
    fn an_empty_building_selector_names_the_buildings_you_do_have() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        spawn_hall_at(&mut app, BuildingKind::Keep, Team::Human, Vec3::ZERO, false);
        spawn_barracks(&mut app, Team::Human, Vec3::new(5.0, 0.0, 5.0));
        spawn_barracks(&mut app, Team::Human, Vec3::new(9.0, 0.0, 9.0));
        // An enemy Workshop must not appear in our refusal.
        spawn_hall_at(
            &mut app,
            BuildingKind::Workshop,
            Team::Claude,
            Vec3::new(40.0, 0.0, 40.0),
            false,
        );
        // ...and neither must our own unfinished one.
        spawn_hall_at(
            &mut app,
            BuildingKind::Sanctum,
            Team::Human,
            Vec3::new(12.0, 0.0, 12.0),
            true,
        );

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"my workshop","unit":"Catapult"}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'my workshop' matches none of your finished buildings")
                && errors[0].contains("you have: Barracks \u{d7}2, Keep"),
            "{}",
            errors[0]
        );
        assert!(
            !errors[0].contains("Workshop") || errors[0].starts_with("cmd 1: train: 'my workshop'"),
            "the enemy's Workshop must not be in our roster: {}",
            errors[0]
        );
        assert!(
            !errors[0].contains("Sanctum"),
            "an unfinished building is not a producer: {}",
            errors[0]
        );
    }

    /// A phrase from the wrong channel earns the RIGHT list. This is the whole
    /// argument for a fourth channel rather than one widened `units`.
    #[test]
    fn a_selector_in_the_wrong_channel_names_the_list_it_should_have_come_from() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        spawn_barracks(&mut app, Team::Human, Vec3::ZERO);

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"my hero","unit":"Footman"}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'my hero' names units, not a building")
                && errors[0].contains(SELECTOR_BUILDING_NAMES),
            "{}",
            errors[0]
        );

        // ...and in the other direction.
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"move","select":"my barracks","x":1.0,"z":2.0}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'my barracks' names a building, not units")
                && errors[0].contains(SELECTOR_UNIT_NAMES),
            "{}",
            errors[0]
        );
    }

    /// **`my hall` follows the ladder.** A hall upgrades in place, so a rule
    /// that named the rung would stop matching the moment it climbed — the
    /// author-time-fact bug wearing a different hat.
    #[test]
    fn my_hall_matches_whichever_rung_is_standing() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(Team::Human, TechTier::T2);
        let keep = spawn_hall_at(&mut app, BuildingKind::Keep, Team::Human, Vec3::ZERO, false);
        app.world_mut().entity_mut(keep).insert(TrainingQueue::default());

        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"my hall","unit":"Worker"}"#,
        ));
        app.update();
        assert!(drain_errors(&mut app, Team::Human).is_empty());
        assert_eq!(queue_of(&app, keep), vec![UnitKind::Worker]);

        // The rung's own name still works, and still means only that rung.
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","select":"my town hall","unit":"Worker"}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'my town hall' matches none of your finished buildings"),
            "{}",
            errors[0]
        );
    }

    /// The other three building verbs go through the same resolver, so one
    /// test each is enough to prove the channel is wired rather than special.
    #[test]
    fn rally_template_and_cancel_take_the_same_phrase() {
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        let barracks = spawn_barracks(&mut app, Team::Human, Vec3::new(10.0, 0.0, 10.0));

        for json in [
            r#"{"type":"rally","select":"my barracks","x":5.0,"z":6.0}"#,
            r#"{"type":"template","select":"my barracks","squad":2}"#,
            r#"{"type":"train","select":"my barracks","unit":"Footman"}"#,
            r#"{"type":"cancel","select":"my barracks","index":0}"#,
        ] {
            app.world_mut().send_event(from_the_wire(Team::Human, json));
            app.update();
            let errors = drain_errors(&mut app, Team::Human);
            assert!(errors.is_empty(), "{json}: {errors:?}");
        }
        assert!(
            app.world().entity(barracks).get::<RallyPoint>().is_some(),
            "rally landed on the building the phrase named"
        );
        assert!(
            app.world().entity(barracks).get::<DoctrineTemplate>().is_some(),
            "template landed on the building the phrase named"
        );
        assert!(
            queue_of(&app, barracks).is_empty(),
            "the train queued a Footman and the cancel took it back out"
        );
    }

    /// Neither channel given is a refusal that names both ways to fix it,
    /// rather than a silent no-op — `building` widened to `Option` and
    /// something has to notice.
    #[test]
    fn a_building_verb_with_neither_id_nor_phrase_teaches() {
        let mut app = compiler_app();
        app.world_mut().send_event(from_the_wire(
            Team::Human,
            r#"{"type":"train","unit":"Footman"}"#,
        ));
        app.update();
        let errors = drain_errors(&mut app, Team::Human);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("train needs a building id or a 'select' phrase")
                && errors[0].contains(SELECTOR_BUILDING_NAMES),
            "{}",
            errors[0]
        );
    }

    // -----------------------------------------------------------------------
    // Cross-pins: the exact JSON `tools/*.py` writes (wc3clone-3xr)
    // -----------------------------------------------------------------------

    /// **The two sides of the wire pin each other.**
    ///
    /// Every string below was copied verbatim from a Python tool's own output —
    /// `tools/intent_compile.py` compiling the directives
    /// `tools/test_intent_compile.py` pins, and `tools/affordances.py` rendering
    /// the templates `tools/test_affordances.py` pins. Nothing here is a shape
    /// invented in Rust to describe what the tooling *probably* emits, which is
    /// exactly the hole this test closes: the NL compiler and the affordance
    /// document are the biggest producers of bridge traffic in the project and
    /// no Rust test crossed the wire with either of them.
    ///
    /// A form template legitimately carries `null` in its judgment-shaped
    /// holes — the commander fills them in — so those are filled here with the
    /// value a commander would send, and the KEY SET is what is being pinned.
    ///
    /// If this test starts failing, read it as "a tool now writes something the
    /// engine does not accept" and fix whichever side is wrong. Do not delete
    /// the line.
    #[test]
    fn the_json_the_tooling_writes_parses_and_compiles() {
        let cases: &[(&str, &str)] = &[
            // -- tools/intent_compile.py -----------------------------------
            // "hold mid with the cavalry" — a squad enrolment followed by the
            // posture. The pair is why `squad` exists in this list at all.
            (
                "hold mid with the cavalry",
                r#"{"type": "squad", "units": [4294968130, 4294968131, 4294968132], "id": 1}"#,
            ),
            (
                "hold mid with the cavalry",
                r#"{"type": "posture", "id": 1, "posture": {"type": "defend", "x": 0.0, "z": 0.0, "radius": 18.0}}"#,
            ),
            (
                "harvest lumber",
                r#"{"type": "harvest", "select": "workers", "target_select": "nearest tree"}"#,
            ),
            (
                "build a farm at our base",
                r#"{"type": "build", "kind": "Farm", "x": 62.0, "z": 62.0, "worker": 4294968100, "site": "nearest legal site"}"#,
            ),
            ("squad 1 stages", r#"{"type": "stance", "squad": 1, "stance": "stage"}"#),
            (
                "squad 1 turtles at our base",
                r#"{"type": "stance", "squad": 1, "stance": "turtle", "x": 70.0, "z": 70.0}"#,
            ),
            (
                "retreat at 35%",
                r#"{"type": "retreat", "select": "all army", "below": 0.35, "x": 70.0, "z": 70.0}"#,
            ),
            (
                "autocast at 3",
                r#"{"type": "autocast", "select": "my hero", "min_enemies": 3}"#,
            ),
            (
                "focus siege > cavalry",
                r#"{"type": "priority", "select": "all army", "classes": ["Siege", "Cavalry"]}"#,
            ),
            (
                "when my base is attacked, squad 1 defends our base",
                r#"{"type": "trigger_set", "name": "base-attacked", "when": {"type": "base_under_attack"}, "then": {"type": "posture", "id": 1, "posture": {"type": "defend", "x": 70.0, "z": 70.0, "radius": 18.0}}}"#,
            ),
            // The `then` that carries a SELECT — the shape the tool writes for
            // every rule since it stopped freezing ids.
            (
                "when my hero drops below 30%, fall back at 50% to our base",
                r#"{"type": "trigger_set", "name": "hero-30", "when": {"type": "hero_below", "frac": 0.3}, "then": {"type": "retreat", "select": "all army", "below": 0.5, "x": 70.0, "z": 70.0}}"#,
            ),
            // ...and the building selector reaching the same channel.
            (
                "whenever my base is attacked, train a footman",
                r#"{"type": "trigger_set", "name": "base-attacked", "when": {"type": "base_under_attack"}, "then": {"type": "train", "select": "idle Barracks", "unit": "Footman"}, "repeat": 45.0}"#,
            ),
            (
                "train a footman (immediate)",
                r#"{"type": "train", "building": 4294968202, "unit": "Footman"}"#,
            ),
            // -- tools/affordances.py templates, holes filled --------------
            (
                "affordance form: squad",
                r#"{"type": "squad", "select": "all army", "id": 1}"#,
            ),
            (
                "affordance form: build",
                r#"{"type": "build", "select": "workers", "kind": "Farm", "region": "home", "site": "nearest legal site"}"#,
            ),
            (
                "affordance form: train:Barracks",
                r#"{"type": "train", "select": "my Barracks", "unit": "Footman"}"#,
            ),
            (
                "affordance form: train:TownHall",
                r#"{"type": "train", "select": "idle TownHall", "unit": "Worker"}"#,
            ),
            (
                "affordance form: stance",
                r#"{"type": "stance", "squad": 1, "stance": "push", "target": "home"}"#,
            ),
            (
                "affordance recipe: hero-save",
                r#"{"type": "trigger_set", "name": "hero-save", "repeat": 45, "when": {"type": "hero_below", "frac": 0.35}, "then": {"type": "move", "select": "my hero", "region": "home"}}"#,
            ),
            (
                "affordance recipe: expand",
                r#"{"type": "trigger_set", "name": "expand", "when": {"type": "mine_dry"}, "then": {"type": "build", "select": "workers", "kind": "TownHall", "region": "home", "site": "nearest legal site"}}"#,
            ),
            (
                "affordance recipe: steady-production",
                r#"{"type": "trigger_set", "name": "steady-production", "repeat": 20, "when": {"type": "unit_count", "kind": "Footman", "count": 6}, "then": {"type": "train", "select": "idle TownHall", "unit": "Worker"}}"#,
            ),
        ];

        // Half one: every string is an `Intent` and re-serializes under its own
        // verb, which is what the replay log and the snapshot echo depend on.
        for (source, json) in cases {
            let parsed: Intent = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("{source}: {json} failed to parse: {e}"));
            assert!(!parsed.sentence().is_empty(), "{source}: no sentence");
            let round: serde_json::Value = serde_json::to_value(&parsed).unwrap();
            assert_eq!(
                round.get("type").and_then(|v| v.as_str()),
                Some(parsed.verb()),
                "{source}: re-serialized under a different tag"
            );
        }

        // Half two: every string COMPILES against a world shaped like the one
        // the tool was reading — a hall, two barracks, a hero, workers, an
        // army, a squad, a tree, a mine and a named region. Parsing proves the
        // shape; compiling proves the sentence does something.
        let mut app = compiler_app();
        rich(&mut app, Team::Human);
        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(Team::Human, TechTier::T2);
        let hall = spawn_hall_at(&mut app, BuildingKind::TownHall, Team::Human, Vec3::ZERO, false);
        app.world_mut().entity_mut(hall).insert(TrainingQueue::default());
        spawn_barracks(&mut app, Team::Human, Vec3::new(6.0, 0.0, 6.0));
        spawn_barracks(&mut app, Team::Human, Vec3::new(9.0, 0.0, 9.0));
        spawn_hero(&mut app, Team::Human, Vec3::new(2.0, 0.0, 2.0));
        let worker = spawn_worker(&mut app, Team::Human, Vec3::new(3.0, 0.0, 3.0));
        let footman = spawn_footman(&mut app, Team::Human, Vec3::new(4.0, 0.0, 4.0));
        spawn_node(&mut app, ResourceKind::Lumber, Vec3::new(8.0, 0.0, 0.0), 500);
        spawn_node(&mut app, ResourceKind::Gold, Vec3::new(0.0, 0.0, 8.0), 500);
        app.world_mut()
            .entity_mut(footman)
            .insert(SquadId(1));
        app.update();
        region_set(&mut app, Team::Human, "home", 0.0, 0.0, 18.0);
        drain_errors(&mut app, Team::Human);

        // The tool's ids belong to the fixture's world, not to this one, so the
        // two id-bearing cases are re-pointed at this world's equivalents. The
        // KEY SET — which is what the cross-pin is about — is untouched.
        let repoint = |json: &str| {
            json.replace("4294968130", &intent_id(footman).to_string())
                .replace("4294968131", &intent_id(footman).to_string())
                .replace("4294968132", &intent_id(footman).to_string())
                .replace("4294968100", &intent_id(worker).to_string())
                .replace("4294968202", &intent_id(hall).to_string())
        };
        let mut queues = app.world_mut().query::<&mut TrainingQueue>();
        for (source, json) in cases {
            // Each case is checked against the SAME world, so the ones that
            // queue a unit must not decide whether `idle <kind>` still matches
            // for the ones after them. This is a shape pin, not a scenario.
            for mut queue in queues.iter_mut(app.world_mut()) {
                queue.queue.clear();
            }
            let json = repoint(json);
            // The hall trains Workers, not Footmen: the one case whose verb
            // depends on the fixture's Barracks becomes the hall's own unit.
            let json = if source == &"train a footman (immediate)" {
                json.replace("\"Footman\"", "\"Worker\"")
            } else {
                json
            };
            app.world_mut()
                .send_event(from_the_wire(Team::Human, &json));
            app.update();
            let errors = drain_errors(&mut app, Team::Human);
            assert!(errors.is_empty(), "{source}: {json} refused: {errors:?}");
        }
    }
}
