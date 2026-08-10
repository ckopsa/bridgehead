//! Shared contract between all game modules.
//! This file is owned by the integrator — module agents must NOT edit it.
//! Modules communicate exclusively through the types, events, and resources here.

use bevy::prelude::*;
use serde::Serialize;
use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Map constants
// ---------------------------------------------------------------------------

/// World spans -MAP_HALF..MAP_HALF on X and Z. Ground is the Y=0 plane.
pub const MAP_HALF: f32 = 100.0;
/// Nav grid cell size in world units.
pub const CELL: f32 = 2.0;
/// Nav grid is GRID_DIM x GRID_DIM cells.
pub const GRID_DIM: usize = 100;

pub const HUMAN_BASE: Vec3 = Vec3::new(-70.0, 0.0, -70.0);
pub const CLAUDE_BASE: Vec3 = Vec3::new(70.0, 0.0, 70.0);

/// Gold mines: one near each base, two neutral expansions.
pub const GOLD_MINE_POSITIONS: [Vec3; 4] = [
    Vec3::new(-82.0, 0.0, -55.0),
    Vec3::new(82.0, 0.0, 55.0),
    Vec3::new(-60.0, 0.0, 60.0),
    Vec3::new(60.0, 0.0, -60.0),
];

/// WC3-style unit oversizing: units render ~2x "realistic" scale so armies
/// read clearly against toy-scale buildings and can't hide behind them.
/// Gameplay stays on the same grid — only visuals and body radii scale.
pub const UNIT_SCALE: f32 = 1.9;
/// World-space body radius of a unit (picking, combat reach, separation).
pub const UNIT_RADIUS: f32 = 0.7 * UNIT_SCALE;

pub const STARTING_GOLD: u32 = 500;
pub const STARTING_LUMBER: u32 = 150;

// ---------------------------------------------------------------------------
// Upkeep: the long-game tax. The bigger your standing supply, the smaller the
// cut of each gold delivery that reaches your bank (lumber is untaxed).
// Applied by economy.rs at deposit time; shown in HUD and bridge snapshots.
// ---------------------------------------------------------------------------

pub const UPKEEP_NONE_MAX: u32 = 40;
pub const UPKEEP_LOW_MAX: u32 = 70;

pub fn upkeep_rate(supply_used: u32) -> f32 {
    if supply_used <= UPKEEP_NONE_MAX {
        1.0
    } else if supply_used <= UPKEEP_LOW_MAX {
        0.7
    } else {
        0.4
    }
}

pub fn upkeep_label(supply_used: u32) -> &'static str {
    if supply_used <= UPKEEP_NONE_MAX {
        "No Upkeep"
    } else if supply_used <= UPKEEP_LOW_MAX {
        "Low Upkeep"
    } else {
        "High Upkeep"
    }
}

/// Material worth of a team's remaining assets — the timeout tiebreaker.
/// Sum of unit + building costs (gold + lumber, weighted equally) plus bank.
pub fn asset_score(
    economy: &Economy,
    units: impl Iterator<Item = UnitKind>,
    buildings: impl Iterator<Item = BuildingKind>,
) -> u32 {
    let unit_value: u32 = units
        .map(|k| {
            let s = unit_stats(k);
            s.cost_gold + s.cost_lumber
        })
        .sum();
    // `building_value`, not `building_stats`, so a Keep counts as the hall
    // plus everything paid to raise it — upgrading is investment, not spending.
    let building_worth: u32 = buildings
        .map(|k| {
            let (gold, lumber) = building_value(k);
            gold + lumber
        })
        .sum();
    unit_value + building_worth + economy.gold + economy.lumber
}

// ---------------------------------------------------------------------------
// Teams
// ---------------------------------------------------------------------------

#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Team {
    Human,
    Claude,
}

impl Team {
    pub fn enemy(self) -> Team {
        match self {
            Team::Human => Team::Claude,
            Team::Claude => Team::Human,
        }
    }
    pub fn base_pos(self) -> Vec3 {
        match self {
            Team::Human => HUMAN_BASE,
            Team::Claude => CLAUDE_BASE,
        }
    }
    /// Team tint used by all meshes so factions are visually distinct.
    pub fn color(self) -> Color {
        match self {
            Team::Human => Color::srgb(0.2, 0.4, 0.9),
            Team::Claude => Color::srgb(0.9, 0.3, 0.2),
        }
    }
}

// ---------------------------------------------------------------------------
// Unit & building kinds + stats tables
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UnitKind {
    Worker,
    Footman,
    Archer,
    /// The Champion — one per team, levels up, casts an AoE slam. Entities of
    /// this kind always carry a `Hero` component (units.rs guarantees it).
    Hero,
    /// Siege engine: outranges towers, wrecks buildings, helpless up close.
    Catapult,
    /// Fast cavalry: dives siege engines and raids workers; dies to massed fire.
    Raider,
    /// The second hero class: ranged, heals allies instead of slamming enemies.
    /// Carries a `Hero` component like the Champion; one hero per team total.
    Priestess,
    /// Anti-cavalry line infantry: cheap, slow, and feeble against everything
    /// except a horse, which it deletes. The tier-1 answer to Raiders.
    Spearman,
}

/// Hero-class unit kinds (carry the `Hero` component, count against the
/// one-hero-per-team rule, revive through `HeroRecords`).
pub fn is_hero_kind(kind: UnitKind) -> bool {
    matches!(kind, UnitKind::Hero | UnitKind::Priestess)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BuildingKind {
    TownHall,
    Barracks,
    Farm,
    /// Static defense: shoots arrows at enemies in range. Requires a Barracks.
    Tower,
    /// Cheap blocking segment: no function except HP and a navgrid footprint.
    Wall,
    /// Siege works: trains Catapults. Requires a Barracks.
    Workshop,
    /// Item vendor: heroes buy consumables here.
    Shop,
    /// Tier 2 of the town hall ladder. Never placed — a TownHall upgrades into
    /// one in place (see `building_upgrades_to`). Trains everything the hall
    /// trained, and is the tech gate future tier-2 content names.
    Keep,
    /// Tier 3 of the town hall ladder, upgraded from a Keep.
    Castle,
}

pub const ALL_UNIT_KINDS: [UnitKind; 8] = [
    UnitKind::Worker,
    UnitKind::Footman,
    UnitKind::Archer,
    UnitKind::Hero,
    UnitKind::Catapult,
    UnitKind::Raider,
    UnitKind::Priestess,
    UnitKind::Spearman,
];
pub const ALL_BUILDING_KINDS: [BuildingKind; 9] = [
    BuildingKind::TownHall,
    BuildingKind::Barracks,
    BuildingKind::Farm,
    BuildingKind::Tower,
    BuildingKind::Wall,
    BuildingKind::Workshop,
    BuildingKind::Shop,
    BuildingKind::Keep,
    BuildingKind::Castle,
];

pub struct UnitStats {
    pub cost_gold: u32,
    pub cost_lumber: u32,
    pub supply: u32,
    pub hp: f32,
    pub damage: f32,
    /// Attack range in world units. Melee ~1.8.
    pub range: f32,
    /// Seconds between attacks.
    pub attack_cooldown: f32,
    /// World units per second.
    pub speed: f32,
    /// Seconds to train.
    pub train_time: f32,
    /// Ranged units fire a visible projectile.
    pub projectile: bool,
    /// Damage multiplier against buildings (1.0 for normal units; siege
    /// engines are the reason this exists). combat.rs applies it.
    pub vs_building_mult: f32,
    /// Damage multiplier against Catapults (cavalry's anti-siege role).
    pub vs_siege_mult: f32,
    /// Damage multiplier against `TargetClass::Cavalry` (the Spearman's
    /// anti-cavalry role). Large on purpose: a spear line is the only thing a
    /// 90g tier-1 unit can do to a 170g Raider, and without it nothing a team
    /// can build before a Workshop answers cavalry at all.
    pub vs_cavalry_mult: f32,

    // --- movement plane & attack envelope ---------------------------------
    /// Airborne: ignores the `NavGrid` entirely (straight-line paths over
    /// trees, mines, walls and buildings), renders at `FLYER_ALTITUDE`, and
    /// only jostles other flyers. The third answer to a tower turtle, after
    /// siege and cavalry: it simply refuses to use the door.
    pub flying: bool,
    /// May this kind attack AIRBORNE targets? The counter-triangle's spine:
    /// melee weapons cannot reach a flyer, missiles can.
    pub can_hit_air: bool,
    /// May this kind attack GROUND targets (units and buildings)? Every
    /// current kind can; the flag exists so a pure interceptor is data, not a
    /// code change.
    pub can_hit_ground: bool,

    // --- perception -------------------------------------------------------
    /// How far this kind SEES, in world units. The only input to fog of war,
    /// and deliberately independent of `range`: what a unit can shoot and what
    /// it can find are different questions, and the gap between them is where
    /// scouting lives. Catapults see less than they shoot (siege needs
    /// spotters); Raiders see far more than they can hit (cavalry is the
    /// scout). Exported in the catalog, so a commander can plan around it the
    /// same way the player reads it off the minimap.
    pub vision: f32,
}

/// How high above the ground plane flying units are drawn and held. Chosen to
/// clear every building silhouette while staying well inside the camera's
/// parallax budget, so a flyer still reads as being "over" the cell it
/// occupies. Range checks are all XZ, so altitude never changes a weapon's
/// effective reach.
pub const FLYER_ALTITUDE: f32 = 6.0;

/// Does this kind fly? The single source of truth every module asks.
pub fn is_flying_kind(kind: UnitKind) -> bool {
    unit_stats(kind).flying
}

/// Is this *target* airborne? Buildings never are, so the whole question
/// collapses to "is it a flying unit". Pass the target's `Unit` kind if it has
/// one — `None` means a building.
pub fn target_is_air(kind: Option<UnitKind>) -> bool {
    kind.is_some_and(is_flying_kind)
}

/// May `attacker` engage a target at the given altitude? combat.rs consults
/// this during acquisition, before every swing, and before retaliating, so a
/// melee unit can never lock onto something it is physically unable to reach.
pub fn unit_can_hit(attacker: UnitKind, target_is_air: bool) -> bool {
    let stats = unit_stats(attacker);
    if target_is_air {
        stats.can_hit_air
    } else {
        stats.can_hit_ground
    }
}

/// Anti-air is decided by ONE rule, applied to every kind below: a weapon that
/// leaves the hand can hit a flyer, a weapon swung by hand cannot. So archers,
/// the Priestess and towers shoot air; footmen, raiders, workers, militia and
/// the melee Champion cannot. Catapults are the deliberate exception to
/// "projectile == anti-air": siege is a ground bombardment weapon, and keeping
/// it air-blind is what makes flyers the clean counter to a siege push (which
/// is in turn the counter to the tower turtle flyers also bypass). Every
/// counter has a counter.
pub fn unit_stats(kind: UnitKind) -> UnitStats {
    match kind {
        UnitKind::Worker => UnitStats {
            cost_gold: 75, cost_lumber: 0, supply: 1, hp: 60.0, damage: 5.0,
            range: 1.8, attack_cooldown: 1.5, speed: 8.0, train_time: 8.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
            vision: 12.0,
        },
        UnitKind::Footman => UnitStats {
            cost_gold: 135, cost_lumber: 0, supply: 2, hp: 140.0, damage: 12.0,
            range: 2.0, attack_cooldown: 1.2, speed: 7.0, train_time: 12.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
            vision: 16.0,
        },
        // The line's anti-air: a footman screen is helpless overhead, archers
        // behind it are not.
        UnitKind::Archer => UnitStats {
            cost_gold: 90, cost_lumber: 30, supply: 2, hp: 70.0, damage: 14.0,
            range: 14.0, attack_cooldown: 1.5, speed: 7.0, train_time: 12.0, projectile: true,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: true, can_hit_ground: true,
            vision: 18.0,
        },
        // Base (level 1) stats; damage/HP grow per level — see `Hero`.
        // The Champion swings a greatsword: no reach into the air, and its
        // Slam is a ground shockwave (see `ability_of_unit`). A team that
        // plays the melee hero needs archers or towers to answer flyers.
        UnitKind::Hero => UnitStats {
            cost_gold: 400, cost_lumber: 100, supply: 5, hp: 320.0, damage: 24.0,
            range: 2.4, attack_cooldown: 1.1, speed: 7.5, train_time: 25.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
            vision: 20.0,
        },
        // Outranges towers (20 vs 16) and pulverizes structures, but 15 damage
        // vs units, 110 hp, and 4.5 speed means anything that reaches it wins.
        // Ground-only by design: a boulder lobbed at a wall cannot track a
        // flyer, so an all-in siege push is the thing air raiders punish.
        UnitKind::Catapult => UnitStats {
            cost_gold: 180, cost_lumber: 120, supply: 3, hp: 110.0, damage: 15.0,
            range: 20.0, attack_cooldown: 3.0, speed: 4.5, train_time: 22.0, projectile: true,
            vs_building_mult: 6.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
            vision: 14.0,
        },
        // Speed is the weapon: dives catapults (2x) and worker lines, melts
        // under focused fire. Gold-heavy so it competes with footmen for budget.
        UnitKind::Raider => UnitStats {
            cost_gold: 170, cost_lumber: 30, supply: 3, hp: 130.0, damage: 16.0,
            range: 2.2, attack_cooldown: 1.1, speed: 10.5, train_time: 16.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 2.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
            vision: 24.0,
        },
        // Ranged support hero: heals instead of slams. Base (level 1) stats.
        // Her bolts track upward, so the support hero is also the hero answer
        // to air.
        UnitKind::Priestess => UnitStats {
            cost_gold: 400, cost_lumber: 100, supply: 5, hp: 240.0, damage: 14.0,
            range: 10.0, attack_cooldown: 1.4, speed: 7.5, train_time: 25.0, projectile: true,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: true, can_hit_ground: true,
            vision: 18.0,
        },
        // The counter-triangle's missing third leg. Before this, a team that
        // met Raiders had nothing at tier 1 to answer them: Footmen are too
        // slow to catch cavalry and too expensive to trade with it, Archers
        // die to it. So the Spearman is deliberately BAD at everything else —
        // 6 damage on a 1.7s thrust is the worst dps in the game, and a
        // Footman beats one in a straight duel without dropping below half —
        // and it buys that weakness back at 5x against a horse. What it is
        // always worth is meat: 160 hp for 90 gold is the cheapest hit points
        // on the field, so a spear line in front of archers is a real
        // formation even in a match where the enemy never builds cavalry.
        // Slow (5.5) so it can screen but never chase; the counter is a wall
        // you walk cavalry into, not a hunter.
        UnitKind::Spearman => UnitStats {
            cost_gold: 90, cost_lumber: 0, supply: 2, hp: 160.0, damage: 6.0,
            range: 2.6, attack_cooldown: 1.7, speed: 5.5, train_time: 10.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 5.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
            // A picket that cannot see the riders coming is not a picket, so
            // the anti-cavalry line watches a little further than a footman.
            // Still well short of the Raider's 24: cavalry keeps the initiative
            // and gets to choose the engagement, which is what makes the
            // counter a wall you walk into rather than a patrol that hunts you.
            vision: 18.0,
        },
    }
}

/// Weapon on a building (towers). Always fires a projectile.
#[derive(Clone, Copy, Debug)]
pub struct BuildingAttack {
    pub damage: f32,
    pub range: f32,
    pub cooldown: f32,
    /// May this emplacement shoot airborne targets? Towers can — static
    /// defense is the one thing a flyer cannot simply walk around, so a base
    /// that invested in towers is never helpless against air.
    pub can_hit_air: bool,
}

pub struct BuildingStats {
    pub cost_gold: u32,
    pub cost_lumber: u32,
    pub hp: f32,
    /// Seconds to construct.
    pub build_time: f32,
    pub supply_provided: u32,
    /// Footprint edge length in world units (square footprint, blocks nav grid).
    pub size: f32,
    /// Some for buildings that fight back (towers). combat.rs executes.
    pub attack: Option<BuildingAttack>,
    /// How far this structure SEES. Buildings are a team's permanent eyes, so
    /// this is what makes a forward outpost worth its gold and a Tower more
    /// than a gun: a Tower sees 20 against a 16 attack range, so it is never
    /// shooting at something the team cannot also see. Vision applies while
    /// under construction too — a builder standing on a foundation is looking
    /// around.
    pub vision: f32,
}

pub fn building_stats(kind: BuildingKind) -> BuildingStats {
    match kind {
        BuildingKind::TownHall => BuildingStats {
            cost_gold: 385, cost_lumber: 205, hp: 1200.0, build_time: 40.0,
            supply_provided: 10, size: 8.0, attack: None, vision: 26.0,
        },
        BuildingKind::Barracks => BuildingStats {
            cost_gold: 160, cost_lumber: 60, hp: 700.0, build_time: 25.0,
            supply_provided: 0, size: 6.0, attack: None, vision: 18.0,
        },
        BuildingKind::Farm => BuildingStats {
            cost_gold: 80, cost_lumber: 20, hp: 350.0, build_time: 12.0,
            supply_provided: 6, size: 4.0, attack: None, vision: 12.0,
        },
        BuildingKind::Tower => BuildingStats {
            cost_gold: 110, cost_lumber: 80, hp: 550.0, build_time: 25.0,
            supply_provided: 0, size: 3.0,
            attack: Some(BuildingAttack {
                damage: 16.0, range: 16.0, cooldown: 1.3, can_hit_air: true,
            }),
            vision: 20.0,
        },
        BuildingKind::Wall => BuildingStats {
            cost_gold: 25, cost_lumber: 10, hp: 300.0, build_time: 8.0,
            supply_provided: 0, size: 2.0, attack: None, vision: 8.0,
        },
        BuildingKind::Workshop => BuildingStats {
            cost_gold: 140, cost_lumber: 100, hp: 550.0, build_time: 22.0,
            supply_provided: 0, size: 5.0, attack: None, vision: 16.0,
        },
        BuildingKind::Shop => BuildingStats {
            cost_gold: 75, cost_lumber: 60, hp: 400.0, build_time: 15.0,
            supply_provided: 0, size: 4.0, attack: None, vision: 14.0,
        },
        // Tier 2/3 halls. `cost_*` and `build_time` are the price and duration
        // of the UPGRADE STEP that produces them, not of a placement — these
        // kinds are unplaceable (`building_placeable`), so there is no other
        // reading, and `upgrade_cost` derives straight from this table.
        // Footprint stays 8.0 all the way up: an upgrade must never need room
        // the original hall did not already occupy, or a tightly packed base
        // could not tier up at all. HP is the visible reward for the money.
        //
        // Vision climbs with the rung for the same reason HP does. A hall is
        // its team's permanent eye over its own base (see TownHall's 26), and
        // the thing you are buying with an upgrade is a taller fortification —
        // so each rung watches a little further over its own ground. It is a
        // real if modest reward: at Castle the hall alone covers the whole
        // approach a Tower would, without the Tower. Strictly increasing, like
        // every other number on this ladder.
        BuildingKind::Keep => BuildingStats {
            cost_gold: 320, cost_lumber: 160, hp: 1700.0, build_time: 40.0,
            supply_provided: 10, size: 8.0, attack: None, vision: 30.0,
        },
        BuildingKind::Castle => BuildingStats {
            cost_gold: 480, cost_lumber: 240, hp: 2200.0, build_time: 50.0,
            supply_provided: 10, size: 8.0, attack: None, vision: 34.0,
        },
    }
}

// ---------------------------------------------------------------------------
// The upgrade ladder
// ---------------------------------------------------------------------------
//
// A building can convert IN PLACE into the next kind up its ladder: same
// entity, same position, same footprint, bigger body, bigger HP pool. Today
// the only ladder is TownHall -> Keep -> Castle, but nothing below names those
// kinds directly: the ladder is data (`building_upgrades_to`), the tier is
// derived from it, and requirement satisfaction is a tier COMPARISON rather
// than kind equality — so a Castle satisfies "requires Keep" for free, and a
// second ladder added later works without touching a single consumer.

/// The kind this one upgrades into, and the sole definition of the ladder.
pub fn building_upgrades_to(kind: BuildingKind) -> Option<BuildingKind> {
    match kind {
        BuildingKind::TownHall => Some(BuildingKind::Keep),
        BuildingKind::Keep => Some(BuildingKind::Castle),
        _ => None,
    }
}

/// The inverse of `building_upgrades_to`.
pub fn building_upgraded_from(kind: BuildingKind) -> Option<BuildingKind> {
    ALL_BUILDING_KINDS
        .into_iter()
        .find(|k| building_upgrades_to(*k) == Some(kind))
}

/// Gold, lumber and seconds to convert `kind` into its next tier. `None` for
/// anything at the top of its ladder (or not on one). The numbers live in
/// `building_stats` of the RESULT, so there is exactly one cost table.
pub fn upgrade_cost(kind: BuildingKind) -> Option<(u32, u32, f32)> {
    building_upgrades_to(kind).map(|to| {
        let s = building_stats(to);
        (s.cost_gold, s.cost_lumber, s.build_time)
    })
}

/// The tier-1 kind at the bottom of this kind's ladder (itself, for the
/// overwhelming majority that are not on one).
pub fn upgrade_root(kind: BuildingKind) -> BuildingKind {
    let mut current = kind;
    // The ladder is short and acyclic; the bound is pure paranoia.
    for _ in 0..ALL_BUILDING_KINDS.len() {
        match building_upgraded_from(current) {
            Some(prev) => current = prev,
            None => break,
        }
    }
    current
}

/// 1 for a base building, 2 and 3 for the rungs above it. This is the number
/// tier-gated content is written against ("requires tier 2") and the number
/// the catalog and every snapshot report.
pub fn building_tier(kind: BuildingKind) -> u32 {
    let mut tier = 1;
    let mut current = kind;
    for _ in 0..ALL_BUILDING_KINDS.len() {
        match building_upgraded_from(current) {
            Some(prev) => {
                tier += 1;
                current = prev;
            }
            None => break,
        }
    }
    tier
}

/// Can a worker place this kind directly? False for everything reachable only
/// by upgrading, which is what keeps a Keep out of the build menu, out of the
/// `build` bridge command, and out of the AI's build order.
pub fn building_placeable(kind: BuildingKind) -> bool {
    building_upgraded_from(kind).is_none()
}

/// Is a standing `owned` building enough to satisfy a requirement naming
/// `req`? Same ladder and at least as high answers yes — so "requires Keep" is
/// met by a Keep OR a Castle, and a team is never punished for teching up.
pub fn building_satisfies(owned: BuildingKind, req: BuildingKind) -> bool {
    upgrade_root(owned) == upgrade_root(req) && building_tier(owned) >= building_tier(req)
}

/// Everything on the town hall ladder. The one question the drop-off logic,
/// Town Portal, rally fallbacks and the AI's base bookkeeping actually mean
/// when they used to ask `kind == TownHall`.
pub fn is_hall(kind: BuildingKind) -> bool {
    upgrade_root(kind) == BuildingKind::TownHall
}

/// Total resources sunk into a building including every upgrade below it — a
/// Keep is a TownHall *plus* its upgrade. Used by `asset_score`, so teching up
/// can never lower a team's material worth.
pub fn building_value(kind: BuildingKind) -> (u32, u32) {
    let mut gold = 0;
    let mut lumber = 0;
    let mut current = Some(kind);
    for _ in 0..ALL_BUILDING_KINDS.len() {
        let Some(k) = current else { break };
        let s = building_stats(k);
        gold += s.cost_gold;
        lumber += s.cost_lumber;
        current = building_upgraded_from(k);
    }
    (gold, lumber)
}

/// A building converting in place. economy.rs inserts it (after taking the
/// money), ticks it down, and swaps `Building.kind` when it hits zero. While
/// it is present the building keeps its supply and its training QUEUE, but
/// trains nothing — the workforce is busy on the scaffolding.
#[derive(Component, Clone, Copy, Debug)]
pub struct Upgrading {
    /// What it becomes on completion.
    pub to: BuildingKind,
    pub remaining: f32,
    /// The full duration, so a renderer can show a progress fraction.
    pub total: f32,
}

/// Ask a building to start upgrading. Written by ui.rs, bridge.rs and ai.rs;
/// economy.rs validates (ours, finished, has a next tier, not already
/// upgrading) and pays — the same division of labour as `Order::Build`.
#[derive(Event, Debug)]
pub struct UpgradeBuilding {
    pub building: Entity,
}

/// Tech requirements: completed buildings a team must own before this
/// building may be PLACED. economy.rs enforces at placement; ui.rs greys the
/// button; bridge.rs reports and validates.
pub fn building_requires(kind: BuildingKind) -> &'static [BuildingKind] {
    match kind {
        BuildingKind::Tower | BuildingKind::Workshop => &[BuildingKind::Barracks],
        _ => &[],
    }
}

