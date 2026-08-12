//! alarm.rs — forced re-decisions that default to continue and never act.
//!
//! docs/AFFORDANCES.md § "Alarms", implementation plan item 4. The contract
//! types (`AlarmKind`, `Alarm`, `Alarms`, `AlarmSet`) are in `shared.rs`
//! because bridge.rs renders them; everything that *decides* is here, which is
//! the same split trigger.rs has with `TriggerWhen`/`Triggers`.
//!
//! ## The problem this exists for
//!
//! r21's boomer boomed with zero army because nothing ever forced a
//! re-evaluation; r23's red seat lost to "one long wrong continue" through an
//! income collapse nothing flagged. Neither is a vocabulary failure — both
//! commanders could have said the words. They are *attention* failures: a
//! stance persists, persistence is right almost always, and the game never
//! interrupted to say that this was one of the times it was not.
//!
//! So four conditions force a fresh choice and everything else defaults to
//! continue:
//!
//! 1. an enemy army at or above the threshold is in the **sightings ledger**,
//! 2. one of your own squads is below half pooled strength,
//! 3. your gold has stopped — mines dry or nobody mining,
//! 4. buildings are being hit in two or more places at once, with the recall
//!    ETA of whatever is already moving.
//!
//! ## The three rules, and where each one is enforced
//!
//! **An alarm never acts.** This module writes exactly two things: the
//! `Alarms` resource and a line on the acting team's own `GameEvents` feed.
//! There is no `SubmitIntent` here, no `Order`, no `Commands`. The one path
//! from a player to the world is still `Intent` → `apply_intents`, and it is
//! shorter to verify that this file contains no such write than to argue that
//! it makes none: grep it.
//!
//! **An alarm fires only after the reflex has.** Twice over. Structurally, the
//! evaluator is in `AlarmSet` inside `SimSet::Feed` — downstream of `Think`
//! (doctrine's executors and the trigger evaluator) and of `Intent` (where
//! what a trigger submitted is compiled), in the same frame. Numerically, each
//! kind's `grace_s` from `assets/data/alarms.ron` says how long the condition
//! must hold before the alarm is raised, which spans several sweeps of the
//! 4 Hz reflex tier. The payoff of an alarm is attention, not speed; a
//! mechanism that could beat the reflex would be claiming the wrong one, and
//! it would also be *wrong*, because a commander answers at LLM latency and
//! anything with a shorter deadline cannot live at the commander layer at all
//! (docs/TEMPO.md).
//!
//! **Every alarm names its running default.** `Alarm::running_default` is a
//! `String` and not an `Option<String>`, so there is no code path that raises
//! an alarm without saying what is already happening about it. When nothing is
//! standing, it says *that*, in those words — "nothing recovers this
//! automatically" is the most useful sentence in the layer and the one a
//! shouting alarm would hide. A silent commander gets the reflex's outcome;
//! the alarm is what makes staying silent a decision instead of an accident.
//!
//! ## Fog
//!
//! Inherited, not re-derived (docs/FOG.md; BUILDER_BRIEF §6.10). The three
//! own-team alarms read this team's own units, buildings and doctrine, which
//! are its own knowledge by construction. The one alarm about the enemy reads
//! `FogGrid::army_groups()` — **the ledger, and no `world.units` access at
//! all**, which is the same structural claim `enemy_army_seen` and
//! `enemy_hero_down` make in trigger.rs and is pinned by
//! `an_unseen_army_raises_no_alarm`. There is exactly one place an enemy fact
//! enters this module and it is that call.
//!
//! ## The human seat
//!
//! Nothing in ui.rs changed, and that is the point rather than an omission.
//! Alarms surface for the human through `GameEvents`, which the alert stack
//! already renders for `Team::Human` — one producer, two renderers, the rule
//! this codebase keeps. What the two seats get is what they have always got:
//! the bridge seat gets the level-triggered `alarms` array in its snapshot
//! (a file reader can hold forty lines of history and a triage list), the
//! human gets the edge-triggered line in the corner of the screen, coloured
//! and pinged. Rendering is where the two seats are allowed to differ
//! (docs/INTENT.md, AFFORDANCES.md § "What the fairness invariant does and
//! does not constrain"); the *fact* is computed once, for both.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use std::collections::BTreeMap;
use std::time::Duration;

use crate::shared::*;

/// Alarm sweep cadence (~2 Hz).
///
/// Half the speed of the reflex tier, deliberately. Everything here is a
/// slow-burn condition whose grace window is measured in seconds, so a faster
/// sweep would buy nothing but `army_groups()` recomputations — and being
/// visibly slower than the thing it reports is the correct relationship
/// between an attention layer and a reaction layer.
const ALARM_MS: u64 = 500;

/// How far back a fired trigger still counts as "the reflex that answered
/// this". Beyond it, a rule that went off four minutes ago is history rather
/// than the running default, and naming it would be a comforting lie.
const REFLEX_MEMORY_S: f32 = 90.0;

/// How near an attacked place a squad's posture point must be for that squad
/// to count as covering it. Wider than the `Defend` radius doctrine seeds
/// (22) because a squad anchored just outside a base is still the thing that
/// will arrive, and narrower than the map so a push on the far side is not
/// credited with defending home.
const RECALL_NEAR: f32 = 35.0;

/// Squads named in one running default before it stops listing and starts
/// counting. Three is what fits in a line a human reads in the corner of a
/// screen, which is the harder of the two constraints.
const MAX_SQUADS_NAMED: usize = 3;

pub struct AlarmPlugin;

impl Plugin for AlarmPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            evaluate_alarms
                .run_if(on_timer(Duration::from_millis(ALARM_MS)))
                // `AlarmSet` is configured into `SimSet::Feed` by `CorePlugin`.
                // The two edges inside that phase, both load-bearing:
                //
                //  * AFTER `produce_game_events` — the frame's ordinary feed
                //    lines land before the alarm that comments on them, and,
                //    more importantly, the two writers of `GameEvents` are
                //    ordered, so `seq` numbering is identical on every run of
                //    one seed. Bevy would otherwise leave two `ResMut` writers
                //    in one set to the executor.
                //  * bridge.rs declares `.after(AlarmSet)` at its end, so a
                //    snapshot carries this frame's alarms rather than the
                //    previous frame's.
                .in_set(AlarmSet)
                .after(crate::shared::produce_game_events),
        );
    }
}

// ---------------------------------------------------------------------------
// The world an alarm may consult
// ---------------------------------------------------------------------------

