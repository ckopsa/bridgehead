//! Shared contract between all game modules.
//! This file is owned by the integrator — module agents must NOT edit it.
//! Modules communicate exclusively through the types, events, and resources here.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
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
    let building_value: u32 = buildings
        .map(|k| {
            let s = building_stats(k);
            s.cost_gold + s.cost_lumber
        })
        .sum();
    unit_value + building_value + economy.gold + economy.lumber
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
}

pub const ALL_UNIT_KINDS: [UnitKind; 7] = [
    UnitKind::Worker,
    UnitKind::Footman,
    UnitKind::Archer,
    UnitKind::Hero,
    UnitKind::Catapult,
    UnitKind::Raider,
    UnitKind::Priestess,
];
pub const ALL_BUILDING_KINDS: [BuildingKind; 7] = [
    BuildingKind::TownHall,
    BuildingKind::Barracks,
    BuildingKind::Farm,
    BuildingKind::Tower,
    BuildingKind::Wall,
    BuildingKind::Workshop,
    BuildingKind::Shop,
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
            vs_building_mult: 1.0, vs_siege_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
        },
        UnitKind::Footman => UnitStats {
            cost_gold: 135, cost_lumber: 0, supply: 2, hp: 140.0, damage: 12.0,
            range: 2.0, attack_cooldown: 1.2, speed: 7.0, train_time: 12.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
        },
        // The line's anti-air: a footman screen is helpless overhead, archers
        // behind it are not.
        UnitKind::Archer => UnitStats {
            cost_gold: 90, cost_lumber: 30, supply: 2, hp: 70.0, damage: 14.0,
            range: 14.0, attack_cooldown: 1.5, speed: 7.0, train_time: 12.0, projectile: true,
            vs_building_mult: 1.0, vs_siege_mult: 1.0,
            flying: false, can_hit_air: true, can_hit_ground: true,
        },
        // Base (level 1) stats; damage/HP grow per level — see `Hero`.
        // The Champion swings a greatsword: no reach into the air, and its
        // Slam is a ground shockwave (see `ability_of_unit`). A team that
        // plays the melee hero needs archers or towers to answer flyers.
        UnitKind::Hero => UnitStats {
            cost_gold: 400, cost_lumber: 100, supply: 5, hp: 320.0, damage: 24.0,
            range: 2.4, attack_cooldown: 1.1, speed: 7.5, train_time: 25.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
        },
        // Outranges towers (20 vs 16) and pulverizes structures, but 15 damage
        // vs units, 110 hp, and 4.5 speed means anything that reaches it wins.
        // Ground-only by design: a boulder lobbed at a wall cannot track a
        // flyer, so an all-in siege push is the thing air raiders punish.
        UnitKind::Catapult => UnitStats {
            cost_gold: 180, cost_lumber: 120, supply: 3, hp: 110.0, damage: 15.0,
            range: 20.0, attack_cooldown: 3.0, speed: 4.5, train_time: 22.0, projectile: true,
            vs_building_mult: 6.0, vs_siege_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
        },
        // Speed is the weapon: dives catapults (2x) and worker lines, melts
        // under focused fire. Gold-heavy so it competes with footmen for budget.
        UnitKind::Raider => UnitStats {
            cost_gold: 170, cost_lumber: 30, supply: 3, hp: 130.0, damage: 16.0,
            range: 2.2, attack_cooldown: 1.1, speed: 10.5, train_time: 16.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 2.0,
            flying: false, can_hit_air: false, can_hit_ground: true,
        },
        // Ranged support hero: heals instead of slams. Base (level 1) stats.
        // Her bolts track upward, so the support hero is also the hero answer
        // to air.
        UnitKind::Priestess => UnitStats {
            cost_gold: 400, cost_lumber: 100, supply: 5, hp: 240.0, damage: 14.0,
            range: 10.0, attack_cooldown: 1.4, speed: 7.5, train_time: 25.0, projectile: true,
            vs_building_mult: 1.0, vs_siege_mult: 1.0,
            flying: false, can_hit_air: true, can_hit_ground: true,
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
}