/// Tech requirements for TRAINING a unit (beyond owning its trainer building).
pub fn unit_requires(kind: UnitKind) -> &'static [BuildingKind] {
    match kind {
        // Round-4 balance: an ungated Raider rush killed 14 workers by t=228
        // before any reactive defense could finish. Workshop-gating makes
        // cavalry the mid-game flanking tool it was designed as.
        UnitKind::Raider => &[BuildingKind::Workshop],
        _ => &[],
    }
}

/// Does `team` satisfy `reqs` right now? Pass an iterator over the team's
/// COMPLETED building kinds.
///
/// Satisfaction is `building_satisfies`, not equality: a requirement naming a
/// tier is met by that tier or anything above it on the same ladder. A team
/// that upgraded its Keep into a Castle keeps everything the Keep unlocked.
pub fn requirements_met(
    reqs: &[BuildingKind],
    completed: impl Iterator<Item = BuildingKind> + Clone,
) -> bool {
    reqs.iter()
        .all(|r| completed.clone().any(|b| building_satisfies(b, *r)))
}

// ---------------------------------------------------------------------------
// Tech tier: how far up the tree a team has climbed
// ---------------------------------------------------------------------------

/// A team's tech tier. Today every team is `T1` from the first frame to the
/// last; the tier exists now because ability unlocks (`AbilityUnlock::TeamTier`)
/// and future upgrades need ONE thing to ask, and a predicate that reads a
/// stub is a predicate that never has to be rewritten.
/// A team's position on the tech ladder, as `AbilityUnlock::TeamTier` and any
/// future tier-gated content name it. Ordered, so "at least T2" is a `>=`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum TechTier {
    #[default]
    T1,
    T2,
    T3,
}

impl TechTier {
    pub fn name(self) -> &'static str {
        match self {
            TechTier::T1 => "T1",
            TechTier::T2 => "T2",
            TechTier::T3 => "T3",
        }
    }
    /// Numeric rung, for HUD captions and snapshot fields. `building_tier`
    /// answers the same question about a single building.
    #[allow(dead_code)]
    pub fn level(self) -> u32 {
        match self {
            TechTier::T1 => 1,
            TechTier::T2 => 2,
            TechTier::T3 => 3,
        }
    }
    /// The inverse: `building_tier` speaks in numbers, unlock predicates speak
    /// in tiers. Anything above the ladder's top clamps to T3.
    pub fn from_level(level: u32) -> TechTier {
        match level {
            0 | 1 => TechTier::T1,
            2 => TechTier::T2,
            _ => TechTier::T3,
        }
    }
}

/// The single function that decides a team's tier from its COMPLETED
/// buildings: the highest rung it holds on the town-hall ladder. A Keep makes
/// a team T2, a Castle T3, and losing the Keep drops it back — tier is a
/// property of what is standing, never a latch.
///
/// Derived, not enumerated: `is_hall` + `building_tier` mean a fourth rung, or
/// a second ladder promoted to count, changes this by changing the ladder data
/// and nothing here. `recount_tech_tiers` feeds the `TechTiers` resource from
/// here every frame, and every unlock predicate in the game reads that
/// resource — so this is the only place tier is decided.
pub fn tech_tier_for(completed: impl Iterator<Item = BuildingKind>) -> TechTier {
    let best = completed
        .filter(|kind| is_hall(*kind))
        .map(building_tier)
        .max()
        .unwrap_or(1);
    TechTier::from_level(best)
}

/// Per-team tech tier, recomputed from the world every frame (like supply) so
/// no module has to track a building finishing or dying.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TechTiers {
    pub human: TechTier,
    pub claude: TechTier,
}

impl TechTiers {
    pub fn get(&self, team: Team) -> TechTier {
        match team {
            Team::Human => self.human,
            Team::Claude => self.claude,
        }
    }
    pub fn set(&mut self, team: Team, tier: TechTier) {
        match team {
            Team::Human => self.human = tier,
            Team::Claude => self.claude = tier,
        }
    }
}

/// What each building can train.
pub fn trainable(kind: BuildingKind) -> &'static [UnitKind] {
    match kind {
        // The whole hall ladder trains the same roster: an upgraded hall is
        // still the hall. This is also what keeps a Keep counting as a
        // PRODUCTION building for the win condition — see `check_game_over`.
        BuildingKind::TownHall | BuildingKind::Keep | BuildingKind::Castle => {
            &[UnitKind::Worker, UnitKind::Hero, UnitKind::Priestess]
        }
        // Spearman is appended rather than slotted next to the Footman on
        // purpose: production hotkeys are positional, and moving Archer off W
        // to make room would retrain every existing pair of hands.
        BuildingKind::Barracks => &[
            UnitKind::Footman,
            UnitKind::Archer,
            UnitKind::Raider,
            UnitKind::Spearman,
        ],
        BuildingKind::Workshop => &[UnitKind::Catapult],
        BuildingKind::Farm | BuildingKind::Tower | BuildingKind::Wall | BuildingKind::Shop => &[],
    }
}

// ---------------------------------------------------------------------------
// The catalog: a declarative, serializable description of everything the game
// affords — what things are, what they cost, what unlocks them, where they
// come from. Single source of truth is the stats/requires tables above; this
// assembles them into one queryable structure. bridge.rs exports it to every
// commander seat; ui.rs may consult it for captions. Add content by extending
// the kind enums + tables — the catalog picks it up automatically.
// ---------------------------------------------------------------------------

pub fn kind_name(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Worker => "Worker",
        UnitKind::Footman => "Footman",
        UnitKind::Archer => "Archer",
        UnitKind::Hero => "Hero",
        UnitKind::Catapult => "Catapult",
        UnitKind::Raider => "Raider",
        UnitKind::Priestess => "Priestess",
        UnitKind::Spearman => "Spearman",
    }
}

pub fn building_name(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::TownHall => "TownHall",
        BuildingKind::Barracks => "Barracks",
        BuildingKind::Farm => "Farm",
        BuildingKind::Tower => "Tower",
        BuildingKind::Wall => "Wall",
        BuildingKind::Workshop => "Workshop",
        BuildingKind::Shop => "Shop",
        BuildingKind::Keep => "Keep",
        BuildingKind::Castle => "Castle",
    }
}

pub fn unit_description(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Worker => "Harvests gold/lumber, constructs buildings. Fights poorly.",
        UnitKind::Footman => "Cheap melee line unit. Tanks for archers.",
        UnitKind::Archer => "Long-range attacker. Fragile; keep behind footmen.",
        UnitKind::Hero => "The Champion: levels from nearby enemy deaths, AoE Slam ability, revivable at reduced cost with level preserved. One per team.",
        UnitKind::Catapult => "Siege engine: outranges towers, 6x damage vs buildings, but slow, fragile, and feeble against units. Escort it.",
        UnitKind::Raider => "Fast cavalry: 2x damage vs Catapults, excels at worker raids and map control. Melts under massed fire.",
        UnitKind::Priestess => "Support hero: ranged attack, Heal ability (AoE ally healing). One hero per team; revival preserves level and class.",
        UnitKind::Spearman => "Cheap anti-cavalry line: 5x damage vs Raiders, and the cheapest hit points in the game. Slow, and feeble against anything that isn't mounted.",
    }
}

