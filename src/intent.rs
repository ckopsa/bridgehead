//! intent.rs — the one place a player's intent becomes game state, and the
//! replay log that records it.
//!
//! `shared::Intent` is the vocabulary; this file is the grammar and the only
//! speaker. Both player-facing interfaces write `SubmitIntent` events and
//! nothing else:
//!
//!   * `ui.rs` compiles mouse gestures, hotkeys and command-card buttons into
//!     `Intent` values — a right-click on an enemy is an `Intent::Attack`, the
//!     `V` key is an `Intent::Retreat` with the parameters the gesture implies.
//!   * `bridge.rs` deserializes `commands.json` straight into `Intent` values;
//!     the wire format *is* the schema, so the protocol did not change when the
//!     compiler moved here.
//!
//! Everything downstream of this file is unchanged: the compiler writes the
//! same `Order` components, `TrainingQueue` pushes, `RallyPoint`s, doctrine
//! components, `SquadOrders` entries and `CastAbility`/`BuyItem`/`UseItem`/
//! `Surrender` events that `bridge.rs::apply_batch` used to write inline.
//! units.rs, combat.rs, economy.rs and doctrine.rs cannot tell the difference.
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
//!   * **Engine systems.** economy.rs's harvest follow-through, combat.rs's
//!     chase, doctrine.rs's squad re-tasking and retreat triggers are the
//!     engine executing standing policy at machine speed. They write `Order`s
//!     directly and always will — that asymmetry *is* the tempo design
//!     (see docs/TEMPO.md §C4).
//!   * **The scripted `ai.rs`.** It is engine baseline, not a seat: it still
//!     writes `Order`s and `TrainingQueue` pushes directly. This is a known
//!     asymmetry, documented in docs/INTENT.md, and the natural next bead.
//!
//! ## Ordering
//!
//! `apply_intents` runs in the `IntentApply` set. bridge.rs orders its poll
//! before it and its snapshot after it, so a batch read this frame is applied
//! this frame and its validation errors ride out in the same snapshot — the
//! behaviour the protocol has always had.
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
            .insert_resource(IntentLog::from_env())
            .add_systems(Update, apply_intents.in_set(IntentApply));
    }
}

// ---------------------------------------------------------------------------
// The world the compiler is allowed to touch
// ---------------------------------------------------------------------------