pub fn building_stats(kind: BuildingKind) -> BuildingStats {
    match kind {
        BuildingKind::TownHall => BuildingStats {
            cost_gold: 385, cost_lumber: 205, hp: 1200.0, build_time: 40.0,
            supply_provided: 10, size: 8.0, attack: None,
        },
        BuildingKind::Barracks => BuildingStats {
            cost_gold: 160, cost_lumber: 60, hp: 700.0, build_time: 25.0,
            supply_provided: 0, size: 6.0, attack: None,
        },
        BuildingKind::Farm => BuildingStats {
            cost_gold: 80, cost_lumber: 20, hp: 350.0, build_time: 12.0,
            supply_provided: 6, size: 4.0, attack: None,
        },
        BuildingKind::Tower => BuildingStats {
            cost_gold: 110, cost_lumber: 80, hp: 550.0, build_time: 25.0,
            supply_provided: 0, size: 3.0,
            attack: Some(BuildingAttack {
                damage: 16.0, range: 16.0, cooldown: 1.3, can_hit_air: true,
            }),
        },
        BuildingKind::Wall => BuildingStats {
            cost_gold: 25, cost_lumber: 10, hp: 300.0, build_time: 8.0,
            supply_provided: 0, size: 2.0, attack: None,
        },
        BuildingKind::Workshop => BuildingStats {
            cost_gold: 140, cost_lumber: 100, hp: 550.0, build_time: 22.0,
            supply_provided: 0, size: 5.0, attack: None,
        },
        BuildingKind::Shop => BuildingStats {
            cost_gold: 75, cost_lumber: 60, hp: 400.0, build_time: 15.0,
            supply_provided: 0, size: 4.0, attack: None,
        },
    }
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
pub fn requirements_met(
    reqs: &[BuildingKind],
    completed: impl Iterator<Item = BuildingKind> + Clone,
) -> bool {
    reqs.iter().all(|r| completed.clone().any(|b| b == *r))
}

/// What each building can train.
pub fn trainable(kind: BuildingKind) -> &'static [UnitKind] {
    match kind {
        BuildingKind::TownHall => &[UnitKind::Worker, UnitKind::Hero, UnitKind::Priestess],
        BuildingKind::Barracks => &[UnitKind::Footman, UnitKind::Archer, UnitKind::Raider],
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
    }
}