pub fn building_description(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::TownHall => {
            "Tier 1 hall. Resource drop-off. Trains Workers and the Hero. Upgrades to a Keep."
        }
        BuildingKind::Keep => {
            "Tier 2 hall: everything the TownHall was, with a deeper HP pool. \
             Satisfies tier-1 requirements and unlocks tier-2 content. Upgrades to a Castle."
        }
        BuildingKind::Castle => {
            "Tier 3 hall: the top of the ladder. Satisfies every hall requirement below it."
        }
        BuildingKind::Barracks => "Trains Footmen, Archers and Spearmen (and Raiders, once a Workshop stands).",
        BuildingKind::Farm => "+6 supply. Build ahead of the cap or production stalls.",
        BuildingKind::Tower => "Static defense: shoots arrows at enemies in range.",
        BuildingKind::Wall => "Cheap blocking segment. No function except HP in the way.",
        BuildingKind::Workshop => "Siege works: trains Catapults. The answer to tower turtles.",
        BuildingKind::Shop => "Item vendor: heroes buy consumables here (see catalog items).",
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogUnit {
    pub id: &'static str,
    pub cost_gold: u32,
    pub cost_lumber: u32,
    pub supply: u32,
    pub hp: f32,
    pub damage: f32,
    pub range: f32,
    pub speed: f32,
    pub train_time: f32,
    pub vs_building_mult: f32,
    /// Airborne: ignores terrain and buildings when moving, and can only be
    /// attacked by things whose `can_hit_air` is true.
    pub flying: bool,
    pub can_hit_air: bool,
    pub can_hit_ground: bool,
    /// Sight radius — how far this kind lifts fog of war for its team.
    pub vision: f32,
    pub trained_at: &'static str,
    pub requires: Vec<&'static str>,
    pub description: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogAttack {
    pub damage: f32,
    pub range: f32,
    pub cooldown: f32,
    pub can_hit_air: bool,
}

/// One rung of an upgrade ladder, as it appears on the lower rung's catalog
/// entry. Everything needed to plan a tier-up is here: what you get, what it
/// costs, and how long the building is busy becoming it.
#[derive(Serialize, Clone, Debug)]
pub struct CatalogUpgrade {
    /// Catalog id of the resulting building.
    pub to: &'static str,
    pub cost_gold: u32,
    pub cost_lumber: u32,
    /// Seconds of in-place conversion. Training pauses for exactly this long.
    pub upgrade_time: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogBuilding {
    pub id: &'static str,
    /// For a placeable building, the price a worker pays to put it down. For an
    /// upgrade-only building (`placeable: false`) this is the price of the
    /// upgrade step that produces it — the same numbers as the lower rung's
    /// `upgrades_to`.
    pub cost_gold: u32,
    pub cost_lumber: u32,
    pub hp: f32,
    /// Seconds to construct — or, for an upgrade-only kind, to convert into.
    pub build_time: f32,
    pub supply_provided: u32,
    pub size: f32,
    pub attack: Option<CatalogAttack>,
    /// Sight radius — how far this structure lifts fog of war for its team.
    pub vision: f32,
    pub built_by: &'static str,
    pub requires: Vec<&'static str>,
    pub trains: Vec<&'static str>,
    /// Rung on this building's upgrade ladder: 1 for everything that is not
    /// upgraded from something else. A requirement naming a tier is satisfied
    /// by that tier OR ANY HIGHER one on the same ladder, so "requires Keep"
    /// is also met by a Castle.
    pub tier: u32,
    /// False for kinds that exist only as the result of an upgrade — they have
    /// no build button, no `build` command, and no place in a build order.
    pub placeable: bool,
    /// The next rung up, or null at the top of the ladder.
    pub upgrades_to: Option<CatalogUpgrade>,
    /// Catalog id of the rung below, or null for a base building.
    pub upgraded_from: Option<&'static str>,
    pub description: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogAbility {
    pub id: &'static str,
    pub caster: &'static str,
    /// Slot in the caster's ability list — what a `cast` command's `ability`
    /// field accepts as an integer, and the order the hotkeys follow.
    pub index: usize,
    pub effect: &'static str,
    /// Status kind applied, for `effect == "status"`.
    pub status: Option<&'static str>,
    pub mana_cost: f32,
    pub cooldown: f32,
    pub radius: f32,
    pub power: f32,
    /// Seconds the applied status lasts (0 for instant effects).
    pub duration: f32,
    pub hits_air: bool,
    /// Human-readable unlock condition: "always", "hero level N", "tier TN".
    pub unlock: String,
    pub description: &'static str,
}

/// Wire text for an unlock predicate — catalog and snapshot share it.
pub fn unlock_label(unlock: AbilityUnlock) -> String {
    match unlock {
        AbilityUnlock::Always => "always".to_string(),
        AbilityUnlock::HeroLevel(n) => format!("hero level {n}"),
        AbilityUnlock::TeamTier(t) => format!("tier {}", t.name()),
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogStatus {
    pub id: &'static str,
    /// "refresh" (strongest wins, no compounding) or "stack" (magnitudes add).
    pub stacking: &'static str,
    /// Ceiling on the summed magnitude.
    pub cap: f32,
    pub debuff: bool,
    pub description: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogItem {
    pub id: &'static str,
    pub cost_gold: u32,
    pub sold_at: &'static str,
    pub description: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct Catalog {
    pub units: Vec<CatalogUnit>,
    pub buildings: Vec<CatalogBuilding>,
    pub abilities: Vec<CatalogAbility>,
    pub items: Vec<CatalogItem>,
    /// The status-effect vocabulary: what a buff/debuff means and how it
    /// stacks, so a commander can reason about them without reading the source.
    pub statuses: Vec<CatalogStatus>,
}

/// Assemble the full content catalog from the stat/requirement tables.
pub fn game_catalog() -> Catalog {
    let trainer_of = |kind: UnitKind| {
        ALL_BUILDING_KINDS
            .iter()
            .find(|b| trainable(**b).contains(&kind))
            .map(|b| building_name(*b))
            .unwrap_or("-")
    };
    Catalog {
        units: ALL_UNIT_KINDS
            .iter()
            .map(|&k| {
                let s = unit_stats(k);
                CatalogUnit {
                    id: kind_name(k),
                    cost_gold: s.cost_gold,
                    cost_lumber: s.cost_lumber,
                    supply: s.supply,
                    hp: s.hp,
                    damage: s.damage,
                    range: s.range,
                    speed: s.speed,
                    train_time: s.train_time,
                    vs_building_mult: s.vs_building_mult,
                    flying: s.flying,
                    can_hit_air: s.can_hit_air,
                    can_hit_ground: s.can_hit_ground,
                    vision: s.vision,
                    trained_at: trainer_of(k),
                    requires: unit_requires(k).iter().map(|b| building_name(*b)).collect(),
                    description: unit_description(k),
                }
            })
            .collect(),
        buildings: ALL_BUILDING_KINDS
            .iter()
            .map(|&k| {
                let s = building_stats(k);
                CatalogBuilding {
                    id: building_name(k),
                    cost_gold: s.cost_gold,
                    cost_lumber: s.cost_lumber,
                    hp: s.hp,
                    build_time: s.build_time,
                    supply_provided: s.supply_provided,
                    size: s.size,
                    attack: s.attack.map(|a| CatalogAttack {
                        damage: a.damage,
                        range: a.range,
                        cooldown: a.cooldown,
                        can_hit_air: a.can_hit_air,
                    }),
                    vision: s.vision,
                    built_by: if building_placeable(k) { "Worker" } else { "Upgrade" },
                    requires: building_requires(k).iter().map(|b| building_name(*b)).collect(),
                    trains: trainable(k).iter().map(|u| kind_name(*u)).collect(),
                    tier: building_tier(k),
                    placeable: building_placeable(k),
                    upgrades_to: upgrade_cost(k).map(|(gold, lumber, time)| CatalogUpgrade {
                        to: building_name(
                            building_upgrades_to(k).expect("upgrade_cost implies a next tier"),
                        ),
                        cost_gold: gold,
                        cost_lumber: lumber,
                        upgrade_time: time,
                    }),
                    upgraded_from: building_upgraded_from(k).map(building_name),
                    description: building_description(k),
                }
            })
            .collect(),
        abilities: {
            let entry = |a: &AbilityDef, caster: &'static str, index: usize| CatalogAbility {
                id: a.name,
                caster,
                index,
                effect: a.effect.name(),
                status: a.effect.status().map(|s| s.name()),
                mana_cost: a.mana_cost,
                cooldown: a.cooldown,
                radius: a.radius,
                power: a.power,
                duration: a.duration,
                hits_air: a.hits_air,
                unlock: unlock_label(a.unlock),
                description: a.description,
            };
            let mut out = Vec::new();
            for &k in &ALL_UNIT_KINDS {
                for (i, a) in abilities_of_unit(k).iter().enumerate() {
                    out.push(entry(a, kind_name(k), i));
                }
            }
            for &k in &ALL_BUILDING_KINDS {
                for (i, a) in abilities_of_building(k).iter().enumerate() {
                    out.push(entry(a, building_name(k), i));
                }
            }
            out
        },
        items: ALL_ITEMS
            .iter()
            .map(|&id| {
                let d = item_def(id);
                CatalogItem {
                    id: d.name,
                    cost_gold: d.cost_gold,
                    sold_at: building_name(BuildingKind::Shop),
                    description: d.description,
                }
            })
            .collect(),
        statuses: ALL_STATUS_KINDS
            .iter()
            .map(|&k| CatalogStatus {
                id: k.name(),
                stacking: match k.stacking() {
                    StackPolicy::Refresh => "refresh",
                    StackPolicy::Stack => "stack",
                },
                cap: k.cap(),
                debuff: k.is_debuff(),
                description: k.description(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Core components
// ---------------------------------------------------------------------------

/// Marker + kind for units. Spawned/owned by units.rs.
#[derive(Component, Clone, Copy, Debug)]
pub struct Unit {
    pub kind: UnitKind,
}

/// Marker + kind for buildings. Spawned/owned by economy.rs.
#[derive(Component, Clone, Copy, Debug)]
pub struct Building {
    pub kind: BuildingKind,
}

#[derive(Component, Clone, Copy, Debug)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Health { current: max, max }
    }
}

/// Present while a building is being constructed. economy.rs owns this.
/// Building is functional (trains, provides supply) only when this is gone.
#[derive(Component, Debug)]
pub struct UnderConstruction {
    pub remaining: f32,
}

/// Training queue on production buildings. UI/AI push, economy.rs processes.
#[derive(Component, Default, Debug)]
pub struct TrainingQueue {
    pub queue: VecDeque<UnitKind>,
    /// Seconds of progress on the front item.
    pub progress: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceKind {
    Gold,
    Lumber,
}

/// Gold mines and trees. Spawned by terrain.rs; harvested/depleted by economy.rs.
#[derive(Component, Debug)]
pub struct ResourceNode {
    pub kind: ResourceKind,
    pub remaining: u32,
}

/// Resource a worker is carrying. economy.rs owns this.
#[derive(Component, Debug)]
pub struct Carrying {
    pub kind: ResourceKind,
    pub amount: u32,
}

/// Marker: entity is currently selected by the human player. ui.rs owns
/// adding/removing; other modules may read it.
#[derive(Component)]
pub struct Selected;

// ---------------------------------------------------------------------------
// Bounty caches: neutral treasure that spawns in the contested middle of the
// map and escalates in value, pulling both armies to the same place at the
// same time. bounty.rs spawns/expires/detects claims; economy.rs banks the
// gold (untaxed — treasure rewards the bold); ui.rs and bridge.rs surface it.
// ---------------------------------------------------------------------------

/// A treasure on the ground. First team with any unit within
/// BOUNTY_CLAIM_RADIUS claims it.
#[derive(Component, Clone, Copy, Debug)]
pub struct Bounty {
    pub gold: u32,
    /// Game-time (elapsed secs) when it vanishes unclaimed.
    pub expires_at: f32,
}

/// A team grabbed a bounty. economy.rs adds the gold (bypassing upkeep).
#[derive(Event, Debug)]
pub struct BountyClaim {
    pub team: Team,
    pub gold: u32,
    pub pos: Vec3,
}

/// Generous on purpose: walking PAST treasure should grab it. Playtest showed
/// troops standing 5-8 units from a cache watching it expire — any unit of
/// either team claims (no hero required), but only inside this radius.
pub const BOUNTY_CLAIM_RADIUS: f32 = 6.0;
/// Seconds between spawns (first one arrives after the opening settles).
pub const BOUNTY_INTERVAL: f32 = 90.0;
pub const BOUNTY_FIRST_AT: f32 = 150.0;
/// Unclaimed bounties vanish after this long.
pub const BOUNTY_LIFETIME: f32 = 75.0;
/// Bounties spawn this far from map center (contested ring, away from bases).
pub const BOUNTY_RING_MIN: f32 = 10.0;
pub const BOUNTY_RING_MAX: f32 = 45.0;

/// Escalating value: 150 gold early, +5 per 10s of game time, UNCAPPED —
/// organic sudden death. By minute 20 each cache is ~750, by minute 30 over
/// 1000: the longer a game drags, the more every midfield fight is worth,
/// until refusing battle IS losing. No clocks; the map itself raises the
/// stakes.
pub fn bounty_value(elapsed_secs: f32) -> u32 {
    150 + (elapsed_secs * 0.5) as u32
}

// ---------------------------------------------------------------------------
// Out-of-combat regeneration
// ---------------------------------------------------------------------------

/// Game-time (elapsed secs) this entity last took damage. combat.rs stamps it
/// in its damage application; the regen system here reads it. Absent = never
/// damaged (eligible to regen, though it'll be at full HP anyway).
#[derive(Component, Clone, Copy, Debug)]
pub struct LastDamaged {
    pub at: f32,
}

// ---------------------------------------------------------------------------
// Status effects: timed stat modifiers (buffs & debuffs)
// ---------------------------------------------------------------------------
//
// A status effect is a magnitude plus a deadline living on an entity. Nothing
// in the simulation is allowed to read a modified stat off the raw tables —
// `effective_stats` below is the ONE place base numbers turn into the numbers
// the game actually runs on, and units.rs / combat.rs both go through it.
//
// The lifecycle is central, exactly like `Militia` and `LastDamaged`: content
// (abilities, items, auras) only ever calls `StatusEffects::apply`, and
// `tick_status_effects` here expires everything and pays out heal-over-time.
// A content bead therefore never writes an expiry system of its own, and can
// never leave a buff stuck on a unit.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StatusKind {
    /// Move AND attack speed down (magnitude = fraction removed, 0.35 = -35%).
    Slow,
    /// Move speed up (magnitude = fraction added).
    Haste,
    /// Incoming damage down (magnitude = fraction removed).
    ArmorBuff,
    /// Outgoing damage up (magnitude = fraction added).
    DamageBuff,
    /// HP per second while it lasts (magnitude = HP/s).
    HealOverTime,
}

pub const ALL_STATUS_KINDS: [StatusKind; 5] = [
    StatusKind::Slow,
    StatusKind::Haste,
    StatusKind::ArmorBuff,
    StatusKind::DamageBuff,
    StatusKind::HealOverTime,
];

/// What happens when the same kind lands on a unit twice.
///
/// The rule is one line and it is deliberate: **debuffs refresh, buffs stack**.
/// Two sorcerers slowing the same footman must not stop it dead (that is a
/// stun, and a stun is a different design decision), so `Slow` takes the
/// strongest magnitude and the latest deadline and compounds with nothing.
/// Buffs are things a team *paid* for — a second banner, a second potion — so
/// they add, bounded by the kind's cap so the ceiling is data, not an accident.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StackPolicy {
    /// One instance per kind: magnitude = max of the two, expiry = later of the
    /// two. A weaker or shorter re-application is silently absorbed.
    Refresh,
    /// Instances coexist; `magnitude` sums them, capped by `StatusKind::cap`.
    Stack,
}

/// Where an effect came from. Purely descriptive today (the HUD and the bridge
/// report it), but it is what lets a future dispel say "remove ability
/// debuffs, leave item buffs alone" without inventing a parallel tag.
// Item/Aura have no producer until the shop-items and banner beads land.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusSource {
    Ability,
    Item,
    Aura,
    /// Dev probes and tests.
    Debug,
}

impl StatusKind {
    pub fn name(self) -> &'static str {
        match self {
            StatusKind::Slow => "Slow",
            StatusKind::Haste => "Haste",
            StatusKind::ArmorBuff => "ArmorBuff",
            StatusKind::DamageBuff => "DamageBuff",
            StatusKind::HealOverTime => "HealOverTime",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            StatusKind::Slow => "Move and attack speed reduced.",
            StatusKind::Haste => "Move speed increased.",
            StatusKind::ArmorBuff => "Incoming damage reduced.",
            StatusKind::DamageBuff => "Outgoing damage increased.",
            StatusKind::HealOverTime => "Regenerates HP every second.",
        }
    }
    /// Debuffs are the effects an ENEMY put on you.
    pub fn is_debuff(self) -> bool {
        matches!(self, StatusKind::Slow)
    }
    pub fn stacking(self) -> StackPolicy {
        if self.is_debuff() {
            StackPolicy::Refresh
        } else {
            StackPolicy::Stack
        }
    }
    /// Ceiling on the summed magnitude. Fractions stay strictly below 1.0 so no
    /// amount of stacking can zero a stat out or invert a sign.
    pub fn cap(self) -> f32 {
        match self {
            StatusKind::Slow => 0.75,
            StatusKind::Haste => 1.0,
            StatusKind::ArmorBuff => 0.8,
            StatusKind::DamageBuff => 2.0,
            StatusKind::HealOverTime => 60.0,
        }
    }
    /// Colour of the ground ring combat.rs draws under an affected unit. One
    /// ring per unit, so a unit carrying several effects shows the one with the
    /// lowest `ring_rank` — the bad news first.
    pub fn tint(self) -> Color {
        match self {
            StatusKind::Slow => Color::srgb(0.45, 0.35, 0.95),
            StatusKind::Haste => Color::srgb(0.35, 0.95, 0.95),
            StatusKind::ArmorBuff => Color::srgb(0.85, 0.85, 0.35),
            StatusKind::DamageBuff => Color::srgb(1.0, 0.45, 0.2),
            StatusKind::HealOverTime => Color::srgb(0.35, 1.0, 0.5),
        }
    }
    fn ring_rank(self) -> u8 {
        match self {
            StatusKind::Slow => 0,
            StatusKind::HealOverTime => 1,
            StatusKind::ArmorBuff => 2,
            StatusKind::DamageBuff => 3,
            StatusKind::Haste => 4,
        }
    }
}

/// One live modifier on one entity.
#[derive(Clone, Copy, Debug)]
pub struct StatusEffect {
    pub kind: StatusKind,
    /// Meaning depends on the kind — see `StatusKind`.
    pub magnitude: f32,
    /// Game time (`Time::elapsed_secs`) at which this instance dies.
    pub expires_at: f32,
    pub source: StatusSource,
}

impl StatusEffect {
    /// Build an instance that lasts `duration` seconds from `now`.
    pub fn new(
        kind: StatusKind,
        magnitude: f32,
        now: f32,
        duration: f32,
        source: StatusSource,
    ) -> Self {
        StatusEffect {
            kind,
            magnitude: magnitude.max(0.0),
            expires_at: now + duration.max(0.0),
            source,
        }
    }
}

/// Every live effect on one entity. Absent component == no effects, which is
/// the common case, so nothing pays for the framework until it is used.
/// Inserted by whoever applies the first effect; REMOVED centrally by
/// `tick_status_effects` once the last one expires.
#[derive(Component, Clone, Debug, Default)]
pub struct StatusEffects {
    active: Vec<StatusEffect>,
}

// The full framework surface, several of whose readers are the content beads
// consuming this one.
#[allow(dead_code)]
impl StatusEffects {
    pub fn new() -> Self {
        Self::default()
    }
    /// The only way effects get on a unit. Honours the kind's `StackPolicy`.
    pub fn apply(&mut self, effect: StatusEffect) {
        if effect.kind.stacking() == StackPolicy::Refresh {
            if let Some(existing) = self.active.iter_mut().find(|e| e.kind == effect.kind) {
                existing.magnitude = existing.magnitude.max(effect.magnitude);
                existing.expires_at = existing.expires_at.max(effect.expires_at);
                existing.source = effect.source;
                return;
            }
        }
        self.active.push(effect);
    }
    /// Summed (capped) magnitude of one kind. This is what `effective_stats`
    /// reads; nothing else should be doing arithmetic on instances.
    pub fn magnitude(&self, kind: StatusKind) -> f32 {
        let total: f32 = self
            .active
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.magnitude)
            .sum();
        total.min(kind.cap())
    }
    pub fn has(&self, kind: StatusKind) -> bool {
        self.active.iter().any(|e| e.kind == kind)
    }
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &StatusEffect> + '_ {
        self.active.iter()
    }
    /// Seconds until the last instance of `kind` runs out (0 = not present).
    pub fn remaining(&self, kind: StatusKind, now: f32) -> f32 {
        self.active
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.expires_at - now)
            .fold(0.0f32, f32::max)
            .max(0.0)
    }
    /// Drop everything past its deadline. Returns true if anything was removed.
    pub fn expire(&mut self, now: f32) -> bool {
        let before = self.active.len();
        self.active.retain(|e| e.expires_at > now);
        self.active.len() != before
    }
    /// The kind that gets to colour this unit's ring.
    pub fn dominant(&self) -> Option<StatusKind> {
        self.active
            .iter()
            .map(|e| e.kind)
            .min_by_key(|k| k.ring_rank())
    }
}

/// Raw numbers a modifier is applied TO. Comes from `unit_stats` for units and
/// from `BuildingAttack` for towers; `BaseStats::STATIC` is the "no weapon, no
/// legs" case used when all that is being asked is a damage multiplier.
#[derive(Clone, Copy, Debug)]
pub struct BaseStats {
    pub speed: f32,
    pub attack_cooldown: f32,
}

impl BaseStats {
    /// Nothing moves, nothing swings — for buildings taking a hit.
    pub const STATIC: BaseStats = BaseStats { speed: 0.0, attack_cooldown: 0.0 };
    pub fn of_unit(kind: UnitKind) -> Self {
        let s = unit_stats(kind);
        BaseStats { speed: s.speed, attack_cooldown: s.attack_cooldown }
    }
    pub fn of_building_attack(attack: &BuildingAttack) -> Self {
        BaseStats { speed: 0.0, attack_cooldown: attack.cooldown }
    }
}

/// What the simulation must actually use.
#[derive(Clone, Copy, Debug)]
pub struct EffectiveStats {
    /// World units per second.
    pub speed: f32,
    /// Seconds between attacks.
    pub attack_cooldown: f32,
    /// Multiplier on damage this entity DEALS.
    pub damage_mult: f32,
    /// Multiplier on damage this entity TAKES.
    pub damage_taken_mult: f32,
    /// HP per second this entity regains from HealOverTime.
    pub heal_per_second: f32,
}

/// **THE modifier function.** One law, one place. `units.rs` asks it for move
/// speed, `combat.rs` asks it for attack cooldown, outgoing damage and incoming
/// damage — none of them read `unit_stats(kind).speed` (or `.attack_cooldown`)
/// straight any more, because a stat that can be buffed has to be asked for,
/// not looked up.
///
/// Slow eats move speed and attack speed together (an attack "cooldown" is the
/// reciprocal of attack speed, so it is divided, not multiplied — that is the
/// whole reason this arithmetic lives in one function instead of five call
/// sites). Haste is legs only.
pub fn effective_stats(base: BaseStats, status: Option<&StatusEffects>) -> EffectiveStats {
    let mut out = EffectiveStats {
        speed: base.speed,
        attack_cooldown: base.attack_cooldown,
        damage_mult: 1.0,
        damage_taken_mult: 1.0,
        heal_per_second: 0.0,
    };
    let Some(status) = status else {
        return out;
    };
    let slow = status.magnitude(StatusKind::Slow);
    let haste = status.magnitude(StatusKind::Haste);
    let armor = status.magnitude(StatusKind::ArmorBuff);
    let power = status.magnitude(StatusKind::DamageBuff);

    // (1 - slow) can never reach 0: `StatusKind::cap` keeps Slow below 1.
    let slow_mult = (1.0 - slow).max(1.0 - StatusKind::Slow.cap());
    out.speed = base.speed * slow_mult * (1.0 + haste);
    out.attack_cooldown = base.attack_cooldown / slow_mult;
    out.damage_mult = 1.0 + power;
    out.damage_taken_mult = (1.0 - armor).max(1.0 - StatusKind::ArmorBuff.cap());
    out.heal_per_second = status.magnitude(StatusKind::HealOverTime);
    out
}

/// Convenience wrapper for the common "a unit of kind K with these effects".
pub fn effective_unit_stats(kind: UnitKind, status: Option<&StatusEffects>) -> EffectiveStats {
    effective_stats(BaseStats::of_unit(kind), status)
}

/// Units (heroes included) heal this fraction of max HP per second once out
/// of combat for UNIT_REGEN_DELAY seconds.
pub const UNIT_REGEN_DELAY: f32 = 12.0;
pub const UNIT_REGEN_RATE: f32 = 0.015;
/// Buildings recover much more slowly — harassment still leaves a mark, and
/// tower duels are still decided before regen matters.
pub const BUILDING_REGEN_DELAY: f32 = 20.0;
pub const BUILDING_REGEN_RATE: f32 = 0.005;

// ---------------------------------------------------------------------------
// Heroes
// ---------------------------------------------------------------------------

pub const HERO_MAX_LEVEL: u32 = 10;
/// Heroes within this XZ range of a dying enemy earn its XP.
pub const HERO_XP_RADIUS: f32 = 30.0;
pub const HERO_MANA_REGEN: f32 = 1.5;
/// The Champion's "Slam": AoE damage around the hero.
pub const HERO_ABILITY_COST: f32 = 40.0;
pub const HERO_ABILITY_COOLDOWN: f32 = 10.0;
pub const HERO_ABILITY_RADIUS: f32 = 7.0;
pub const HERO_ABILITY_DAMAGE: f32 = 45.0;
/// Reviving a fallen hero (level preserved) is cheaper and faster than the
/// first training. See `hero_train_cost`.
pub const HERO_REVIVE_COST_GOLD: u32 = 250;
pub const HERO_REVIVE_TIME: f32 = 15.0;

/// Per-entity hero state. Lives on `UnitKind::Hero` units (units.rs inserts it
/// at spawn, restoring level/xp from `HeroRecords`). shared.rs owns mana
/// regen, XP awarding, and level-ups; combat.rs reads `damage_mult` and spends
/// mana when executing `CastAbility`.
///
/// Cooldowns are NOT here: a hero has a list of abilities now, so they live in
/// the per-entity, per-ability `AbilityCooldowns` component that heroes and
/// building casters share.
#[derive(Component, Clone, Copy, Debug)]
pub struct Hero {
    pub level: u32,
    pub xp: f32,
    pub mana: f32,
}

impl Hero {
    pub fn from_record(record: Option<HeroRecord>) -> Self {
        let (level, xp) = record.map_or((1, 0.0), |r| (r.level, r.xp));
        Hero { level, xp, mana: Self::max_mana(level) }
    }
    pub fn max_mana(level: u32) -> f32 {
        80.0 + 20.0 * (level.saturating_sub(1)) as f32
    }
    /// XP needed to go from `level` to `level + 1`.
    pub fn xp_to_next(level: u32) -> f32 {
        100.0 + 80.0 * (level.saturating_sub(1)) as f32
    }
    /// Multiplier applied to base damage (attacks and the slam).
    pub fn damage_mult(level: u32) -> f32 {
        1.0 + 0.15 * (level.saturating_sub(1)) as f32
    }
    /// Extra max HP on top of the base stats.
    pub fn bonus_hp(level: u32) -> f32 {
        40.0 * (level.saturating_sub(1)) as f32
    }
    /// Class-aware max HP (Champion and Priestess have different bases).
    pub fn max_hp_for(kind: UnitKind, level: u32) -> f32 {
        unit_stats(kind).hp + Self::bonus_hp(level)
    }
}

// ---------------------------------------------------------------------------
// Abilities v2: a LIST per caster kind, described as data, with per-ability
// unlock conditions and per-ability cooldowns.
// ---------------------------------------------------------------------------
//
// v1 was "one ability per caster, inferred from the caster's kind". That made
// the caster the identity of the ability, so a second Champion spell, a shop
// item that grants one, or a tier-3 upgrade all had nowhere to go. v2 keeps
// everything data:
//
//   * `abilities_of_unit` / `abilities_of_building` return a `&'static` LIST;
//     an ability's INDEX in that list is its handle everywhere (hotkey slot,
//     cooldown slot, autocast rule, bridge selector).
//   * each entry carries an `AbilityUnlock` predicate — always, hero level, or
//     team tech tier — evaluated against an `UnlockCtx`.
//   * `CastAbility` carries an optional `AbilitySelector`; `None` means "first
//     unlocked", which is exactly v1's behaviour, so every old call site and
//     the old bridge `cast` command keep working untouched.
//   * cooldowns live in `AbilityCooldowns`, one slot per index, shared by hero
//     and building casters (heroes additionally pay mana).
//
// Content beads add abilities by adding table rows. No system changes.

/// Who an AoE effect looks for. Damage and Heal have this baked in
/// historically; `ApplyStatus` has to be told, because a Slow and a Warcry are
/// the same machinery pointed at different people.
// Allies/OwnWorkers wait on the buff-ability content beads.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilityTargets {
    Enemies,
    /// Own units (buildings are never "allies" for buff purposes).
    Allies,
    OwnWorkers,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilityEffect {
    /// AoE damage around the caster (scaled by hero level).
    Damage,
    /// AoE ally healing around the caster (scaled by hero level).
    Heal,
    /// Own workers in radius become Militia for `power` seconds.
    Militia,
    /// AoE timed status effect: `power` is the magnitude, `duration` the
    /// seconds. This is the variant that makes the whole status framework
    /// reachable from pure data — Sorcerer's Slow, Boots of Speed, Warcry and
    /// Sanctuary are all table rows, not code.
    ApplyStatus {
        status: StatusKind,
        targets: AbilityTargets,
    },
}

impl AbilityEffect {
    /// Wire name used by the catalog and the bridge snapshot.
    pub fn name(self) -> &'static str {
        match self {
            AbilityEffect::Damage => "damage",
            AbilityEffect::Heal => "heal",
            AbilityEffect::Militia => "militia",
            AbilityEffect::ApplyStatus { .. } => "status",
        }
    }
    /// The status kind this effect applies, when it applies one.
    pub fn status(self) -> Option<StatusKind> {
        match self {
            AbilityEffect::ApplyStatus { status, .. } => Some(status),
            _ => None,
        }
    }
}

/// When an ability becomes castable.
// `HeroLevel` has no shipping ability behind it yet — the hero ultimates bead
// is what fills it in; the predicate and its tests exist so that bead is data.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilityUnlock {
    /// Available from the moment the caster exists.
    Always,
    /// Hero must have reached this level (non-hero casters never satisfy it).
    HeroLevel(u32),
    /// The caster's team must be at this tech tier or better.
    TeamTier(TechTier),
}

#[derive(Clone, Copy, Debug)]
pub struct AbilityDef {
    /// Stable id: the catalog key, the bridge selector, the button caption.
    pub name: &'static str,
    pub effect: AbilityEffect,
    pub mana_cost: f32,
    pub cooldown: f32,
    pub radius: f32,
    /// Damage / heal amount, duration seconds for Militia, or the status
    /// MAGNITUDE for `ApplyStatus`.
    pub power: f32,
    /// Seconds a status effect lasts. `ApplyStatus` only; 0.0 elsewhere.
    pub duration: f32,
    /// Does the effect reach AIRBORNE units in its radius? A shockwave that
    /// travels along the ground does not; healing light does. combat.rs
    /// filters by this, and doctrine.rs will not auto-cast at targets the
    /// ability cannot affect.
    pub hits_air: bool,
    pub unlock: AbilityUnlock,
    pub description: &'static str,
}

const SLAM: AbilityDef = AbilityDef {
    name: "Slam",
    effect: AbilityEffect::Damage,
    mana_cost: HERO_ABILITY_COST,
    cooldown: HERO_ABILITY_COOLDOWN,
    radius: HERO_ABILITY_RADIUS,
    power: HERO_ABILITY_DAMAGE,
    duration: 0.0,
    // The Champion slams the ground. Flyers overhead feel nothing — the melee
    // hero's air answer is his archers, not his ability.
    hits_air: false,
    unlock: AbilityUnlock::Always,
    description: "AoE damage around the Champion (ground only), scales with level.",
};

const HEAL: AbilityDef = AbilityDef {
    name: "Heal",
    effect: AbilityEffect::Heal,
    mana_cost: 45.0,
    cooldown: 12.0,
    radius: 8.0,
    power: 60.0,
    duration: 0.0,
    // Healing light reaches up: air allies are still your allies.
    hits_air: true,
    unlock: AbilityUnlock::Always,
    description: "Restores HP to all allies around the Priestess, air included, scales with level.",
};

const CALL_TO_ARMS: AbilityDef = AbilityDef {
    name: "CallToArms",
    effect: AbilityEffect::Militia,
    mana_cost: 0.0,
    cooldown: 90.0,
    radius: 16.0,
    power: 40.0,
    duration: 0.0,
    // Workers are ground units, so this never had an air question.
    hits_air: false,
    unlock: AbilityUnlock::Always,
    description: "Own workers near the TownHall become fighters for 40s.",
};

/// Dev-only second Champion ability, present only under `WC3_STATUS_PROBE=1`.
/// It exists so a real match can exercise the v2 path end to end — a second
/// ability on a caster, a level-gated unlock, its own cooldown slot, an
/// explicit selector, and an `ApplyStatus` effect feeding the status framework
/// — without shipping balance content the content beads have not designed yet.
const PROBE_CHILL: AbilityDef = AbilityDef {
    name: "ProbeChill",
    effect: AbilityEffect::ApplyStatus {
        status: StatusKind::Slow,
        targets: AbilityTargets::Enemies,
    },
    mana_cost: 10.0,
    cooldown: 8.0,
    radius: 9.0,
    power: 0.4,
    duration: 6.0,
    hits_air: false,
    // Gated on the TEAM TIER on purpose: this is the live test of the join
    // between the ability framework and the hall ladder. It is locked while a
    // team has only a TownHall and opens the moment its Keep finishes.
    unlock: AbilityUnlock::TeamTier(TechTier::T2),
    description: "Dev probe: slows enemies around the Champion by 40% for 6s. Requires tier 2.",
};

const NO_ABILITIES: [AbilityDef; 0] = [];
const HERO_ABILITIES: [AbilityDef; 1] = [SLAM];
const HERO_ABILITIES_PROBE: [AbilityDef; 2] = [SLAM, PROBE_CHILL];
const PRIESTESS_ABILITIES: [AbilityDef; 1] = [HEAL];
const TOWNHALL_ABILITIES: [AbilityDef; 1] = [CALL_TO_ARMS];

/// `WC3_STATUS_PROBE=1`: dev instrumentation for the status + ability-v2
/// frameworks. Read once per process so the ability tables stay constant for
/// the whole run.
pub fn status_probe_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WC3_STATUS_PROBE").is_ok_and(|v| v != "0"))
}

/// Every ability this unit kind can ever cast, unlocked or not, in slot order.
pub fn abilities_of_unit(kind: UnitKind) -> &'static [AbilityDef] {
    match kind {
        UnitKind::Hero => {
            if status_probe_enabled() {
                &HERO_ABILITIES_PROBE
            } else {
                &HERO_ABILITIES
            }
        }
        UnitKind::Priestess => &PRIESTESS_ABILITIES,
        _ => &NO_ABILITIES,
    }
}

/// Every ability this building kind can ever cast, in slot order.
pub fn abilities_of_building(kind: BuildingKind) -> &'static [AbilityDef] {
    // The whole hall ladder keeps Call to Arms: an upgrade must never take an
    // ability away from the player who paid for it. Asked as `is_hall` rather
    // than by naming the three kinds, so a fourth rung inherits it for free.
    if is_hall(kind) {
        &TOWNHALL_ABILITIES
    } else {
        &NO_ABILITIES
    }
}

