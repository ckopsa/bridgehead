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
//! **No player-facing mutation path exists except intent submission.** That is
//! what makes THESIS.md's structural claim checkable rather than aspirational:
//! the AI cannot act in ways the human cannot, and — the half we had been
//! failing — the human cannot be denied a verb the AI has, because there is one
//! list of verbs and one compiler reading it.
//!
//! Two things are deliberately *not* players and stay as they are:
//!
//!   * **Engine systems.** economy.rs's harvest follow-through and payments,
//!     combat.rs's chase, doctrine.rs's squad re-tasking and retreat triggers
//!     are the engine executing standing policy at machine speed. They write
//!     `Order`s directly and always will — that asymmetry *is* the tempo design
//!     (see docs/TEMPO.md §C4).
//!   * **The scripted `ai.rs`.** It is engine baseline, not a seat: it still
//!     writes `Order`s, queue pushes and `UpgradeBuilding` directly. This is a
//!     known asymmetry, documented in docs/INTENT.md, and the natural next bead.
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
//! (`WC3_INTENT_LOG`, default `bridge/intent_log.jsonl`) as a human-readable
//! sentence *and* its serialized form. The sentence does not record how the
//! intent was spelled, which is the point: a replay reads identically whether
//! the match was played with a mouse or with JSON.

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
const INTENT_LOG_ENV: &str = "WC3_INTENT_LOG";
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
            // `IntentErrors` is: bridge.rs reads both, but this file is the
            // only thing that ever writes them.
            .init_resource::<IntentJournal>()
            .init_resource::<UiNotices>()
            .insert_resource(IntentLog::from_env())
            // `.after(FogSet)`: an intent is judged against the visibility its
            // issuer has right now, the same grid the snapshot and the HUD are
            // about to show them.
            .add_systems(Update, apply_intents.in_set(IntentApply).after(FogSet));
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
    ),
>;