pub fn building_description(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::TownHall => "Resource drop-off. Trains Workers and the Hero.",
        BuildingKind::Barracks => "Trains Footmen and Archers.",
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

#[derive(Serialize, Clone, Debug)]
pub struct CatalogBuilding {
    pub id: &'static str,
    pub cost_gold: u32,
    pub cost_lumber: u32,
    pub hp: f32,
    pub build_time: f32,
    pub supply_provided: u32,
    pub size: f32,
    pub attack: Option<CatalogAttack>,
    pub built_by: &'static str,
    pub requires: Vec<&'static str>,
    pub trains: Vec<&'static str>,
    pub description: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogAbility {
    pub id: &'static str,
    pub caster: &'static str,
    pub effect: &'static str,
    pub mana_cost: f32,
    pub cooldown: f32,
    pub radius: f32,
    pub power: f32,
    pub hits_air: bool,
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
                    built_by: "Worker",
                    requires: building_requires(k).iter().map(|b| building_name(*b)).collect(),
                    trains: trainable(k).iter().map(|u| kind_name(*u)).collect(),
                    description: building_description(k),
                }
            })
            .collect(),
        abilities: {
            let effect_name = |e: AbilityEffect| match e {
                AbilityEffect::Damage => "damage",
                AbilityEffect::Heal => "heal",
                AbilityEffect::Militia => "militia",
            };
            let mut out = Vec::new();
            for &k in &ALL_UNIT_KINDS {
                if let Some(a) = ability_of_unit(k) {
                    out.push(CatalogAbility {
                        id: a.name,
                        caster: kind_name(k),
                        effect: effect_name(a.effect),
                        mana_cost: a.mana_cost,
                        cooldown: a.cooldown,
                        radius: a.radius,
                        power: a.power,
                        hits_air: a.hits_air,
                        description: a.description,
                    });
                }
            }
            for &k in &ALL_BUILDING_KINDS {
                if let Some(a) = ability_of_building(k) {
                    out.push(CatalogAbility {
                        id: a.name,
                        caster: building_name(k),
                        effect: effect_name(a.effect),
                        mana_cost: a.mana_cost,
                        cooldown: a.cooldown,
                        radius: a.radius,
                        power: a.power,
                        hits_air: a.hits_air,
                        description: a.description,
                    });
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
/// regen, cooldown ticking, XP awarding, and level-ups; combat.rs reads
/// `damage_mult` and spends mana/cooldown when executing `CastAbility`.
#[derive(Component, Clone, Copy, Debug)]
pub struct Hero {
    pub level: u32,
    pub xp: f32,
    pub mana: f32,
    /// Seconds until the ability may be cast again (0 = ready).
    pub ability_cooldown: f32,
}

impl Hero {
    pub fn from_record(record: Option<HeroRecord>) -> Self {
        let (level, xp) = record.map_or((1, 0.0), |r| (r.level, r.xp));
        Hero { level, xp, mana: Self::max_mana(level), ability_cooldown: 0.0 }
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
    pub fn ability_ready(&self) -> bool {
        self.ability_cooldown <= 0.0 && self.mana >= HERO_ABILITY_COST
    }
}

// ---------------------------------------------------------------------------
// Abilities: one active ability per caster kind, described as data. The
// catalog exports these; combat.rs executes casts; doctrine.rs auto-casts.
// `CastAbility { caster }` stays unchanged — the ability is inferred from the
// caster's kind (heroes use `Hero.ability_cooldown` + mana; building casters
// use the `AbilityCooldown` component, no mana).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AbilityEffect {
    /// AoE damage around the caster (scaled by hero level).
    Damage,
    /// AoE ally healing around the caster (scaled by hero level).
    Heal,
    /// Own workers in radius become Militia for `power` seconds.
    Militia,
}

#[derive(Clone, Copy, Debug)]
pub struct AbilityDef {
    pub name: &'static str,
    pub effect: AbilityEffect,
    pub mana_cost: f32,
    pub cooldown: f32,
    pub radius: f32,
    /// Damage / heal amount, or duration seconds for Militia.
    pub power: f32,
    /// Does the effect reach AIRBORNE units in its radius? A shockwave that
    /// travels along the ground does not; healing light does. combat.rs
    /// filters by this, and doctrine.rs will not auto-cast at targets the
    /// ability cannot affect.
    pub hits_air: bool,
    pub description: &'static str,
}

pub fn ability_of_unit(kind: UnitKind) -> Option<AbilityDef> {
    match kind {
        UnitKind::Hero => Some(AbilityDef {
            name: "Slam",
            effect: AbilityEffect::Damage,
            mana_cost: HERO_ABILITY_COST,
            cooldown: HERO_ABILITY_COOLDOWN,
            radius: HERO_ABILITY_RADIUS,
            power: HERO_ABILITY_DAMAGE,
            // The Champion slams the ground. Flyers overhead feel nothing —
            // the melee hero's air answer is his archers, not his ability.
            hits_air: false,
            description: "AoE damage around the Champion (ground only), scales with level.",
        }),
        UnitKind::Priestess => Some(AbilityDef {
            name: "Heal",
            effect: AbilityEffect::Heal,
            mana_cost: 45.0,
            cooldown: 12.0,
            radius: 8.0,
            power: 60.0,
            // Healing light reaches up: air allies are still your allies.
            hits_air: true,
            description: "Restores HP to all allies around the Priestess, air included, scales with level.",
        }),
        _ => None,
    }
}

pub fn ability_of_building(kind: BuildingKind) -> Option<AbilityDef> {
    match kind {
        BuildingKind::TownHall => Some(AbilityDef {
            name: "CallToArms",
            effect: AbilityEffect::Militia,
            mana_cost: 0.0,
            cooldown: 90.0,
            radius: 16.0,
            power: 40.0,
            // Workers are ground units, so this never had an air question.
            hits_air: false,
            description: "Own workers near the TownHall become fighters for 40s.",
        }),
        _ => None,
    }
}

/// Cooldown state for building casters (heroes track theirs in `Hero`).
/// Ticked centrally by shared.rs; combat.rs checks/sets it on cast.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct AbilityCooldown(pub f32);

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
            (Some(UnitKind::Footman), _) => Some(TargetClass::Footman),
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

/// Hero auto-cast doctrine: cast the ability whenever it is ready and at
/// least `min_enemies` enemy units are inside the ability radius.
#[derive(Component, Clone, Copy, Debug)]
pub struct AutoCastPolicy {
    pub min_enemies: u32,
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
// Intent — the shared strategic language
//
// This is the whole vocabulary of things a *player* can mean, as one
// serializable type. It lives here, next to the catalog and the doctrine
// components, because it is the cross-interface contract: ui.rs compiles mouse
// gestures into these values, bridge.rs deserializes commands.json into the
// same values, and intent.rs is the only thing that turns them into ECS state.
//
// The fairness invariant this exists to enforce: *there is no player-facing
// mutation path except an Intent.* Whatever the human can express, the bridge
// commander can express, because both are spelling words from this one list.
//
// The serde shape is deliberately the bridge's historical wire format — the
// tag is `type`, ids are `Entity::to_bits`, positions are flat `x`/`z` — so
// `commands.json` parses straight into `Intent` with no translation layer, and
// the replay log's serialized form is a command an operator could replay by
// hand. Backward compatibility is not an adapter here; it is the schema.
// ---------------------------------------------------------------------------

/// An entity as it appears in the shared language: `Entity::to_bits`. Both
/// interfaces name units by this id — the bridge because it always did, the UI
/// because naming them the same way is what makes the two logs comparable.
pub type IntentId = u64;

pub fn intent_id(entity: Entity) -> IntentId {
    entity.to_bits()
}

/// Ids on the wire are `Entity::to_bits`; invalid bit patterns resolve to None
/// instead of panicking.
pub fn intent_entity(id: IntentId) -> Option<Entity> {
    Entity::try_from_bits(id).ok()
}

/// Everything a player can mean.
///
/// Grouped by what it is for: unit orders, production, the doctrine layer that
/// runs at machine speed for whoever set it, abilities and items, and the two
/// match-level statements. Adding a verb here is adding it to *both* seats at
/// once, which is the point.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Intent {
    // --- unit orders ---
    Move {
        units: Vec<IntentId>,
        x: f32,
        z: f32,
    },
    AttackMove {
        units: Vec<IntentId>,
        x: f32,
        z: f32,
    },
    Attack {
        units: Vec<IntentId>,
        target: IntentId,
    },
    /// Gold mines and trees alike; workers only.
    Harvest {
        units: Vec<IntentId>,
        target: IntentId,
    },
    Return {
        units: Vec<IntentId>,
    },
    Follow {
        units: Vec<IntentId>,
        target: IntentId,
    },
    /// Halt in place and drop any attack target.
    Stop {
        units: Vec<IntentId>,
    },

    // --- production ---
    Build {
        worker: IntentId,
        kind: String,
        x: f32,
        z: f32,
    },
    Train {
        building: IntentId,
        unit: String,
    },
    Cancel {
        building: IntentId,
        index: usize,
    },
    /// Where units this building trains should go. `x`/`z` for ground, or
    /// `target` for a resource node (new workers harvest it) or an own unit
    /// (new units follow it).
    Rally {
        building: IntentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentId>,
    },

    // --- abilities & items ---
    /// Cast the caster's one ability. The caster is a hero or one of our own
    /// finished ability buildings (the TownHall's Call to Arms). `hero` is the
    /// historical field name; `caster` says what it really means now.
    Cast {
        #[serde(alias = "caster")]
        hero: IntentId,
    },
    /// Buy a consumable at one of our own finished Shops. The buyer is implied:
    /// a team has at most one living hero, and only heroes carry an inventory.
    Buy {
        shop: IntentId,
        item: String,
    },
    #[serde(rename = "use_item")]
    UseItem {
        slot: usize,
    },

    // --- doctrine: standing policy, executed by the engine at machine speed ---
    /// Focus-fire order. An empty/omitted `classes` clears the policy.
    Priority {
        units: Vec<IntentId>,
        #[serde(default)]
        classes: Vec<String>,
    },
    /// Break off below `below` (a fraction in the open range 0..1) and fall
    /// back to x/z. `below` omitted, null, or 0 clears the policy.
    Retreat {
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
    },
    /// Anchor to x/z within `radius`. `radius <= 0` clears the policy.
    Leash {
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
    },
    /// Heroes only. `min_enemies` omitted, null, or 0 clears the policy.
    Autocast {
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_enemies: Option<u32>,
    },
    /// Squad membership. `id` omitted or null removes the units from any squad.
    Squad {
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u8>,
    },
    /// What a squad is for. `posture` omitted or null clears the entry, which
    /// leaves the members where they are without disbanding the squad.
    Posture {
        id: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        posture: Option<PostureIntent>,
    },
    /// Standing doctrine for everything a production building trains from now
    /// on. Each piece is independent and absolute: whatever is given replaces
    /// the whole template, and every piece omitted or null is left unset. An
    /// intent with no pieces at all removes the template entirely.
    Template {
        building: IntentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        squad: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retreat: Option<RetreatIntent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        priority: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        autocast: Option<u32>,
    },

    // --- match level ---
    /// Hand this faction to the scripted AI (or take it back).
    Autopilot {
        on: bool,
    },
    /// Concede: the opponent wins immediately.
    Surrender,
}

/// The `retreat` piece of a `template` intent.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct RetreatIntent {
    pub below: f32,
    pub x: f32,
    pub z: f32,
}

/// The inner object of a `posture` intent.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PostureIntent {
    Defend { x: f32, z: f32, radius: f32 },
    Push { x: f32, z: f32 },
    Escort { unit: IntentId },
    /// Hunt bounty caches; x/z is the muster point held while none exist.
    Forage { x: f32, z: f32 },
}

impl Intent {
    /// The verb, as it appears in the wire format and in the replay log.
    pub fn verb(&self) -> &'static str {
        match self {
            Intent::Move { .. } => "move",
            Intent::AttackMove { .. } => "attackmove",
            Intent::Attack { .. } => "attack",
            Intent::Harvest { .. } => "harvest",
            Intent::Return { .. } => "return",
            Intent::Follow { .. } => "follow",
            Intent::Stop { .. } => "stop",
            Intent::Build { .. } => "build",
            Intent::Train { .. } => "train",
            Intent::Cancel { .. } => "cancel",
            Intent::Rally { .. } => "rally",
            Intent::Cast { .. } => "cast",
            Intent::Buy { .. } => "buy",
            Intent::UseItem { .. } => "use_item",
            Intent::Priority { .. } => "priority",
            Intent::Retreat { .. } => "retreat",
            Intent::Leash { .. } => "leash",
            Intent::Autocast { .. } => "autocast",
            Intent::Squad { .. } => "squad",
            Intent::Posture { .. } => "posture",
            Intent::Template { .. } => "template",
            Intent::Autopilot { .. } => "autopilot",
            Intent::Surrender => "surrender",
        }
    }

    /// One English sentence saying what this intent means.
    ///
    /// This is the half of the replay log a person reads. It deliberately does
    /// not mention who issued it or how: an intent written by a mouse and an
    /// intent written by JSON produce the *same sentence*, which is the whole
    /// claim this module exists to make checkable.
    pub fn sentence(&self) -> String {
        fn at(x: f32, z: f32) -> String {
            format!("({x:.1}, {z:.1})")
        }
        fn group(units: &[IntentId]) -> String {
            match units.len() {
                1 => format!("unit {}", units[0]),
                n => format!("{n} units"),
            }
        }
        match self {
            Intent::Move { units, x, z } => {
                format!("move {} to {}", group(units), at(*x, *z))
            }
            Intent::AttackMove { units, x, z } => {
                format!("attack-move {} to {}", group(units), at(*x, *z))
            }
            Intent::Attack { units, target } => {
                format!("{} attack {target}", group(units))
            }
            Intent::Harvest { units, target } => {
                format!("{} harvest node {target}", group(units))
            }
            Intent::Return { units } => format!("{} return cargo", group(units)),
            Intent::Follow { units, target } => {
                format!("{} follow {target}", group(units))
            }
            Intent::Stop { units } => format!("{} hold position", group(units)),
            Intent::Build {
                worker,
                kind,
                x,
                z,
            } => format!("worker {worker} builds {kind} at {}", at(*x, *z)),
            Intent::Train { building, unit } => {
                format!("building {building} trains {unit}")
            }
            Intent::Cancel { building, index } => {
                format!("building {building} cancels queue slot {index}")
            }
            Intent::Rally {
                building,
                x,
                z,
                target,
            } => match (x, z, target) {
                (Some(x), Some(z), _) => {
                    format!("building {building} rallies to {}", at(*x, *z))
                }
                (_, _, Some(t)) => format!("building {building} rallies onto {t}"),
                _ => format!("building {building} rally (unspecified)"),
            },
            Intent::Cast { hero } => format!("{hero} casts its ability"),
            Intent::Buy { shop, item } => format!("buy {item} at shop {shop}"),
            Intent::UseItem { slot } => format!("hero uses item in slot {slot}"),
            Intent::Priority { units, classes } => {
                if classes.is_empty() {
                    format!("{} clear focus-fire priority", group(units))
                } else {
                    format!("{} focus {}", group(units), classes.join(" > "))
                }
            }
            Intent::Retreat { units, below, x, z } => match (below, x, z) {
                (Some(b), Some(x), Some(z)) if *b > 0.0 => format!(
                    "{} fall back to {} below {:.0}% health",
                    group(units),
                    at(*x, *z),
                    b * 100.0
                ),
                _ => format!("{} clear retreat policy", group(units)),
            },
            Intent::Leash { units, x, z, radius } => match (x, z, radius) {
                (Some(x), Some(z), Some(r)) if *r > 0.0 => format!(
                    "{} hold within {r:.0} of {}",
                    group(units),
                    at(*x, *z)
                ),
                _ => format!("{} clear leash", group(units)),
            },
            Intent::Autocast { units, min_enemies } => match min_enemies {
                Some(n) if *n > 0 => {
                    format!("{} auto-cast at {n}+ enemies", group(units))
                }
                _ => format!("{} clear auto-cast", group(units)),
            },
            Intent::Squad { units, id } => match id {
                Some(id) => format!("{} join squad {id}", group(units)),
                None => format!("{} leave their squad", group(units)),
            },
            Intent::Posture { id, posture } => match posture {
                None => format!("squad {id} stands down (posture cleared)"),
                Some(PostureIntent::Defend { x, z, radius }) => {
                    format!("squad {id} defends {} within {radius:.0}", at(*x, *z))
                }
                Some(PostureIntent::Push { x, z }) => {
                    format!("squad {id} pushes to {}", at(*x, *z))
                }
                Some(PostureIntent::Escort { unit }) => {
                    format!("squad {id} escorts {unit}")
                }
                Some(PostureIntent::Forage { x, z }) => {
                    format!("squad {id} forages, mustering at {}", at(*x, *z))
                }
            },
            Intent::Template {
                building,
                squad,
                retreat,
                priority,
                autocast,
            } => {
                let mut parts: Vec<String> = Vec::new();
                if let Some(s) = squad {
                    parts.push(format!("squad {s}"));
                }
                if let Some(r) = retreat {
                    parts.push(format!(
                        "retreat below {:.0}% to {:.1},{:.1}",
                        r.below * 100.0,
                        r.x,
                        r.z
                    ));
                }
                if let Some(p) = priority {
                    parts.push(format!("focus {}", p.join(" > ")));
                }
                if let Some(a) = autocast {
                    parts.push(format!("auto-cast at {a}+"));
                }
                if parts.is_empty() {
                    format!("building {building} clears its doctrine template")
                } else {
                    format!(
                        "building {building} stamps every unit it trains with {}",
                        parts.join(", ")
                    )
                }
            }
            Intent::Autopilot { on } => {
                if *on {
                    "hand the faction to the scripted AI".to_string()
                } else {
                    "take the faction back from the scripted AI".to_string()
                }
            }
            Intent::Surrender => "surrender the match".to_string(),
        }
    }
}