/// Everything an unlock predicate needs to know about a caster.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnlockCtx {
    /// 0 for a caster that is not a hero.
    pub hero_level: u32,
    pub tier: TechTier,
}

impl UnlockCtx {
    pub fn new(hero_level: u32, tier: TechTier) -> Self {
        UnlockCtx { hero_level, tier }
    }
    /// A building caster: no level, just the team's tier.
    pub fn building(tier: TechTier) -> Self {
        UnlockCtx { hero_level: 0, tier }
    }
}

pub fn ability_unlocked(def: &AbilityDef, ctx: UnlockCtx) -> bool {
    match def.unlock {
        AbilityUnlock::Always => true,
        AbilityUnlock::HeroLevel(n) => ctx.hero_level >= n,
        AbilityUnlock::TeamTier(t) => ctx.tier >= t,
    }
}

/// Slot indices of the abilities this caster may use right now, in slot order.
pub fn unlocked_abilities(list: &[AbilityDef], ctx: UnlockCtx) -> Vec<usize> {
    list.iter()
        .enumerate()
        .filter(|(_, def)| ability_unlocked(def, ctx))
        .map(|(i, _)| i)
        .collect()
}

/// The default target of a selector-less `CastAbility` — v1's behaviour.
pub fn first_unlocked_ability(list: &[AbilityDef], ctx: UnlockCtx) -> Option<usize> {
    list.iter().position(|def| ability_unlocked(def, ctx))
}

/// Slot of the ability with this id (case-insensitive), unlocked or not.
pub fn ability_index_by_id(list: &[AbilityDef], id: &str) -> Option<usize> {
    list.iter().position(|def| def.name.eq_ignore_ascii_case(id))
}

/// Which ability of `list` a cast request means. `None` selector = the first
/// unlocked one; an explicit selector that names a locked or missing ability
/// resolves to `None` and the cast is refused by the caller.
pub fn resolve_ability(
    list: &[AbilityDef],
    selector: Option<&AbilitySelector>,
    ctx: UnlockCtx,
) -> Option<usize> {
    let index = match selector {
        None => return first_unlocked_ability(list, ctx),
        Some(AbilitySelector::Index(i)) => *i,
        Some(AbilitySelector::Id(id)) => ability_index_by_id(list, id)?,
    };
    let def = list.get(index)?;
    ability_unlocked(def, ctx).then_some(index)
}

/// May this caster fire ability `def` in slot `index` right now? The one gate:
/// cooldown, plus mana for heroes. combat.rs, ui.rs, doctrine.rs and ai.rs all
/// ask here, so the button, the auto-cast and the executor can never disagree.
pub fn ability_ready(
    def: &AbilityDef,
    hero: Option<&Hero>,
    cooldowns: Option<&AbilityCooldowns>,
    index: usize,
) -> bool {
    cooldowns.is_none_or(|c| c.ready(index)) && hero.is_none_or(|h| h.mana >= def.mana_cost)
}

/// Which ability a `CastAbility` means. Omit it for "the first one I can use".
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AbilitySelector {
    /// Slot in the caster's `abilities_of_*` list.
    Index(usize),
    /// `AbilityDef::name`, case-insensitive — what the bridge speaks.
    Id(String),
}

/// Per-entity, per-ability cooldown store, indexed by ability slot. Used by
/// hero AND building casters (v1 had two mechanisms; this is the one).
/// Inserted lazily on the first cast, ticked centrally by shared.rs, and
/// treated as "everything ready" while absent.
#[derive(Component, Clone, Debug, Default)]
pub struct AbilityCooldowns(Vec<f32>);

#[allow(dead_code)]
impl AbilityCooldowns {
    /// Seconds left on a slot (0 = ready). Unknown slots are ready.
    pub fn remaining(&self, index: usize) -> f32 {
        self.0.get(index).copied().unwrap_or(0.0)
    }
    pub fn ready(&self, index: usize) -> bool {
        self.remaining(index) <= 0.0
    }
    /// Put a slot on cooldown, growing the store as needed.
    pub fn start(&mut self, index: usize, secs: f32) {
        if self.0.len() <= index {
            self.0.resize(index + 1, 0.0);
        }
        self.0[index] = secs.max(0.0);
    }
    pub fn tick(&mut self, dt: f32) {
        for slot in &mut self.0 {
            *slot = (*slot - dt).max(0.0);
        }
    }
    /// Every slot ready — the component can be dropped.
    pub fn is_idle(&self) -> bool {
        self.0.iter().all(|s| *s <= 0.0)
    }
    /// Raw slots, for snapshots and HUD readouts.
    pub fn slots(&self) -> &[f32] {
        &self.0
    }
}

/// A worker under Call to Arms: fights like a soldier until the deadline
/// (game seconds). combat.rs boosts damage/aggro; shared.rs expires it.
#[derive(Component, Clone, Copy, Debug)]
pub struct Militia {
    pub until: f32,
}

/// Damage a Militia worker deals in place of its normal 5.
pub const MILITIA_DAMAGE: f32 = 16.0;

// ---------------------------------------------------------------------------
// Hero items: bought at a Shop, carried in a small hero inventory,
// consumed on use. economy.rs handles buying (money!), combat.rs executes
// potion effects, units.rs executes teleports (it owns Transforms).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemId {
    HealingPotion,
    TownPortal,
}

pub const ALL_ITEMS: [ItemId; 2] = [ItemId::HealingPotion, ItemId::TownPortal];

pub struct ItemDef {
    pub name: &'static str,
    pub cost_gold: u32,
    pub description: &'static str,
}

pub fn item_def(id: ItemId) -> ItemDef {
    match id {
        ItemId::HealingPotion => ItemDef {
            name: "HealingPotion",
            cost_gold: 100,
            description: "Instantly restores 150 HP to the hero.",
        },
        ItemId::TownPortal => ItemDef {
            name: "TownPortal",
            cost_gold: 150,
            description: "Teleports the hero and nearby own units to the nearest own TownHall.",
        },
    }
}

pub const POTION_HEAL: f32 = 150.0;
pub const PORTAL_RADIUS: f32 = 8.0;

/// Two consumable slots, heroes only. units.rs inserts it empty at hero spawn.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Inventory(pub [Option<ItemId>; 2]);

/// Buy `item` at `shop` for `hero`. economy.rs validates (own completed Shop,
/// own living hero, free slot, gold) and pays.
#[derive(Event, Debug)]
pub struct BuyItem {
    pub shop: Entity,
    pub hero: Entity,
    pub item: ItemId,
}

/// Consume the item in `slot`. combat.rs executes (potion) or delegates
/// (portal -> TeleportRequest).
#[derive(Event, Debug)]
pub struct UseItem {
    pub hero: Entity,
    pub slot: usize,
}

/// Move `center` and own units within `radius` of it to `dest` instantly.
/// Handled by units.rs (the only Transform mover); it also clears MoveTo/paths.
#[derive(Event, Debug)]
pub struct TeleportRequest {
    pub center: Entity,
    pub radius: f32,
    pub dest: Vec3,
}

/// XP granted to nearby enemy heroes when this thing dies.
pub fn xp_for_kill(unit: Option<UnitKind>, building: Option<BuildingKind>) -> f32 {
    match (unit, building) {
        (Some(UnitKind::Worker), _) => 15.0,
        (Some(UnitKind::Footman), _) | (Some(UnitKind::Archer), _) => 30.0,
        (Some(UnitKind::Hero), _) => 120.0,
        (_, Some(_)) => 60.0,
        _ => 0.0,
    }
}

/// A team's hero progression, kept up to date while the hero lives and
/// preserved when it dies so revival restores the level.
#[derive(Clone, Copy, Debug)]
pub struct HeroRecord {
    pub level: u32,
    pub xp: f32,
    /// Which hero class this team plays. Locked in by the first hero trained;
    /// revival restores the same class.
    pub kind: UnitKind,
}

#[derive(Resource, Default)]
pub struct HeroRecords {
    pub human: Option<HeroRecord>,
    pub claude: Option<HeroRecord>,
}

impl HeroRecords {
    pub fn get(&self, team: Team) -> Option<HeroRecord> {
        match team {
            Team::Human => self.human,
            Team::Claude => self.claude,
        }
    }
    pub fn set(&mut self, team: Team, record: HeroRecord) {
        match team {
            Team::Human => self.human = Some(record),
            Team::Claude => self.claude = Some(record),
        }
    }
}

/// Gold/lumber/time to put a Hero in a training queue right now: full price
/// for the first hero, revival price once a record exists.
pub fn hero_train_cost(records: &HeroRecords, team: Team) -> (u32, u32, f32) {
    let base = unit_stats(UnitKind::Hero);
    match records.get(team) {
        Some(_) => (HERO_REVIVE_COST_GOLD, 0, HERO_REVIVE_TIME),
        None => (base.cost_gold, base.cost_lumber, base.train_time),
    }
}

/// What a building's rally point points at. Set by ui.rs, read by economy.rs
/// (when a training item finishes) and applied by units.rs to the fresh unit.
#[derive(Clone, Copy, Debug)]
pub enum RallyTarget {
    Ground(Vec3),
    Node(Entity),   // ResourceNode → new workers start harvesting it
    Unit(Entity),   // own unit → new units follow it
}

/// Rally point set on a production building; applied to units it trains.
#[derive(Component, Clone, Copy, Debug)]
pub struct RallyPoint {
    pub target: RallyTarget,
}

/// Marker on the single RTS camera, spawned by terrain.rs.
#[derive(Component)]
pub struct MainCamera;

// ---------------------------------------------------------------------------
// Doctrine: standing tactical orders, evaluated continuously by doctrine.rs.
// The strategic layer (bridge commander, or later the scripted AI) SETS these;
// the doctrine executor and combat.rs carry them out every tick. Policies are
// deliberately small orthogonal primitives that compose, not a scripting
// language.
// ---------------------------------------------------------------------------

/// Broad target classes for focus-fire rules. Every unit kind maps to exactly
/// one class — an unclassifiable kind would be silently un-focusable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetClass {
    Hero,
    Archer,
    Footman,
    Worker,
    Building,
    /// Catapults and future siege engines.
    Siege,
    /// Raiders and future fast flankers.
    Cavalry,
    /// Anything airborne, whatever else it is. Flying outranks every other
    /// classification because "can I even shoot it" is the first question a
    /// focus-fire list has to answer — "prioritise Air" is the doctrine that
    /// turns an archer line into dedicated anti-air.
    Air,
}

pub const ALL_TARGET_CLASSES: [TargetClass; 8] = [
    TargetClass::Hero,
    TargetClass::Archer,
    TargetClass::Footman,
    TargetClass::Worker,
    TargetClass::Building,
    TargetClass::Siege,
    TargetClass::Cavalry,
    TargetClass::Air,
];

impl TargetClass {
    pub fn of(unit: Option<UnitKind>, building: bool) -> Option<TargetClass> {
        // Altitude first, and derived from the stat table rather than from a
        // list of kinds: any future flying kind is classifiable — and so
        // focus-fireable — the moment its stats say `flying: true`, with no
        // edit here.
        if target_is_air(unit) {
            return Some(TargetClass::Air);
        }
        match (unit, building) {
            // Both hero classes are "Hero" for targeting purposes.
            (Some(UnitKind::Hero) | Some(UnitKind::Priestess), _) => Some(TargetClass::Hero),
            (Some(UnitKind::Archer), _) => Some(TargetClass::Archer),
            // The Spearman answers to "Footman" for targeting purposes: the
            // class is the melee line, and a doctrine that says "focus the
            // front rank" means the front rank, whatever it is holding.
            (Some(UnitKind::Footman) | Some(UnitKind::Spearman), _) => Some(TargetClass::Footman),
            (Some(UnitKind::Worker), _) => Some(TargetClass::Worker),
            (Some(UnitKind::Catapult), _) => Some(TargetClass::Siege),
            (Some(UnitKind::Raider), _) => Some(TargetClass::Cavalry),
            (None, true) => Some(TargetClass::Building),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TargetClass::Hero => "Hero",
            TargetClass::Archer => "Archer",
            TargetClass::Footman => "Footman",
            TargetClass::Worker => "Worker",
            TargetClass::Building => "Building",
            TargetClass::Siege => "Siege",
            TargetClass::Cavalry => "Cavalry",
            TargetClass::Air => "Air",
        }
    }
}

/// Focus-fire doctrine: when acquiring a target, prefer the earliest matching
/// class in this list (distance breaks ties); unlisted classes rank last.
/// combat.rs consults this during acquisition.
#[derive(Component, Clone, Debug)]
pub struct TargetPriority(pub Vec<TargetClass>);

/// Disengage-and-fall-back doctrine: when HP drops below `below_frac` of max,
/// the unit breaks off (Order::Move) to `rally`. Re-arms only after the unit
/// gets a new order from the strategic layer. doctrine.rs executes.
#[derive(Component, Clone, Copy, Debug)]
pub struct RetreatPolicy {
    pub below_frac: f32,
    pub rally: Vec3,
}

/// Anchor doctrine: the unit never operates farther than `radius` from
/// `anchor` — combat.rs won't acquire targets beyond it, and doctrine.rs
/// recalls the unit if it strays (bait-proofing).
#[derive(Component, Clone, Copy, Debug)]
pub struct LeashPolicy {
    pub anchor: Vec3,
    pub radius: f32,
}

/// Hero auto-cast doctrine, PER ABILITY: cast slot `index` whenever it is
/// ready and at least `min_enemies` valid targets are inside its radius.
///
/// A hero with two spells has two independent opinions about when to use them,
/// so the policy is a list of `(ability slot, min targets)` rules rather than
/// one number. An ability with no rule is never auto-cast — silence is off.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoCastPolicy {
    pub rules: Vec<(usize, u32)>,
}

impl AutoCastPolicy {
    /// The v1 shape: auto-cast the caster's first ability. Every existing call
    /// site (`T` on the command card, the bridge `autocast` command with no
    /// ability, `DoctrineTemplate`) lands here.
    pub fn first(min_enemies: u32) -> Self {
        AutoCastPolicy { rules: vec![(0, min_enemies)] }
    }
    pub fn min_enemies_for(&self, index: usize) -> Option<u32> {
        self.rules
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, n)| *n)
    }
    /// One number for one-line summaries (HUD tally, snapshot field): the rule
    /// on slot 0 if there is one, else whatever rule exists.
    pub fn primary(&self) -> Option<u32> {
        self.min_enemies_for(0)
            .or_else(|| self.rules.first().map(|(_, n)| *n))
    }
    pub fn set(&mut self, index: usize, min_enemies: u32) {
        match self.rules.iter_mut().find(|(i, _)| *i == index) {
            Some(rule) => rule.1 = min_enemies,
            None => self.rules.push((index, min_enemies)),
        }
        self.rules.sort_unstable();
    }
    pub fn clear_ability(&mut self, index: usize) {
        self.rules.retain(|(i, _)| *i != index);
    }
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Standing doctrine stamped onto every unit a production building trains.
/// Set by the strategic layer on the building (bridge `template` command);
/// units.rs applies the pieces at spawn via `SpawnUnitEvent::source`. Solves
/// "every new unit spawns doctrine-less" without per-spawn micromanagement.
#[derive(Component, Clone, Debug, Default)]
pub struct DoctrineTemplate {
    pub squad: Option<u8>,
    pub retreat: Option<RetreatPolicy>,
    pub priority: Option<Vec<TargetClass>>,
    /// Heroes only: auto-cast min_enemies.
    pub autocast: Option<u32>,
}

/// Squad membership (small integer handle chosen by the strategic layer).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct SquadId(pub u8);

/// What a squad is currently for. doctrine.rs re-tasks members that have
/// drifted idle so squads act like formations, not one-shot orders.
#[derive(Clone, Copy, Debug)]
pub enum SquadPosture {
    /// Hold an area: idle members outside `radius` attack-move back to `pos`.
    Defend { pos: Vec3, radius: f32 },
    /// Sustained offensive: idle members attack-move toward `pos` until
    /// re-tasked. New members joining mid-push join the push.
    Push { pos: Vec3 },
    /// Screen a specific unit (usually the hero): idle members Follow it.
    Escort { unit: Entity },
    /// Autonomous treasure-hunting: members continuously attack-move to the
    /// nearest live bounty cache; with none on the map, hold at `muster`.
    Forage { muster: Vec3 },
}

/// The default squad every army unit belongs to unless assigned elsewhere.
/// doctrine.rs auto-enrolls postureless army units here and seeds a Defend
/// posture at the team's base, so "commander does nothing" still yields a
/// pooled, reactive army — never scattered statues.
pub const DEFAULT_SQUAD: u8 = 0;

/// Posture per (team, squad id). Bridge/AI writes; doctrine.rs executes.
#[derive(Resource, Default)]
pub struct SquadOrders(pub std::collections::HashMap<(Team, u8), SquadPosture>);

// ---------------------------------------------------------------------------
// Orders (high-level intents) & movement (low-level)
// ---------------------------------------------------------------------------

/// High-level intent, set by ui.rs (human) and ai.rs (Claude).
/// Handlers detect changes via `Changed<Order>`:
///   - units.rs executes Move / AttackMove / Follow movement
///   - combat.rs executes Attack and auto-acquisition (incl. during AttackMove/Idle)
///   - economy.rs executes Harvest / ReturnResources / Build
#[derive(Component, Clone, Debug, Default)]
pub enum Order {
    #[default]
    Idle,
    Move(Vec3),
    AttackMove(Vec3),
    Attack(Entity),
    Harvest(Entity),
    ReturnResources,
    Build { kind: BuildingKind, pos: Vec3 },
    /// Stay near another (friendly) unit: units.rs keeps re-issuing `MoveTo`
    /// toward the followee while it is farther than a few world units, and
    /// falls back to `Idle` once the followee is gone. Followers never
    /// auto-acquire combat targets.
    Follow(Entity),
}

/// Low-level pathfound movement request. Any module may insert this on a unit;
/// units.rs pathfinds, steers the Transform, and REMOVES the component on
/// arrival (within ~1.5 world units) or if the target is unreachable.
/// "Has no MoveTo" therefore means "not currently moving".
#[derive(Component, Clone, Copy, Debug)]
pub struct MoveTo {
    pub target: Vec3,
}

// ---------------------------------------------------------------------------
// Nav grid
// ---------------------------------------------------------------------------

/// Coarse walkability grid over the map. terrain.rs blocks trees/mines,
/// economy.rs blocks/unblocks building footprints, units.rs pathfinds over it.
#[derive(Resource)]
pub struct NavGrid {
    pub blocked: Vec<bool>,
}

impl Default for NavGrid {
    fn default() -> Self {
        NavGrid { blocked: vec![false; GRID_DIM * GRID_DIM] }
    }
}