type IntentBuildings<'w, 's> = Query<
    'w,
    's,
    (
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

type IntentNodes<'w, 's> = Query<'w, 's, &'static ResourceNode>;

/// Forges currently working. A separate query rather than a sixth column on
/// `IntentBuildings` on purpose: `research` is the only verb that asks, and
/// widening the shared tuple would rewrite every `buildings.get` destructure
/// in this file for one caller's benefit.
type IntentResearching<'w, 's> = Query<'w, 's, &'static Researching>;

#[derive(SystemParam)]
pub struct IntentWorld<'w, 's> {
    units: IntentUnits<'w, 's>,
    buildings: IntentBuildings<'w, 's>,
    targets: IntentTargets<'w, 's>,
    nodes: IntentNodes<'w, 's>,
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
    nav: Res<'w, NavGrid>,
    fog: Res<'w, FogGrids>,
    team_research: Res<'w, TeamResearch>,
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
            let message = format!("{UI_NOTICE_PREFIX}: {body}");
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
    mut squad_orders: ResMut<SquadOrders>,
    mut ai_controlled: ResMut<AiControlled>,
    mut error_log: ResMut<IntentErrors>,
    mut log: ResMut<IntentLog>,
    mut journal: ResMut<IntentJournal>,
    mut feed: ResMut<GameEvents>,
    mut notices: ResMut<UiNotices>,
    mut events: IntentEvents,
    mut world: IntentWorld,
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
    for submission in batch {
        let mut errors: Vec<String> = Vec::new();
        compile_intent(
            submission.intent.clone(),
            submission.team,
            &submission.tag,
            // Who is speaking and when. Every order this call mints stamps
            // itself with this, so a unit can name the sentence that moved it.
            IntentMark {
                source: submission.source,
                at: now,
            },
            &mut errors,
            &mut ai_controlled,
            &tables.economies,
            &tables.records,
            // The issuing team's tech tier: what hero slots it has open.
            tables.tiers.get(submission.team),
            &tables.nav,
            &tables.team_research,
            // The issuer's own fog: what *they* can see decides what they may
            // order, and neither seat gets to borrow the other's eyes.
            tables.fog.get(submission.team),
            &mut squad_orders,
            &mut commands,
            &mut events,
            &mut world,
        );
        log.record(now, &submission, &errors);
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
            notices.raise(
                &mut feed,
                submission.team,
                now,
                &submission.tag,
                &errors,
                &mut notice_budget,
            );
        }
        let sink = error_log.get_mut(submission.team);
        sink.extend(errors);
        if sink.len() > MAX_ERRORS {
            let overflow = sink.len() - MAX_ERRORS;
            sink.drain(..overflow);
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
    nav: &NavGrid,
    team_research: &TeamResearch,
    fog: &FogGrid,
    squad_orders: &mut SquadOrders,
    commands: &mut Commands,
    events: &mut IntentEvents,
    world: &mut IntentWorld,
) {
    // Named locally so the arms below read exactly as they did when this was
    // one interface's private applier.
    let IntentWorld {
        units,
        buildings,
        targets,
        nodes,
        researching,
    } = world;
    match intent {
        Intent::Move { units: ids, x, z } => {
            ground_order(
                commands,
                errors,
                tag,
                mark.order("move"),
                &ids,
                units,
                me,
                Vec3::new(x, 0.0, z),
                false,
            );
        }
        Intent::AttackMove { units: ids, x, z } => {
            ground_order(
                commands,
                errors,
                tag,
                mark.order("attackmove"),
                &ids,
                units,
                me,
                Vec3::new(x, 0.0, z),
                true,
            );
        }
        Intent::Attack { units: ids, target } => {
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
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
                commands
                    .entity(entity)
                    .try_insert((Order::Attack(target_entity), mark.order("attack")));
            }
        }
        Intent::Harvest { units: ids, target } => {
            // Resource nodes are neutral: either seat may harvest any of
            // them.
            let node = match intent_entity(target).filter(|e| nodes.get(*e).is_ok()) {
                Some(node) => node,
                None => {
                    errors.push(format!("{tag}: resource node {target} not found"));
                    return;
                }
            };
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
                // Only workers can gather; anyone else would just stand there.
                if !is_worker(units, entity) {
                    errors.push(format!(
                        "{tag}: unit {} is not a Worker",
                        entity.to_bits()
                    ));
                    continue;
                }
                commands
                    .entity(entity)
                    .try_insert((Order::Harvest(node), mark.order("harvest")));
            }
        }
        Intent::Return { units: ids } => {
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
                commands
                    .entity(entity)
                    .try_insert((Order::ReturnResources, mark.order("return")));
            }
        }
        Intent::Follow { units: ids, target } => {
            let leader = match intent_entity(target) {
                Some(e) => match units.get(e) {
                    Ok((_, _, team, _, _)) if *team == me => e,
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
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
                if entity == leader {
                    continue; // a unit following itself would deadlock its own order
                }
                commands
                    .entity(entity)
                    .try_insert((Order::Follow(leader), mark.order("follow")));
            }
        }
        Intent::Stop { units: ids } => {
            // The established Stop: re-issue a Move to the unit's own spot,
            // which halts it and clears any attack target.
            for (entity, pos) in own_units(&ids, units, me, tag, errors) {
                commands
                    .entity(entity)
                    .try_insert((Order::Move(pos), mark.order("stop")));
            }
        }
        Intent::Build {
            worker,
            kind,
            x,
            z,
        } => {
            let Some(building_kind) = parse_building_kind(&kind) else {
                errors.push(format!("{tag}: unknown building kind '{kind}'"));
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
            let pos = snap_footprint(clamp_to_map(Vec3::new(x, 0.0, z)), stats.size);
            if !nav.rect_is_free(pos, stats.size) {
                errors.push(format!(
                    "{tag}: site ({:.1}, {:.1}) is blocked for {kind}",
                    pos.x, pos.z
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
            commands.entity(entity).try_insert((
                Order::Build {
                    kind: building_kind,
                    pos,
                },
                mark.order("build"),
            ));
        }
        Intent::Upgrade { building } => {
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((b, team, under, _, upgrading)) = buildings.get(entity) else {
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
        Intent::Train { building, unit } => {
            let Some(kind) = parse_unit_kind(&unit) else {
                errors.push(format!("{tag}: unknown unit kind '{unit}'"));
                return;
            };
            // Read the tech state before taking the mutable borrow of the
            // producing building below.
            let completed = completed_kinds(buildings, me);
            if let Some(err) =
                requirement_error(tag, kind_name(kind), unit_requires(kind), &completed)
            {
                errors.push(err);
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
            if is_hero_kind(kind) {
                let mut held: Vec<UnitKind> = units
                    .iter()
                    .filter(|(_, u, t, _, _)| **t == me && is_hero_kind(u.kind))
                    .map(|(_, u, _, _, _)| u.kind)
                    .collect();
                for (_, b_team, _, b_queue, _) in buildings.iter() {
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
            let Ok((b, team, under, queue, _)) = buildings.get_mut(entity) else {
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
                    "{tag}: {} cannot train {unit}",
                    building_name(b.kind)
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
            // Hero classes are priced by `hero_train_cost` (full, then
            // revival) — every hero kind, not just the Champion: pricing
            // the Priestess off her raw stats let a seat buy a revival at
            // full price (or worse, a first hero cheaply) depending on the
            // record. `is_hero_kind` is the same test economy.rs charges by.
            let (cost_gold, cost_lumber) = if is_hero_kind(kind) {
                let (g, l, _) = hero_train_cost(records, me, kind);
                (g, l)
            } else {
                let s = unit_stats(kind);
                (s.cost_gold, s.cost_lumber)
            };
            if !economies.get(me).can_afford(cost_gold, cost_lumber) {
                errors.push(format!(
                    "{tag}: cannot afford {unit} ({cost_gold}g {cost_lumber}l)"
                ));
                return;
            }
            // Gate only — economy.rs deducts when training starts.
            queue.queue.push_back(kind);
        }
        Intent::Cancel { building, index } => {
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, team, _, queue, _)) = buildings.get_mut(entity) else {
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
            let Ok((b, team, under, _, upgrading)) = buildings.get(entity) else {
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
        } => {
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((b, team, _, _, _)) = buildings.get(entity) else {
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
                        Ok((_, _, team, _, _)) if *team == me => Some(RallyTarget::Unit(e)),
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
        Intent::Cast { hero, ability } => {
            let Some(entity) = intent_entity(hero) else {
                errors.push(format!("{tag}: caster {hero} not found/not yours"));
                return;
            };
            // A caster is either one of our heroes (any class — the Hero
            // component and the unit ability table agree on which kinds
            // have one) or one of our finished buildings with an ability.
            // combat.rs owns the unlock/mana/cooldown verdict either way,
            // exactly as it does for the R and C hotkeys.
            let unit_list = match units.get(entity) {
                Ok((_, u, team, _, _)) if *team == me => abilities_of_unit(u.kind),
                _ => &[][..],
            };
            let list = if !unit_list.is_empty() {
                unit_list
            } else {
                match buildings.get(entity) {
                    Ok((b, team, under, _, _)) if *team == me => {
                        if under.is_some() {
                            errors.push(format!(
                                "{tag}: building {hero} is under construction"
                            ));
                            return;
                        }
                        let list = abilities_of_building(b.kind);
                        if list.is_empty() {
                            errors.push(format!(
                                "{tag}: {} has no ability",
                                building_name(b.kind)
                            ));
                            return;
                        }
                        list
                    }
                    _ => {
                        errors.push(format!(
                            "{tag}: caster {hero} is not a hero or an own ability building"
                        ));
                        return;
                    }
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
            events.casts.write(CastAbility { caster: entity, ability: selector });
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
            let Ok((b, team, under, _, _)) = buildings.get(entity) else {
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
                    .filter(|(_, team, under, _, _)| **team == me && under.is_none())
                    .map(|(b, _, _, _, _)| b.kind),
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
        Intent::UseItem { slot, hero } => {
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
            // combat.rs checks the slot is actually filled.
            events.item_uses.write(UseItem { hero, slot });
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
        Intent::Priority {
            units: ids,
            classes,
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
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
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
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
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
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
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
        } => {
            let min_enemies = min_enemies.unwrap_or(0);
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
                // Any CASTER can auto-cast — heroes were merely the only ones
                // that existed when this verb was written. The gate is "does
                // this kind have an ability list", which is the same question
                // `Intent::Cast` already asks.
                let Ok((_, unit, _, _, policy)) = units.get(entity) else {
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
            }
        }
        Intent::Squad { units: ids, id } => {
            for (entity, _) in own_units(&ids, units, me, tag, errors) {
                let mut ec = commands.entity(entity);
                match id {
                    Some(id) => {
                        ec.try_insert(SquadId(id));
                    }
                    None => {
                        ec.try_remove::<SquadId>();
                    }
                }
            }
        }
        Intent::Posture { id, posture } => {
            // Squad ids are per-team, so red's squad 1 and blue's squad 1
            // are different squads.
            let posture = match posture {
                None => {
                    // Clearing a posture leaves membership intact: the squad
                    // simply stops being re-tasked.
                    squad_orders.0.remove(&(me, id));
                    return;
                }
                Some(PostureIntent::Defend { x, z, radius }) => {
                    if !(radius > 0.0) {
                        errors.push(format!(
                            "{tag}: defend radius must be > 0, got {radius}"
                        ));
                        return;
                    }
                    SquadPosture::Defend {
                        pos: clamp_to_map(Vec3::new(x, 0.0, z)),
                        radius,
                    }
                }
                Some(PostureIntent::Push { x, z }) => SquadPosture::Push {
                    pos: clamp_to_map(Vec3::new(x, 0.0, z)),
                },
                Some(PostureIntent::Forage { x, z }) => SquadPosture::Forage {
                    muster: clamp_to_map(Vec3::new(x, 0.0, z)),
                },
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
        Intent::Template {
            building,
            squad,
            retreat,
            priority,
            autocast,
        } => {
            // Only our own, finished, unit-producing buildings can carry a
            // template — anywhere else it would never be read.
            let Some(entity) = intent_entity(building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((b, team, under, queue, _)) = buildings.get(entity) else {
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
    /// The half a person reads.
    sentence: String,
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
    /// `WC3_INTENT_LOG` nor the filesystem, and leave no file behind.
    #[cfg(test)]
    pub fn disabled() -> Self {
        IntentLog {
            path: None,
            file: None,
            broken: false,
        }
    }

    fn record(&mut self, now: f32, submission: &SubmitIntent, errors: &[String]) {
        if self.path.is_none() || self.broken {
            return;
        }
        let record = IntentRecord {
            wall_ms: wall_ms(),
            t: (now * 10.0).round() / 10.0,
            team: team_name(submission.team),
            source: submission.source.name(),
            tag: &submission.tag,
            verb: submission.intent.verb(),
            sentence: submission.intent.sentence(),
            why: submission.intent.provenance_verb().map(|verb| {
                IntentMark {
                    source: submission.source,
                    at: now,
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
            // Point WC3_INTENT_LOG somewhere unique to keep a series.
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

/// Resolve one id to a living unit of the seat's own team.
fn own_unit(id: IntentId, units: &IntentUnits, me: Team) -> Option<(Entity, Vec3)> {
    let entity = intent_entity(id)?;
    match units.get(entity) {
        Ok((_, _, team, tf, _)) if *team == me => Some((entity, tf.translation)),
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
        .filter(|(_, u, team, _, _)| **team == me && is_hero_kind(u.kind))
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
    out
}

/// The seat's completed (not under construction) buildings — the input to
/// every requirement check on the command path.
fn completed_kinds(buildings: &IntentBuildings, me: Team) -> Vec<BuildingKind> {
    buildings
        .iter()
        .filter(|(_, team, under, _, _)| **team == me && under.is_none())
        .map(|(building, ..)| building.kind)
        .collect()
}

fn is_worker(units: &IntentUnits, entity: Entity) -> bool {
    matches!(units.get(entity), Ok((_, u, _, _, _)) if u.kind == UnitKind::Worker)
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
) {
    let group = own_units(ids, units, me, tag, errors);
    let count = group.len();
    for (i, (entity, _)) in group.into_iter().enumerate() {
        let p = clamp_to_map(ground + formation_offset(i, count));
        let order = if attack_move {
            Order::AttackMove(p)
        } else {
            Order::Move(p)
        };
        commands.entity(entity).try_insert((order, why));
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

    /// An app running the real compiler against a real (if bare) world.
    ///
    /// Everything `apply_intents` reads, defaulted, plus the five event
    /// channels it writes. `IntentLog` is replaced with a disabled one after
    /// the plugin installs it: a unit test must not depend on `WC3_INTENT_LOG`
    /// and must not leave a file behind.
    fn compiler_app() -> App {
        let mut app = App::new();
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
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .add_plugins(IntentPlugin);
        app.insert_resource(IntentLog::disabled());
        // Pin the fog mode: the ambient `WC3_FOG` must not decide an outcome.
        app.insert_resource(FogGrids::test_dark());
        app
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
                x: 12.0,
                z: -4.0,
            },
        ));
        app.world_mut().send_event(SubmitIntent {
            team: Team::Human,
            source: IntentSource::Copilot,
            tag: "cmd 0".to_string(),
            intent: Intent::Squad {
                units: vec![soldier.to_bits()],
                id: Some(1),
            },
        });
        app.update();

        let journal = app.world().resource::<IntentJournal>();
        let human: Vec<(&str, &str)> = journal
            .get(Team::Human)
            .iter()
            .map(|e| (e.source.name(), e.sentence.as_str()))
            .collect();
        assert_eq!(
            human,
            vec![
                ("ui", format!("move unit {} to (12.0, -4.0)", soldier.to_bits()).as_str()),
                ("copilot", format!("{} join squad 1", format_args!("unit {}", soldier.to_bits())).as_str()),
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
                    x: 0.0,
                    z: 0.0,
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
                worker: soldier.to_bits(),
                kind: "Nonsense".to_string(),
                x: 0.0,
                z: 0.0,
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
            },
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
            r#"{"type":"buy","shop":1,"item":"HealingPotion"}"#,
            r#"{"type":"use_item","slot":0}"#,
            r#"{"type":"autopilot","on":true}"#,
            r#"{"type":"surrender"}"#,
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
            x: 12.5,
            z: -30.5,
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

        let use_addressed = Intent::UseItem { slot: 0, hero: Some(42) };
        let back: Intent =
            serde_json::from_str(&serde_json::to_string(&use_addressed).unwrap()).unwrap();
        assert_eq!(back.sentence(), "hero 42 uses item in slot 0");

        // Omitting it must not serialize a null — the wire shape is unchanged
        // for every command that does not care.
        let plain = Intent::UseItem { slot: 0, hero: None };
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
            radius: Some(18.0),
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
            hero: 5,
            ability: Some(AbilitySelector::Index(1)),
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
}