/// Own units with everything four predicates and their running defaults need:
/// what it is, whose it is, where it is, how hurt it is, whose squad it is in,
/// what is buffing it (for an honest ETA), whether it has a retreat rule, and
/// what it is currently doing (for "is anybody mining?").
type AlarmUnits<'w, 's> = Query<
    'w,
    's,
    (
        &'static Unit,
        &'static Team,
        &'static Transform,
        &'static Health,
        Option<&'static SquadId>,
        Option<&'static StatusEffects>,
        Option<&'static RetreatPolicy>,
        Option<&'static Order>,
        Option<&'static Carrying>,
    ),
>;

type AlarmBuildings<'w, 's> = Query<
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

type AlarmNodes<'w, 's> = Query<'w, 's, (Entity, &'static ResourceNode, &'static Transform)>;

/// The read-only world one alarm sweep consults. Bundled on the same reasoning
/// `TriggerWorld` is: nothing outside this file cares about these five, and a
/// system that grows a parameter per alarm would hit Bevy's ceiling on the
/// fifth one.
///
/// Every member is read-only. An alarm that could write would be an alarm that
/// acts, and the borrow checker is a better guarantee of that than a comment.
#[derive(SystemParam)]
pub struct AlarmWorld<'w, 's> {
    units: AlarmUnits<'w, 's>,
    buildings: AlarmBuildings<'w, 's>,
    nodes: AlarmNodes<'w, 's>,
    /// What each squad is currently for — the continuous half of the running
    /// default, and the thing "continue" actually means.
    squads: Res<'w, SquadOrders>,
    /// The contingent half. Read to NAME the reflex that already answered,
    /// never to decide anything: a trigger's own evaluator owns whether it
    /// fires.
    triggers: Res<'w, Triggers>,
    /// The one door an enemy fact comes through, and only via
    /// `FogGrid::army_groups`.
    fog: Res<'w, FogGrids>,
}

// ---------------------------------------------------------------------------
// The running default: what is already happening
// ---------------------------------------------------------------------------

fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    Vec2::new(d.x, d.z).length()
}

/// One of this team's squads, as a running default has to describe it.
struct SquadLine {
    id: u8,
    members: usize,
    /// `"holds defend near our base"` — reads after "squad 1".
    phrase: String,
    /// Where the posture is pointed, when it is pointed anywhere.
    point: Option<Vec3>,
    /// Centre of mass of the living members.
    centre: Vec3,
}

/// Every squad with somebody in it, in id order.
///
/// A `BTreeMap` on the collection rule (BUILDER_BRIEF §6.4): this decides the
/// order squads are named in a sentence that ends up in a log, and std's
/// `HashMap` reseeds per process.
fn squad_lines(me: Team, world: &AlarmWorld) -> Vec<SquadLine> {
    let mut counts: BTreeMap<u8, (usize, Vec3)> = BTreeMap::new();
    for (_, team, tf, hp, squad, ..) in world.units.iter() {
        if *team != me || hp.current <= 0.0 {
            continue;
        }
        let Some(squad) = squad else { continue };
        let slot = counts.entry(squad.0).or_insert((0, Vec3::ZERO));
        slot.0 += 1;
        slot.1 += tf.translation;
    }
    counts
        .into_iter()
        .map(|(id, (members, sum))| {
            let (phrase, point) = match world.squads.0.get(&(me, id)) {
                Some(SquadPosture::Defend { pos, .. }) => {
                    (format!("holds defend {}", place_name(*pos, me)), Some(*pos))
                }
                Some(SquadPosture::Push { pos }) => {
                    (format!("pushes {}", place_name(*pos, me)), Some(*pos))
                }
                Some(SquadPosture::Escort { .. }) => ("escorts".to_string(), None),
                Some(SquadPosture::Forage { muster }) => (
                    format!("forages, mustering {}", place_name(*muster, me)),
                    Some(*muster),
                ),
                None => ("has no posture".to_string(), None),
            };
            SquadLine {
                id,
                members,
                phrase,
                point,
                centre: sum / members.max(1) as f32,
            }
        })
        .collect()
}

/// "Continue" spelled out: what the standing postures will go on doing if the
/// commander says nothing.
///
/// This is the *floor* of every running default. An alarm whose reflex tier
/// has nothing specific to say still has this, and it is never the empty
/// string — a commander told "an army is coming" and nothing else has to go
/// look up its own squads, which is the lookup the layer exists to delete.
fn continue_note(me: Team, world: &AlarmWorld) -> String {
    let lines = squad_lines(me, world);
    if lines.is_empty() {
        return "you have no squad in the field — production continues".to_string();
    }
    let named: Vec<String> = lines
        .iter()
        .take(MAX_SQUADS_NAMED)
        .map(|l| format!("squad {} ({} units) {}", l.id, l.members, l.phrase))
        .collect();
    let rest = lines.len().saturating_sub(MAX_SQUADS_NAMED);
    match rest {
        0 => named.join("; "),
        n => format!("{} (+{n} more)", named.join("; ")),
    }
}

/// The contingent reflex that already answered, if the commander armed one.
///
/// `relevant` decides which predicates count as covering this alarm. Only
/// rules that have actually FIRED are named: an armed-but-quiet rule is a
/// policy, not an answer, and reporting it as the running default would tell a
/// commander that something was handled when nothing was.
///
/// The list is walked in arming order and the most recent fire wins, ties
/// going to the earliest-armed rule — deterministic, because `Triggers` is a
/// `Vec` kept in the order the commander wrote it.
fn reflex_note(
    me: Team,
    now: f32,
    world: &AlarmWorld,
    relevant: fn(&TriggerWhen) -> bool,
) -> Option<String> {
    let mut best: Option<(&TriggerName, f32)> = None;
    for rule in world.triggers.get(me) {
        let Some(fired) = rule.last_fired else { continue };
        if !relevant(&rule.when) || (now - fired).max(0.0) > REFLEX_MEMORY_S {
            continue;
        }
        if best.is_none_or(|(_, best_t)| fired > best_t) {
            best = Some((&rule.name, fired));
        }
    }
    let (name, fired) = best?;
    Some(format!("your trigger {name} fired at t={fired:.0}"))
}

/// Prefix the reflex's answer to the standing postures, when there is one.
fn default_with(reflex: Option<String>, fallback: &str, tail: String) -> String {
    match reflex {
        Some(note) => format!("{note} — {tail}"),
        None => format!("{fallback} — {tail}"),
    }
}

// ---------------------------------------------------------------------------
// The four alarms
// ---------------------------------------------------------------------------