impl NavGrid {
    pub fn idx(cx: usize, cz: usize) -> usize {
        cz * GRID_DIM + cx
    }
    pub fn world_to_cell(pos: Vec3) -> Option<(usize, usize)> {
        let cx = ((pos.x + MAP_HALF) / CELL).floor();
        let cz = ((pos.z + MAP_HALF) / CELL).floor();
        if cx < 0.0 || cz < 0.0 || cx >= GRID_DIM as f32 || cz >= GRID_DIM as f32 {
            return None;
        }
        Some((cx as usize, cz as usize))
    }
    pub fn cell_to_world(cx: usize, cz: usize) -> Vec3 {
        Vec3::new(
            cx as f32 * CELL - MAP_HALF + CELL * 0.5,
            0.0,
            cz as f32 * CELL - MAP_HALF + CELL * 0.5,
        )
    }
    pub fn is_blocked(&self, cx: usize, cz: usize) -> bool {
        self.blocked[Self::idx(cx, cz)]
    }
    pub fn is_blocked_world(&self, pos: Vec3) -> bool {
        match Self::world_to_cell(pos) {
            Some((cx, cz)) => self.is_blocked(cx, cz),
            None => true,
        }
    }
    /// Block or unblock every cell overlapping a square footprint centered at
    /// `center` with edge length `size`.
    pub fn set_blocked_rect(&mut self, center: Vec3, size: f32, blocked: bool) {
        let half = size * 0.5;
        let min = Vec3::new(center.x - half, 0.0, center.z - half);
        let max = Vec3::new(center.x + half - 0.01, 0.0, center.z + half - 0.01);
        let (Some((x0, z0)), Some((x1, z1))) =
            (Self::world_to_cell(min), Self::world_to_cell(max))
        else {
            return;
        };
        for cz in z0..=z1 {
            for cx in x0..=x1 {
                self.blocked[Self::idx(cx, cz)] = blocked;
            }
        }
    }
    /// True if a square footprint fits entirely on unblocked, in-bounds cells.
    pub fn rect_is_free(&self, center: Vec3, size: f32) -> bool {
        let half = size * 0.5;
        let min = Vec3::new(center.x - half, 0.0, center.z - half);
        let max = Vec3::new(center.x + half - 0.01, 0.0, center.z + half - 0.01);
        let (Some((x0, z0)), Some((x1, z1))) =
            (Self::world_to_cell(min), Self::world_to_cell(max))
        else {
            return false;
        };
        for cz in z0..=z1 {
            for cx in x0..=x1 {
                if self.is_blocked(cx, cz) {
                    return false;
                }
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Fog of war — one rule of knowability, computed once, rendered twice
// ---------------------------------------------------------------------------
//
// This is the last of the three asymmetries in THESIS.md. Until now the
// snapshot handed a commander the whole board while the player at the keyboard
// got one screenful, and the scripted AI read enemy positions straight out of
// the ECS. Three different notions of "what is knowable" for one game.
//
// So knowability is defined exactly once, here, in the same file that owns the
// event feed and for the same reason: a rule with two implementations is two
// rules, and two rules is an information advantage for whoever has the better
// one. `update_fog` walks the world at ~4 Hz and writes one `FogGrid` per
// team. Everything downstream only ever *reads* it:
//
//   * bridge.rs filters each seat's snapshot through that seat's grid,
//   * ui.rs draws the identical grid as a terrain overlay and minimap fog and
//     hides the same entities the snapshot omits,
//   * ai.rs and doctrine.rs take their decision inputs through it,
//
// which is what makes "snapshot content == renderable content" a property of
// the code rather than a promise in a comment.
//
// WHAT IS *NOT* FOG-GATED, and why. Fog models a commander's ATTENTION, not a
// unit's senses. A tower that shoots what walks past it, a footman that swings
// at whatever closes on him, a leashed squad that holds its anchor — those are
// the units' own eyes, they run in combat.rs, and gating them would produce
// soldiers who stand still while being stabbed because headquarters had not
// noticed yet. The line is: *where a unit is sent* obeys fog; *what a unit does
// when something arrives in front of it* does not. See docs/FOG.md.
//
// Map GEOGRAPHY is public and always was: terrain layout, chokepoints, and
// gold mine positions ship in every snapshot and paint on every minimap. Fog
// hides what the enemy is DOING, not where the map's furniture is.

/// `WC3_FOG=0` restores the pre-v2 omniscient baseline: every cell permanently
/// `Visible`, no memory, nothing filtered anywhere. It exists so old AARs and
/// balance tooling have something to compare against — not as a gameplay
/// option. Default is on.
pub const FOG_ENV: &str = "WC3_FOG";

/// Game-seconds between recomputes (~4 Hz). Deliberately GAME time and not
/// real time, unlike the event feed's cadence: the feed keeps a *watcher*
/// current and a watcher's attention runs at one second per second, but fog is
/// a gameplay input that the scripted AI and the doctrine layer both read, so
/// a `WC3_SPEED=16` run has to resolve the same number of fog updates per
/// game-second as a 1x run or the two are not the same match.
const FOG_INTERVAL: f32 = 0.25;

/// Ordering handle for the fog recompute. Consumers (`bridge.rs`, `ui.rs`,
/// `ai.rs`, `doctrine.rs`) declare `.after(FogSet)` so that every reader in a
/// frame sees the same grid, and never the previous tick's on the frame the
/// grid flips.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FogSet;

/// Classic two-level fog. `Explored` is the interesting one: it is the state
/// that lets a player remember terrain and structures without being told what
/// is standing on them right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CellVis {
    /// Never seen by this team. Terrain unknown, everything on it unknown.
    #[default]
    Unexplored,
    /// Seen before, not seen now. Terrain and remembered enemy BUILDINGS
    /// persist at their last observed state; units do not (an army is not
    /// furniture, and a remembered army is a lie that gets people killed).
    Explored,
    /// In sight of a living unit or building of this team, right now.
    Visible,
}

impl CellVis {
    /// Currently in sight — the test for "may this team act on it".
    pub fn sees(self) -> bool {
        matches!(self, CellVis::Visible)
    }
    /// Visible now or seen at some point — the test for "may this team
    /// remember it".
    pub fn known(self) -> bool {
        !matches!(self, CellVis::Unexplored)
    }
}

/// An enemy structure as this team last observed it. Buildings are remembered
/// because they do not move: reporting one where it was is honest, and it is
/// exactly what a human remembers after scouting. HP is the observed HP, so a
/// ghost can be stale — a razed barracks keeps its ghost until somebody looks
/// at the spot again, which is the correct amount of wrong.
#[derive(Clone, Copy, Debug)]
pub struct RememberedBuilding {
    /// The real entity's `to_bits()`, so a renderer can match a ghost against
    /// the live entity and a commander can keep referring to it by id.
    pub id: u64,
    pub team: Team,
    pub kind: BuildingKind,
    pub pos: Vec3,
    pub hp: f32,
    pub max_hp: f32,
    /// Was it finished when last observed?
    pub done: bool,
    /// Game time of the observation.
    pub last_seen: f32,
}

/// One team's knowledge of the map: what it can see now, what it has ever
/// seen, and what it remembers standing there.
pub struct FogGrid {
    /// `GRID_DIM * GRID_DIM`, indexed exactly like `NavGrid` — fog reuses the
    /// nav grid's cell geometry rather than inventing a second one, so "the
    /// cell a unit stands in" means one thing in this codebase.
    cells: Vec<CellVis>,
    ghosts: std::collections::HashMap<u64, RememberedBuilding>,
    explored: usize,
    visible: usize,
}

impl FogGrid {
    fn dark() -> Self {
        FogGrid {
            cells: vec![CellVis::Unexplored; GRID_DIM * GRID_DIM],
            ghosts: std::collections::HashMap::new(),
            explored: 0,
            visible: 0,
        }
    }

    /// The `WC3_FOG=0` grid: permanently and entirely lit. Every reader works
    /// unchanged against it, which is why the escape hatch needs no `if` at
    /// any call site.
    fn revealed() -> Self {
        let n = GRID_DIM * GRID_DIM;
        FogGrid {
            cells: vec![CellVis::Visible; n],
            ghosts: std::collections::HashMap::new(),
            explored: n,
            visible: n,
        }
    }

    pub fn cell(&self, cx: usize, cz: usize) -> CellVis {
        self.cells[NavGrid::idx(cx, cz)]
    }

    /// Visibility of a world position. Off-grid resolves to `Visible`: a grid
    /// miss must never be the reason something is hidden, because a silently
    /// invisible unit is far worse than a briefly over-shared one.
    pub fn at(&self, pos: Vec3) -> CellVis {
        match NavGrid::world_to_cell(pos) {
            Some((cx, cz)) => self.cell(cx, cz),
            None => CellVis::Visible,
        }
    }

    /// The one question every consumer asks: can this team see that spot now?
    pub fn sees(&self, pos: Vec3) -> bool {
        self.at(pos).sees()
    }

    /// Has this team ever seen that spot?
    pub fn known(&self, pos: Vec3) -> bool {
        self.at(pos).known()
    }

    /// Raw cells, for renderers that paint the whole grid in one pass.
    pub fn cells(&self) -> &[CellVis] {
        &self.cells
    }

    /// Enemy structures this team remembers but cannot currently see.
    ///
    /// The filter is load-bearing. The backing map holds a record for every
    /// enemy structure ever observed, refreshed while it is in sight — that is
    /// what makes the memory current the moment sight is lost. But a consumer
    /// iterating it would then emit a ghost for a building it is also
    /// reporting live, and every renderer would show the enemy base twice.
    /// "Ghost" means *memory standing in for sight*, so sight wins here.
    pub fn ghosts(&self) -> impl Iterator<Item = &RememberedBuilding> + '_ {
        self.ghosts.values().filter(|g| !self.at(g.pos).sees())
    }

    /// May this team act on that entity at all — because it can see it, or
    /// because it remembers a structure there? The gate bridge.rs uses to
    /// reject orders against things a seat should not know exist.
    pub fn knows_entity(&self, id: u64, pos: Vec3) -> bool {
        self.sees(pos) || self.ghosts.contains_key(&id)
    }

    pub fn explored_frac(&self) -> f32 {
        self.explored as f32 / (GRID_DIM * GRID_DIM) as f32
    }

    pub fn visible_frac(&self) -> f32 {
        self.visible as f32 / (GRID_DIM * GRID_DIM) as f32
    }

    fn recount(&mut self) {
        self.explored = self.cells.iter().filter(|c| c.known()).count();
        self.visible = self.cells.iter().filter(|c| c.sees()).count();
    }
}

/// Per-team fog, plus the cadence. One producer, several renderers.
#[derive(Resource)]
pub struct FogGrids {
    human: FogGrid,
    claude: FogGrid,
    enabled: bool,
    timer: Timer,
    /// Light the opening position on frame one instead of a quarter-second in,
    /// so nothing ever reads an all-dark grid for a team that has a town hall
    /// standing in the middle of its own vision.
    force: bool,
}

impl Default for FogGrids {
    fn default() -> Self {
        // Read at resource-init rather than in a Startup system so that no
        // system can ever observe the wrong mode, not even for one frame.
        let enabled = std::env::var(FOG_ENV)
            .map(|v| v.trim() != "0")
            .unwrap_or(true);
        let grid = || if enabled { FogGrid::dark() } else { FogGrid::revealed() };
        FogGrids {
            human: grid(),
            claude: grid(),
            enabled,
            timer: Timer::from_seconds(FOG_INTERVAL, TimerMode::Repeating),
            force: true,
        }
    }
}

impl FogGrids {
    /// False under `WC3_FOG=0`. Readers do not need this to be *correct* — the
    /// revealed grid makes every query answer "yes" — but renderers use it to
    /// skip painting an overlay that would be entirely transparent.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn get(&self, team: Team) -> &FogGrid {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }

    fn get_mut(&mut self, team: Team) -> &mut FogGrid {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
}

/// Light every cell whose centre is within `radius` of `pos`. Vision is
/// radial and terrain does NOT block it: the game has no elevation model, the
/// `crossings` canyon is a nav barrier rather than a cliff, and a
/// line-of-sight pass would cost more than the whole rest of the fog system
/// for a fidelity nobody can act on. Both teams get the same simple rule.
fn fog_stamp(cells: &mut [CellVis], pos: Vec3, radius: f32) {
    let reach = (radius / CELL).ceil() as i32;
    let cx0 = ((pos.x + MAP_HALF) / CELL).floor() as i32;
    let cz0 = ((pos.z + MAP_HALF) / CELL).floor() as i32;
    let r2 = radius * radius;
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let (cx, cz) = (cx0 + dx, cz0 + dz);
            if cx < 0 || cz < 0 || cx >= GRID_DIM as i32 || cz >= GRID_DIM as i32 {
                continue;
            }
            let (cx, cz) = (cx as usize, cz as usize);
            let w = NavGrid::cell_to_world(cx, cz);
            if (w.x - pos.x).powi(2) + (w.z - pos.z).powi(2) <= r2 {
                cells[NavGrid::idx(cx, cz)] = CellVis::Visible;
            }
        }
    }
}

/// Recompute both teams' grids and refresh their building memory.
///
/// Runs after `apply_death` so a unit that died this frame has already stopped
/// seeing, and before every consumer (see `FogSet`).
fn update_fog(
    time: Res<Time>,
    mut fog: ResMut<FogGrids>,
    units: Query<(&Unit, &Team, &Transform)>,
    buildings: Query<(Entity, &Building, &Team, &Transform, &Health, Has<UnderConstruction>)>,
) {
    if !fog.enabled {
        return;
    }
    let due = fog.timer.tick(time.delta()).just_finished();
    if !due && !fog.force {
        return;
    }
    fog.force = false;
    let now = ev_r1(time.elapsed_secs());

    for team in [Team::Human, Team::Claude] {
        let grid = fog.get_mut(team);

        // Everything lit last tick decays to remembered; live eyes re-light it
        // below. A team that loses its last scout loses sight of the midfield
        // in the same quarter-second the scout dies.
        for c in grid.cells.iter_mut() {
            if *c == CellVis::Visible {
                *c = CellVis::Explored;
            }
        }
        for (unit, t, tf) in &units {
            if *t == team {
                fog_stamp(&mut grid.cells, tf.translation, unit_stats(unit.kind).vision);
            }
        }
        for (_, building, t, tf, _, _) in &buildings {
            if *t == team {
                fog_stamp(
                    &mut grid.cells,
                    tf.translation,
                    building_stats(building.kind).vision,
                );
            }
        }
        grid.recount();

        // --- building memory ------------------------------------------------
        // Split the borrow so the retain below can read cells while writing
        // ghosts.
        let FogGrid { cells, ghosts, .. } = grid;
        let vis_at = |p: Vec3| match NavGrid::world_to_cell(p) {
            Some((cx, cz)) => cells[NavGrid::idx(cx, cz)],
            None => CellVis::Visible,
        };
        let mut live: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (e, building, t, tf, health, under) in &buildings {
            if *t == team {
                continue;
            }
            let id = e.to_bits();
            live.insert(id);
            if vis_at(tf.translation).sees() {
                ghosts.insert(
                    id,
                    RememberedBuilding {
                        id,
                        team: *t,
                        kind: building.kind,
                        pos: tf.translation,
                        hp: ev_r1(health.current),
                        max_hp: ev_r1(health.max),
                        done: !under,
                        last_seen: now,
                    },
                );
            }
        }
        // Forget a ghost only when we can actually see that the thing is gone.
        // Walk back onto the rubble and the memory clears; stay away and you
        // keep believing the barracks is still standing, which is precisely
        // the mistake fog of war is supposed to let you make.
        ghosts.retain(|id, g| live.contains(id) || !vis_at(g.pos).sees());
    }
}

/// Announce the mode once, so a log or an AAR says which rules it was played
/// under without anyone having to guess from the environment.
fn log_fog_mode(fog: Res<FogGrids>) {
    if fog.enabled {
        info!("fog of war: ON (WC3_FOG=0 to disable)");
    } else {
        info!("fog of war: OFF ({FOG_ENV}=0) — omniscient baseline");
    }
}

/// Nearest walkable cell this team has never seen. The scripted AI's scouting
/// primitive: with no enemy structure known, "go look over there" is the only
/// move that can ever end the game, and it has to be reachable to be worth
/// walking to.
pub fn nearest_unexplored(grid: &FogGrid, from: Vec3, nav: &NavGrid) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for cz in 0..GRID_DIM {
        for cx in 0..GRID_DIM {
            if grid.cell(cx, cz) != CellVis::Unexplored || nav.is_blocked(cx, cz) {
                continue;
            }
            let w = NavGrid::cell_to_world(cx, cz);
            let d = (w.x - from.x).hypot(w.z - from.z);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, w));
            }
        }
    }
    best.map(|(_, w)| w)
}

#[cfg(test)]
mod fog_tests {
    use super::*;

    /// A grid with one lit disc, built without running the Bevy schedule.
    fn lit(pos: Vec3, radius: f32) -> FogGrid {
        let mut grid = FogGrid::dark();
        fog_stamp(&mut grid.cells, pos, radius);
        grid.recount();
        grid
    }

    #[test]
    fn vision_is_a_disc_around_the_seer_and_nothing_else() {
        let grid = lit(Vec3::ZERO, 10.0);
        assert!(grid.sees(Vec3::ZERO));
        assert!(grid.sees(Vec3::new(8.0, 0.0, 0.0)));
        // Well outside the radius: never seen, so not even remembered terrain.
        assert_eq!(grid.at(Vec3::new(40.0, 0.0, 40.0)), CellVis::Unexplored);
        assert!(!grid.known(Vec3::new(40.0, 0.0, 40.0)));
    }

    /// The two-level model: leaving an area demotes it to remembered terrain,
    /// it does NOT return to unexplored.
    #[test]
    fn leaving_an_area_remembers_the_terrain() {
        let mut grid = lit(Vec3::ZERO, 10.0);
        for c in grid.cells.iter_mut() {
            if *c == CellVis::Visible {
                *c = CellVis::Explored;
            }
        }
        grid.recount();
        assert!(!grid.sees(Vec3::ZERO));
        assert!(grid.known(Vec3::ZERO));
        assert_eq!(grid.at(Vec3::ZERO), CellVis::Explored);
    }

    /// Altitude must not change what a unit sees: every fog query is XZ, so a
    /// flyer overhead lights the same cells it would standing on them.
    #[test]
    fn vision_ignores_altitude() {
        let grid = lit(Vec3::new(0.0, FLYER_ALTITUDE, 0.0), 10.0);
        assert!(grid.sees(Vec3::new(4.0, 0.0, 4.0)));
    }

    /// `WC3_FOG=0` must make every question answer "yes" so that no consumer
    /// needs a special case for it.
    #[test]
    fn the_revealed_grid_hides_nothing() {
        let grid = FogGrid::revealed();
        assert!(grid.sees(Vec3::new(-99.0, 0.0, 99.0)));
        assert!(grid.known(Vec3::new(0.0, 0.0, 0.0)));
        assert_eq!(grid.explored_frac(), 1.0);
    }

    /// Off-grid positions must never be the reason something is hidden.
    #[test]
    fn out_of_bounds_resolves_to_visible() {
        let grid = FogGrid::dark();
        assert!(grid.sees(Vec3::new(1000.0, 0.0, 1000.0)));
    }

    /// A ghost is memory STANDING IN FOR sight. While the spot is visible the
    /// live entity is reported instead, so `ghosts()` must stay silent — this
    /// is what stops every renderer drawing a scouted base twice.
    #[test]
    fn ghosts_yield_only_what_is_currently_unseen() {
        let mut grid = lit(Vec3::ZERO, 10.0);
        let remembered = |pos: Vec3, id: u64| RememberedBuilding {
            id,
            team: Team::Claude,
            kind: BuildingKind::Barracks,
            pos,
            hp: 700.0,
            max_hp: 700.0,
            done: true,
            last_seen: 12.0,
        };
        // One inside the lit disc, one far outside it.
        let seen = Vec3::new(2.0, 0.0, 2.0);
        let unseen = Vec3::new(60.0, 0.0, 60.0);
        grid.ghosts.insert(1, remembered(seen, 1));
        grid.ghosts.insert(2, remembered(unseen, 2));

        let ids: Vec<u64> = grid.ghosts().map(|g| g.id).collect();
        assert_eq!(ids, vec![2]);

        // Both remain addressable: you may act on what you can see AND on what
        // you remember. That is the union the bridge validates orders against.
        assert!(grid.knows_entity(1, seen));
        assert!(grid.knows_entity(2, unseen));
        // Something never seen and not remembered is neither.
        assert!(!grid.knows_entity(3, unseen));
    }

    /// The full memory lifecycle through the real system and a real schedule:
    /// see it, leave it, come back to find it gone. The pure-data tests above
    /// cannot catch a mistake in `update_fog` itself — a demotion pass that
    /// forgets to run, a ghost recorded for the wrong team, or a `retain` that
    /// clears memory the moment sight is lost, which would make the whole
    /// feature a no-op that still passes every other assertion here.
    #[test]
    fn a_building_seen_then_left_is_remembered_then_forgotten() {
        use std::time::Duration;

        let barracks_at = Vec3::new(6.0, 0.0, 0.0);
        let far_corner = Vec3::new(-90.0, 0.0, -90.0);

        let mut app = App::new();
        app.init_resource::<Time>();
        let mut grids = FogGrids::default();
        // Pin the mode: the ambient WC3_FOG must not decide a test's outcome.
        grids.enabled = true;
        grids.human = FogGrid::dark();
        grids.claude = FogGrid::dark();
        app.insert_resource(grids);
        app.add_systems(Update, update_fog);

        let scout = app
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

        // --- in sight -----------------------------------------------------
        app.update();
        {
            let human = app.world().resource::<FogGrids>().get(Team::Human);
            assert!(human.sees(barracks_at));
            assert_eq!(human.ghosts().count(), 0, "what is visible is not a ghost");
            // The owner is not haunted by its own buildings.
            let claude = app.world().resource::<FogGrids>().get(Team::Claude);
            assert_eq!(claude.ghosts().count(), 0);
        }

        // --- scout walks away: sight lost, memory kept ---------------------
        app.world_mut()
            .entity_mut(scout)
            .insert(Transform::from_translation(far_corner));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(300));
        app.update();
        {
            let human = app.world().resource::<FogGrids>().get(Team::Human);
            assert!(!human.sees(barracks_at), "sight should have lapsed");
            assert!(human.known(barracks_at), "terrain stays explored");
            let ghosts: Vec<&RememberedBuilding> = human.ghosts().collect();
            assert_eq!(ghosts.len(), 1);
            assert_eq!(ghosts[0].kind, BuildingKind::Barracks);
            assert_eq!(ghosts[0].team, Team::Claude);
            assert_eq!(ghosts[0].pos, barracks_at);
        }

        // --- razed while unseen: the stale belief SURVIVES ------------------
        app.world_mut().entity_mut(barracks).despawn();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(300));
        app.update();
        assert_eq!(
            app.world().resource::<FogGrids>().get(Team::Human).ghosts().count(),
            1,
            "a building destroyed behind our back must still be believed in"
        );

        // --- walk back onto the rubble: the belief clears -------------------
        app.world_mut()
            .entity_mut(scout)
            .insert(Transform::from_translation(barracks_at));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(300));
        app.update();
        assert_eq!(
            app.world().resource::<FogGrids>().get(Team::Human).ghosts().count(),
            0,
            "seeing the empty spot is the only thing that corrects the memory"
        );
    }

    /// The upgrade ladder meets the memory model. `Building.kind` mutates in
    /// place when a hall tiers up, so a scouted TownHall can become a Keep with
    /// nobody watching. The memory must keep working and must stay HONESTLY
    /// stale — reporting the TownHall the scout actually saw, not the Keep it
    /// has no way to know about. The footprint is 8.0 at every rung, so a ghost
    /// drawn from the remembered kind is still the right size on screen.
    #[test]
    fn a_hall_that_tiers_up_behind_the_fog_keeps_its_stale_ghost() {
        use std::time::Duration;

        let hall_at = Vec3::new(6.0, 0.0, 0.0);
        let far_corner = Vec3::new(-90.0, 0.0, -90.0);

        let mut app = App::new();
        app.init_resource::<Time>();
        let mut grids = FogGrids::default();
        grids.enabled = true;
        grids.human = FogGrid::dark();
        grids.claude = FogGrid::dark();
        app.insert_resource(grids);
        app.add_systems(Update, update_fog);

        let scout = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
            ))
            .id();
        let hall = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::TownHall },
                Team::Claude,
                Transform::from_translation(hall_at),
                Health::new(1200.0),
            ))
            .id();

        app.update();
        // Walk away, so the hall is remembered as a TownHall.
        app.world_mut()
            .entity_mut(scout)
            .insert(Transform::from_translation(far_corner));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(300));
        app.update();
        {
            let ghosts: Vec<RememberedBuilding> = app
                .world()
                .resource::<FogGrids>()
                .get(Team::Human)
                .ghosts()
                .copied()
                .collect();
            assert_eq!(ghosts.len(), 1);
            assert_eq!(ghosts[0].kind, BuildingKind::TownHall);
        }

        // economy.rs finishes the upgrade in place, unobserved.
        app.world_mut().entity_mut(hall).insert(Building { kind: BuildingKind::Keep });
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(300));
        app.update();
        {
            let ghosts: Vec<RememberedBuilding> = app
                .world()
                .resource::<FogGrids>()
                .get(Team::Human)
                .ghosts()
                .copied()
                .collect();
            assert_eq!(ghosts.len(), 1, "the ghost must survive a tier-up");
            assert_eq!(
                ghosts[0].kind,
                BuildingKind::TownHall,
                "memory reports what was seen, not what it has become"
            );
            // Every rung shares a footprint, so the drawn ghost is still right.
            assert_eq!(
                building_stats(ghosts[0].kind).size,
                building_stats(BuildingKind::Keep).size
            );
        }

        // Scout back: the memory is replaced by the truth, at the new rung.
        app.world_mut()
            .entity_mut(scout)
            .insert(Transform::from_translation(hall_at));
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(300));
        app.update();
        {
            let human = app.world().resource::<FogGrids>().get(Team::Human);
            assert_eq!(human.ghosts().count(), 0, "sight outranks memory");
            assert!(human.sees(hall_at));
        }
        // And the upgraded hall now sees further for ITS owner than a TownHall
        // did — the tier-up reward, applied from the live kind every tick
        // rather than cached at spawn.
        assert!(building_stats(BuildingKind::Keep).vision > building_stats(BuildingKind::TownHall).vision);
    }

    #[test]
    fn nearest_unexplored_prefers_close_and_skips_blocked() {
        let grid = lit(Vec3::ZERO, 10.0);
        let nav = NavGrid::default();
        let target = nearest_unexplored(&grid, Vec3::ZERO, &nav).expect("map is not fully lit");
        // Just outside the lit disc, not on the far rim of the map.
        let d = (target.x * target.x + target.z * target.z).sqrt();
        assert!(d > 9.0 && d < 16.0, "picked {target:?} at distance {d}");

        // A fully explored map has nowhere left to scout, and the caller has
        // to cope with that rather than being handed a bogus destination.
        assert!(nearest_unexplored(&FogGrid::revealed(), Vec3::ZERO, &nav).is_none());
    }
}