/// Who spelled the intent. The compiler treats every source identically — this
/// is recorded for the replay log, not consulted for authority.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum IntentSource {
    /// A human gesture in ui.rs (mouse, hotkey, command card).
    Ui,
    /// A command batch through the file bridge.
    Bridge,
}

impl IntentSource {
    pub fn name(self) -> &'static str {
        match self {
            IntentSource::Ui => "ui",
            IntentSource::Bridge => "bridge",
        }
    }
}

/// Submit an intent for validation and application. **This is the only
/// player-facing way to change the game.** ui.rs and bridge.rs write it;
/// intent.rs is the only reader.
#[derive(Event, Clone, Debug)]
pub struct SubmitIntent {
    /// The faction the intent is issued on behalf of. Every ownership check in
    /// the compiler is taken against this, so no interface can reach across.
    pub team: Team,
    pub source: IntentSource,
    /// Prefix for validation errors — `"cmd 3"` for the fourth command of a
    /// bridge batch, `"ui"` for a gesture. Keeps the bridge's historical error
    /// strings byte-identical.
    pub tag: String,
    pub intent: Intent,
}

impl SubmitIntent {
    /// A gesture from the human at the keyboard.
    pub fn ui(team: Team, intent: Intent) -> Self {
        SubmitIntent {
            team,
            source: IntentSource::Ui,
            tag: "ui".to_string(),
            intent,
        }
    }
}