/// **Enemy army sighted.** Reads the sightings ledger and nothing else.
///
/// `army_groups()` already excludes workers (a mining crew is not an army) and
/// already refuses to merge observations taken at different times into a body
/// that existed at no instant. Freshness is `ARMY_EVENT_FRESH_S`, the same
/// constant the event feed uses to tell "spotted" from "merely remembered":
/// the ledger keeps a sighting for ninety seconds, but an alarm about a
/// minute-old memory would be an interruption with no decision in it. The age
/// rides in the fact regardless, because reading current sight as ground truth
/// is exactly how r23's red seat lost.
fn army_alarm(me: Team, now: f32, threshold: f32, world: &AlarmWorld) -> Option<Alarm> {
    let min = threshold.max(1.0).round() as usize;
    let mut best: Option<ArmyGroup> = None;
    for group in world.fog.get(me).army_groups() {
        if group.size < min || (now - group.t_seen).max(0.0) > ARMY_EVENT_FRESH_S {
            continue;
        }
        // Biggest wins; freshest breaks the tie; `army_groups` is in a
        // deterministic order, so the third tie-break is arrival and is stable.
        let better = best
            .as_ref()
            .is_none_or(|b| group.size > b.size || (group.size == b.size && group.t_seen > b.t_seen));
        if better {
            best = Some(group);
        }
    }
    let group = best?;
    let fact = format!(
        "enemy army of {} ({}) {}, last seen {:.0}s ago",
        group.size,
        group.summary(),
        place_name(group.centroid, me),
        (now - group.t_seen).max(0.0)
    );
    let reflex = reflex_note(me, now, world, |when| {
        matches!(
            when,
            TriggerWhen::EnemyArmySeen { .. }
                | TriggerWhen::EnemySighted { .. }
                | TriggerWhen::EnemyIn { .. }
        )
    });
    Some(Alarm {
        kind: AlarmKind::EnemyArmySighted,
        fact,
        running_default: default_with(
            reflex,
            "no armed trigger covers a sighting",
            continue_note(me, world),
        ),
        since_t: 0.0,
        severity: EventSeverity::Warning,
        eta_s: None,
        pos: Some(group.centroid),
    })
}

/// **Own squad below half strength.** Pooled, not per-member — one wounded
/// footman in a healthy line is not a squad in trouble, which is the reading
/// `TriggerWhen::SquadBelow` already uses.
///
/// The worst squad is the subject; ties go to the lowest id, because the map
/// is walked in id order and `min_by` keeps the first minimum.
fn squad_alarm(me: Team, now: f32, threshold: f32, world: &AlarmWorld) -> Option<Alarm> {
    struct Pool {
        current: f32,
        max: f32,
        members: usize,
        retreaters: usize,
        rally: Option<Vec3>,
        worst_retreat_frac: f32,
    }
    let mut pools: BTreeMap<u8, Pool> = BTreeMap::new();
    for (_, team, _, hp, squad, _, retreat, _, _) in world.units.iter() {
        if *team != me {
            continue;
        }
        let Some(squad) = squad else { continue };
        let pool = pools.entry(squad.0).or_insert(Pool {
            current: 0.0,
            max: 0.0,
            members: 0,
            retreaters: 0,
            rally: None,
            worst_retreat_frac: 0.0,
        });
        pool.current += hp.current;
        pool.max += hp.max;
        pool.members += 1;
        if let Some(policy) = retreat {
            pool.retreaters += 1;
            pool.rally.get_or_insert(policy.rally);
            pool.worst_retreat_frac = pool.worst_retreat_frac.max(policy.below_frac);
        }
    }
    let (id, pool) = pools
        .iter()
        .filter(|(_, p)| p.max > 0.0 && p.current / p.max < threshold)
        .min_by(|a, b| {
            let fa = a.1.current / a.1.max;
            let fb = b.1.current / b.1.max;
            fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
        })?;
    let frac = pool.current / pool.max;
    let line = squad_lines(me, world).into_iter().find(|l| l.id == *id);
    let fact = format!(
        "squad {id} at {:.0}% pooled health across {} unit{}",
        frac * 100.0,
        pool.members,
        if pool.members == 1 { "" } else { "s" }
    );
    // The reflex here is doctrine rather than a trigger — a retreat threshold
    // is standing policy, and it has already pulled whoever crossed it.
    let doctrine = if pool.retreaters > 0 {
        let rally = pool
            .rally
            .map(|r| place_name(r, me))
            .unwrap_or_else(|| "their rally".to_string());
        Some(format!(
            "retreat doctrine falls {} of {} members back to {rally} below {:.0}% health",
            pool.retreaters,
            pool.members,
            pool.worst_retreat_frac * 100.0
        ))
    } else {
        None
    };
    let trigger = reflex_note(me, now, world, |when| {
        matches!(when, TriggerWhen::SquadBelow { .. } | TriggerWhen::HeroBelow { .. })
    });
    let posture = match &line {
        Some(l) => format!("squad {id} {}", l.phrase),
        None => continue_note(me, world),
    };
    let running_default = match (trigger, doctrine) {
        (Some(t), Some(d)) => format!("{t} — {d}; {posture}"),
        (Some(t), None) => format!("{t} — {posture}"),
        (None, Some(d)) => format!("{d}; {posture}"),
        (None, None) => format!("nothing pulls them out (no retreat threshold set) — {posture}"),
    };
    Some(Alarm {
        kind: AlarmKind::SquadBelowHalf,
        fact,
        running_default,
        since_t: 0.0,
        severity: EventSeverity::Warning,
        eta_s: None,
        pos: line.map(|l| l.centre),
    })
}