// ---------------------------------------------------------------------------
// Economy resource
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Economy {
    pub gold: u32,
    pub lumber: u32,
    pub supply_used: u32,
    pub supply_cap: u32,
}

impl Default for Economy {
    fn default() -> Self {
        Economy { gold: STARTING_GOLD, lumber: STARTING_LUMBER, supply_used: 0, supply_cap: 0 }
    }
}

impl Economy {
    pub fn can_afford(&self, gold: u32, lumber: u32) -> bool {
        self.gold >= gold && self.lumber >= lumber
    }
    pub fn pay(&mut self, gold: u32, lumber: u32) -> bool {
        if self.can_afford(gold, lumber) {
            self.gold -= gold;
            self.lumber -= lumber;
            true
        } else {
            false
        }
    }
}

#[derive(Resource, Default)]
pub struct Economies {
    pub human: Economy,
    pub claude: Economy,
}

impl Economies {
    pub fn get(&self, team: Team) -> &Economy {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }
    pub fn get_mut(&mut self, team: Team) -> &mut Economy {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Request a unit spawn. Handled by units.rs (creates mesh, Health, Order,
/// Team, Unit components). Cost/supply is NOT checked here — the requester
/// (economy training, initial setup) is responsible.
#[derive(Event, Debug)]
pub struct SpawnUnitEvent {
    pub kind: UnitKind,
    pub team: Team,
    pub pos: Vec3,
    /// Rally of the producing building, if any — units.rs turns this into the
    /// unit's initial `Order` (Move / Harvest / Follow).
    pub rally: Option<RallyTarget>,
    /// The building that produced this unit (None for initial spawns).
    /// units.rs reads its `DoctrineTemplate`, if any, and applies it.
    pub source: Option<Entity>,
}

/// A team concedes the match. Written by ui.rs (player) or bridge.rs
/// (commander); CorePlugin resolves it into a GameOver for the opponent.
#[derive(Event, Debug)]
pub struct Surrender {
    pub team: Team,
}

/// Ask a caster (hero or ability building) to cast. Written by ui.rs
/// (hotkey/button), ai.rs, doctrine.rs and bridge.rs; combat.rs validates
/// (alive, unlocked, mana, cooldown) and executes the AoE.
///
/// `ability: None` means "the first ability this caster has unlocked" — the v1
/// meaning of the event — so nothing that only knows about one ability per
/// caster had to change.
#[derive(Event, Clone, Debug)]
pub struct CastAbility {
    pub caster: Entity,
    pub ability: Option<AbilitySelector>,
}

#[allow(dead_code)]
impl CastAbility {
    /// Backward-compatible cast: the caster's first unlocked ability.
    pub fn new(caster: Entity) -> Self {
        CastAbility { caster, ability: None }
    }
    /// Cast a specific slot of the caster's ability list.
    pub fn index(caster: Entity, index: usize) -> Self {
        CastAbility { caster, ability: Some(AbilitySelector::Index(index)) }
    }
    /// Cast an ability by `AbilityDef::name` (what the bridge sends).
    pub fn id(caster: Entity, id: impl Into<String>) -> Self {
        CastAbility { caster, ability: Some(AbilitySelector::Id(id.into())) }
    }
}

/// Internal to shared.rs: a death dropping XP for nearby enemy heroes.
#[derive(Event, Debug)]
pub struct XpDrop {
    pub victim_team: Team,
    pub pos: Vec3,
    pub amount: f32,
}

/// Snap the RTS camera's ground focus to a world position (minimap clicks,
/// idle-worker jumps). Handled by terrain.rs.
#[derive(Event, Debug)]
pub struct CameraFocus {
    pub pos: Vec3,
}

/// Request a building spawn. Handled by economy.rs (creates mesh, Health,
/// blocks nav grid; if `completed` is false it starts UnderConstruction).
#[derive(Event, Debug)]
pub struct SpawnBuildingEvent {
    pub kind: BuildingKind,
    pub team: Team,
    pub pos: Vec3,
    pub completed: bool,
}

// ---------------------------------------------------------------------------
// Game over
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct GameOver(pub Option<Team>); // Some(winner) once decided

/// Which teams the scripted AI drives. Claude is always AI; the Human side
/// can be AI too (AI-vs-AI spectating): `WC3_AI_BOTH=1` at launch or F9 at
/// runtime. ai.rs owns the env/hotkey wiring; ui.rs reads it so the game-over
/// banner says "Blue/Red wins" instead of VICTORY/DEFEAT while spectating.
#[derive(Resource)]
pub struct AiControlled {
    pub human: bool,
    pub claude: bool,
}

impl Default for AiControlled {
    fn default() -> Self {
        AiControlled { human: false, claude: true }
    }
}

/// Which teams have an external bridge commander seated. Set by bridge.rs at
/// startup. Together with `AiControlled` this answers "is a machine driving
/// this team?" — doctrine's default-squad autonomy applies only then; a human
/// with a mouse keeps full authority over where their idle units stand.
#[derive(Resource, Default)]
pub struct ExternallyCommanded {
    pub human: bool,
    pub claude: bool,
}

impl ExternallyCommanded {
    pub fn get(&self, team: Team) -> bool {
        match team {
            Team::Human => self.human,
            Team::Claude => self.claude,
        }
    }
}

/// Is any machine (scripted AI or bridge commander) driving this team?
pub fn machine_driven(ai: &AiControlled, external: &ExternallyCommanded, team: Team) -> bool {
    external.get(team)
        || match team {
            Team::Human => ai.human,
            Team::Claude => ai.claude,
        }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Project the cursor onto the Y=0 ground plane.
pub fn cursor_to_ground(camera: &Camera, cam_tf: &GlobalTransform, cursor: Vec2) -> Option<Vec3> {
    cursor_to_plane(camera, cam_tf, cursor, 0.0)
}

/// Project the cursor onto the horizontal plane at height `y`.
///
/// Picking a flying unit needs this: the camera looks down at a fixed pitch,
/// so the ground point under the cursor and the point at `FLYER_ALTITUDE`
/// under the same cursor are several world units apart. Testing an airborne
/// unit against the y=0 projection would make it unclickable — and a unit a
/// bridge commander can order but a human cannot click is exactly the
/// interface asymmetry this game exists to remove.
pub fn cursor_to_plane(
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cursor: Vec2,
    y: f32,
) -> Option<Vec3> {
    let ray = cursor_ray(camera, cam_tf, cursor)?;
    ray_at_height(ray, y)
}

/// The cursor's world-space ray. Useful when one click must be tested against
/// several different heights (units on the ground, units in the air) — compute
/// the ray once, then call `ray_at_height` per candidate.
pub fn cursor_ray(camera: &Camera, cam_tf: &GlobalTransform, cursor: Vec2) -> Option<Ray3d> {
    camera.viewport_to_world(cam_tf, cursor).ok()
}

/// Where a precomputed cursor ray crosses the horizontal plane at height `y`.
pub fn ray_at_height(ray: Ray3d, y: f32) -> Option<Vec3> {
    let dist = ray.intersect_plane(Vec3::new(0.0, y, 0.0), InfinitePlane3d::new(Vec3::Y))?;
    Some(ray.get_point(dist))
}

// ---------------------------------------------------------------------------
// Core plugin: initial spawns, death, supply recount, win condition
// ---------------------------------------------------------------------------

pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavGrid>()
            .init_resource::<Economies>()
            .init_resource::<GameOver>()
            .init_resource::<HeroRecords>()
            .init_resource::<AiControlled>()
            .init_resource::<ExternallyCommanded>()
            .init_resource::<SquadOrders>()
            .init_resource::<TechTiers>()
            .init_resource::<GameEvents>()
            .init_resource::<FogGrids>()
            .add_event::<SpawnUnitEvent>()
            .add_event::<SpawnBuildingEvent>()
            .add_event::<CameraFocus>()
            .add_event::<CastAbility>()
            .add_event::<XpDrop>()
            .add_event::<Surrender>()
            .add_event::<BountyClaim>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<TeleportRequest>()
            .add_event::<UpgradeBuilding>()
            .add_systems(Startup, (initial_spawns, apply_env_speed, log_fog_mode))
            .add_systems(
                Update,
                (
                    apply_death,
                    // The one producer of knowability. After `apply_death` so
                    // the dead have stopped seeing; ahead of every consumer in
                    // every other module via `FogSet`.
                    update_fog.in_set(FogSet).after(apply_death),
                    award_xp,
                    hero_progression,
                    regen_health,
                    tick_militia_and_cooldowns,
                    tick_status_effects,
                    recount_supply,
                    recount_tech_tiers,
                    check_game_over,
                    debug_log,
                    status_probe,
                    speed_hotkeys,
                    // After `apply_death`, so a unit that died this frame is
                    // already gone from the picture the diff walks — the feed
                    // reports losses on the tick they happen, not the next one.
                    // After `FogSet` because the feed is now vision-filtered:
                    // a team is told about hostiles and treasure it can see.
                    produce_game_events.after(apply_death).after(FogSet),
                ),
            );
    }
}

fn initial_spawns(
    mut unit_events: EventWriter<SpawnUnitEvent>,
    mut building_events: EventWriter<SpawnBuildingEvent>,
) {
    for team in [Team::Human, Team::Claude] {
        let base = team.base_pos();
        building_events.write(SpawnBuildingEvent {
            kind: BuildingKind::TownHall,
            team,
            pos: base,
            completed: true,
        });
        for i in 0..5 {
            let toward_center = -base.normalize();
            let side = Vec3::new(-toward_center.z, 0.0, toward_center.x);
            let pos = base + toward_center * 8.0 + side * (i as f32 - 2.0) * 2.5;
            unit_events.write(SpawnUnitEvent {
                kind: UnitKind::Worker,
                team,
                pos,
                rally: None,
                source: None,
            });
        }
    }
}

/// Despawn anything whose Health reached zero; free building footprints,
/// snapshot dying heroes for revival, and drop XP for nearby enemy heroes.
fn apply_death(
    mut commands: Commands,
    mut nav: ResMut<NavGrid>,
    mut records: ResMut<HeroRecords>,
    mut xp_drops: EventWriter<XpDrop>,
    query: Query<(
        Entity,
        &Health,
        Option<&Building>,
        Option<&Unit>,
        Option<&Hero>,
        Option<&Team>,
        &Transform,
    )>,
) {
    for (entity, health, building, unit, hero, team, transform) in &query {
        if health.current > 0.0 {
            continue;
        }
        if let Some(building) = building {
            let stats = building_stats(building.kind);
            nav.set_blocked_rect(transform.translation, stats.size, false);
        }
        if let Some(team) = team {
            if let Some(hero) = hero {
                records.set(
                    *team,
                    HeroRecord {
                        level: hero.level,
                        xp: hero.xp,
                        kind: unit.map(|u| u.kind).unwrap_or(UnitKind::Hero),
                    },
                );
            }
            let amount = xp_for_kill(unit.map(|u| u.kind), building.map(|b| b.kind));
            if amount > 0.0 {
                xp_drops.write(XpDrop {
                    victim_team: *team,
                    pos: transform.translation,
                    amount,
                });
            }
        }
        commands.entity(entity).despawn();
    }
}

/// Hand dropped XP to enemy heroes near the kill.
fn award_xp(
    mut drops: EventReader<XpDrop>,
    mut heroes: Query<(&mut Hero, &Team, &Transform)>,
) {
    for drop in drops.read() {
        for (mut hero, team, tf) in &mut heroes {
            if *team == drop.victim_team {
                continue;
            }
            let d = tf.translation - drop.pos;
            if Vec2::new(d.x, d.z).length() <= HERO_XP_RADIUS {
                hero.xp += drop.amount;
            }
        }
    }
}

/// Mana regen, cooldown ticking, level-ups, and keeping `HeroRecords` in sync
/// so revival always restores the latest progression.
fn hero_progression(
    time: Res<Time>,
    mut records: ResMut<HeroRecords>,
    mut heroes: Query<(&mut Hero, &mut Health, &Team, &Unit)>,
) {
    let dt = time.delta_secs();
    for (mut hero, mut health, team, unit) in &mut heroes {
        hero.mana = (hero.mana + HERO_MANA_REGEN * dt).min(Hero::max_mana(hero.level));

        while hero.level < HERO_MAX_LEVEL && hero.xp >= Hero::xp_to_next(hero.level) {
            hero.xp -= Hero::xp_to_next(hero.level);
            hero.level += 1;
            hero.mana = Hero::max_mana(hero.level);
            health.max = Hero::max_hp_for(unit.kind, hero.level);
            health.current = (health.current + 60.0).min(health.max);
        }

        records.set(*team, HeroRecord { level: hero.level, xp: hero.xp, kind: unit.kind });
    }
}

/// Out-of-combat healing: units regen after 12s unhurt, buildings after 20s
/// (much slower). Under-construction buildings are economy.rs's business.
fn regen_health(
    time: Res<Time>,
    mut query: Query<
        (&mut Health, Option<&LastDamaged>, Option<&Building>),
        (Or<(With<Unit>, With<Building>)>, Without<UnderConstruction>),
    >,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    for (mut health, last, building) in &mut query {
        if health.current <= 0.0 || health.current >= health.max {
            continue;
        }
        let (delay, rate) = if building.is_some() {
            (BUILDING_REGEN_DELAY, BUILDING_REGEN_RATE)
        } else {
            (UNIT_REGEN_DELAY, UNIT_REGEN_RATE)
        };
        if last.is_none_or(|l| now - l.at >= delay) {
            health.current = (health.current + health.max * rate * dt).min(health.max);
        }
    }
}

/// Expire Call-to-Arms militia and tick every caster's per-ability cooldowns.
/// One system for heroes and buildings alike — `AbilityCooldowns` is the only
/// cooldown store there is.
fn tick_militia_and_cooldowns(
    time: Res<Time>,
    mut commands: Commands,
    militia: Query<(Entity, &Militia)>,
    mut cooldowns: Query<(Entity, &mut AbilityCooldowns)>,
) {
    let now = time.elapsed_secs();
    for (entity, m) in &militia {
        if now >= m.until {
            commands.entity(entity).try_remove::<Militia>();
        }
    }
    let dt = time.delta_secs();
    for (entity, mut cd) in &mut cooldowns {
        cd.tick(dt);
        // Everything ready again: drop the component so an idle caster costs
        // nothing, exactly like a unit with no status effects.
        if cd.is_idle() {
            commands.entity(entity).try_remove::<AbilityCooldowns>();
        }
    }
}

/// The status framework's central clock: pay out heal-over-time, drop expired
/// instances, and remove the component once an entity is clean again.
///
/// This is the counterpart of `StatusEffects::apply`: content applies, shared
/// expires. No content bead ever schedules its own removal, so a buff can
/// never outlive its duration because somebody forgot a system.
fn tick_status_effects(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut StatusEffects, Option<&mut Health>)>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    for (entity, mut status, health) in &mut query {
        // Heal-over-time is paid BEFORE expiry, so the final partial second of
        // a regeneration buff still lands.
        let heal = status.magnitude(StatusKind::HealOverTime);
        if heal > 0.0 {
            if let Some(mut health) = health {
                if health.current > 0.0 {
                    health.current = (health.current + heal * dt).min(health.max);
                }
            }
        }
        status.expire(now);
        if status.is_empty() {
            commands.entity(entity).try_remove::<StatusEffects>();
        }
    }
}

/// Recompute each team's tech tier from its completed buildings, every frame,
/// through the one `tech_tier_for` gate.
fn recount_tech_tiers(
    mut tiers: ResMut<TechTiers>,
    buildings: Query<(&Building, &Team), Without<UnderConstruction>>,
) {
    for team in [Team::Human, Team::Claude] {
        let tier = tech_tier_for(
            buildings
                .iter()
                .filter(|(_, t)| **t == team)
                .map(|(b, _)| b.kind),
        );
        tiers.set(team, tier);
    }
}

/// `WC3_STATUS_PROBE=1`: dev instrumentation, off in every normal run.
///
/// Once the match is under way it applies a Slow to one live unit per team
/// through the public `StatusEffects::apply` path and logs the unit's
/// effective move speed as the debuff lands, while it holds, and after the
/// central expiry has cleared it. Combined with the Champion's probe-only
/// second ability (`ProbeChill`, gated on tier 2), one headless run
/// demonstrates the whole chain: hall ladder → `tech_tier_for` → unlock
/// predicate → ability list → selector → per-ability cooldown → `ApplyStatus`
/// → `effective_stats` → central expiry.
fn status_probe(
    time: Res<Time>,
    mut commands: Commands,
    tiers: Res<TechTiers>,
    mut stage: Local<u32>,
    mut subject: Local<Option<Entity>>,
    mut casts: EventWriter<CastAbility>,
    mut next_cast: Local<f32>,
    mut seen_tier: Local<Option<TechTier>>,
    units: Query<(Entity, &Unit, &Team, Option<&StatusEffects>)>,
    heroes: Query<(Entity, &Unit, &Team, &Hero)>,
) {
    if !status_probe_enabled() {
        return;
    }
    let now = time.elapsed_secs();

    // Report every tier change, and what it did to the gated ability. This is
    // the integration under test: nothing here knows about Keeps, only that
    // the team's tier moved and an unlock predicate changed its mind.
    let human_tier = tiers.get(Team::Human);
    if *seen_tier != Some(human_tier) {
        *seen_tier = Some(human_tier);
        let ctx = UnlockCtx::new(1, human_tier);
        let gated = abilities_of_unit(UnitKind::Hero)
            .iter()
            .map(|def| format!("{}={}", def.name, ability_unlocked(def, ctx)))
            .collect::<Vec<_>>()
            .join(" ");
        info!(
            "[{now:>6.1}s] STATUS PROBE: Human tier -> {} | unlocked {gated}",
            human_tier.name()
        );
    }

    // Keep asking the Champion for its SECOND ability by explicit index. The
    // executor refuses while it is locked, on cooldown, or short of mana; every
    // cast that does land slows whatever is standing around it.
    if now >= *next_cast {
        *next_cast = now + 5.0;
        for (entity, unit, team, hero) in &heroes {
            if unit.kind != UnitKind::Hero {
                continue;
            }
            let list = abilities_of_unit(unit.kind);
            if list.len() > 1 {
                let ctx = UnlockCtx::new(hero.level, tiers.get(*team));
                if ability_unlocked(&list[1], ctx) {
                    casts.write(CastAbility::index(entity, 1));
                }
            }
        }
    }

    // The direct-application probe: one unit, one Slow, three log lines.
    match *stage {
        0 if now >= 20.0 => {
            let Some((entity, unit, _, _)) = units
                .iter()
                .find(|(_, u, t, _)| u.kind == UnitKind::Worker && **t == Team::Human)
            else {
                return;
            };
            let base = effective_unit_stats(unit.kind, None).speed;
            let mut effects = StatusEffects::new();
            effects.apply(StatusEffect::new(
                StatusKind::Slow,
                0.5,
                now,
                10.0,
                StatusSource::Debug,
            ));
            let slowed = effective_unit_stats(unit.kind, Some(&effects)).speed;
            commands.entity(entity).try_insert(effects);
            *subject = Some(entity);
            *stage = 1;
            info!(
                "[{now:>6.1}s] STATUS PROBE: applied Slow 0.5/10s to {:?} — speed {base:.2} -> {slowed:.2}",
                unit.kind
            );
        }
        1 if now >= 25.0 => {
            let Some(entity) = *subject else {
                *stage = 3;
                return;
            };
            let Ok((_, unit, _, status)) = units.get(entity) else {
                // Probe subject died; nothing more to say.
                *stage = 3;
                return;
            };
            let speed = effective_unit_stats(unit.kind, status).speed;
            info!(
                "[{now:>6.1}s] STATUS PROBE: still slowed — effective speed {speed:.2}, active {}",
                status.map_or(0, |s| s.iter().count())
            );
            *stage = 2;
        }
        2 if now >= 33.0 => {
            let Some(entity) = *subject else {
                *stage = 3;
                return;
            };
            let Ok((_, unit, _, status)) = units.get(entity) else {
                *stage = 3;
                return;
            };
            let speed = effective_unit_stats(unit.kind, status).speed;
            info!(
                "[{now:>6.1}s] STATUS PROBE: expired — effective speed {speed:.2}, active {}",
                status.map_or(0, |s| s.iter().count())
            );
            *stage = 3;
        }
        _ => {}
    }
}

/// Supply is recomputed from the world every frame so no module has to
/// track increments/decrements on death.
fn recount_supply(
    mut economies: ResMut<Economies>,
    units: Query<(&Unit, &Team)>,
    buildings: Query<(&Building, &Team), Without<UnderConstruction>>,
) {
    for team in [Team::Human, Team::Claude] {
        let used: u32 = units
            .iter()
            .filter(|(_, t)| **t == team)
            .map(|(u, _)| unit_stats(u.kind).supply)
            .sum();
        let cap: u32 = buildings
            .iter()
            .filter(|(_, t)| **t == team)
            .map(|(b, _)| building_stats(b.kind).supply_provided)
            .sum();
        let economy = economies.get_mut(team);
        economy.supply_used = used;
        economy.supply_cap = cap.min(100);
    }
}

/// Every gameplay system reads scaled time from the virtual clock, so one
/// multiplier fast-forwards the whole simulation. F1-F4 = 1x/2x/4x/8x.
fn speed_hotkeys(keys: Res<ButtonInput<KeyCode>>, mut time: ResMut<Time<Virtual>>) {
    for (key, speed) in [
        (KeyCode::F1, 1.0),
        (KeyCode::F2, 2.0),
        (KeyCode::F3, 4.0),
        (KeyCode::F4, 8.0),
    ] {
        if keys.just_pressed(key) {
            time.set_relative_speed(speed);
            info!("Game speed set to {speed}x");
        }
    }
}

/// `WC3_SPEED=4 cargo run` — accelerated headless-ish testing.
fn apply_env_speed(mut time: ResMut<Time<Virtual>>) {
    if let Ok(raw) = std::env::var("WC3_SPEED") {
        if let Ok(speed) = raw.parse::<f32>() {
            let speed = speed.clamp(0.1, 16.0);
            time.set_relative_speed(speed);
            info!("WC3_SPEED: game speed set to {speed}x");
        }
    }
}

/// Periodic state snapshot in the console — sim health check without a screen.
fn debug_log(
    time: Res<Time>,
    mut last: Local<f32>,
    economies: Res<Economies>,
    units: Query<(&Unit, &Team)>,
    buildings: Query<(&Building, &Team)>,
) {
    if time.elapsed_secs() - *last < 15.0 {
        return;
    }
    *last = time.elapsed_secs();
    for team in [Team::Human, Team::Claude] {
        let e = economies.get(team);
        let u = units.iter().filter(|(_, t)| **t == team).count();
        let b = buildings.iter().filter(|(_, t)| **t == team).count();
        // Per-kind breakdown, driven by ALL_UNIT_KINDS so new content shows up
        // here the day it is added. A bare unit count cannot answer the one
        // question every balance run asks — "did anyone actually build the
        // thing, and did it live?" — and an army is its composition.
        let army = ALL_UNIT_KINDS
            .iter()
            .filter_map(|kind| {
                let n = units
                    .iter()
                    .filter(|(unit, t)| **t == team && unit.kind == *kind)
                    .count();
                (n > 0).then(|| format!("{n} {}", kind_name(*kind)))
            })
            .collect::<Vec<_>>()
            .join(", ");
        info!(
            "[{:>6.1}s] {:?}: gold {} lumber {} supply {}/{} | {} units, {} buildings | {}",
            time.elapsed_secs(), team, e.gold, e.lumber, e.supply_used, e.supply_cap, u, b, army
        );
    }
}

/// A team loses when it has no PRODUCTION buildings left (TownHall, Barracks,
/// Workshop — anything that can train) — or the moment it concedes. Support
/// structures (farms, towers, walls, shops) can't rebuild an army, so hunting
/// them down after the war is decided would only prolong a lost game.
fn check_game_over(
    time: Res<Time>,
    mut game_over: ResMut<GameOver>,
    mut surrenders: EventReader<Surrender>,
    buildings: Query<(&Building, &Team)>,
) {
    if game_over.0.is_some() {
        surrenders.clear();
        return;
    }
    if let Some(surrender) = surrenders.read().next() {
        info!("{:?} surrenders — {:?} wins", surrender.team, surrender.team.enemy());
        game_over.0 = Some(surrender.team.enemy());
        return;
    }
    if time.elapsed_secs() < 10.0 {
        return;
    }
    for team in [Team::Human, Team::Claude] {
        let has_production = buildings
            .iter()
            .any(|(b, t)| *t == team && !trainable(b.kind).is_empty());
        if !has_production {
            game_over.0 = Some(team.enemy());
        }
    }
}

// ---------------------------------------------------------------------------
// Event feed — one producer, two renderers
// ---------------------------------------------------------------------------
//
// A commander polling a file every ten seconds used to see only the aftermath:
// fewer units, less base, no idea what happened. bridge.rs closed that gap with
// a private snapshot-to-snapshot diff — and in doing so handed the machine a
// faculty the human at the keyboard did not have. A human can miss a raid on
// the far side of the map. The bridge commander never missed anything.
//
// So the diff lives here now, in the contract, and it runs once per team per
// tick regardless of who is watching. bridge.rs serializes the feed into
// `state.json`; ui.rs renders the same feed as HUD notifications. Neither one
// produces. Equitable access means the *feed* is the shared artifact and the
// renderer is merely a matter of which interface you happen to sit behind.
//
// Nothing here hooks combat or economy: the producer remembers the last
// per-team picture of the world and reports what changed. Everything is
// game-time stamped and kept in a ring buffer that outlives individual reads,
// so a slow reader misses nothing — it filters by `seq` against what it saw.
//
// "Own" means the team the feed belongs to, so the two feeds are mirror images
// built from one world, and neither carries knowledge the other's owner could
// not have had. A team's feed reports *its* losses, *its* hero, threats to
// *its* base.
//
// Two categories are about things that are not ours, and both are now filtered
// through this team's `FogGrid` (see the fog section above):
//
//   * "hostiles near base" counts enemies within THREAT_RADIUS of home — 45
//     world units, which is FARTHER than any vision radius in the stat table.
//     It was tempting to assume anything near your own base is inside your own
//     vision by definition; it is not, and unfiltered this event was a
//     free early-warning radar ringing the whole approach to your base.
//     Now it reports the hostiles you can actually see.
//   * bounty caches. Treasure glowing on open ground is public information
//     only to somebody who is looking at that ground. A cache is announced
//     when it enters your vision and "gone" only when you are watching the
//     spot it vanished from — never as news of an empty patch of map you
//     have no eyes on.
//
// Everything else in the vocabulary is own-team knowledge by construction and
// needs no gate: your losses, your buildings, your hero, your squads.

/// Enemy combat units this close to home count toward the base-threat event.
const THREAT_RADIUS: f32 = 45.0;
/// Hero HP fraction whose downward crossing raises a "hero low" event.
const HERO_LOW_FRAC: f32 = 0.35;
/// Building HP fraction whose downward crossing raises an "under attack" event.
const BUILDING_HURT_FRAC: f32 = 0.5;
/// A tick that loses this many units of one kind is reported as one line.
const LOSS_AGGREGATE: usize = 3;
/// Sudden growth in the base-threat count that re-raises the event.
const THREAT_SPIKE: usize = 3;
/// Slack on a vanished bounty's deadline before we call its disappearance
/// early (i.e. claimed rather than timed out). Event clocks are rounded to one
/// decimal, so an exact comparison would misread a natural expiry.
const BOUNTY_EXPIRY_EPS: f32 = 0.5;

/// Ring-buffer capacity per team. A bridge snapshot carries the whole buffer,
/// so a commander polling every ~15s still sees everything that happened in
/// between; the reader filters by `seq`.
pub const MAX_GAME_EVENTS: usize = 40;

/// Wall-clock seconds between diffs. Deliberately real time, not game time:
/// the feed exists to keep a *watcher* current, and a watcher's attention runs
/// at one second per second no matter what `WC3_SPEED` is doing.
const EVENT_INTERVAL: f32 = 1.0;

/// How loud an event is. The bridge ignores this — its reader has the message
/// text and all the time in the world to think about it. The HUD colours by it,
/// because a human glancing at the corner of the screen has neither.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventSeverity {
    /// Worth knowing, costs nothing: a level-up, a bounty appearing.
    Info,
    /// Something of yours is being spent: units lost, a building taking damage.
    Warning,
    /// Act now: hero down or nearly down, a building gone, hostiles at home.
    Critical,
}