/// Validation failures from the last intents each team submitted, in
/// submission order. bridge.rs copies its seat's list into the next snapshot's
/// `errors`; the compiler only ever appends.
#[derive(Resource, Default)]
pub struct IntentErrors {
    pub human: Vec<String>,
    pub claude: Vec<String>,
}

impl IntentErrors {
    pub fn get(&self, team: Team) -> &Vec<String> {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }
    pub fn get_mut(&mut self, team: Team) -> &mut Vec<String> {
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

/// Ask a hero to cast its ability. Written by ui.rs (hotkey/button) and ai.rs;
/// combat.rs validates (alive, mana, cooldown) and executes the AoE.
#[derive(Event, Debug)]
pub struct CastAbility {
    pub caster: Entity,
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
            .init_resource::<GameEvents>()
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
            .add_systems(Startup, (initial_spawns, apply_env_speed))
            .add_systems(
                Update,
                (
                    apply_death,
                    award_xp,
                    hero_progression,
                    regen_health,
                    tick_militia_and_cooldowns,
                    recount_supply,
                    check_game_over,
                    debug_log,
                    speed_hotkeys,
                    // After `apply_death`, so a unit that died this frame is
                    // already gone from the picture the diff walks — the feed
                    // reports losses on the tick they happen, not the next one.
                    produce_game_events.after(apply_death),
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
        hero.ability_cooldown = (hero.ability_cooldown - dt).max(0.0);
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

/// Expire Call-to-Arms militia and tick building ability cooldowns.
fn tick_militia_and_cooldowns(
    time: Res<Time>,
    mut commands: Commands,
    militia: Query<(Entity, &Militia)>,
    mut cooldowns: Query<&mut AbilityCooldown>,
) {
    let now = time.elapsed_secs();
    for (entity, m) in &militia {
        if now >= m.until {
            commands.entity(entity).try_remove::<Militia>();
        }
    }
    let dt = time.delta_secs();
    for mut cd in &mut cooldowns {
        cd.0 = (cd.0 - dt).max(0.0);
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
        info!(
            "[{:>6.1}s] {:?}: gold {} lumber {} supply {}/{} | {} units, {} buildings",
            time.elapsed_secs(), team, e.gold, e.lumber, e.supply_used, e.supply_cap, u, b
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
// *its* base. Bounty caches are the one shared entry: treasure glowing on open
// ground is public information.

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
            // Workers wander; only combat units count as a threat.
            let d = (u.pos[0] - home.x).hypot(u.pos[1] - home.z);
            if d <= THREAT_RADIUS {
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

    let cur_bounties: HashMap<u64, ([f32; 2], u32, f32)> = bounties
        .iter()
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
        memo.bounties = cur_bounties;
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
    for b in bounties {
        if !memo.bounties.contains_key(&b.id) {
            out.push((
                format!("bounty spawned: {}g @({:.1},{:.1})", b.gold, b.pos[0], b.pos[1]),
                EventSeverity::Info,
                Some(ev_ground(b.pos)),
            ));
        }
    }
    let mut gone: Vec<(&u64, &([f32; 2], u32, f32))> = memo
        .bounties
        .iter()
        .filter(|(id, _)| !cur_bounties.contains_key(*id))
        .collect();
    gone.sort_unstable_by_key(|(id, _)| **id);
    for (_, (pos, _, expires_at)) in gone {
        // Tolerance absorbs the rounded clock; anything still short of its
        // deadline was taken, not timed out.
        if now + BOUNTY_EXPIRY_EPS < *expires_at {
            out.push((
                format!("bounty gone @({:.1},{:.1})", pos[0], pos[1]),
                EventSeverity::Info,
                Some(ev_ground(*pos)),
            ));
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