/// **Income collapse.** The gold has stopped: every mine your halls work is
/// dry, or there are fewer workers on gold than the threshold.
///
/// Gated on owning a completed hall. A team with no hall is not suffering an
/// income problem, it is losing the game, and the win check says so.
///
/// "Ours" for a mine is geometry, the same definition `TriggerWhen::MineDry`
/// uses: a gold node within `MINE_HOME_RADIUS` of one of our completed halls.
/// Mines are neutral and unowned; proximity to your own hall is the only
/// honest reading of the mine you are losing.
fn income_alarm(me: Team, now: f32, threshold: f32, world: &AlarmWorld) -> Option<Alarm> {
    let halls: Vec<Vec3> = world
        .buildings
        .iter()
        .filter(|(b, team, _, _, uc)| **team == me && is_hall(b.kind) && uc.is_none())
        .map(|(_, _, tf, _, _)| tf.translation)
        .collect();
    if halls.is_empty() {
        return None;
    }

    let mut near_home = 0usize;
    let mut live_mines = 0usize;
    let mut gold_nodes: Vec<Entity> = Vec::new();
    for (entity, node, tf) in world.nodes.iter() {
        if node.kind != ResourceKind::Gold {
            continue;
        }
        gold_nodes.push(entity);
        if halls
            .iter()
            .any(|hall| xz_dist(*hall, tf.translation) <= MINE_HOME_RADIUS)
        {
            near_home += 1;
            if node.remaining > 0 {
                live_mines += 1;
            }
        }
    }

    let mut workers = 0usize;
    let mut on_gold = 0usize;
    for (unit, team, _, hp, _, _, _, order, carrying) in world.units.iter() {
        if *team != me || !is_worker_kind(unit.kind) || hp.current <= 0.0 {
            continue;
        }
        workers += 1;
        // On gold means one of the two halves of the harvest cycle: swinging
        // at a gold node, or walking a load of gold home. Counting only the
        // first would read every full worker as idle.
        let mining = match order {
            Some(Order::Harvest(node)) => gold_nodes.contains(node),
            Some(Order::ReturnResources) => {
                carrying.is_some_and(|c| c.kind == ResourceKind::Gold)
            }
            _ => false,
        };
        if mining {
            on_gold += 1;
        }
    }

    let dry = live_mines == 0;
    let starving = (on_gold as f32) < threshold;
    if !dry && !starving {
        return None;
    }
    let mut clauses: Vec<String> = Vec::new();
    if dry {
        clauses.push(match near_home {
            0 => "no gold mine is in reach of any of your halls".to_string(),
            1 => "the one gold mine your hall works is dry".to_string(),
            n => format!("all {n} gold mines your halls work are dry"),
        });
    }
    if starving {
        clauses.push(match (on_gold, workers) {
            (0, 0) => "you have no workers left".to_string(),
            (0, w) => format!("none of your {w} workers is on gold"),
            (n, w) => format!("only {n} of {w} workers is on gold"),
        });
    }
    let reflex = reflex_note(me, now, world, |when| {
        matches!(when, TriggerWhen::MineDry)
    });
    Some(Alarm {
        kind: AlarmKind::IncomeCollapse,
        fact: format!("income collapse: {}", clauses.join("; ")),
        running_default: default_with(
            reflex,
            "nothing recovers this automatically",
            format!(
                "workers continue their current assignment; {}",
                continue_note(me, world)
            ),
        ),
        since_t: 0.0,
        severity: EventSeverity::Warning,
        // Deliberately no `pos`: an economy has no one place, and pinging the
        // minimap at an arbitrary mine would point the camera at the symptom.
        eta_s: None,
        pos: None,
    })
}