/// Entity first so a seat's own hero can be *found*, not just checked — `buy`
/// and `use_item` name no unit and infer it from the team.
#[derive(SystemParam)]
pub struct IntentWorld<'w, 's> {
    units: Query<'w, 's, (Entity, &'static Unit, &'static Team, &'static Transform)>,
    buildings: Query<
        'w,
        's,
        (
            &'static Building,
            &'static Team,
            Option<&'static UnderConstruction>,
            Option<&'static mut TrainingQueue>,
        ),
    >,
    /// Anything that can be attacked: a live unit or building with a team.
    targets: Query<
        'w,
        's,
        (
            &'static Team,
            Option<&'static Unit>,
            Option<&'static Building>,
        ),
    >,
    nodes: Query<'w, 's, &'static ResourceNode>,
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
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    nav: Res<NavGrid>,
    mut squad_orders: ResMut<SquadOrders>,
    mut ai_controlled: ResMut<AiControlled>,
    mut error_log: ResMut<IntentErrors>,
    mut log: ResMut<IntentLog>,
    mut casts: EventWriter<CastAbility>,
    mut buys: EventWriter<BuyItem>,
    mut item_uses: EventWriter<UseItem>,
    mut world: IntentWorld,
) {
    // Owned copies: the compiler needs `&mut` on resources the reader borrows
    // from, and a batch is a handful of values.
    let batch: Vec<SubmitIntent> = submissions.read().cloned().collect();
    if batch.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    for submission in batch {
        let mut errors: Vec<String> = Vec::new();
        compile_intent(
            &submission.intent,
            submission.team,
            &submission.tag,
            &mut errors,
            &mut ai_controlled,
            &economies,
            &records,
            &nav,
            &mut squad_orders,
            &mut commands,
            &mut casts,
            &mut buys,
            &mut item_uses,
            &mut world,
        );
        log.record(now, &submission, &errors);
        let sink = error_log.get_mut(submission.team);
        sink.extend(errors);
        if sink.len() > MAX_ERRORS {
            let overflow = sink.len() - MAX_ERRORS;
            sink.drain(..overflow);
        }
    }
}

/// Apply one intent. `me` is the issuing team: every ownership check, economy
/// read and squad key below is taken against it, so the same code runs for a
/// human gesture and a bridge command without either being able to touch the
/// other faction.
///
/// Errors are appended rather than returned so that a partially-valid intent
/// (six live units and one corpse) still does what it can, exactly as the
/// bridge always did.
#[allow(clippy::too_many_arguments)]
fn compile_intent(
    intent: &Intent,
    me: Team,
    tag: &str,
    errors: &mut Vec<String>,
    ai_controlled: &mut AiControlled,
    economies: &Economies,
    records: &HeroRecords,
    nav: &NavGrid,
    squad_orders: &mut SquadOrders,
    commands: &mut Commands,
    casts: &mut EventWriter<CastAbility>,
    buys: &mut EventWriter<BuyItem>,
    item_uses: &mut EventWriter<UseItem>,
    world: &mut IntentWorld,
) {
    match intent {
        Intent::Move { units: ids, x, z } => {
            ground_order(
                commands,
                errors,
                tag,
                ids,
                world,
                me,
                Vec3::new(*x, 0.0, *z),
                false,
            );
        }
        Intent::AttackMove { units: ids, x, z } => {
            ground_order(
                commands,
                errors,
                tag,
                ids,
                world,
                me,
                Vec3::new(*x, 0.0, *z),
                true,
            );
        }
        Intent::Attack { units: ids, target } => {
            let Some(target_entity) = intent_entity(*target) else {
                errors.push(format!("{tag}: target {target} not found"));
                return;
            };
            match world.targets.get(target_entity) {
                Ok((team, unit, building)) => {
                    // Only the issuer's enemy is a legal attack target.
                    if *team != me.enemy() {
                        errors.push(format!("{tag}: target {target} is your own"));
                        return;
                    }
                    if unit.is_none() && building.is_none() {
                        errors.push(format!("{tag}: target {target} is not attackable"));
                        return;
                    }
                }
                Err(_) => {
                    errors.push(format!("{tag}: target {target} not found"));
                    return;
                }
            }
            for (entity, _) in own_units(ids, world, me, tag, errors) {
                commands
                    .entity(entity)
                    .try_insert(Order::Attack(target_entity));
            }
        }
        Intent::Harvest { units: ids, target } => {
            // Resource nodes are neutral: either team may harvest any of them.
            let node = match intent_entity(*target).filter(|e| world.nodes.get(*e).is_ok()) {
                Some(node) => node,
                None => {
                    errors.push(format!("{tag}: resource node {target} not found"));
                    return;
                }
            };
            for (entity, _) in own_units(ids, world, me, tag, errors) {
                // Only workers can gather; anyone else would just stand there.
                if !is_worker(world, entity) {
                    errors.push(format!("{tag}: unit {} is not a Worker", entity.to_bits()));
                    continue;
                }
                commands.entity(entity).try_insert(Order::Harvest(node));
            }
        }
        Intent::Return { units: ids } => {
            for (entity, _) in own_units(ids, world, me, tag, errors) {
                commands.entity(entity).try_insert(Order::ReturnResources);
            }
        }
        Intent::Follow { units: ids, target } => {
            let leader = match intent_entity(*target) {
                Some(e) => match world.units.get(e) {
                    Ok((_, _, team, _)) if *team == me => e,
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
            for (entity, _) in own_units(ids, world, me, tag, errors) {
                if entity == leader {
                    continue; // a unit following itself would deadlock its own order
                }
                commands.entity(entity).try_insert(Order::Follow(leader));
            }
        }
        Intent::Stop { units: ids } => {
            // The established Stop: re-issue a Move to the unit's own spot,
            // which halts it and clears any attack target.
            for (entity, pos) in own_units(ids, world, me, tag, errors) {
                commands.entity(entity).try_insert(Order::Move(pos));
            }
        }
        Intent::Build {
            worker,
            kind,
            x,
            z,
        } => {
            let Some(building_kind) = parse_building_kind(kind) else {
                errors.push(format!("{tag}: unknown building kind '{kind}'"));
                return;
            };
            let Some((entity, _)) = own_unit(*worker, world, me) else {
                errors.push(format!("{tag}: unit {worker} not found/not yours"));
                return;
            };
            if !is_worker(world, entity) {
                errors.push(format!("{tag}: unit {worker} is not a Worker"));
                return;
            }
            // Same tech gate economy.rs applies at placement — reported here so
            // the player learns why instead of watching a worker walk out and
            // come back empty-handed.
            if let Some(err) = requirement_error(
                tag,
                building_name(building_kind),
                building_requires(building_kind),
                &completed_kinds(world, me),
            ) {
                errors.push(err);
                return;
            }
            let stats = building_stats(building_kind);
            // Snap to nav-cell boundaries exactly like the placement ghost.
            let pos = snap_footprint(clamp_to_map(Vec3::new(*x, 0.0, *z)), stats.size);
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
            // economy.rs pays when the worker reaches the site.
            commands.entity(entity).try_insert(Order::Build {
                kind: building_kind,
                pos,
            });
        }
        Intent::Train { building, unit } => {
            let Some(kind) = parse_unit_kind(unit) else {
                errors.push(format!("{tag}: unknown unit kind '{unit}'"));
                return;
            };
            // Read the tech state before taking the mutable borrow of the
            // producing building below.
            let completed = completed_kinds(world, me);
            if let Some(err) = requirement_error(tag, kind_name(kind), unit_requires(kind), &completed)
            {
                errors.push(err);
                return;
            }
            let Some(entity) = intent_entity(*building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((b, team, under, queue)) = world.buildings.get_mut(entity) else {
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
                errors.push(format!("{tag}: {} cannot train {unit}", building_name(b.kind)));
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
            // The Champion is priced by `hero_train_cost` (full, then revival).
            let (cost_gold, cost_lumber) = if kind == UnitKind::Hero {
                let (g, l, _) = hero_train_cost(records, me);
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
            let index = *index;
            let Some(entity) = intent_entity(*building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((_, team, _, queue)) = world.buildings.get_mut(entity) else {
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
        Intent::Rally {
            building,
            x,
            z,
            target,
        } => {
            let Some(entity) = intent_entity(*building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((b, team, _, _)) = world.buildings.get(entity) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            }
            if trainable(b.kind).is_empty() {
                errors.push(format!("{tag}: {} produces no units", building_name(b.kind)));
                return;
            }
            let rally = match (x, z, target) {
                (Some(x), Some(z), _) => {
                    Some(RallyTarget::Ground(clamp_to_map(Vec3::new(*x, 0.0, *z))))
                }
                (_, _, Some(id)) => match intent_entity(*id) {
                    // A resource node (neutral, so either team may name one)
                    // makes new workers start gathering; one of our own units
                    // makes new units follow it.
                    Some(e) if world.nodes.get(e).is_ok() => Some(RallyTarget::Node(e)),
                    Some(e) => match world.units.get(e) {
                        Ok((_, _, team, _)) if *team == me => Some(RallyTarget::Unit(e)),
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
        Intent::Cast { hero } => {
            let Some(entity) = intent_entity(*hero) else {
                errors.push(format!("{tag}: caster {hero} not found/not yours"));
                return;
            };
            // A caster is either one of our heroes (any class — the Hero
            // component and `ability_of_unit` agree on which kinds have an
            // ability) or one of our finished buildings with an ability.
            // combat.rs owns the mana/cooldown verdict either way.
            let unit_caster = matches!(
                world.units.get(entity),
                Ok((_, u, team, _)) if *team == me && ability_of_unit(u.kind).is_some()
            );
            if !unit_caster {
                match world.buildings.get(entity) {
                    Ok((b, team, under, _)) if *team == me => {
                        if under.is_some() {
                            errors.push(format!("{tag}: building {hero} is under construction"));
                            return;
                        }
                        if ability_of_building(b.kind).is_none() {
                            errors
                                .push(format!("{tag}: {} has no ability", building_name(b.kind)));
                            return;
                        }
                    }
                    _ => {
                        errors.push(format!(
                            "{tag}: caster {hero} is not a hero or an own ability building"
                        ));
                        return;
                    }
                }
            }
            casts.write(CastAbility { caster: entity });
        }
        Intent::Buy { shop, item } => {
            let Some(item) = parse_item(item) else {
                errors.push(format!("{tag}: unknown item '{item}'"));
                return;
            };
            let Some(entity) = intent_entity(*shop) else {
                errors.push(format!("{tag}: building {shop} not found/not yours"));
                return;
            };
            let Ok((b, team, under, _)) = world.buildings.get(entity) else {
                errors.push(format!("{tag}: building {shop} not found/not yours"));
                return;
            };
            if *team != me {
                errors.push(format!("{tag}: building {shop} not found/not yours"));
                return;
            }
            if b.kind != BuildingKind::Shop {
                errors.push(format!("{tag}: {} does not sell items", building_name(b.kind)));
                return;
            }
            if under.is_some() {
                errors.push(format!("{tag}: building {shop} is under construction"));
                return;
            }
            // The buyer is implied: a team fields exactly one hero.
            let Some(hero) = own_hero(world, me) else {
                errors.push(format!("{tag}: no living hero to buy for"));
                return;
            };
            // economy.rs re-validates and pays.
            buys.write(BuyItem {
                shop: entity,
                hero,
                item,
            });
        }
        Intent::UseItem { slot } => {
            let slot = *slot;
            if slot >= INVENTORY_SLOTS {
                errors.push(format!(
                    "{tag}: item slot {slot} out of range (0..{})",
                    INVENTORY_SLOTS - 1
                ));
                return;
            }
            let Some(hero) = own_hero(world, me) else {
                errors.push(format!("{tag}: no living hero to use an item"));
                return;
            };
            // combat.rs checks the slot is actually filled.
            item_uses.write(UseItem { hero, slot });
        }
        Intent::Autopilot { on } => {
            // Only ever the issuing faction.
            set_autopilot(ai_controlled, me, *on);
            info!(
                "intent: autopilot {} for {:?} — scripted AI {} the macro game",
                if *on { "ON" } else { "OFF" },
                me,
                if *on { "takes over" } else { "releases" }
            );
        }
        Intent::Surrender => {
            info!("intent: {me:?} surrenders");
            commands.send_event(Surrender { team: me });
        }
        Intent::Priority {
            units: ids,
            classes,
        } => {
            // One bad class name invalidates the whole list rather than
            // silently installing a priority order nobody asked for.
            let parsed = match parse_target_classes(classes) {
                Ok(parsed) => parsed,
                Err(name) => {
                    errors.push(format!("{tag}: unknown target class '{name}'"));
                    return;
                }
            };
            for (entity, _) in own_units(ids, world, me, tag, errors) {
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
                (Some(x), Some(z)) => Some(clamp_to_map(Vec3::new(*x, 0.0, *z))),
                _ => None,
            };
            if !clear && rally.is_none() {
                errors.push(format!("{tag}: retreat needs a rally x/z"));
                return;
            }
            for (entity, _) in own_units(ids, world, me, tag, errors) {
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
                (Some(x), Some(z)) => Some(clamp_to_map(Vec3::new(*x, 0.0, *z))),
                _ => None,
            };
            if !clear && anchor.is_none() {
                errors.push(format!("{tag}: leash needs an anchor x/z"));
                return;
            }
            for (entity, _) in own_units(ids, world, me, tag, errors) {
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
        } => {
            let min_enemies = min_enemies.unwrap_or(0);
            for (entity, _) in own_units(ids, world, me, tag, errors) {
                // Any hero class can auto-cast; nothing else has an ability.
                if !matches!(world.units.get(entity), Ok((_, u, _, _)) if is_hero_kind(u.kind)) {
                    errors.push(format!("{tag}: unit {} is not a hero", entity.to_bits()));
                    continue;
                }
                let mut ec = commands.entity(entity);
                if min_enemies == 0 {
                    ec.try_remove::<AutoCastPolicy>();
                } else {
                    ec.try_insert(AutoCastPolicy { min_enemies });
                }
            }
        }
        Intent::Squad { units: ids, id } => {
            for (entity, _) in own_units(ids, world, me, tag, errors) {
                let mut ec = commands.entity(entity);
                match id {
                    Some(id) => {
                        ec.try_insert(SquadId(*id));
                    }
                    None => {
                        ec.try_remove::<SquadId>();
                    }
                }
            }
        }
        Intent::Posture { id, posture } => {
            // Squad ids are per-team, so red's squad 1 and blue's squad 1 are
            // different squads.
            let posture = match posture {
                None => {
                    // Clearing a posture leaves membership intact: the squad
                    // simply stops being re-tasked.
                    squad_orders.0.remove(&(me, *id));
                    return;
                }
                Some(PostureIntent::Defend { x, z, radius }) => {
                    if !(*radius > 0.0) {
                        errors.push(format!("{tag}: defend radius must be > 0, got {radius}"));
                        return;
                    }
                    SquadPosture::Defend {
                        pos: clamp_to_map(Vec3::new(*x, 0.0, *z)),
                        radius: *radius,
                    }
                }
                Some(PostureIntent::Push { x, z }) => SquadPosture::Push {
                    pos: clamp_to_map(Vec3::new(*x, 0.0, *z)),
                },
                Some(PostureIntent::Forage { x, z }) => SquadPosture::Forage {
                    muster: clamp_to_map(Vec3::new(*x, 0.0, *z)),
                },
                Some(PostureIntent::Escort { unit }) => {
                    let Some((target, _)) = own_unit(*unit, world, me) else {
                        errors.push(format!("{tag}: unit {unit} not found/not yours"));
                        return;
                    };
                    SquadPosture::Escort { unit: target }
                }
            };
            squad_orders.0.insert((me, *id), posture);
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
            let Some(entity) = intent_entity(*building) else {
                errors.push(format!("{tag}: building {building} not found/not yours"));
                return;
            };
            let Ok((b, team, under, queue)) = world.buildings.get(entity) else {
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
                errors.push(format!("{tag}: {} has no training queue", building_name(b.kind)));
                return;
            }
            // Same class parsing (and same all-or-nothing rule) as `priority`;
            // an empty list means "no priority piece".
            let priority = match priority {
                Some(names) => match parse_target_classes(names) {
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
                            "{tag}: template retreat 'below' must be a fraction in (0,1), got {}",
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
                squad: *squad,
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

/// Flip one faction's scripted-AI flag. Never the other's.
pub fn set_autopilot(ai_controlled: &mut AiControlled, team: Team, on: bool) {
    match team {
        Team::Human => ai_controlled.human = on,
        Team::Claude => ai_controlled.claude = on,
    }
}

/// Resolve one id to a living unit of the issuing team.
fn own_unit(id: IntentId, world: &IntentWorld, me: Team) -> Option<(Entity, Vec3)> {
    let entity = intent_entity(id)?;
    match world.units.get(entity) {
        Ok((_, _, team, tf)) if *team == me => Some((entity, tf.translation)),
        _ => None,
    }
}

fn own_hero(world: &IntentWorld, me: Team) -> Option<Entity> {
    world
        .units
        .iter()
        .find(|(_, u, team, _)| **team == me && is_hero_kind(u.kind))
        .map(|(entity, ..)| entity)
}

/// Resolve a list of ids to living units of the issuing team, recording one
/// error per id that doesn't qualify (an enemy's unit included).
fn own_units(
    ids: &[IntentId],
    world: &IntentWorld,
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
        match own_unit(id, world, me) {
            Some(found) => out.push(found),
            None => errors.push(format!("{tag}: unit {id} not found/not yours")),
        }
    }
    out
}

/// The team's completed (not under construction) buildings — the input to
/// every requirement check on the intent path.
fn completed_kinds(world: &IntentWorld, me: Team) -> Vec<BuildingKind> {
    world
        .buildings
        .iter()
        .filter(|(_, team, under, _)| **team == me && under.is_none())
        .map(|(building, ..)| building.kind)
        .collect()
}

fn is_worker(world: &IntentWorld, entity: Entity) -> bool {
    matches!(world.units.get(entity), Ok((_, u, _, _)) if u.kind == UnitKind::Worker)
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
    let missing: Vec<&str> = reqs
        .iter()
        .filter(|r| !completed.contains(r))
        .map(|r| building_name(*r))
        .collect();
    Some(format!("{tag}: {what} requires {}", missing.join(" + ")))
}

/// Move / AttackMove for a group, spread over the shared formation grid.
#[allow(clippy::too_many_arguments)]
fn ground_order(
    commands: &mut Commands,
    errors: &mut Vec<String>,
    tag: &str,
    ids: &[IntentId],
    world: &IntentWorld,
    me: Team,
    ground: Vec3,
    attack_move: bool,
) {
    let group = own_units(ids, world, me, tag, errors);
    let count = group.len();
    for (i, (entity, _)) in group.into_iter().enumerate() {
        let p = clamp_to_map(ground + formation_offset(i, count));
        let order = if attack_move {
            Order::AttackMove(p)
        } else {
            Order::Move(p)
        };
        commands.entity(entity).try_insert(order);
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

// --- name parsing: the vocabulary's spelling rules ---------------------------

/// Loose form of a name on the wire: case, spaces, dashes and underscores are
/// all noise, so `"town_hall"`, `"Town Hall"` and `"townhall"` are one name.
fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Both parsers match against the catalog's own ids (`shared::kind_name` /
/// `building_name`), so a kind added to the shared enums is orderable the
/// moment it exists — no table here to fall out of date.
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

/// Items parse off the catalog's own ids too (`item_def(..).name`), so
/// `"town_portal"`, `"Town Portal"` and `"TownPortal"` are one item.
pub fn parse_item(name: &str) -> Option<ItemId> {
    let wanted = normalize_name(name);
    ALL_ITEMS
        .into_iter()
        .find(|id| normalize_name(item_def(*id).name) == wanted)
}

/// Parse a whole class list, all-or-nothing: `Err(name)` names the first
/// unknown class so the caller can reject the intent outright rather than
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
    ALL_TARGET_CLASSES
        .iter()
        .copied()
        .find(|c| c.name().eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            r#"{"type":"cancel","building":1,"index":0}"#,
            r#"{"type":"rally","building":1,"x":1.0,"z":2.0}"#,
            r#"{"type":"rally","building":1,"target":7}"#,
            r#"{"type":"cast","hero":1}"#,
            r#"{"type":"cast","caster":1}"#,
            r#"{"type":"buy","shop":1,"item":"HealingPotion"}"#,
            r#"{"type":"use_item","slot":0}"#,
            r#"{"type":"autopilot","on":true}"#,
            r#"{"type":"surrender"}"#,
            r#"{"type":"priority","units":[1],"classes":["Hero","Siege"]}"#,
            r#"{"type":"priority","units":[1]}"#,
            r#"{"type":"retreat","units":[1],"below":0.35,"x":1.0,"z":2.0}"#,
            r#"{"type":"leash","units":[1],"x":1.0,"z":2.0,"radius":20.0}"#,
            r#"{"type":"autocast","units":[1],"min_enemies":3}"#,
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
            let parsed: Intent =
                serde_json::from_str(case).unwrap_or_else(|e| panic!("{case} failed to parse: {e}"));
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

    #[test]
    fn names_parse_loosely() {
        assert_eq!(parse_building_kind("town_hall"), Some(BuildingKind::TownHall));
        assert_eq!(parse_building_kind("Town Hall"), Some(BuildingKind::TownHall));
        assert_eq!(parse_unit_kind("footman"), Some(UnitKind::Footman));
        assert_eq!(parse_item("town portal"), Some(ItemId::TownPortal));
        assert_eq!(parse_target_class("siege"), Some(TargetClass::Siege));
        assert!(parse_building_kind("nonsense").is_none());
    }
}