/// One notable happening, from one team's point of view.
#[derive(Clone, Debug)]
pub struct GameEvent {
    /// Monotonic across the whole match and both teams. A reader that remembers
    /// the highest `seq` it has handled can neither double-report nor silently
    /// skip, even when the ring buffer drops entries between reads.
    pub seq: u64,
    /// Game time (`Time::elapsed_secs`), one decimal.
    pub t: f32,
    /// The wire text. bridge.rs ships this verbatim; the HUD shows it verbatim.
    pub message: String,
    pub severity: EventSeverity,
    /// Where on the ground it happened, when that is meaningful — a renderer
    /// can focus the camera here. `None` for events without a place.
    pub pos: Option<Vec3>,
}

/// Per-team ring buffers plus the memory the diff runs against.
#[derive(Resource)]
pub struct GameEvents {
    human: TeamFeed,
    claude: TeamFeed,
    /// Real-time cadence of the diff, shared by both teams so their feeds are
    /// built from the identical instant.
    timer: Timer,
    /// Run the first diff immediately rather than a second in, so the memo is
    /// seeded from the opening position.
    force: bool,
    next_seq: u64,
}

impl Default for GameEvents {
    fn default() -> Self {
        GameEvents {
            human: TeamFeed::default(),
            claude: TeamFeed::default(),
            timer: Timer::from_seconds(EVENT_INTERVAL, TimerMode::Repeating),
            force: true,
            next_seq: 1,
        }
    }
}

impl GameEvents {
    /// That team's events, oldest first. This is the whole public surface:
    /// readers never write, and a reader for one team cannot stumble into the
    /// other's feed because it must name a team to get anything at all.
    pub fn feed(&self, team: Team) -> &VecDeque<GameEvent> {
        match team {
            Team::Human => &self.human.events,
            Team::Claude => &self.claude.events,
        }
    }

    /// Append one event to a team's OWN feed, out of band of the once-a-second
    /// diff. The diff can only report what it can see in two consecutive
    /// snapshots of the world; a discrete act with no lasting trace — an
    /// upgrade being ordered, say — has to announce itself. Callers must push
    /// to the acting team only: this is the seam through which an information
    /// asymmetry could be introduced, and the rule that keeps it shut is that
    /// nobody ever pushes to `team.enemy()`.
    pub fn push(
        &mut self,
        team: Team,
        t: f32,
        message: String,
        severity: EventSeverity,
        pos: Option<Vec3>,
    ) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let events = &mut self.team_mut(team).events;
        events.push_back(GameEvent {
            seq,
            t: ev_r1(t),
            message,
            severity,
            pos,
        });
        while events.len() > MAX_GAME_EVENTS {
            events.pop_front();
        }
    }

    fn team_mut(&mut self, team: Team) -> &mut TeamFeed {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
}

#[derive(Default)]
struct TeamFeed {
    events: VecDeque<GameEvent>,
    memo: EventMemo,
}

/// Everything the diff needs to remember from one tick to the next, for one
/// team. Private: the memo is the producer's business, and exposing it would
/// invite a renderer to start producing.
#[derive(Default)]
struct EventMemo {
    /// False until the first picture has been recorded. The first tick only
    /// seeds — with nothing to diff against, every unit would look newly
    /// noteworthy.
    seeded: bool,
    /// own unit id -> (kind, last known position)
    units: std::collections::HashMap<u64, (UnitKind, [f32; 2])>,
    /// own building id -> (kind, position, hp, max_hp)
    buildings: std::collections::HashMap<u64, (BuildingKind, [f32; 2], f32, f32)>,
    hero_alive: bool,
    hero_level: u32,
    /// Last place the hero was seen, so "hero died" still has somewhere to
    /// point a camera after the entity is gone.
    hero_pos: [f32; 2],
    /// Latched so "hero low" fires once per crossing rather than every tick.
    hero_low: bool,
    threat: usize,
    squad_members: std::collections::HashMap<u8, usize>,
    /// Largest membership seen since each squad was last empty. A squad that
    /// bleeds out one member per tick is still a squad that got wiped, so the
    /// report keys off this rather than the previous tick's count.
    squad_peak: std::collections::HashMap<u8, usize>,
    /// Last known centre of mass per squad — where to look when one is wiped.
    squad_pos: std::collections::HashMap<u8, [f32; 2]>,
    /// bounty entity id -> (position, gold, expiry deadline). Bounties are the
    /// one thing in this memo that isn't own-team: treasure is neutral.
    bounties: std::collections::HashMap<u64, ([f32; 2], u32, f32)>,
}

/// One decimal place — event text stays terse and diffs cleanly.
fn ev_r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn ev_centroid(points: &[[f32; 2]]) -> (f32, f32) {
    let n = points.len().max(1) as f32;
    let sx: f32 = points.iter().map(|p| p[0]).sum();
    let sz: f32 = points.iter().map(|p| p[1]).sum();
    (sx / n, sz / n)
}

/// XZ pair back onto the ground plane, for `GameEvent::pos`.
fn ev_ground(p: [f32; 2]) -> Vec3 {
    Vec3::new(p[0], 0.0, p[1])
}

/// One tick's flat view of a unit. The world is walked once and both teams'
/// diffs read the same slice, so the two feeds can never disagree about what
/// was on the map.
struct EvUnit {
    id: u64,
    team: Team,
    kind: UnitKind,
    pos: [f32; 2],
    hp: f32,
    max_hp: f32,
    hero_level: Option<u32>,
    squad: Option<u8>,
}

struct EvBuilding {
    id: u64,
    team: Team,
    kind: BuildingKind,
    pos: [f32; 2],
    hp: f32,
    max_hp: f32,
}

struct EvBounty {
    id: u64,
    pos: [f32; 2],
    gold: u32,
    expires_at: f32,
}

/// Walk the world once per real second and append what changed to each team's
/// ring buffer. Registered by `CorePlugin`, so the feed exists in every run
/// mode — headless, windowed, bridged — and every renderer can rely on it.
fn produce_game_events(
    time: Res<Time>,
    real: Res<Time<Real>>,
    mut feed: ResMut<GameEvents>,
    squad_orders: Res<SquadOrders>,
    fog: Res<FogGrids>,
    unit_q: Query<(
        Entity,
        &Unit,
        &Team,
        &Transform,
        &Health,
        Option<&Hero>,
        Option<&SquadId>,
    )>,
    building_q: Query<(Entity, &Building, &Team, &Transform, &Health)>,
    bounty_q: Query<(Entity, &Bounty, &Transform)>,
) {
    let due = feed.timer.tick(real.delta()).just_finished();
    if !due && !feed.force {
        return;
    }
    feed.force = false;
    let now = ev_r1(time.elapsed_secs());

    let mut units: Vec<EvUnit> = unit_q
        .iter()
        .map(|(e, unit, team, tf, health, hero, squad)| EvUnit {
            id: e.to_bits(),
            team: *team,
            kind: unit.kind,
            pos: [ev_r1(tf.translation.x), ev_r1(tf.translation.z)],
            hp: ev_r1(health.current),
            max_hp: ev_r1(health.max),
            hero_level: hero.map(|h| h.level),
            squad: squad.map(|s| s.0),
        })
        .collect();
    units.sort_unstable_by_key(|u| u.id);

    let mut buildings: Vec<EvBuilding> = building_q
        .iter()
        .map(|(e, building, team, tf, health)| EvBuilding {
            id: e.to_bits(),
            team: *team,
            kind: building.kind,
            pos: [ev_r1(tf.translation.x), ev_r1(tf.translation.z)],
            hp: ev_r1(health.current),
            max_hp: ev_r1(health.max),
        })
        .collect();
    buildings.sort_unstable_by_key(|b| b.id);

    let mut bounties: Vec<EvBounty> = bounty_q
        .iter()
        .map(|(e, bounty, tf)| EvBounty {
            id: e.to_bits(),
            pos: [ev_r1(tf.translation.x), ev_r1(tf.translation.z)],
            gold: bounty.gold,
            expires_at: bounty.expires_at,
        })
        .collect();
    bounties.sort_unstable_by_key(|b| b.id);

    for team in [Team::Human, Team::Claude] {
        let produced = diff_team(
            team,
            now,
            &mut feed.team_mut(team).memo,
            &units,
            &buildings,
            &bounties,
            &squad_orders,
            fog.get(team),
        );
        for (message, severity, pos) in produced {
            let seq = feed.next_seq;
            feed.next_seq += 1;
            let events = &mut feed.team_mut(team).events;
            events.push_back(GameEvent {
                seq,
                t: now,
                message,
                severity,
                pos,
            });
            while events.len() > MAX_GAME_EVENTS {
                events.pop_front();
            }
        }
    }
}