/// **Multiple places under attack at once, with recall ETA.**
///
/// Places rather than buildings, via `place_name` — the same public geography
/// the event feed and the snapshot's `map` block use, so "near the east ford"
/// means one thing in this game. Two buildings of one base are one place; a
/// hall at home and a hall at an expansion are two, which is the whole
/// content of the alarm.
///
/// The recall ETA is what makes this answerable at LLM latency: "full recall
/// or sacrifice the expansion?" is still the right question a minute later,
/// but only if you know when the recall lands.
fn places_alarm(me: Team, now: f32, threshold: f32, world: &AlarmWorld) -> Option<Alarm> {
    let min = threshold.max(2.0).round() as usize;
    // Keyed by the PLACE's name, so the map's own vocabulary decides what
    // counts as one place. A `BTreeMap` for the usual reason and one more: the
    // fact string lists them, and hash order would reorder the sentence.
    let mut places: BTreeMap<String, (Vec<&'static str>, Vec3)> = BTreeMap::new();
    for (building, team, tf, hit, _) in world.buildings.iter() {
        if *team != me || !hit.is_some_and(|h| now - h.at <= BASE_ATTACK_WINDOW_S) {
            continue;
        }
        let entry = places
            .entry(place_name(tf.translation, me))
            .or_insert_with(|| (Vec::new(), tf.translation));
        entry.0.push(building_name(building.kind));
    }
    if places.len() < min {
        return None;
    }
    let listed: Vec<String> = places
        .iter()
        .map(|(name, (kinds, _))| {
            let mut kinds = kinds.clone();
            kinds.sort_unstable();
            format!("{name} ({})", kinds.join(", "))
        })
        .collect();
    let fact = format!(
        "{} places under attack at once: {}",
        places.len(),
        listed.join("; ")
    );

    // What is already moving toward one of them, and when it gets there.
    let mut arriving: Option<(u8, String, f32)> = None;
    for line in squad_lines(me, world) {
        let Some(point) = line.point else { continue };
        let covers = places
            .values()
            .any(|(_, pos)| xz_dist(point, *pos) <= RECALL_NEAR);
        if !covers {
            continue;
        }
        let Some(eta) = squad_eta(me, line.id, point, world) else {
            continue;
        };
        if arriving.as_ref().is_none_or(|(_, _, best)| eta < *best) {
            arriving = Some((line.id, place_name(point, me), eta));
        }
    }
    let reflex = reflex_note(me, now, world, |when| {
        matches!(when, TriggerWhen::BaseUnderAttack | TriggerWhen::EnemyIn { .. })
    });
    let (movement, eta_s) = match &arriving {
        Some((id, where_, eta)) => (
            format!("squad {id} is closing on {where_} (ETA {:.0}s)", eta),
            Some(ev_round1(*eta)),
        ),
        None => (
            format!("nothing is moving to either place — {}", continue_note(me, world)),
            None,
        ),
    };
    Some(Alarm {
        kind: AlarmKind::PlacesUnderAttack,
        fact,
        running_default: default_with(reflex, "no armed trigger covers a base raid", movement),
        since_t: 0.0,
        severity: EventSeverity::Critical,
        eta_s,
        // The first place in name order, so the HUD ping is stable while the
        // alarm stands rather than jittering between two fronts.
        pos: places.values().next().map(|(_, pos)| *pos),
    })
}

/// Seconds until the whole of `squad` has reached `point`.
///
/// The **slowest** member decides, because a squad that arrives in packets has
/// not arrived (doctrine.rs's cohesion rule exists for exactly this reason,
/// and an ETA that reported the fastest unit would be promising a defence that
/// shows up one footman at a time). Members already there are excluded rather
/// than counted as zero, so a squad that is home reports `None` — nothing is
/// in transit, which is a real answer and not a missing one.
///
/// Speed comes from `effective_stats`, the one stat law, so a hasted or slowed
/// squad's ETA is the one the sim will actually produce.
fn squad_eta(me: Team, squad: u8, point: Vec3, world: &AlarmWorld) -> Option<f32> {
    let mut worst: Option<f32> = None;
    for (unit, team, tf, hp, id, status, _, _, _) in world.units.iter() {
        if *team != me || hp.current <= 0.0 || id.map(|s| s.0) != Some(squad) {
            continue;
        }
        let distance = xz_dist(tf.translation, point);
        if distance <= SQUAD_ARRIVED {
            continue;
        }
        let speed = effective_stats(BaseStats::of_unit(unit.kind), status).speed;
        if speed <= 0.0 {
            continue;
        }
        let eta = distance / speed;
        if worst.is_none_or(|w| eta > w) {
            worst = Some(eta);
        }
    }
    worst
}

/// How close to the posture point counts as arrived, for the ETA only.
/// Matches doctrine.rs's own `SQUAD_ARRIVE` in spirit — a unit inside this is
/// not something anybody is waiting for.
const SQUAD_ARRIVED: f32 = 3.0;

/// One decimal, the rounding every number on this wire uses.
fn ev_round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

/// Sweep both teams' four alarms; raise, refresh and clear.
///
/// `pub` so a test can register it without its cadence, the same idiom
/// trigger.rs and doctrine.rs use: the timer is a tuning constant and a test
/// that waited out 500ms of clock would be testing the clock.
///
/// Teams in a fixed order and kinds in `ALL_ALARM_KINDS` order, so two alarms
/// crossing their grace window on the same sweep write their feed lines in the
/// same order on every run of one seed.
pub fn evaluate_alarms(
    time: Res<Time>,
    mut alarms: ResMut<Alarms>,
    mut feed: ResMut<GameEvents>,
    world: AlarmWorld,
) {
    let now = time.elapsed_secs();
    for me in [Team::Human, Team::Claude] {
        for kind in ALL_ALARM_KINDS {
            let tuning = alarm_tuning(kind);
            let raised = match kind {
                AlarmKind::EnemyArmySighted => army_alarm(me, now, tuning.threshold, &world),
                AlarmKind::SquadBelowHalf => squad_alarm(me, now, tuning.threshold, &world),
                AlarmKind::IncomeCollapse => income_alarm(me, now, tuning.threshold, &world),
                AlarmKind::PlacesUnderAttack => places_alarm(me, now, tuning.threshold, &world),
            };
            // The feed line is built before the state machine consumes the
            // alarm, so the edge and the status say the identical words.
            let line = raised
                .as_ref()
                .map(|a| format!("alarm: {} — default: {}", a.fact, a.running_default));
            let severity = raised.as_ref().map_or(EventSeverity::Info, |a| a.severity);
            let pos = raised.as_ref().and_then(|a| a.pos);
            match alarms.observe(me, kind, now, tuning.grace_s, raised) {
                // Pushed to `me` and nobody else. An alarm is a reading of one
                // team's own situation, and the enemy learning that your
                // income collapsed would be the single most valuable leak in
                // the protocol.
                Some(AlarmEdge::Fired) => {
                    if let Some(line) = line {
                        feed.push(me, now, line, severity, pos);
                    }
                }
                // The exit edge we owe anyone we told about the entry one.
                // Without it a reader has to poll to find out it recovered,
                // which is the polling this layer deletes.
                Some(AlarmEdge::Cleared) => feed.push(
                    me,
                    now,
                    format!("alarm clear: {}", kind.label()),
                    EventSeverity::Info,
                    None,
                ),
                None => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::{evaluate_triggers, TriggerPlugin};

    /// The evaluator against a bare world, on a hand-driven clock and without
    /// its `on_timer` — the idiom trigger.rs and doctrine.rs use, for the
    /// reason their comments give: the cadence is a tuning constant.
    fn alarm_app() -> App {
        let mut app = App::new();
        app.init_resource::<Races>()
            .init_resource::<Time>()
            .init_resource::<Alarms>()
            .init_resource::<GameEvents>()
            .init_resource::<SquadOrders>()
            .init_resource::<Triggers>()
            .add_systems(Update, evaluate_alarms);
        // Pin the fog mode rather than inheriting `BH_FOG`: one alarm here is
        // ABOUT knowability, so the ambient env must not decide it.
        app.insert_resource(FogGrids::test_dark());
        app
    }

    fn advance(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
    }

    /// Step the sweep `n` times, advancing the clock by `dt` before each — the
    /// grace window is a duration, so a test that only called `update()` would
    /// never leave it.
    fn run_for(app: &mut App, secs: f32) {
        let steps = (secs / 1.0).ceil().max(1.0) as u32;
        for _ in 0..steps {
            advance(app, 1.0);
            app.update();
        }
    }

    fn now_of(app: &App) -> f32 {
        app.world().resource::<Time>().elapsed_secs()
    }

    fn alarm_of(app: &App, team: Team, kind: AlarmKind) -> Option<Alarm> {
        app.world()
            .resource::<Alarms>()
            .get(team)
            .iter()
            .find(|a| a.kind == kind)
            .cloned()
    }

    fn feed_lines(app: &App, team: Team) -> Vec<String> {
        app.world()
            .resource::<GameEvents>()
            .feed(team)
            .iter()
            .map(|e| e.message.clone())
            .collect()
    }

    fn spawn_unit(app: &mut App, team: Team, kind: UnitKind, at: Vec3, squad: Option<u8>) -> Entity {
        let hp = unit_stats(kind).hp;
        let mut e = app.world_mut().spawn((
            Unit { kind },
            team,
            Transform::from_translation(at),
            Health::new(hp),
        ));
        if let Some(squad) = squad {
            e.insert(SquadId(squad));
        }
        e.id()
    }

    fn spawn_building(app: &mut App, team: Team, kind: BuildingKind, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Building { kind },
                team,
                Transform::from_translation(at),
                Health::new(building_stats(kind).hp),
            ))
            .id()
    }

    fn sighting(id: u64, kind: UnitKind, at: Vec3, t_seen: f32) -> Sighting {
        Sighting {
            id,
            team: Team::Claude,
            kind,
            pos: at,
            hp_frac: 1.0,
            heading: None,
            t_seen,
        }
    }

    /// Plant `n` enemy troops in Human's ledger, standing together so they
    /// cluster into one body.
    fn see_army(app: &mut App, n: usize, at: Vec3, t_seen: f32) {
        let mut fog = app.world_mut().resource_mut::<FogGrids>();
        for i in 0..n {
            let offset = Vec3::new(i as f32 * 1.5, 0.0, 0.0);
            fog.test_sight(
                Team::Human,
                sighting(100 + i as u64, UnitKind::Footman, at + offset, t_seen),
            );
        }
    }

    // -- the shape of the contract ----------------------------------------

    #[test]
    fn every_alarm_kind_has_a_row_and_a_stable_wire_id() {
        let mut ids: Vec<&str> = ALL_ALARM_KINDS.iter().map(|k| k.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two alarm kinds share a wire id");
        for kind in ALL_ALARM_KINDS {
            let tuning = alarm_tuning(kind);
            assert!(
                tuning.grace_s >= 0.0,
                "{kind:?} could outrun the reflex it reports"
            );
            assert!(!kind.label().is_empty());
        }
    }

    // -- 1. enemy army sighted ---------------------------------------------

    #[test]
    fn an_army_in_the_ledger_raises_the_alarm_after_its_grace_window() {
        let mut app = alarm_app();
        let grace = alarm_tuning(AlarmKind::EnemyArmySighted).grace_s;
        see_army(&mut app, 10, Vec3::new(20.0, 0.0, 20.0), 0.0);

        app.update();
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted).is_none(),
            "an alarm must not fire on the sweep the condition first holds — \
             that window belongs to the reflex"
        );

        // Keep the ledger fresh across the window; the sighting is a memory
        // and would otherwise age out of `ARMY_EVENT_FRESH_S`.
        for _ in 0..(grace.ceil() as u32 + 1) {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            see_army(&mut app, 10, Vec3::new(20.0, 0.0, 20.0), now);
            app.update();
        }
        let alarm = alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted)
            .expect("ten troops is over the threshold and the window has passed");
        assert!(alarm.fact.contains("enemy army of 10"), "{}", alarm.fact);
        assert!(
            !alarm.running_default.is_empty(),
            "every alarm names its running default"
        );
        assert!(
            feed_lines(&app, Team::Human)
                .iter()
                .any(|l| l.starts_with("alarm: enemy army of 10")),
            "the human's alert stack renders the same fact through GameEvents"
        );
        assert!(
            feed_lines(&app, Team::Claude).is_empty(),
            "and the opponent is told nothing — an alarm is not intelligence"
        );
    }

    #[test]
    fn a_patrol_under_the_threshold_is_not_an_alarm() {
        let mut app = alarm_app();
        for _ in 0..12 {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            see_army(&mut app, 3, Vec3::new(20.0, 0.0, 20.0), now);
            app.update();
        }
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted).is_none(),
            "three troops is a patrol; the feed mentions it and the alarm does not"
        );
    }

    /// The leak test. An enemy army standing on the board that this team has
    /// never seen must not raise anything — the alarm reads the ledger, and
    /// the ledger cannot contain what nobody observed.
    #[test]
    fn an_unseen_army_raises_no_alarm() {
        let mut app = alarm_app();
        for i in 0..12 {
            spawn_unit(
                &mut app,
                Team::Claude,
                UnitKind::Footman,
                Vec3::new(20.0 + i as f32, 0.0, 20.0),
                None,
            );
        }
        run_for(&mut app, 30.0);
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted).is_none(),
            "twelve enemies on the board with nothing in the ledger raised an \
             alarm — the alarm layer is reading omniscient state"
        );
        assert!(
            feed_lines(&app, Team::Human).is_empty(),
            "and it said nothing on the feed either"
        );
    }

    #[test]
    fn the_army_alarm_clears_when_the_sighting_goes_stale_and_can_re_arm() {
        let mut app = alarm_app();
        let grace = alarm_tuning(AlarmKind::EnemyArmySighted).grace_s;
        for _ in 0..(grace.ceil() as u32 + 1) {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            see_army(&mut app, 10, Vec3::new(20.0, 0.0, 20.0), now);
            app.update();
        }
        assert!(alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted).is_some());

        // Stop refreshing. Past `ARMY_EVENT_FRESH_S` the group is a memory
        // rather than news, and the alarm has to say so.
        run_for(&mut app, ARMY_EVENT_FRESH_S + 2.0);
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted).is_none(),
            "a standing alarm over a stale memory is an alarm nobody can act on"
        );
        assert!(
            feed_lines(&app, Team::Human)
                .iter()
                .any(|l| l == "alarm clear: enemy army sighted"),
            "told once it started, a reader is owed the line that says it stopped"
        );

        // And it re-arms: a fresh sighting goes through the whole cycle again.
        for _ in 0..(grace.ceil() as u32 + 1) {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            see_army(&mut app, 10, Vec3::new(20.0, 0.0, 20.0), now);
            app.update();
        }
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::EnemyArmySighted).is_some(),
            "a cleared alarm must be able to fire again"
        );
    }

    // -- 2. own squad below half strength ----------------------------------

    #[test]
    fn a_squad_below_half_raises_the_alarm_and_names_its_retreat_doctrine() {
        let mut app = alarm_app();
        let mut wounded = Vec::new();
        for i in 0..4 {
            wounded.push(spawn_unit(
                &mut app,
                Team::Human,
                UnitKind::Footman,
                Vec3::new(i as f32 * 2.0, 0.0, 0.0),
                Some(1),
            ));
        }
        app.world_mut()
            .resource_mut::<SquadOrders>()
            .0
            .insert((Team::Human, 1), SquadPosture::Push { pos: Vec3::new(40.0, 0.0, 40.0) });
        for entity in &wounded {
            let mut e = app.world_mut().entity_mut(*entity);
            let mut hp = e.get_mut::<Health>().unwrap();
            hp.current = hp.max * 0.2;
            e.insert(RetreatPolicy {
                below_frac: 0.35,
                rally: Team::Human.base_pos(),
            });
        }

        run_for(&mut app, alarm_tuning(AlarmKind::SquadBelowHalf).grace_s + 2.0);
        let alarm = alarm_of(&app, Team::Human, AlarmKind::SquadBelowHalf)
            .expect("20% pooled health is below half");
        assert!(alarm.fact.contains("squad 1 at 20%"), "{}", alarm.fact);
        assert!(
            alarm.running_default.contains("retreat doctrine falls 4 of 4 members back"),
            "the running default must name the reflex that already answered: {}",
            alarm.running_default
        );
        assert!(
            alarm.running_default.contains("squad 1 pushes"),
            "and what continues if the commander says nothing: {}",
            alarm.running_default
        );
    }

    #[test]
    fn a_squad_with_no_retreat_rule_is_told_that_nothing_pulls_it_out() {
        let mut app = alarm_app();
        let unit = spawn_unit(&mut app, Team::Human, UnitKind::Footman, Vec3::ZERO, Some(2));
        {
            let mut e = app.world_mut().entity_mut(unit);
            let mut hp = e.get_mut::<Health>().unwrap();
            hp.current = hp.max * 0.1;
        }
        run_for(&mut app, alarm_tuning(AlarmKind::SquadBelowHalf).grace_s + 2.0);
        let alarm = alarm_of(&app, Team::Human, AlarmKind::SquadBelowHalf).expect("below half");
        assert!(
            alarm.running_default.contains("nothing pulls them out"),
            "an honest running default says when the default is nothing: {}",
            alarm.running_default
        );
    }

    #[test]
    fn a_healed_squad_clears_its_alarm() {
        let mut app = alarm_app();
        let unit = spawn_unit(&mut app, Team::Human, UnitKind::Footman, Vec3::ZERO, Some(1));
        {
            let mut e = app.world_mut().entity_mut(unit);
            let mut hp = e.get_mut::<Health>().unwrap();
            hp.current = hp.max * 0.1;
        }
        run_for(&mut app, alarm_tuning(AlarmKind::SquadBelowHalf).grace_s + 2.0);
        assert!(alarm_of(&app, Team::Human, AlarmKind::SquadBelowHalf).is_some());
        {
            let mut e = app.world_mut().entity_mut(unit);
            let mut hp = e.get_mut::<Health>().unwrap();
            hp.current = hp.max;
        }
        run_for(&mut app, 2.0);
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::SquadBelowHalf).is_none(),
            "the condition lapsed and the alarm did not"
        );
        assert!(feed_lines(&app, Team::Human)
            .iter()
            .any(|l| l == "alarm clear: squad below half strength"));
    }

    // -- 3. income collapse -------------------------------------------------

    #[test]
    fn a_dry_mine_and_idle_workers_raise_the_income_alarm() {
        let mut app = alarm_app();
        spawn_building(&mut app, Team::Human, BuildingKind::TownHall, Vec3::ZERO);
        app.world_mut().spawn((
            ResourceNode {
                kind: ResourceKind::Gold,
                remaining: 0,
            },
            Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        ));
        spawn_unit(
            &mut app,
            Team::Human,
            race_worker(Race::Kingdom),
            Vec3::new(2.0, 0.0, 0.0),
            None,
        );

        run_for(&mut app, alarm_tuning(AlarmKind::IncomeCollapse).grace_s + 2.0);
        let alarm = alarm_of(&app, Team::Human, AlarmKind::IncomeCollapse)
            .expect("no gold is coming in");
        assert!(alarm.fact.contains("dry"), "{}", alarm.fact);
        assert!(alarm.fact.contains("on gold"), "{}", alarm.fact);
        assert!(
            alarm.running_default.contains("nothing recovers this automatically"),
            "{}",
            alarm.running_default
        );
        assert!(alarm.pos.is_none(), "an economy has no one place to point at");
    }

    #[test]
    fn a_working_mine_and_a_working_worker_raise_nothing() {
        let mut app = alarm_app();
        spawn_building(&mut app, Team::Human, BuildingKind::TownHall, Vec3::ZERO);
        let node = app
            .world_mut()
            .spawn((
                ResourceNode {
                    kind: ResourceKind::Gold,
                    remaining: 5000,
                },
                Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            ))
            .id();
        let worker = spawn_unit(
            &mut app,
            Team::Human,
            race_worker(Race::Kingdom),
            Vec3::new(2.0, 0.0, 0.0),
            None,
        );
        app.world_mut()
            .entity_mut(worker)
            .insert(Order::Harvest(node));

        run_for(&mut app, alarm_tuning(AlarmKind::IncomeCollapse).grace_s + 4.0);
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::IncomeCollapse).is_none(),
            "a live mine with a worker on it is not an income collapse"
        );
    }

    #[test]
    fn a_team_with_no_hall_gets_no_income_alarm() {
        let mut app = alarm_app();
        run_for(&mut app, alarm_tuning(AlarmKind::IncomeCollapse).grace_s + 2.0);
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::IncomeCollapse).is_none(),
            "no hall is not an income problem, it is the win check's problem"
        );
    }

    // -- 4. multiple places under attack ------------------------------------

    /// The headline case, and the one AFFORDANCES.md writes out longhand:
    /// two fronts, a home-guard trigger that has ALREADY fired, and a running
    /// default that names it with the recall ETA attached.
    #[test]
    fn two_fronts_raise_the_alarm_only_after_the_home_guard_and_carry_its_eta() {
        let mut app = alarm_app();
        // The reflex, armed the way a commander arms it — and evaluated by the
        // real trigger evaluator, in front of the alarm sweep.
        app.add_event::<SubmitIntent>()
            .init_resource::<Regions>()
            .init_resource::<TechTiers>()
            .init_resource::<Economies>()
            .add_systems(Update, evaluate_triggers.before(evaluate_alarms));
        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(TriggerRule {
                name: TriggerName::new("home-guard").unwrap(),
                when: TriggerWhen::BaseUnderAttack,
                then: Intent::Stop { units: vec![] },
                repeat: Some(30.0),
                source: IntentSource::Bridge,
                armed: true,
                last_fired: None,
            });

        let home = Team::Human.base_pos();
        let away = home + Vec3::new(80.0, 0.0, 80.0);
        let hall = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, home);
        let expansion = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, away);

        // A squad anchored on home, standing well off it, so there is a real
        // ETA to report.
        for i in 0..3 {
            spawn_unit(
                &mut app,
                Team::Human,
                UnitKind::Footman,
                home + Vec3::new(40.0 + i as f32 * 2.0, 0.0, 0.0),
                Some(1),
            );
        }
        app.world_mut().resource_mut::<SquadOrders>().0.insert(
            (Team::Human, 1),
            SquadPosture::Defend {
                pos: home,
                radius: 22.0,
            },
        );

        let grace = alarm_tuning(AlarmKind::PlacesUnderAttack).grace_s;
        let mut trigger_fired_at: Option<f32> = None;
        let mut alarm_fired_at: Option<f32> = None;
        for _ in 0..(grace.ceil() as u32 + 3) {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            // Keep both fronts inside `BASE_ATTACK_WINDOW_S`.
            for b in [hall, expansion] {
                app.world_mut().entity_mut(b).insert(LastDamaged { at: now });
            }
            app.update();
            if trigger_fired_at.is_none() {
                if let Some(t) = app.world().resource::<Triggers>().get(Team::Human)[0].last_fired {
                    trigger_fired_at = Some(t);
                }
            }
            if alarm_fired_at.is_none()
                && alarm_of(&app, Team::Human, AlarmKind::PlacesUnderAttack).is_some()
            {
                alarm_fired_at = Some(now);
            }
        }

        let reflex_at = trigger_fired_at.expect("the home-guard trigger fired");
        let alarm_at = alarm_fired_at.expect("two fronts is over the threshold");
        assert!(
            reflex_at < alarm_at,
            "the alarm ({alarm_at}) beat its reflex ({reflex_at}) — an alarm is \
             never the first responder"
        );

        let alarm = alarm_of(&app, Team::Human, AlarmKind::PlacesUnderAttack).unwrap();
        assert!(alarm.fact.starts_with("2 places under attack at once"), "{}", alarm.fact);
        assert!(
            alarm.running_default.contains("your trigger home-guard fired"),
            "the running default must name the reflex: {}",
            alarm.running_default
        );
        assert!(
            alarm.running_default.contains("squad 1 is closing on")
                && alarm.running_default.contains("ETA"),
            "and carry the recall ETA: {}",
            alarm.running_default
        );
        assert!(alarm.eta_s.is_some_and(|e| e > 0.0), "the ETA is a number too");
        assert_eq!(alarm.severity, EventSeverity::Critical);
    }

    #[test]
    fn one_front_is_not_the_multi_place_alarm() {
        let mut app = alarm_app();
        let home = Team::Human.base_pos();
        let hall = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, home);
        let barracks =
            spawn_building(&mut app, Team::Human, BuildingKind::Barracks, home + Vec3::new(6.0, 0.0, 0.0));
        for _ in 0..(alarm_tuning(AlarmKind::PlacesUnderAttack).grace_s.ceil() as u32 + 3) {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            for b in [hall, barracks] {
                app.world_mut().entity_mut(b).insert(LastDamaged { at: now });
            }
            app.update();
        }
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::PlacesUnderAttack).is_none(),
            "two buildings of one base are ONE place — that is base_under_attack, \
             which the feed and the trigger vocabulary already cover"
        );
    }

    #[test]
    fn with_nothing_moving_the_running_default_says_so() {
        let mut app = alarm_app();
        let home = Team::Human.base_pos();
        let away = home + Vec3::new(80.0, 0.0, 80.0);
        let hall = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, home);
        let expansion = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, away);
        for _ in 0..(alarm_tuning(AlarmKind::PlacesUnderAttack).grace_s.ceil() as u32 + 3) {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            for b in [hall, expansion] {
                app.world_mut().entity_mut(b).insert(LastDamaged { at: now });
            }
            app.update();
        }
        let alarm = alarm_of(&app, Team::Human, AlarmKind::PlacesUnderAttack).unwrap();
        assert!(
            alarm.running_default.contains("nothing is moving"),
            "{}",
            alarm.running_default
        );
        assert!(alarm.eta_s.is_none(), "nothing in transit is not an ETA of zero");
    }

    // -- the layer's own promises ------------------------------------------

    /// The `alarms` list is a STATUS and the feed line is an EDGE
    /// (BUILDER_BRIEF §6.11). A condition that stays true must not re-announce
    /// itself: that is the fire hose r23's blue seat drowned in.
    #[test]
    fn a_standing_alarm_announces_itself_exactly_once() {
        let mut app = alarm_app();
        let unit = spawn_unit(&mut app, Team::Human, UnitKind::Footman, Vec3::ZERO, Some(1));
        {
            let mut e = app.world_mut().entity_mut(unit);
            let mut hp = e.get_mut::<Health>().unwrap();
            hp.current = hp.max * 0.1;
        }
        run_for(&mut app, alarm_tuning(AlarmKind::SquadBelowHalf).grace_s + 30.0);
        let fires = feed_lines(&app, Team::Human)
            .iter()
            .filter(|l| l.starts_with("alarm: squad 1"))
            .count();
        assert_eq!(fires, 1, "an alarm that repeats is a stream, not a feed");
        assert!(
            alarm_of(&app, Team::Human, AlarmKind::SquadBelowHalf).is_some(),
            "and it is still standing in the status list, where a reader can look"
        );
    }

    /// The list is sorted by kind, not by arrival, so a commander diffing two
    /// polls sees WHICH alarms stand rather than the order they arrived in.
    #[test]
    fn the_alarm_list_is_ordered_by_kind_however_they_arrived() {
        let mut app = alarm_app();
        // Places first (grace 6s), then the squad (grace 4s) — but the squad
        // alarm sorts ahead of it.
        let home = Team::Human.base_pos();
        let away = home + Vec3::new(80.0, 0.0, 80.0);
        let hall = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, home);
        let expansion = spawn_building(&mut app, Team::Human, BuildingKind::TownHall, away);
        for _ in 0..10 {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            for b in [hall, expansion] {
                app.world_mut().entity_mut(b).insert(LastDamaged { at: now });
            }
            app.update();
        }
        let unit = spawn_unit(&mut app, Team::Human, UnitKind::Footman, home, Some(1));
        {
            let mut e = app.world_mut().entity_mut(unit);
            let mut hp = e.get_mut::<Health>().unwrap();
            hp.current = hp.max * 0.1;
        }
        for _ in 0..10 {
            advance(&mut app, 1.0);
            let now = now_of(&app);
            for b in [hall, expansion] {
                app.world_mut().entity_mut(b).insert(LastDamaged { at: now });
            }
            app.update();
        }
        let kinds: Vec<AlarmKind> = app
            .world()
            .resource::<Alarms>()
            .get(Team::Human)
            .iter()
            .map(|a| a.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![AlarmKind::SquadBelowHalf, AlarmKind::PlacesUnderAttack],
            "the list is in ALL_ALARM_KINDS order regardless of arrival order"
        );
    }

    /// The `AlarmPlugin` files its system where the module docs claim, so the
    /// structural half of "after the reflex" is not merely a comment.
    #[test]
    fn the_alarm_plugin_schedules_inside_the_reporting_phase() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_plugins((CorePlugin, TriggerPlugin, AlarmPlugin));
        // Two frames: the first builds and validates the schedule (a set
        // ordering contradiction is only caught then), the second proves the
        // world it left behind can be stepped again.
        app.update();
        app.update();
        assert!(app.world().get_resource::<Alarms>().is_some());
    }
}