/// Compare this tick's picture against `memo` from one team's point of view and
/// return the notable differences, in a stable order.
///
/// This is the whole event vocabulary. The message text is a wire format —
/// external commanders parse it — so the strings must not drift.
fn diff_team(
    me: Team,
    now: f32,
    memo: &mut EventMemo,
    units: &[EvUnit],
    buildings: &[EvBuilding],
    bounties: &[EvBounty],
    squad_orders: &SquadOrders,
    fog: &FogGrid,
) -> Vec<(String, EventSeverity, Option<Vec3>)> {
    use std::collections::HashMap;

    let home = me.base_pos();

    // --- gather the current picture -------------------------------------
    let mut cur_units: HashMap<u64, (UnitKind, [f32; 2])> = HashMap::new();
    let mut hero_alive = false;
    let mut hero_level = memo.hero_level;
    let mut hero_frac = 1.0f32;
    let mut hero_pos = memo.hero_pos;
    let mut hostiles: Vec<[f32; 2]> = Vec::new();
    let mut members: HashMap<u8, usize> = HashMap::new();
    let mut squad_points: HashMap<u8, Vec<[f32; 2]>> = HashMap::new();
    for u in units {
        if u.team == me {
            cur_units.insert(u.id, (u.kind, u.pos));
            if let Some(level) = u.hero_level {
                hero_alive = true;
                hero_level = level;
                hero_pos = u.pos;
                hero_frac = if u.max_hp > 0.0 { u.hp / u.max_hp } else { 1.0 };
            }
            if let Some(id) = u.squad {
                *members.entry(id).or_insert(0) += 1;
                squad_points.entry(id).or_default().push(u.pos);
            }
        } else if u.kind != UnitKind::Worker {
            // Workers wander; only combat units count as a threat — and only
            // ones we can see. THREAT_RADIUS (45) is wider than any vision
            // radius, so without the fog test this event would report an
            // approach nothing of ours has laid eyes on.
            let d = (u.pos[0] - home.x).hypot(u.pos[1] - home.z);
            if d <= THREAT_RADIUS && fog.sees(ev_ground(u.pos)) {
                hostiles.push(u.pos);
            }
        }
    }

    let mut cur_buildings: HashMap<u64, (BuildingKind, [f32; 2], f32, f32)> = HashMap::new();
    for b in buildings {
        if b.team == me {
            cur_buildings.insert(b.id, (b.kind, b.pos, b.hp, b.max_hp));
        }
    }

    let threat = hostiles.len();

    // What this team can actually see of the treasure on the map right now.
    let seen_bounties: HashMap<u64, ([f32; 2], u32, f32)> = bounties
        .iter()
        .filter(|b| fog.sees(ev_ground(b.pos)))
        .map(|b| (b.id, (b.pos, b.gold, b.expires_at)))
        .collect();

    // Remember where each live squad stands, so a wipe report has a location.
    for (id, points) in &squad_points {
        let (cx, cz) = ev_centroid(points);
        memo.squad_pos.insert(*id, [cx, cz]);
    }

    // The very first tick has nothing to compare against; seed and stay quiet.
    if !memo.seeded {
        memo.seeded = true;
        memo.units = cur_units;
        memo.buildings = cur_buildings;
        memo.hero_alive = hero_alive;
        memo.hero_level = hero_level;
        memo.hero_pos = hero_pos;
        memo.hero_low = hero_alive && hero_frac < HERO_LOW_FRAC;
        memo.threat = threat;
        memo.squad_members = members.clone();
        memo.squad_peak = members;
        memo.bounties = seen_bounties;
        return Vec::new();
    }

    let mut out: Vec<(String, EventSeverity, Option<Vec3>)> = Vec::new();

    // --- unit losses ----------------------------------------------------
    // Grouped by kind so a wiped squad reads as one line, not eight. Keyed and
    // ordered by the *name*, which is what goes out on the wire.
    let mut lost: HashMap<&'static str, Vec<[f32; 2]>> = HashMap::new();
    for (id, (kind, pos)) in &memo.units {
        if cur_units.contains_key(id) || is_hero_kind(*kind) {
            continue; // the hero gets its own, better, event below
        }
        lost.entry(kind_name(*kind)).or_default().push(*pos);
    }
    let mut lost: Vec<(&'static str, Vec<[f32; 2]>)> = lost.into_iter().collect();
    lost.sort_unstable_by_key(|(kind, _)| *kind);
    for (kind, positions) in lost {
        if positions.len() >= LOSS_AGGREGATE {
            let (cx, cz) = ev_centroid(&positions);
            out.push((
                format!("lost {} {kind} near ({cx:.1},{cz:.1})", positions.len()),
                EventSeverity::Warning,
                Some(ev_ground([cx, cz])),
            ));
        } else {
            for p in positions {
                out.push((
                    format!("lost {kind} @({:.1},{:.1})", p[0], p[1]),
                    EventSeverity::Warning,
                    Some(ev_ground(p)),
                ));
            }
        }
    }

    // --- buildings destroyed or newly hurt ------------------------------
    let mut building_ids: Vec<u64> = memo.buildings.keys().copied().collect();
    building_ids.sort_unstable();
    for id in building_ids {
        let (kind, pos, hp, max_hp) = memo.buildings[&id];
        let name = building_name(kind);
        match cur_buildings.get(&id) {
            None => out.push((
                format!("{name} @({:.1},{:.1}) destroyed", pos[0], pos[1]),
                EventSeverity::Critical,
                Some(ev_ground(pos)),
            )),
            Some((_, now_pos, now_hp, now_max)) => {
                let was_hurt = max_hp > 0.0 && hp / max_hp < BUILDING_HURT_FRAC;
                let is_hurt = *now_max > 0.0 && now_hp / now_max < BUILDING_HURT_FRAC;
                if is_hurt && !was_hurt {
                    out.push((
                        format!(
                            "{name} @({:.1},{:.1}) under attack ({:.0}/{:.0})",
                            now_pos[0], now_pos[1], now_hp, now_max
                        ),
                        EventSeverity::Warning,
                        Some(ev_ground(*now_pos)),
                    ));
                }
            }
        }
    }

    // --- the Champion ---------------------------------------------------
    if memo.hero_alive && !hero_alive {
        out.push((
            "hero died".to_string(),
            EventSeverity::Critical,
            Some(ev_ground(memo.hero_pos)),
        ));
    }
    if memo.hero_alive && hero_alive && hero_level > memo.hero_level {
        out.push((
            format!("hero level up: {hero_level}"),
            EventSeverity::Info,
            Some(ev_ground(hero_pos)),
        ));
    }
    let hero_low = hero_alive && hero_frac < HERO_LOW_FRAC;
    if hero_low && !memo.hero_low {
        out.push((
            format!("hero low: {}%", (hero_frac * 100.0).round() as i32),
            EventSeverity::Critical,
            Some(ev_ground(hero_pos)),
        ));
    }

    // --- pressure on the base -------------------------------------------
    // Report the arrival of a threat, and any sharp escalation of one, but not
    // every tick a siege continues — a watcher can see the rest for itself.
    if threat > 0 && (memo.threat == 0 || threat >= memo.threat + THREAT_SPIKE) {
        let (cx, cz) = ev_centroid(&hostiles);
        out.push((
            format!("{threat} hostiles near base @({cx:.1},{cz:.1})"),
            EventSeverity::Critical,
            Some(ev_ground([cx, cz])),
        ));
    }

    // --- squad wipes ----------------------------------------------------
    let mut posture_ids: Vec<u8> = squad_orders
        .0
        .keys()
        .filter(|(team, _)| *team == me)
        .map(|(_, id)| *id)
        .collect();
    posture_ids.sort_unstable();
    for (&id, &n) in &members {
        let peak = memo.squad_peak.entry(id).or_insert(0);
        *peak = (*peak).max(n);
    }
    for id in posture_ids {
        let before = memo.squad_members.get(&id).copied().unwrap_or(0);
        let after = members.get(&id).copied().unwrap_or(0);
        let peak = memo.squad_peak.get(&id).copied().unwrap_or(0);
        if after == 0 && before > 0 && peak >= 2 {
            out.push((
                format!("squad {id} wiped"),
                EventSeverity::Critical,
                memo.squad_pos.get(&id).copied().map(ev_ground),
            ));
        }
    }
    // An emptied squad forgets its peak and its place, so a rebuilt one can be
    // wiped again — and reported where it died the second time, not the first.
    memo.squad_peak
        .retain(|id, _| members.get(id).copied().unwrap_or(0) > 0);
    memo.squad_pos.retain(|id, _| members.contains_key(id));

    // --- bounty caches ---------------------------------------------------
    // Deliberately *unattributed*. bounty.rs despawns a cache the moment
    // somebody claims it, and it does not record who — a claim is not visible
    // in any snapshot, and diffing our own gold cannot separate a bounty from
    // the harvest income arriving in the same second. So the feed reports the
    // observable fact only: a cache appeared, and a cache went away before its
    // deadline (i.e. somebody took it — possibly us). A watcher with a unit on
    // the spot knows the gold was its own; one without knows it lost the race.
    // Caches that simply time out say nothing: the glow on the ground and the
    // snapshot's `expires_in` already counted them down.
    //
    // Under fog the memo is this team's BELIEF about the treasure on the map,
    // not the map's truth, so the three transitions have to be kept apart:
    // a cache entering our vision is news, a cache leaving our vision is not
    // (we go on believing it is there), and a cache we are looking straight at
    // that is no longer there is news again.
    let live_ids: std::collections::HashSet<u64> = bounties.iter().map(|b| b.id).collect();
    let mut cur_bounties = memo.bounties.clone();

    for (id, entry) in &seen_bounties {
        if !memo.bounties.contains_key(id) {
            let (pos, gold, _) = entry;
            out.push((
                format!("bounty spawned: {gold}g @({:.1},{:.1})", pos[0], pos[1]),
                EventSeverity::Info,
                Some(ev_ground(*pos)),
            ));
        }
        cur_bounties.insert(*id, *entry);
    }

    let mut gone: Vec<(u64, ([f32; 2], u32, f32))> = memo
        .bounties
        .iter()
        .filter(|(id, _)| !live_ids.contains(*id))
        .map(|(id, entry)| (*id, *entry))
        .collect();
    gone.sort_unstable_by_key(|(id, _)| *id);
    for (id, (pos, _, expires_at)) in gone {
        if fog.sees(ev_ground(pos)) {
            // We are watching the spot and it is empty. Tolerance absorbs the
            // rounded clock; anything still short of its deadline was taken,
            // not timed out.
            if now + BOUNTY_EXPIRY_EPS < expires_at {
                out.push((
                    format!("bounty gone @({:.1},{:.1})", pos[0], pos[1]),
                    EventSeverity::Info,
                    Some(ev_ground(pos)),
                ));
            }
            cur_bounties.remove(&id);
        } else if now >= expires_at {
            // Out of sight and past its deadline: it cannot still be there, so
            // stop believing in it — silently, because we did not witness it.
            cur_bounties.remove(&id);
        }
    }

    // --- remember --------------------------------------------------------
    memo.units = cur_units;
    memo.buildings = cur_buildings;
    memo.hero_alive = hero_alive;
    memo.hero_level = hero_level;
    memo.hero_pos = hero_pos;
    memo.hero_low = hero_low;
    memo.threat = threat;
    memo.squad_members = members;
    memo.bounties = cur_bounties;

    out
}

// ---------------------------------------------------------------------------
// Tests: the two frameworks' laws, and the upgrade ladder's invariants
// ---------------------------------------------------------------------------
//
// These are the properties the content beads are about to build on, so they
// are pinned here rather than left to a sim to notice. Every one is written
// against the derived helpers, not against `Keep`/`Castle` literals where it
// can be avoided — a second ladder should inherit the guarantees.

#[cfg(test)]
mod tests {
    use super::*;

    fn slow(magnitude: f32, now: f32, duration: f32) -> StatusEffect {
        StatusEffect::new(StatusKind::Slow, magnitude, now, duration, StatusSource::Debug)
    }

    /// The end-to-end shape of a debuff: apply, read a reduced effective stat
    /// through the ONE modifier function, expire centrally, read the base stat
    /// back. Everything else in the framework is a variation on this.
    #[test]
    fn slow_drops_effective_speed_and_recovers_after_expiry() {
        let kind = UnitKind::Footman;
        let base = effective_unit_stats(kind, None);
        assert_eq!(base.speed, unit_stats(kind).speed);

        let mut effects = StatusEffects::new();
        effects.apply(slow(0.4, 0.0, 5.0));

        let slowed = effective_unit_stats(kind, Some(&effects));
        assert!(
            slowed.speed < base.speed,
            "Slow must reduce move speed: {} !< {}",
            slowed.speed,
            base.speed
        );
        assert!((slowed.speed - base.speed * 0.6).abs() < 1e-4);
        // Attack speed falls with move speed: the cooldown gets LONGER.
        assert!(slowed.attack_cooldown > base.attack_cooldown);
        assert!((slowed.attack_cooldown - base.attack_cooldown / 0.6).abs() < 1e-4);

        // Still live one tick before the deadline...
        assert!(!effects.expire(4.9));
        assert!(effects.has(StatusKind::Slow));
        // ...and gone one tick after it.
        assert!(effects.expire(5.1));
        assert!(effects.is_empty());
        assert_eq!(effective_unit_stats(kind, Some(&effects)).speed, base.speed);
    }

    #[test]
    fn debuffs_refresh_and_buffs_stack() {
        // Slow refreshes: strongest magnitude, latest deadline, no compounding.
        let mut effects = StatusEffects::new();
        effects.apply(slow(0.3, 0.0, 4.0));
        effects.apply(slow(0.5, 0.0, 2.0));
        assert_eq!(effects.iter().count(), 1);
        assert!((effects.magnitude(StatusKind::Slow) - 0.5).abs() < 1e-6);
        assert!((effects.remaining(StatusKind::Slow, 0.0) - 4.0).abs() < 1e-6);

        // Haste stacks: two sources add up.
        let mut buffs = StatusEffects::new();
        buffs.apply(StatusEffect::new(StatusKind::Haste, 0.2, 0.0, 5.0, StatusSource::Item));
        buffs.apply(StatusEffect::new(StatusKind::Haste, 0.3, 0.0, 5.0, StatusSource::Aura));
        assert_eq!(buffs.iter().count(), 2);
        assert!((buffs.magnitude(StatusKind::Haste) - 0.5).abs() < 1e-6);
        let hasted = effective_unit_stats(UnitKind::Footman, Some(&buffs));
        assert!((hasted.speed - unit_stats(UnitKind::Footman).speed * 1.5).abs() < 1e-4);
    }

    #[test]
    fn magnitudes_are_capped_so_no_stat_can_be_zeroed_or_inverted() {
        let mut effects = StatusEffects::new();
        for _ in 0..10 {
            effects.apply(StatusEffect::new(
                StatusKind::ArmorBuff,
                0.5,
                0.0,
                5.0,
                StatusSource::Ability,
            ));
        }
        assert!(
            (effects.magnitude(StatusKind::ArmorBuff) - StatusKind::ArmorBuff.cap()).abs() < 1e-6
        );
        let eff = effective_stats(BaseStats::STATIC, Some(&effects));
        assert!(eff.damage_taken_mult > 0.0);

        // Slow is Refresh, so one instance — but even at its cap it must not
        // stop a unit dead.
        let mut hard = StatusEffects::new();
        hard.apply(slow(5.0, 0.0, 5.0));
        let crawling = effective_unit_stats(UnitKind::Footman, Some(&hard));
        assert!(crawling.speed > 0.0);
        assert!(crawling.attack_cooldown.is_finite());
    }

    #[test]
    fn damage_buff_scales_dealt_and_armor_scales_taken() {
        let mut effects = StatusEffects::new();
        effects.apply(StatusEffect::new(
            StatusKind::DamageBuff,
            0.25,
            0.0,
            5.0,
            StatusSource::Ability,
        ));
        effects.apply(StatusEffect::new(
            StatusKind::ArmorBuff,
            0.25,
            0.0,
            5.0,
            StatusSource::Ability,
        ));
        let eff = effective_stats(BaseStats::STATIC, Some(&effects));
        assert!((eff.damage_mult - 1.25).abs() < 1e-6);
        assert!((eff.damage_taken_mult - 0.75).abs() < 1e-6);
    }

    #[test]
    fn heal_over_time_reports_a_rate_and_nothing_else_changes() {
        let mut effects = StatusEffects::new();
        effects.apply(StatusEffect::new(
            StatusKind::HealOverTime,
            8.0,
            0.0,
            5.0,
            StatusSource::Ability,
        ));
        let eff = effective_unit_stats(UnitKind::Footman, Some(&effects));
        assert!((eff.heal_per_second - 8.0).abs() < 1e-6);
        assert_eq!(eff.speed, unit_stats(UnitKind::Footman).speed);
    }

    // --- ability framework v2 ------------------------------------------------

    #[test]
    fn existing_abilities_are_slot_zero_and_unchanged() {
        let champion = abilities_of_unit(UnitKind::Hero);
        assert_eq!(champion[0].name, "Slam");
        assert_eq!(champion[0].effect, AbilityEffect::Damage);
        assert!(!champion[0].hits_air);
        assert_eq!(champion[0].mana_cost, HERO_ABILITY_COST);

        let priestess = abilities_of_unit(UnitKind::Priestess);
        assert_eq!(priestess[0].name, "Heal");
        assert!(priestess[0].hits_air);

        let hall = abilities_of_building(BuildingKind::TownHall);
        assert_eq!(hall[0].name, "CallToArms");
        assert!(abilities_of_unit(UnitKind::Footman).is_empty());
        assert!(abilities_of_building(BuildingKind::Barracks).is_empty());
    }

    /// A synthetic two-ability caster: the shape every content bead will use.
    fn two_ability_list() -> [AbilityDef; 2] {
        [
            SLAM,
            AbilityDef {
                name: "TestWarcry",
                effect: AbilityEffect::ApplyStatus {
                    status: StatusKind::DamageBuff,
                    targets: AbilityTargets::Allies,
                },
                mana_cost: 50.0,
                cooldown: 30.0,
                radius: 10.0,
                power: 0.3,
                duration: 12.0,
                hits_air: true,
                unlock: AbilityUnlock::HeroLevel(4),
                description: "test",
            },
        ]
    }

    #[test]
    fn unlock_predicates_gate_by_hero_level_and_team_tier() {
        let list = two_ability_list();
        let low = UnlockCtx::new(1, TechTier::T1);
        let high = UnlockCtx::new(4, TechTier::T1);
        assert_eq!(unlocked_abilities(&list, low), vec![0]);
        assert_eq!(unlocked_abilities(&list, high), vec![0, 1]);

        let tiered = [AbilityDef { unlock: AbilityUnlock::TeamTier(TechTier::T2), ..SLAM }];
        assert!(unlocked_abilities(&tiered, UnlockCtx::building(TechTier::T1)).is_empty());
        assert_eq!(
            unlocked_abilities(&tiered, UnlockCtx::building(TechTier::T2)),
            vec![0]
        );
    }

    /// The join between the two beads: the hall ladder decides the tier, and
    /// the tier opens a `TeamTier` ability. A team that upgrades its TownHall
    /// into a Keep gains the spell without anything else changing.
    #[test]
    fn upgrading_the_hall_raises_the_tier_and_unlocks_a_tier_gated_ability() {
        use BuildingKind::*;
        // Tier is the highest hall rung STANDING, and nothing else counts.
        assert_eq!(tech_tier_for(std::iter::empty()), TechTier::T1);
        assert_eq!(tech_tier_for([TownHall, Barracks, Farm].into_iter()), TechTier::T1);
        assert_eq!(tech_tier_for([Barracks, Workshop, Tower].into_iter()), TechTier::T1);
        assert_eq!(tech_tier_for([TownHall, Keep].into_iter()), TechTier::T2);
        assert_eq!(tech_tier_for([Keep, Castle].into_iter()), TechTier::T3);
        // Losing the Keep drops the team back: tier is a fact, not a latch.
        assert_eq!(tech_tier_for([TownHall].into_iter()), TechTier::T1);

        let gated = [AbilityDef { unlock: AbilityUnlock::TeamTier(TechTier::T2), ..SLAM }];
        let before = UnlockCtx::new(1, tech_tier_for([TownHall].into_iter()));
        let after = UnlockCtx::new(1, tech_tier_for([Keep].into_iter()));
        assert_eq!(resolve_ability(&gated, None, before), None, "locked at T1");
        assert_eq!(resolve_ability(&gated, None, after), Some(0), "open once a Keep stands");
        // A Castle is strictly better, never a regression.
        let castle = UnlockCtx::new(1, tech_tier_for([Castle].into_iter()));
        assert_eq!(resolve_ability(&gated, None, castle), Some(0));
    }

    #[test]
    fn selectorless_cast_resolves_to_the_first_unlocked_ability() {
        let list = two_ability_list();
        let low = UnlockCtx::new(1, TechTier::T1);
        let high = UnlockCtx::new(9, TechTier::T1);

        // v1 behaviour: no selector -> slot 0.
        assert_eq!(resolve_ability(&list, None, low), Some(0));
        // Explicit index and id both work once unlocked...
        assert_eq!(resolve_ability(&list, Some(&AbilitySelector::Index(1)), high), Some(1));
        assert_eq!(
            resolve_ability(&list, Some(&AbilitySelector::Id("testwarcry".into())), high),
            Some(1)
        );
        // ...and are refused while locked or out of range.
        assert_eq!(resolve_ability(&list, Some(&AbilitySelector::Index(1)), low), None);
        assert_eq!(resolve_ability(&list, Some(&AbilitySelector::Index(7)), high), None);
        assert_eq!(
            resolve_ability(&list, Some(&AbilitySelector::Id("nope".into())), high),
            None
        );

        // A caster whose whole list is locked has nothing to default to.
        let locked = [AbilityDef { unlock: AbilityUnlock::HeroLevel(5), ..SLAM }];
        assert_eq!(resolve_ability(&locked, None, low), None);
    }

    #[test]
    fn cooldowns_are_per_ability() {
        let mut cds = AbilityCooldowns::default();
        assert!(cds.ready(0) && cds.ready(1) && cds.is_idle());

        cds.start(1, 8.0);
        assert!(cds.ready(0), "slot 0 must be unaffected by slot 1");
        assert!(!cds.ready(1));
        assert!(!cds.is_idle());

        cds.tick(3.0);
        assert!((cds.remaining(1) - 5.0).abs() < 1e-6);
        cds.tick(10.0);
        assert!(cds.ready(1));
        assert!(cds.is_idle());
    }

    #[test]
    fn ability_ready_gates_on_cooldown_and_mana() {
        let def = &abilities_of_unit(UnitKind::Priestess)[0];
        let mut broke = Hero::from_record(None);
        broke.mana = 1.0;
        let rich = Hero::from_record(None);

        assert!(ability_ready(def, Some(&rich), None, 0));
        assert!(!ability_ready(def, Some(&broke), None, 0));

        let mut cds = AbilityCooldowns::default();
        cds.start(0, 4.0);
        assert!(!ability_ready(def, Some(&rich), Some(&cds), 0));
        // A building caster pays no mana, only cooldown.
        assert!(ability_ready(&CALL_TO_ARMS, None, Some(&cds), 1));
    }

    #[test]
    fn autocast_policy_is_per_ability() {
        let mut policy = AutoCastPolicy::first(3);
        assert_eq!(policy.min_enemies_for(0), Some(3));
        assert_eq!(policy.min_enemies_for(1), None);
        assert_eq!(policy.primary(), Some(3));

        policy.set(1, 5);
        assert_eq!(policy.min_enemies_for(1), Some(5));
        assert_eq!(policy.primary(), Some(3));

        policy.clear_ability(0);
        assert_eq!(policy.min_enemies_for(0), None);
        assert_eq!(policy.primary(), Some(5));
        policy.clear_ability(1);
        assert!(policy.is_empty());
    }

    #[test]
    fn catalog_exports_every_ability_slot_and_the_status_vocabulary() {
        let catalog = game_catalog();
        for name in ["Slam", "Heal", "CallToArms"] {
            assert!(
                catalog.abilities.iter().any(|a| a.id == name),
                "catalog lost {name}"
            );
        }
        assert!(catalog.abilities.iter().all(|a| !a.unlock.is_empty()));
        assert_eq!(catalog.statuses.len(), ALL_STATUS_KINDS.len());
        assert!(catalog
            .statuses
            .iter()
            .any(|s| s.id == "Slow" && s.stacking == "refresh"));
        assert!(catalog
            .statuses
            .iter()
            .any(|s| s.id == "Haste" && s.stacking == "stack"));
    }

    // --- the upgrade ladder --------------------------------------------------

    #[test]
    fn the_hall_ladder_is_three_rungs_and_agrees_with_itself() {
        assert_eq!(building_upgrades_to(BuildingKind::TownHall), Some(BuildingKind::Keep));
        assert_eq!(building_upgrades_to(BuildingKind::Keep), Some(BuildingKind::Castle));
        assert_eq!(building_upgrades_to(BuildingKind::Castle), None);
        // `upgraded_from` is derived, so this is a real round-trip check.
        for kind in ALL_BUILDING_KINDS {
            if let Some(next) = building_upgrades_to(kind) {
                assert_eq!(building_upgraded_from(next), Some(kind));
                assert_eq!(building_tier(next), building_tier(kind) + 1);
                assert_eq!(upgrade_root(next), upgrade_root(kind));
            }
        }
        assert_eq!(building_tier(BuildingKind::TownHall), 1);
        assert_eq!(building_tier(BuildingKind::Keep), 2);
        assert_eq!(building_tier(BuildingKind::Castle), 3);
    }

    #[test]
    fn requirements_compare_tiers_rather_than_kinds() {
        // The whole point of the tier rule: a team that teched past the
        // requirement still satisfies it.
        let castle = [BuildingKind::Castle];
        assert!(requirements_met(&[BuildingKind::Keep], castle.iter().copied()));
        assert!(requirements_met(&[BuildingKind::TownHall], castle.iter().copied()));
        // ...but teching does not run backwards.
        let hall = [BuildingKind::TownHall];
        assert!(!requirements_met(&[BuildingKind::Keep], hall.iter().copied()));
        // And ladders never bleed into each other.
        assert!(!building_satisfies(BuildingKind::Castle, BuildingKind::Barracks));
        assert!(building_satisfies(BuildingKind::Barracks, BuildingKind::Barracks));
    }

    #[test]
    fn every_rung_of_the_hall_ladder_is_a_hall_and_a_production_building() {
        for kind in [BuildingKind::TownHall, BuildingKind::Keep, BuildingKind::Castle] {
            assert!(is_hall(kind), "{} must count as a hall", building_name(kind));
            // The win condition is "has any building that can train", so a
            // team whose only building is a Castle must not be declared dead.
            assert!(
                !trainable(kind).is_empty(),
                "{} must stay a production building",
                building_name(kind)
            );
            // Hero training/revival lives on the hall card.
            assert!(trainable(kind).contains(&UnitKind::Hero));
            // Call to Arms must survive the upgrade — and keep its slot, since
            // an ability's index is its handle for hotkeys, cooldowns and the
            // bridge selector alike.
            let abilities = abilities_of_building(kind);
            assert_eq!(abilities.len(), 1);
            assert_eq!(abilities[0].name, "CallToArms");
        }
    }

    #[test]
    fn upgrade_only_kinds_are_never_placeable_and_never_shrink_the_building() {
        for kind in ALL_BUILDING_KINDS {
            let placeable = building_placeable(kind);
            assert_eq!(placeable, building_upgraded_from(kind).is_none());
            let Some(next) = building_upgrades_to(kind) else {
                continue;
            };
            assert!(!building_placeable(next));
            let (from, to) = (building_stats(kind), building_stats(next));
            // A tier-up is a reward: more HP, never a smaller HP pool...
            assert!(to.hp > from.hp, "{} must out-HP {}", building_name(next), building_name(kind));
            // ...and never a bigger footprint, or a packed base could not tech.
            assert!(
                to.size <= from.size,
                "{} must not need more ground than {}",
                building_name(next),
                building_name(kind)
            );
            // Supply must not drop, or upgrading could strand an army.
            assert!(to.supply_provided >= from.supply_provided);
            // Neither may sight: a tier-up must never narrow what the hall
            // watches, or teching up would blind you in your own base and the
            // fog would punish the reward.
            assert!(
                to.vision >= from.vision,
                "{} must not see less than {}",
                building_name(next),
                building_name(kind)
            );
            // Cumulative worth strictly grows, so `asset_score` can never
            // punish a team for teching up.
            let (gold_before, lumber_before) = building_value(kind);
            let (gold_after, lumber_after) = building_value(next);
            assert!(gold_after > gold_before && lumber_after > lumber_before);
        }
    }

    #[test]
    fn the_catalog_alone_reconstructs_the_whole_ladder() {
        // An agent reading catalog.json and nothing else must be able to walk
        // TownHall -> Keep -> Castle and price every step.
        let catalog = game_catalog();
        let find = |id: &str| {
            catalog
                .buildings
                .iter()
                .find(|b| b.id == id)
                .unwrap_or_else(|| panic!("{id} missing from the catalog"))
        };
        let mut id = "TownHall";
        let mut walked = vec![id];
        let mut paid = (0, 0);
        while let Some(step) = find(id).upgrades_to.as_ref() {
            paid = (paid.0 + step.cost_gold, paid.1 + step.cost_lumber);
            assert!(step.upgrade_time > 0.0);
            let next = find(step.to);
            assert_eq!(next.upgraded_from, Some(id));
            assert_eq!(next.tier, find(id).tier + 1);
            assert!(!next.placeable);
            // The rung's own cost IS the price of the step that makes it.
            assert_eq!((next.cost_gold, next.cost_lumber), (step.cost_gold, step.cost_lumber));
            id = step.to;
            walked.push(id);
        }
        assert_eq!(walked, vec!["TownHall", "Keep", "Castle"]);
        assert_eq!(paid, (320 + 480, 160 + 240));
    }
}
