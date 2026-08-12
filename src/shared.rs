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
    // `unit_value`, not `unit_stats`, for the same reason `building_value`
    // appears below: a hero's cost fields are 0 (training is free) and its
    // worth is what bringing it back costs. Summing the raw row would score a
    // hero army as owning nothing.
    let unit_value: u32 = units
        .map(|k| {
            let (gold, lumber) = unit_value(k);
            gold + lumber
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

/// `Ord` is not decoration: it is what lets `SquadOrders` be a `BTreeMap` and
/// therefore what makes doctrine execute squads in the same order every run
/// (DESIGN.md § Determinism).
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
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
// Races
// ---------------------------------------------------------------------------
//
// A RACE is a per-team choice of which rows of the content tables that team is
// allowed to use. It is deliberately NOT a second copy of the rules: both
// races share the map, the economy, the win condition, the upkeep curve, the
// research ladders, the item shelf, the status framework and every formula.
// What a race owns is a ROSTER — which units, which buildings, which hall
// ladder — and the roster is data (`races` on every unit and building row).
//
// The reason the concept had to exist at all, rather than "just add rows", is
// that four questions in the engine had no race-free answer: what hall does a
// team start with, what worker does it start with, which build buttons does a
// worker draw, and which kind does the scripted commander reach for when it
// wants "a line unit". The first two are answered by `race_hall`/`race_worker`,
// the third by `race_can_build`, the fourth by `race_unit(race, role)` — all
// four derived from the tables, none of them a match on a kind.

/// The two playable rosters. `BH_RACE_BLUE` / `BH_RACE_RED` pick one per
/// team; the default is `Kingdom` for both, which is exactly the game that
/// shipped before this existed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Deserialize)]
pub enum Race {
    /// The original roster: Footman / Archer / Raider / Spearman, the
    /// TownHall→Keep→Castle ladder, siege out of a Workshop.
    Kingdom,
    /// The asymmetric counterpart: cheaper, faster, more numerous bodies out
    /// of fewer buildings, on the Stronghold→Fortress→Hold ladder.
    Horde,
}

pub const ALL_RACES: [Race; 2] = [Race::Kingdom, Race::Horde];

impl Race {
    pub fn name(self) -> &'static str {
        match self {
            Race::Kingdom => "kingdom",
            Race::Horde => "horde",
        }
    }
    /// Parse an env value / wire word. Case-insensitive; `None` for anything
    /// unrecognised, so the caller can warn and fall back rather than panic on
    /// a typo in a shell variable.
    pub fn parse(text: &str) -> Option<Race> {
        match text.trim().to_ascii_lowercase().as_str() {
            "kingdom" | "human" | "alliance" => Some(Race::Kingdom),
            "horde" | "orc" | "orcs" => Some(Race::Horde),
            _ => None,
        }
    }
}

/// Env var naming the BLUE team's race (`Team::Human`, the player, base SW).
pub const RACE_BLUE_ENV: &str = "BH_RACE_BLUE";
/// Env var naming the RED team's race (`Team::Claude`, base NE).
pub const RACE_RED_ENV: &str = "BH_RACE_RED";

/// Which race each team is playing. Written once at startup from the
/// environment and never again — a race is a property of the match, not a
/// thing a unit can change mid-game.
#[derive(Resource, Clone, Copy, Debug)]
pub struct Races {
    pub human: Race,
    pub claude: Race,
}

impl Default for Races {
    fn default() -> Self {
        Races { human: Race::Kingdom, claude: Race::Kingdom }
    }
}

impl Races {
    pub fn get(&self, team: Team) -> Race {
        match team {
            Team::Human => self.human,
            Team::Claude => self.claude,
        }
    }
    pub fn set(&mut self, team: Team, race: Race) {
        match team {
            Team::Human => self.human = race,
            Team::Claude => self.claude = race,
        }
    }
    /// Read the pair out of the environment. An unset variable — or one that
    /// does not name a race — leaves that side on `Kingdom`, so a run with no
    /// env at all is byte-for-byte the pre-race game.
    pub fn from_env() -> Races {
        let pick = |var: &str| {
            std::env::var(var).ok().and_then(|raw| {
                let parsed = Race::parse(&raw);
                if parsed.is_none() {
                    warn!("{var}=\"{raw}\" is not a race (kingdom|horde) — ignoring");
                }
                parsed
            })
        };
        Races {
            human: pick(RACE_BLUE_ENV).unwrap_or(Race::Kingdom),
            claude: pick(RACE_RED_ENV).unwrap_or(Race::Kingdom),
        }
    }
}

/// **What a unit is FOR**, independent of which race fields it.
///
/// This is the second half of the race concept and the more load-bearing one.
/// Without it, every consumer that wanted "the cheap melee line" had to name
/// `UnitKind::Footman`, and a second race would have meant a fork of that
/// consumer. With it, `race_unit(race, UnitRole::Line)` answers the same
/// question for any roster, and `ai.rs` — the biggest such consumer — maps
/// roles instead of forking.
///
/// It also pays for itself inside the ORIGINAL race: `is_hero_kind` and
/// `TargetClass::of` used to be lists of kinds that a new unit had to be added
/// to by hand (and would classify as `None` if you forgot). Both are now
/// derived from the role, so a kind is classifiable the moment its row exists.
///
/// Roles are single-occupancy per race *by validation* for `Worker`, and free
/// for everything else — a race may field two `Line` units or no `Siege` at
/// all, and the AI's lookup takes the first row in table order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize)]
pub enum UnitRole {
    /// Harvests, builds, cannot fight. Exactly one per race.
    Worker,
    /// The cheap melee body the army is mostly made of.
    Line,
    /// The fragile ranged back rank.
    Ranged,
    /// Fast flankers. `TargetClass::Cavalry`, so the spear line answers them.
    Cavalry,
    /// The answer to `Cavalry` — cheap, slow, enormous `vs_cavalry_mult`.
    AntiCavalry,
    /// Bombardment: outranges static defense, feeble against bodies.
    Siege,
    /// A body bought for an EFFECT rather than for its stats.
    Caster,
    /// Airborne capstone.
    Flyer,
    /// Tier-3 ground line-breaker. Also `TargetClass::Cavalry` — the triangle
    /// is drawn on classes, not on tiers.
    Shock,
    /// Melee hero class.
    HeroMelee,
    /// Ranged/support hero class.
    HeroSupport,
}

/// **What a building is FOR.** Same argument as `UnitRole`, and it retires the
/// last derived-fact that was still keyed on a kind: `is_hall` used to ask
/// `upgrade_root(kind) == TownHall`, which is precisely the sentence a second
/// hall ladder makes false.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize)]
pub enum BuildingRole {
    /// A rung of the town-hall ladder: drop-off point, supply, hero slots.
    Hall,
    /// Trains the army.
    Production,
    /// Supply only (a farm, or a farm that shoots).
    Supply,
    /// Static defense.
    Defense,
    /// Blocking segment with no other function.
    Barrier,
    /// Unlocks and trains tier-2 casters.
    Tech,
    /// Runs the research ladders.
    Forge,
    /// Siege / air works.
    Siegeworks,
    /// Item vendor.
    Vendor,
}

/// Which races may field this unit kind. A row that names no race is NEUTRAL
/// and belongs to every race (nothing uses that today for units; the Shop uses
/// it for buildings).
pub fn unit_races(kind: UnitKind) -> &'static [Race] {
    &crate::data::unit_row(kind).races
}

/// Which races may build this building kind.
pub fn building_races(kind: BuildingKind) -> &'static [Race] {
    &crate::data::building_row(kind).races
}

pub fn unit_role(kind: UnitKind) -> UnitRole {
    crate::data::unit_row(kind).role
}

/// The catalog spelling of a row's `races`. A NEUTRAL row (empty list) is
/// reported as every race rather than as an absence: the field answers "may I
/// build this", and an empty array is the one answer a commander would read
/// backwards.
fn catalog_races(list: &[Race]) -> Vec<&'static str> {
    if list.is_empty() {
        ALL_RACES.iter().map(|r| r.name()).collect()
    } else {
        list.iter().map(|r| r.name()).collect()
    }
}

/// Wire spelling of a unit role — the catalog's `role` field.
pub fn role_name(role: UnitRole) -> &'static str {
    match role {
        UnitRole::Worker => "Worker",
        UnitRole::Line => "Line",
        UnitRole::Ranged => "Ranged",
        UnitRole::Cavalry => "Cavalry",
        UnitRole::AntiCavalry => "AntiCavalry",
        UnitRole::Siege => "Siege",
        UnitRole::Caster => "Caster",
        UnitRole::Flyer => "Flyer",
        UnitRole::Shock => "Shock",
        UnitRole::HeroMelee => "HeroMelee",
        UnitRole::HeroSupport => "HeroSupport",
    }
}

/// Wire spelling of a building role.
pub fn building_role_name(role: BuildingRole) -> &'static str {
    match role {
        BuildingRole::Hall => "Hall",
        BuildingRole::Production => "Production",
        BuildingRole::Supply => "Supply",
        BuildingRole::Defense => "Defense",
        BuildingRole::Barrier => "Barrier",
        BuildingRole::Tech => "Tech",
        BuildingRole::Forge => "Forge",
        BuildingRole::Siegeworks => "Siegeworks",
        BuildingRole::Vendor => "Vendor",
    }
}

pub fn building_role(kind: BuildingKind) -> BuildingRole {
    crate::data::building_row(kind).role
}

/// May `race` field this unit at all? The training path checks this in
/// addition to the tech gate, so a captured/odd code path can never queue the
/// other roster's unit.
pub fn race_has_unit(race: Race, kind: UnitKind) -> bool {
    let list = unit_races(kind);
    list.is_empty() || list.contains(&race)
}

/// May `race` place this building at all?
pub fn race_has_building(race: Race, kind: BuildingKind) -> bool {
    let list = building_races(kind);
    list.is_empty() || list.contains(&race)
}

/// The first unit this race fields in `role`, in table order. `None` if the
/// race simply has no such thing — which is a legal roster asymmetry (the
/// Horde has no `Shock` unit and no `Barrier` building), and the reason every
/// consumer of this takes an `Option`.
pub fn race_unit(race: Race, role: UnitRole) -> Option<UnitKind> {
    ALL_UNIT_KINDS
        .into_iter()
        .find(|&k| unit_role(k) == role && race_has_unit(race, k))
}

/// The first building this race places in `role`, in table order. Hall lookups
/// come back as the ROOT rung, because table order puts the tier-1 rung first.
pub fn race_building(race: Race, role: BuildingRole) -> Option<BuildingKind> {
    ALL_BUILDING_KINDS
        .into_iter()
        .find(|&k| building_role(k) == role && race_has_building(race, k) && building_placeable(k))
}

/// This race's tier-1 hall — what it starts the match with, and what its
/// expansions are.
pub fn race_hall(race: Race) -> BuildingKind {
    race_building(race, BuildingRole::Hall)
        .unwrap_or_else(|| panic!("{race:?} has no placeable Hall — the loader should have caught it"))
}

/// This race's worker.
pub fn race_worker(race: Race) -> UnitKind {
    race_unit(race, UnitRole::Worker)
        .unwrap_or_else(|| panic!("{race:?} has no Worker — the loader should have caught it"))
}

/// Every building kind this race may PLACE, in table order — the build menu,
/// before tech gating.
pub fn race_build_menu(race: Race) -> Vec<BuildingKind> {
    ALL_BUILDING_KINDS
        .into_iter()
        .filter(|&k| building_placeable(k) && race_has_building(race, k))
        .collect()
}

/// The hero CLASSES this race may train, in table order.
pub fn race_hero_classes(race: Race) -> Vec<UnitKind> {
    ALL_UNIT_KINDS
        .into_iter()
        .filter(|&k| is_hero_kind(k) && race_has_unit(race, k))
        .collect()
}

// ---------------------------------------------------------------------------
// Unit & building kinds + stats tables
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize)]
pub enum UnitKind {
    Worker,
    Footman,
    Archer,
    /// The Champion, levels up, casts an AoE slam. Entities of this kind
    /// always carry a `Hero` component (units.rs guarantees it). Occupies one
    /// of the team's tier-scaled hero slots — see `hero_slots`.
    Hero,
    /// Siege engine: outranges towers, wrecks buildings, helpless up close.
    Catapult,
    /// Fast cavalry: dives siege engines and raids workers; dies to massed fire.
    Raider,
    /// The second hero class: ranged, heals allies instead of slamming enemies.
    /// Carries a `Hero` component like the Champion, and occupies a hero slot
    /// of its own — at a Keep a team may field both (`hero_slots`).
    Priestess,
    /// Anti-cavalry line infantry: cheap, slow, and feeble against everything
    /// except a horse, which it deletes. The tier-1 answer to Raiders.
    Spearman,
    /// The game's first non-hero caster: a fragile tier-2 support unit whose
    /// whole job is the `Slow` debuff. Trained at the Arcane Sanctum.
    Sorcerer,
    /// Castle-gated heavy shock cavalry: the tier-3 line-breaker. Raw stats and
    /// speed with no type bonus at all — and `TargetClass::Cavalry`, so the
    /// 90-gold Spearman is still the answer.
    Knight,
    /// Castle-gated air capstone: the first `flying: true` kind. Ignores the
    /// nav grid, hits ground and air, and can only be answered by something
    /// that shoots.
    GryphonRider,

    // ---- Horde -----------------------------------------------------------
    // The second race. Every variant below is a ROW in the same tables the
    // Kingdom's variants live in; what makes them a race is the `races` field
    // on those rows, not anything here.
    /// The Horde's worker.
    Peon,
    /// Melee line: tankier than a Footman and slower. The Horde's body.
    Grunt,
    /// Ranged line: shorter reach than an Archer, harder hit.
    Headhunter,
    /// Fast cavalry, `TargetClass::Cavalry` — so a Spearman AND an Impaler
    /// both delete it, which is the cross-race half of the triangle.
    Wolfrider,
    /// The Horde's anti-cavalry pike line.
    Impaler,
    /// The Horde's siege engine, out of the WarCamp rather than a Workshop.
    Demolisher,
    /// Tier-2 caster: Bloodlust, an ally buff rather than an enemy debuff.
    Shaman,
    /// Horde melee hero class.
    Warchief,
    /// Horde ranged support hero class.
    FarSeer,
    /// Tier-3 air capstone.
    Wyvern,
}

/// Hero-class unit kinds (carry the `Hero` component, occupy one of the team's
/// tier-scaled hero SLOTS, revive through `HeroRecords`).
///
/// Derived from the row's ROLE rather than from a list of kinds: a second
/// race's hero classes are hero classes because their rows say so, and the
/// list this used to be would have silently omitted them.
pub fn is_hero_kind(kind: UnitKind) -> bool {
    matches!(unit_role(kind), UnitRole::HeroMelee | UnitRole::HeroSupport)
}

/// Does this kind harvest, build, and decline to fight? Every `kind ==
/// UnitKind::Worker` in the engine became this, for exactly the reason
/// `is_hero_kind` did: a Peon is a worker because its row says `role: Worker`,
/// and the eleven call sites that used to name the Kingdom's Peasant would
/// otherwise have quietly stopped meaning "worker" for half the game.
pub fn is_worker_kind(kind: UnitKind) -> bool {
    unit_role(kind) == UnitRole::Worker
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize)]
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
    /// Tier 2 forge: researches the team-wide attack and armor ladders
    /// (`ResearchKind`). Requires a Keep. Trains nothing — it converts a bank
    /// balance into a permanent multiplier on every soldier the team will ever
    /// field, which is the one thing an economic lead could not previously buy.
    Blacksmith,
    /// Tier 2 of the town hall ladder. Never placed — a TownHall upgrades into
    /// one in place (see `building_upgrades_to`). Trains everything the hall
    /// trained, and is the tech gate future tier-2 content names.
    Keep,
    /// Tier 3 of the town hall ladder, upgraded from a Keep.
    Castle,
    /// Tier-2 magic college: trains Sorcerers. Requires a Keep, which is the
    /// first thing in the game a hall upgrade actually *buys* you.
    Sanctum,

    // ---- Horde -----------------------------------------------------------
    /// Tier 1 of the Horde hall ladder.
    Stronghold,
    /// The Horde's one martial building: trains the whole army, gated by tier
    /// instead of by a second and third production building. This is the
    /// "fewer buildings, more aggression" identity in one row.
    WarCamp,
    /// Supply + a weak emplacement. A farm that bites — the first building in
    /// the game to combine `supply_provided` with an `attack`, and it needed
    /// no code at all, because combat.rs's tower systems key off
    /// `building_stats(kind).attack.is_some()` rather than on the Tower kind.
    Burrow,
    /// The Horde's static defense.
    Watchtower,
    /// Tier-2 caster hut: trains Shamans. Requires a Fortress.
    SpiritLodge,
    /// The Horde's forge: runs the SAME two research ladders the Blacksmith
    /// does. Research is shared content, the building that sells it is not.
    WarMill,
    /// Tier 2 of the Horde hall ladder.
    Fortress,
    /// Tier 3 of the Horde hall ladder.
    Hold,
}

pub const ALL_UNIT_KINDS: [UnitKind; 21] = [
    UnitKind::Worker,
    UnitKind::Footman,
    UnitKind::Archer,
    UnitKind::Hero,
    UnitKind::Catapult,
    UnitKind::Raider,
    UnitKind::Priestess,
    UnitKind::Spearman,
    UnitKind::Sorcerer,
    UnitKind::Knight,
    UnitKind::GryphonRider,
    UnitKind::Peon,
    UnitKind::Grunt,
    UnitKind::Headhunter,
    UnitKind::Wolfrider,
    UnitKind::Impaler,
    UnitKind::Demolisher,
    UnitKind::Shaman,
    UnitKind::Warchief,
    UnitKind::FarSeer,
    UnitKind::Wyvern,
];
pub const ALL_BUILDING_KINDS: [BuildingKind; 19] = [
    BuildingKind::TownHall,
    BuildingKind::Barracks,
    BuildingKind::Farm,
    BuildingKind::Tower,
    BuildingKind::Wall,
    BuildingKind::Workshop,
    BuildingKind::Shop,
    BuildingKind::Blacksmith,
    BuildingKind::Keep,
    BuildingKind::Castle,
    BuildingKind::Sanctum,
    BuildingKind::Stronghold,
    BuildingKind::WarCamp,
    BuildingKind::Burrow,
    BuildingKind::Watchtower,
    BuildingKind::SpiritLodge,
    BuildingKind::WarMill,
    BuildingKind::Fortress,
    BuildingKind::Hold,
];

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

    // --- heroes only ------------------------------------------------------
    /// What it costs to bring THIS hero class back after it dies.
    ///
    /// Heroes are the one kind whose price is not `cost_gold`/`cost_lumber`:
    /// the first fielding of a class is **free** (its cost fields are 0, and
    /// the validator insists on it) and the bill arrives on DEATH instead.
    /// Five straight arena rounds were won by commanders who looked at 400g
    /// for a hero, counted three Footmen, and skipped it — so the price moved
    /// to the place where skipping is not an option.
    ///
    /// Zero on every non-hero row, and `> 0` on every hero row; both halves
    /// are checked by data.rs's validator, so `unit_value` and
    /// `hero_train_cost` can read this without a branch on race or tier.
    ///
    /// The revival *time* is not here: `HERO_REVIVE_TIME` is one number shared
    /// by every class because no design wants a Far Seer to come back slower
    /// than a Warchief, whereas the revival PRICE is exactly the per-class
    /// balance knob this bead exists to turn.
    #[serde(default)]
    pub revive_gold: u32,
    #[serde(default)]
    pub revive_lumber: u32,
}

/// **What a unit is WORTH**, as opposed to what it costs you to queue one.
///
/// For everything except a hero these are the same number and this is
/// `cost_gold`/`cost_lumber` with extra steps. For a hero they diverge on
/// purpose: training is free, so the cost fields are 0, and a worth of zero
/// would quietly break the two places that ask "how much did that body
/// represent" — `xp_for_kill` (killing the enemy's Champion would grant no
/// experience at all) and `asset_score` (a hero army would settle a timeout
/// as though it owned nothing).
///
/// The answer for a hero is its REPLACEMENT cost, which is exactly what
/// `building_value` already does one type over: a thing is worth what it
/// takes to have it standing again.
pub fn unit_value(kind: UnitKind) -> (u32, u32) {
    let s = unit_stats(kind);
    if is_hero_kind(kind) {
        (s.revive_gold, s.revive_lumber)
    } else {
        (s.cost_gold, s.cost_lumber)
    }
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
    crate::data::unit_row(kind).stats
}

/// One alarm kind's threshold and reflex window. See `AlarmTuning`.
pub fn alarm_tuning(kind: AlarmKind) -> AlarmTuning {
    crate::data::alarm_row(kind).tuning
}

/// Weapon on a building (towers). Always fires a projectile.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingAttack {
    pub damage: f32,
    pub range: f32,
    pub cooldown: f32,
    /// May this emplacement shoot airborne targets? Towers can — static
    /// defense is the one thing a flyer cannot simply walk around, so a base
    /// that invested in towers is never helpless against air.
    pub can_hit_air: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    crate::data::building_row(kind).stats
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
    crate::data::building_row(kind).upgrades_to
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

/// Everything on a town hall ladder. The one question the drop-off logic,
/// Town Portal, rally fallbacks and the AI's base bookkeeping actually mean
/// when they used to ask `kind == TownHall`.
///
/// Was `upgrade_root(kind) == TownHall`, which is exactly the sentence a
/// second hall ladder makes false. It is the row's ROLE now, so the Horde's
/// Stronghold→Fortress→Hold is a hall ladder for the same reason the
/// Kingdom's is: because the table says `role: Hall`.
pub fn is_hall(kind: BuildingKind) -> bool {
    building_role(kind) == BuildingRole::Hall
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

/// Ask a forge to begin the next rung of a research ladder. Written by
/// intent.rs (for both player seats) and ai.rs; economy.rs validates, pays and
/// inserts `Researching` — the same division of labour as `UpgradeBuilding`,
/// and the reason "all money in the game is spent in economy.rs" stays true.
///
/// The LEVEL is not on the event: it is always the team's current level plus
/// one, resolved by economy.rs at the instant it takes the money. Carrying a
/// level would let two events queued in one frame both claim to produce
/// level 2.
#[derive(Event, Debug)]
pub struct StartResearch {
    pub building: Entity,
    pub kind: ResearchKind,
}

/// Tech requirements: completed buildings a team must own before this
/// building may be PLACED. economy.rs enforces at placement; ui.rs greys the
/// button; bridge.rs reports and validates.
pub fn building_requires(kind: BuildingKind) -> &'static [BuildingKind] {
    &crate::data::building_row(kind).requires
}

/// Tech requirements for TRAINING a unit (beyond owning its trainer building).
pub fn unit_requires(kind: UnitKind) -> &'static [BuildingKind] {
    &crate::data::unit_row(kind).requires
}

/// The LOWEST rung that trains `kind`, which is the one worth naming:
/// `building_satisfies` makes a requirement of "TownHall" mean "TownHall or
/// better", so the base rung covers the Keep and Castle that train Workers too.
///
/// `None` only if nothing trains it — impossible today, but the table is data.
/// Folded out of three copies of the same `find` (`unit_tech_chain`,
/// `game_catalog`'s `trainer_of`, and the error strings below), because "where
/// does this come from" is now asked in enough places that three answers is
/// three chances to disagree.
pub fn unit_trainer(kind: UnitKind) -> Option<BuildingKind> {
    ALL_BUILDING_KINDS
        .iter()
        .copied()
        .find(|b| trainable(*b).contains(&kind))
}

/// The subset of `reqs` this team does not meet. Tier-aware exactly like
/// `requirements_met`, so a Castle is never reported as a missing Keep.
pub fn missing_requirements(reqs: &[BuildingKind], completed: &[BuildingKind]) -> Vec<BuildingKind> {
    reqs.iter()
        .copied()
        .filter(|r| !completed.iter().any(|owned| building_satisfies(*owned, *r)))
        .collect()
}

/// `"a Workshop"` / `"a Workshop and a Castle"`.
fn a_list(kinds: &[BuildingKind]) -> String {
    let names: Vec<String> = kinds
        .iter()
        .map(|k| format!("a {}", building_name(*k)))
        .collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, head)) => format!("{} and {last}", head.join(", ")),
    }
}

fn stands(n: usize) -> &'static str {
    if n == 1 { "stands" } else { "stand" }
}

/// What the team holds toward a requirement it does not meet, parenthesised.
///
/// The hall ladder is the case worth spelling out: `you have none` is a lie to
/// a commander staring at a TownHall while being told they need a Castle, and
/// `upgrade it` is the entire instruction they were missing.
fn holdings_clause(missing: &[BuildingKind], completed: &[BuildingKind]) -> String {
    let lower = missing.iter().find_map(|r| {
        completed
            .iter()
            .filter(|owned| upgrade_root(**owned) == upgrade_root(*r))
            .max_by_key(|owned| building_tier(**owned))
            .map(|owned| building_name(*owned))
    });
    match lower {
        Some(have) => format!("yours is a {have} — upgrade it"),
        None => "you have none".to_string(),
    }
}

/// Why a `train` order at the RIGHT building bounced: the unit's own tech gate.
/// `None` when the team already meets it.
///
/// Round-9 AAR (`wc3clone-pbd`) — this used to render through the generic
/// `requirement_error` as `Raider requires Workshop`, which is true and
/// useless. The commander read it *at the Barracks*, concluded the Barracks
/// was the wrong building, walked the next order over to the Workshop, and got
/// `Workshop cannot train Raider`. Between them the two strings never said the
/// one thing that unsticks you: **keep training it here, once a Workshop
/// stands**. So the string names the trainer even though the reader is already
/// standing at it — the redundancy is the fix.
pub fn train_gate_error(kind: UnitKind, completed: &[BuildingKind]) -> Option<String> {
    let missing = missing_requirements(unit_requires(kind), completed);
    if missing.is_empty() {
        return None;
    }
    let trainer = unit_trainer(kind).map(building_name).unwrap_or("-");
    Some(format!(
        "{} trains at the {trainer} once {} {} ({})",
        kind_name(kind),
        a_list(&missing),
        stands(missing.len()),
        holdings_clause(&missing, completed),
    ))
}

/// Why a `train` order at the WRONG building bounced — and where to send it.
///
/// The other half of the round-9 pair. `Workshop cannot train Raider` is
/// technically true and maximally confusing: it is the reply to the correction
/// the *previous* error talked the commander into making. Naming the real
/// trainer, its gate, and the unit's gate turns a dead end into a build order.
///
/// The Sorcerer is the case that shaped the last clause. Its `unit_requires`
/// is empty — the gate is on the Arcane Sanctum (which needs a Keep), not on
/// the unit — so a commander with no Sanctum has no building to name and can
/// only ever reach this string. It therefore has to carry the *trainer's* gate
/// too, or the one unit whose gate is invisible in `unit_requires` stays
/// invisible here.
pub fn wrong_trainer_error(
    at: BuildingKind,
    kind: UnitKind,
    completed: &[BuildingKind],
) -> String {
    let unit = kind_name(kind);
    let head = format!("{} cannot train {unit}", building_name(at));
    let Some(trainer) = unit_trainer(kind) else {
        return format!("{head} — nothing trains it");
    };
    let trainer_name = building_name(trainer);
    let mut out = format!("{head} — {unit} trains at the {trainer_name}");
    let unit_missing = missing_requirements(unit_requires(kind), completed);
    if !unit_missing.is_empty() {
        out.push_str(&format!(
            " once {} {}",
            a_list(&unit_missing),
            stands(unit_missing.len())
        ));
    }
    if !completed.iter().any(|owned| building_satisfies(*owned, trainer)) {
        let trainer_missing = missing_requirements(building_requires(trainer), completed);
        if trainer_missing.is_empty() {
            out.push_str(&format!(" (you have no {trainer_name})"));
        } else {
            out.push_str(&format!(
                " (you have no {trainer_name}; it needs {})",
                a_list(&trainer_missing)
            ));
        }
    }
    out
}

/// Everything that must be STANDING before a team can train `kind`: the
/// building that trains it, whatever gates that building, and whatever gates
/// the unit itself — transitively, to the bottom.
///
/// `unit_requires` above is deliberately partial. It lists requirements
/// *beyond* owning the trainer, which is the right shape for
/// `requirements_met` (the trainer is checked separately, by the order being
/// given at it) and the wrong shape for a CATALOG. Exported raw it says
/// `Footman: requires []` — which reads as "buildable from nothing" — and
/// `Catapult: requires []`, hiding a real Barracks→Workshop chain behind a
/// join the JSON never advertises. The caveat that made those entries true
/// lived in a Rust doc comment, and serde does not export doc comments.
///
/// So this is the shape the catalog ships: complete, and needing no prose.
/// Order is a usable build order — trainer first, then its gates.
pub fn unit_tech_chain(kind: UnitKind) -> Vec<BuildingKind> {
    fn add(chain: &mut Vec<BuildingKind>, b: BuildingKind) {
        if !chain.contains(&b) {
            chain.push(b);
        }
    }
    let mut chain: Vec<BuildingKind> = Vec::new();
    // The LOWEST rung that trains it is the one to name — see `unit_trainer`.
    if let Some(trainer) = unit_trainer(kind) {
        add(&mut chain, trainer);
    }
    for req in unit_requires(kind) {
        add(&mut chain, *req);
    }
    // Breadth-first over what those in turn need, so the list stays in build
    // order and a cycle (there are none, but the data is editable) terminates.
    let mut i = 0;
    while i < chain.len() {
        for req in building_requires(chain[i]) {
            add(&mut chain, *req);
        }
        i += 1;
    }
    chain
}

/// The team tech tier a unit needs — the highest rung anything in its chain
/// sits on. Non-hall buildings are all tier 1, so this reduces to "how far up
/// the hall ladder must I be", which is exactly what `tech_tier_for` measures.
///
/// Honest rather than flattering: a Catapult is tier 1, because a Workshop
/// needs only a Barracks and no hall upgrade at all.
pub fn unit_tier(kind: UnitKind) -> u32 {
    unit_tech_chain(kind)
        .iter()
        .map(|b| building_tier(*b))
        .max()
        .unwrap_or(1)
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Deserialize)]
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

// ---------------------------------------------------------------------------
// Research: the bank-to-power conversion
// ---------------------------------------------------------------------------
//
// Everything else a team can buy is a THING — a unit that can die, a building
// that can be razed, an item that gets consumed. Research is the one purchase
// that buys a *property of the faction*: it is retroactive to every soldier
// already standing, applies to every soldier trained afterwards, cannot be
// killed, and survives the Blacksmith that produced it. That is deliberate and
// it is the whole point of the mechanic. Every AAR in this repo that read
// "Human ended with 2,400 banked gold and lost anyway" was describing a game
// with no sink that converts money into fighting strength faster than a
// production queue can. This is that sink.
//
// The shape of the design, and why:
//
//   * **Flat, not percentage.** +1 damage per swing and −1 damage per hit
//     taken. A percentage would scale with whatever the biggest number on the
//     field is (a Catapult's 6x siege multiplier, a level-10 hero) and turn
//     research into a rich-get-richer multiplier on an existing lead. Flat
//     bonuses are worth proportionally MORE to cheap line infantry — +3 on a
//     Footman's 12 is +25%, on a Hero's 24 it is +12.5% — so the ladder
//     rewards the player who has an army over the player who has a deathball.
//   * **Applied after the multipliers, never through them.** See
//     `EffectiveStats::bonus_damage`.
//   * **Units only, never structures.** Attack research does not arm Towers and
//     armor research does not thicken walls. Research equips the army; masonry
//     is what a Keep upgrade is for. Without this rule a turtle could buy 3
//     levels of armor and make every building in the base 25% harder to raze,
//     which is a fortification upgrade wearing a research label.
//   * **A floor of `MIN_DAMAGE_PER_HIT`.** Three levels of armor against a
//     Spearman's 6 damage is a 50% reduction; against a Worker's 5 it is 60%.
//     Neither can ever reach zero, so no amount of research makes a team immune
//     to anything, and chip damage always chips.

/// The two team-wide ladders a Blacksmith researches. Ids here are what the
/// `research` intent's `upgrade` field accepts and what the catalog exports.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize)]
pub enum ResearchKind {
    /// +1 flat damage per level on every attack a UNIT makes.
    Attack,
    /// −1 flat damage per level on every hit a UNIT takes.
    Armor,
}

pub const ALL_RESEARCH_KINDS: [ResearchKind; 2] = [ResearchKind::Attack, ResearchKind::Armor];

/// Top of both ladders. The bonus at max is +3/−3.
pub const RESEARCH_MAX_LEVEL: u32 = 3;

/// No hit, however armoured the victim, ever lands for less than this. The
/// floor that keeps armor a discount rather than an immunity.
pub const MIN_DAMAGE_PER_HIT: f32 = 1.0;

impl ResearchKind {
    /// Wire id: the `upgrade` field of a `research` intent, and the catalog id.
    pub fn id(self) -> &'static str {
        crate::data::research_ladder(self).0
    }
    /// What a HUD button and a log line call it.
    pub fn label(self) -> &'static str {
        crate::data::research_ladder(self).1
    }
    /// Stable slot in `ResearchState`, and the order the command card lays the
    /// buttons out in.
    pub fn index(self) -> usize {
        match self {
            ResearchKind::Attack => 0,
            ResearchKind::Armor => 1,
        }
    }
    pub fn description(self) -> &'static str {
        crate::data::research_ladder(self).2
    }
}

/// One rung of a research ladder: what it costs and how long the forge is busy.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchStep {
    /// The level this step produces (1..=`RESEARCH_MAX_LEVEL`).
    pub level: u32,
    pub cost_gold: u32,
    pub cost_lumber: u32,
    /// Seconds of forge time, timed exactly like a training queue item.
    pub research_time: f32,
}

/// Cost and duration of advancing a ladder TO `level`. `None` above the cap or
/// at level 0 — the one table, so catalog, UI, intent validation, economy and
/// the AI all quote the same numbers.
///
/// The escalation is deliberately steeper in lumber than in gold (2x, 2x)
/// because gold is the resource a winning economy floods with; a team that
/// wants all three rungs has to have kept workers on trees, not just on the
/// mine it is out-expanding you at.
pub fn research_step(kind: ResearchKind, level: u32) -> Option<ResearchStep> {
    // Both ladders share one price list — see research.ron for why — so the
    // ladder is not part of the lookup today. The parameter stays in the
    // signature because a per-ladder price list is a data change, not an API
    // change: research.ron would grow an optional `steps` per ladder.
    let _ = kind;
    if level == 0 || level > RESEARCH_MAX_LEVEL {
        return None;
    }
    crate::data::research_step(level)
}

/// The flat bonus a ladder at `level` confers. Linear on purpose: a commander
/// reading "attack 2" should be able to add 2 to a damage number in their head
/// without consulting a table.
pub fn research_bonus(kind: ResearchKind, level: u32) -> f32 {
    let _ = kind;
    level.min(RESEARCH_MAX_LEVEL) as f32
}

/// Which building kind researches a ladder, for the roster that shipped
/// first. One function so a second forge (or moving a ladder to another
/// building) is a data change.
///
/// **Research is SHARED content and the forge is not.** Both races buy the
/// same two ladders — the same ids, the same prices, the same `+1/+2/+3` — out
/// of their own building (`research_buildings` lists them all). That is the
/// honest v1 call: the ladders are a *property of the faction* expressed as
/// two flat terms in one stat law, and a per-race price list would have been a
/// balance lever nobody asked for dressed up as content. If a race ever needs
/// its own ladder, `research.ron` grows a race field and this returns an
/// `Option<BuildingKind>` per race — which is exactly the shape below.
pub fn research_building(kind: ResearchKind) -> BuildingKind {
    let _ = kind;
    crate::data::research_buildings()[0]
}

/// Every forge in the game — one per race.
pub fn research_buildings() -> &'static [BuildingKind] {
    crate::data::research_buildings()
}

/// The forge THIS race builds, if it has one.
pub fn research_building_for(race: Race) -> Option<BuildingKind> {
    research_buildings()
        .iter()
        .copied()
        .find(|&k| race_has_building(race, k))
}

/// Can this building kind run research at all? What the command card asks
/// before drawing research buttons and what the compiler asks before accepting
/// a `research` intent.
pub fn building_researches(kind: BuildingKind) -> &'static [ResearchKind] {
    &crate::data::building_row(kind).researches
}

/// One team's completed research. Levels only ever go up: unlike `TechTier`,
/// which is recounted from standing buildings every frame, research is a LATCH.
/// Razing the forge does not unlearn the metallurgy, and that asymmetry is the
/// reason research is worth its price — a Keep upgrade you can be denied by
/// losing the hall, a completed upgrade nobody can take back.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ResearchState {
    levels: [u32; ALL_RESEARCH_KINDS.len()],
}

impl ResearchState {
    pub fn level(&self, kind: ResearchKind) -> u32 {
        self.levels[kind.index()]
    }
    /// Advance a ladder by one rung, saturating at the cap. Returns the new
    /// level, or `None` if it was already maxed.
    pub fn advance(&mut self, kind: ResearchKind) -> Option<u32> {
        let slot = &mut self.levels[kind.index()];
        if *slot >= RESEARCH_MAX_LEVEL {
            return None;
        }
        *slot += 1;
        Some(*slot)
    }
    /// The step that would come next, or `None` at the cap.
    pub fn next_step(&self, kind: ResearchKind) -> Option<ResearchStep> {
        research_step(kind, self.level(kind) + 1)
    }
    /// Flat damage this team adds to every unit attack.
    pub fn attack_bonus(&self) -> f32 {
        research_bonus(ResearchKind::Attack, self.level(ResearchKind::Attack))
    }
    /// Flat damage this team subtracts from every hit one of its units takes.
    pub fn armor_bonus(&self) -> f32 {
        research_bonus(ResearchKind::Armor, self.level(ResearchKind::Armor))
    }
    /// Both numbers in the shape the stat law wants them.
    pub fn bonus(&self) -> ResearchBonus {
        ResearchBonus {
            bonus_damage: self.attack_bonus(),
            flat_armor: self.armor_bonus(),
        }
    }
}

/// Per-team research levels. Written in exactly one place (economy.rs, when a
/// `Researching` timer runs out) and read by combat.rs through the stat law,
/// by the UI, by the snapshot and by the AI.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TeamResearch {
    pub human: ResearchState,
    pub claude: ResearchState,
}

impl TeamResearch {
    pub fn get(&self, team: Team) -> ResearchState {
        match team {
            Team::Human => self.human,
            Team::Claude => self.claude,
        }
    }
    pub fn get_mut(&mut self, team: Team) -> &mut ResearchState {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
}

/// A forge working. `intent.rs` inserts it (after economy.rs has taken the
/// money), economy.rs ticks it and applies the level on completion — the same
/// division of labour as `Upgrading` and `Order::Build`.
///
/// **One at a time, and concurrent requests are REJECTED, not queued.** A
/// queue here would let a team pre-commit its whole research plan in one click
/// and then walk away, which turns the interesting part (spending 250 gold now
/// versus three more Footmen now) into a single early decision. Rejecting
/// makes every rung its own choice, and the answer to "I want both ladders at
/// once" is the same answer an RTS gives to "I want two units at once": build
/// a second building.
#[derive(Component, Clone, Copy, Debug)]
pub struct Researching {
    pub kind: ResearchKind,
    /// The level this will produce when it finishes.
    pub to_level: u32,
    pub remaining: f32,
    /// Full duration, so a renderer can show a fraction.
    pub total: f32,
}

/// What each building can train.
pub fn trainable(kind: BuildingKind) -> &'static [UnitKind] {
    &crate::data::building_row(kind).trains
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
    crate::data::unit_row(kind).name.as_str()
}

pub fn building_name(kind: BuildingKind) -> &'static str {
    crate::data::building_row(kind).name.as_str()
}

pub fn unit_description(kind: UnitKind) -> &'static str {
    crate::data::unit_row(kind).description.as_str()
}

pub fn building_description(kind: BuildingKind) -> &'static str {
    crate::data::building_row(kind).description.as_str()
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogUnit {
    pub id: &'static str,
    /// **Which rosters may field this.** The catalog is one document for the
    /// whole session (both seats get byte-identical files), so a commander
    /// finds its own roster by matching this against the `race` its snapshot
    /// carries. A row that lists both races is neutral content.
    pub race: Vec<&'static str>,
    /// What this unit is FOR, race-independently (`Line`, `Ranged`,
    /// `Cavalry`, `AntiCavalry`, `Siege`, `Caster`, `Flyer`, `Shock`,
    /// `Worker`, `HeroMelee`, `HeroSupport`). The cross-race Rosetta stone: a
    /// commander that knows to screen `Ranged` behind `Line` needs no table of
    /// kind names to do it on either side.
    pub role: &'static str,
    pub cost_gold: u32,
    pub cost_lumber: u32,
    /// **Heroes only, and the reason a hero's cost above reads 0/0.** Your
    /// first hero of a class is free to train; this is what putting it back on
    /// the map costs after it dies, with its level preserved. Omitted entirely
    /// for every non-hero row, where it would always be zero.
    ///
    /// It is also what the hero is WORTH: the experience an enemy gains for
    /// killing it, and what it contributes to the material score that settles
    /// a timeout, are both counted off this number rather than off the zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revive_gold: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revive_lumber: Option<u32>,
    pub supply: u32,
    pub hp: f32,
    pub damage: f32,
    pub range: f32,
    /// Seconds between attacks. Exported because every balance claim in the
    /// descriptions is stated in dps, and `damage` alone cannot produce one.
    pub attack_cooldown: f32,
    pub speed: f32,
    pub train_time: f32,
    /// Which `TargetClass` this kind IS — the class a `priority` command
    /// names, and the class the `vs_*` multipliers below are keyed against.
    /// Load-bearing for the counter triangle: a Knight and a Raider are both
    /// `Cavalry`, so a Spearman's `vs_cavalry_mult` lands on both, and no
    /// amount of reading the two unit entries side by side would reveal that
    /// without this field.
    pub class: Option<&'static str>,
    /// Damage multipliers, all three of them. Only `vs_building_mult` used to
    /// be here, which made tools/COMMANDER_BRIEF.md's "check catalog `vs_*`
    /// multipliers" an instruction that could not be followed: the anti-siege
    /// and anti-cavalry legs of the counter triangle existed in `combat.rs`
    /// and in English inside `description`, and nowhere a machine could read.
    pub vs_building_mult: f32,
    pub vs_siege_mult: f32,
    pub vs_cavalry_mult: f32,
    /// Airborne: ignores terrain and buildings when moving, and can only be
    /// attacked by things whose `can_hit_air` is true.
    pub flying: bool,
    pub can_hit_air: bool,
    pub can_hit_ground: bool,
    /// Sight radius — how far this kind lifts fog of war for its team.
    pub vision: f32,
    /// The lowest rung that trains this kind. Higher rungs of the same ladder
    /// train it too — `tier`/`upgraded_from` on the building say so.
    pub trained_at: &'static str,
    /// **Everything that must be standing to train this**, transitively,
    /// including `trained_at` itself and anything that gates it. In build
    /// order. See `unit_tech_chain`: this used to list only the extras beyond
    /// the trainer, so a Footman claimed to require nothing at all.
    pub requires: Vec<&'static str>,
    /// Team tech tier this unit needs — the highest `tier` in `requires`.
    pub tier: u32,
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

/// One entry of `buildings[].trains_gated`: a unit on this building's roster,
/// with the tech gate that applies **at this building**.
///
/// Round-9 AAR (`wc3clone-pbd`): `buildings[].trains` is a bare list of ids, so
/// the Barracks advertised `["Footman","Archer","Spearman","Raider","Knight",
/// "Champion","Priestess"]` with nothing anywhere to say that two of those
/// wait on a Workshop and a Castle. `units[].requires` had the answer, but it
/// is on the other side of the catalog and a commander reading a *roster* has
/// no reason to suspect a join is needed. So the gate now sits where the
/// roster is read. It cost a commander their scout timing to learn otherwise.
#[derive(Serialize, Clone, Debug)]
pub struct CatalogTrains {
    /// `units[].id`.
    pub unit: &'static str,
    /// Buildings that must ALSO stand before this trainer accepts the order —
    /// `unit_requires`, i.e. the gate BEYOND owning the trainer itself. Empty
    /// for an ungated unit. Whatever gates the trainer is `requires` on this
    /// same building entry, so the two fields together are the whole chain.
    pub requires: Vec<&'static str>,
    /// Team tech tier the unit needs — the same 1/2/3 scale as `units[].tier`.
    pub tier: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogBuilding {
    pub id: &'static str,
    /// Which rosters may build this. See `CatalogUnit::race`.
    pub race: Vec<&'static str>,
    /// What this building is FOR (`Hall`, `Production`, `Supply`, `Defense`,
    /// `Barrier`, `Tech`, `Forge`, `Siegeworks`, `Vendor`).
    pub role: &'static str,
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
    /// The roster as bare ids. Kept verbatim and in the same order — it is the
    /// historical shape and tools read it (`verify_research_bridge.py` asserts
    /// the Blacksmith's is empty) — but read `trains_gated` instead: this list
    /// cannot tell you that half of it is locked.
    pub trains: Vec<&'static str>,
    /// The same roster with each unit's gate attached. Parallel to `trains`,
    /// element for element, so the two can never disagree about who trains
    /// what — only about how much they say.
    pub trains_gated: Vec<CatalogTrains>,
    /// Research ladders this building can start (`research[].id`). The inverse
    /// of `research[].researched_at`, which was the only direction exported —
    /// so "what is a Blacksmith FOR" needed the reader to scan a different
    /// array on the off chance.
    pub researches: Vec<&'static str>,
    /// Items this building sells (`items[].id`), in shelf order. The inverse
    /// of `items[].sold_at`, and the catalog half of the live snapshot's
    /// `buildings[].sells` — which carries the same ids with this team's tier
    /// already applied.
    pub sells: Vec<&'static str>,
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

/// One atom of an ability, as a commander reads it. Every field beyond `atom`
/// and `schedule` is optional because the atoms genuinely differ: a summon has
/// a `count` and no `magnitude`, a hex has a `magnitude` and no `count`, and
/// padding both with nulls would only invite a commander to average them.
#[derive(Serialize, Clone, Debug)]
pub struct CatalogEffect {
    /// `"damage" | "heal" | "status" | "militia" | "summon" | "teleport"`.
    pub atom: &'static str,
    /// `"instant" | "over_time"`. (`on_hit`/`on_death` are schema only — the
    /// loader refuses them, so they never reach the wire.)
    pub schedule: &'static str,
    /// Whose bodies in the radius this atom looks for: `"enemies"`,
    /// `"allies"`, `"own_workers"`. Absent for atoms that fire at the centre.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<&'static str>,
    /// HP dealt or restored, per application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magnitude: Option<f32>,
    /// Seconds the status (or the militia service) lasts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// Seconds a summon survives; absent means permanent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<&'static str>,
    /// `over_time` only: seconds between applications, and how many there are
    /// (the FIRST lands at the cast).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u32>,
}

impl CatalogEffect {
    pub fn of(effect: &Effect) -> CatalogEffect {
        let (interval, ticks) = match effect.schedule {
            EffectSchedule::OverTime { interval, ticks } => (Some(interval), Some(ticks)),
            _ => (None, None),
        };
        let mut out = CatalogEffect {
            atom: effect.atom.name(),
            schedule: effect.schedule.name(),
            targets: effect.atom.targets().map(|t| t.name()),
            amount: None,
            status: None,
            magnitude: None,
            duration: None,
            unit_kind: None,
            count: None,
            lifetime: None,
            destination: None,
            interval,
            ticks,
        };
        match effect.atom {
            EffectAtom::Damage { amount, .. } | EffectAtom::Heal { amount, .. } => {
                out.amount = Some(amount);
            }
            EffectAtom::ApplyStatus { status, magnitude, duration, .. } => {
                out.status = Some(status.name());
                out.magnitude = Some(magnitude);
                out.duration = Some(duration);
            }
            EffectAtom::Militia { duration, .. } => out.duration = Some(duration),
            EffectAtom::Summon { unit_kind, count, lifetime } => {
                out.unit_kind = Some(kind_name(unit_kind));
                out.count = Some(count);
                out.lifetime = lifetime;
            }
            EffectAtom::Teleport { destination, .. } => {
                out.destination = Some(destination.name());
            }
        }
        out
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogAbility {
    pub id: &'static str,
    pub caster: &'static str,
    /// Slot in the caster's ability list — what a `cast` command's `ability`
    /// field accepts as an integer, and the order the hotkeys follow.
    pub index: usize,
    /// **The v2 headline**: the wire name of the FIRST atom. Kept so a
    /// commander written before v3 still reads. `effects` below is the whole
    /// sentence, and it is what a commander should read now.
    pub effect: &'static str,
    /// **The composition** — every atom this ability applies, in the order it
    /// applies them, with its own numbers, its own targets and its own
    /// schedule. An ability is a sentence: the demo row in abilities.ron is
    /// `[damage 60 enemies, status slow 0.35 for 4s enemies]`, and nothing in
    /// the engine knows its name.
    pub effects: Vec<CatalogEffect>,
    /// Status kind applied, for `effect == "status"`.
    pub status: Option<&'static str>,
    /// A second status the same cast lays down, with its own magnitude —
    /// `[kind, magnitude]`. Only Sanctuary has one today. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status2: Option<(&'static str, f32)>,
    /// **Geometry**: `"caster"`, `"point"` or `"unit"` — where the `radius`
    /// is centred. `"caster"` takes no target payload; `"point"` takes
    /// `x`/`z`; `"unit"` takes `target`. Omit the payload on any of them and
    /// the engine aims for you (biggest reachable clump of whatever the
    /// ability affects).
    pub target: &'static str,
    /// How far from the caster the centre may be, for a targeted ability.
    /// Null for `"caster"`. The spell's total reach is `target_range + radius`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_range: Option<f32>,
    pub mana_cost: f32,
    pub cooldown: f32,
    /// How far the effect spreads from its CENTRE — which is the caster only
    /// for `target == "caster"`.
    pub radius: f32,
    pub power: f32,
    /// Seconds the applied status lasts (0 for instant effects).
    pub duration: f32,
    pub hits_air: bool,
    /// Human-readable unlock condition: "always", "hero level N", "tier TN".
    /// Kept verbatim — the snapshot's `requires` uses the same text and the
    /// HUD prints it — but it is prose, so the two fields below carry the same
    /// predicate as numbers. Both null means "always".
    pub unlock: String,
    /// Hero level the caster must reach, when that is the gate. Nothing else
    /// in the catalog says how a hero levels, but this at least names the
    /// quantity rather than burying `5` inside a sentence.
    pub unlock_hero_level: Option<u32>,
    /// Team tech tier required, when that is the gate — the same 1/2/3 scale
    /// as `buildings[].tier` and `items[].tier`, so the three gating systems
    /// are finally comparable without parsing "tier T2" out of a string.
    pub unlock_tier: Option<u32>,
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

/// One rung of a research ladder, as the catalog exports it.
#[derive(Serialize, Clone, Debug)]
pub struct CatalogResearchLevel {
    /// The level this step produces (1-based).
    pub level: u32,
    pub cost_gold: u32,
    pub cost_lumber: u32,
    /// Seconds the forge is busy. Timed exactly like a training queue item.
    pub research_time: f32,
    /// The flat bonus the team holds ONCE this level is complete (cumulative,
    /// not incremental — level 2 reads 2.0, not another 1.0).
    pub bonus: f32,
}

/// A team-wide passive upgrade ladder. The CURRENT level is deliberately not
/// here: the catalog is static content written once at startup, and a level is
/// match state. Read it from the snapshot (`me.research`) instead.
#[derive(Serialize, Clone, Debug)]
pub struct CatalogResearch {
    /// Wire id — what the `research` command's `upgrade` field accepts.
    pub id: &'static str,
    pub name: &'static str,
    /// Catalog id of the building that researches it, for the roster that
    /// shipped first. See `forges` for the full per-race list.
    pub researched_at: &'static str,
    /// **Every** building that runs this ladder — one per race. The ladders
    /// themselves are shared content: same ids, same prices, same bonuses,
    /// whichever forge you paid.
    pub forges: Vec<&'static str>,
    pub max_level: u32,
    /// What the bonus applies to, in one phrase — the answer to "do my towers
    /// get this?" without reading the source. See `description` for why.
    pub applies_to: &'static str,
    pub levels: Vec<CatalogResearchLevel>,
    pub description: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogItem {
    pub id: &'static str,
    pub cost_gold: u32,
    pub sold_at: &'static str,
    /// Team tech tier needed to buy it: 1, 2 or 3. A Shop stocks the whole
    /// shelf from the moment it is built; this is what decides which rungs a
    /// given team may actually take off it.
    pub tier: u32,
    /// `"choosable"` on the two teleport items and absent on everything else:
    /// `use_item` takes an optional `destination` (a building id of one of
    /// your own standing halls), and omitting it sends the scroll to the hall
    /// nearest the hero. A field rather than a sentence buried in
    /// `description` because this is the one item property a commander has to
    /// ACT on — the scroll that saves a main from an army standing at the
    /// expansion is the scroll aimed away from where the hero is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<&'static str>,
    pub description: &'static str,
}

/// One stance word, with the bundle it installs.
///
/// The catalog half of [`StanceDef`]: same numbers, wire-shaped. Published
/// because a *form* has to be able to state a field's domain, and the five
/// words are a domain. Without this the affordance document
/// (`tools/bridge_view.py --doc`) would have to carry its own copy of the
/// vocabulary, which is a second source of truth for the one thing the fixed
/// vocabulary exists to keep single.
#[derive(Serialize, Clone, Debug)]
pub struct CatalogStance {
    /// The word the `stance` command takes.
    pub id: &'static str,
    pub label: String,
    pub description: String,
    /// `"defend"`, `"push"` or `"forage"` — the posture this stance installs.
    pub posture: &'static str,
    /// Ring radius for a `defend` stance; `0` for the two that have no ring.
    pub radius: f32,
    /// Leash radius; `0` means no leash, which is a setting and not an omission.
    pub leash: f32,
    /// Retreat threshold as a fraction of max HP; `0` installs none.
    pub retreat_below: f32,
    /// Where the retreat falls back TO: `"anchor"` or `"base"`.
    pub rally: &'static str,
    /// Focus-fire classes, in order. Empty leaves acquisition alone.
    pub priority: Vec<&'static str>,
}

/// The late-bound role vocabulary, split by where each phrase is legal.
///
/// Every list here is a split of the `SELECTOR_*_NAMES` consts, so the refusal
/// a commander reads and the domain a form serves can never drift apart. See
/// [`Selector`] and docs/INTENT.md § "The role channel".
#[derive(Serialize, Clone, Debug)]
pub struct CatalogSelectors {
    /// Phrases legal in `"select"`, wherever a verb takes `units`.
    pub units: Vec<&'static str>,
    /// Phrases legal in `harvest`'s `"target_select"`.
    pub nodes: Vec<&'static str>,
    /// Phrases legal in `build`'s `"site"`.
    pub sites: Vec<&'static str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Catalog {
    pub units: Vec<CatalogUnit>,
    pub buildings: Vec<CatalogBuilding>,
    pub abilities: Vec<CatalogAbility>,
    /// The team-wide passive ladders a Blacksmith buys. Costs and durations
    /// only — current levels are match state and live in the snapshot.
    pub research: Vec<CatalogResearch>,
    pub items: Vec<CatalogItem>,
    /// The status-effect vocabulary: what a buff/debuff means and how it
    /// stacks, so a commander can reason about them without reading the source.
    pub statuses: Vec<CatalogStatus>,
    /// The five stance words and what each one installs.
    pub stances: Vec<CatalogStance>,
    /// The selector phrases, by channel.
    pub selectors: CatalogSelectors,
}

/// Assemble the full content catalog from the stat/requirement tables.
pub fn game_catalog() -> Catalog {
    let trainer_of = |kind: UnitKind| unit_trainer(kind).map(building_name).unwrap_or("-");
    Catalog {
        units: ALL_UNIT_KINDS
            .iter()
            .map(|&k| {
                let s = unit_stats(k);
                CatalogUnit {
                    id: kind_name(k),
                    race: catalog_races(unit_races(k)),
                    role: role_name(unit_role(k)),
                    cost_gold: s.cost_gold,
                    cost_lumber: s.cost_lumber,
                    revive_gold: is_hero_kind(k).then_some(s.revive_gold),
                    revive_lumber: is_hero_kind(k).then_some(s.revive_lumber),
                    supply: s.supply,
                    hp: s.hp,
                    damage: s.damage,
                    range: s.range,
                    attack_cooldown: s.attack_cooldown,
                    speed: s.speed,
                    train_time: s.train_time,
                    class: TargetClass::of(Some(k), false).map(|c| c.name()),
                    vs_building_mult: s.vs_building_mult,
                    vs_siege_mult: s.vs_siege_mult,
                    vs_cavalry_mult: s.vs_cavalry_mult,
                    flying: s.flying,
                    can_hit_air: s.can_hit_air,
                    can_hit_ground: s.can_hit_ground,
                    vision: s.vision,
                    trained_at: trainer_of(k),
                    // The FULL chain, not just the extras beyond the trainer.
                    requires: unit_tech_chain(k).iter().map(|b| building_name(*b)).collect(),
                    tier: unit_tier(k),
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
                    race: catalog_races(building_races(k)),
                    role: building_role_name(building_role(k)),
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
                    trains_gated: trainable(k)
                        .iter()
                        .map(|u| CatalogTrains {
                            unit: kind_name(*u),
                            requires: unit_requires(*u)
                                .iter()
                                .map(|b| building_name(*b))
                                .collect(),
                            tier: unit_tier(*u),
                        })
                        .collect(),
                    researches: building_researches(k).iter().map(|r| r.id()).collect(),
                    sells: if k == BuildingKind::Shop {
                        ALL_ITEMS.iter().map(|i| item_def(*i).name).collect()
                    } else {
                        Vec::new()
                    },
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
                effect: a.effect_name(),
                effects: a.effects.iter().map(CatalogEffect::of).collect(),
                status: a.status().map(|s| s.name()),
                status2: a.extra_status().map(|(k, m)| (k.name(), m)),
                target: a.target.name(),
                target_range: a.target.range(),
                mana_cost: a.mana_cost,
                cooldown: a.cooldown,
                radius: a.radius,
                power: a.power(),
                duration: a.duration(),
                hits_air: a.hits_air,
                unlock: unlock_label(a.unlock),
                unlock_hero_level: match a.unlock {
                    AbilityUnlock::HeroLevel(n) => Some(n),
                    _ => None,
                },
                unlock_tier: match a.unlock {
                    AbilityUnlock::TeamTier(t) => Some(t.level()),
                    _ => None,
                },
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
        research: ALL_RESEARCH_KINDS
            .iter()
            .map(|&k| CatalogResearch {
                id: k.id(),
                name: k.label(),
                researched_at: building_name(research_building(k)),
                forges: research_buildings().iter().map(|b| building_name(*b)).collect(),
                max_level: RESEARCH_MAX_LEVEL,
                applies_to: "units only (not buildings or towers)",
                levels: (1..=RESEARCH_MAX_LEVEL)
                    .filter_map(|level| {
                        research_step(k, level).map(|s| CatalogResearchLevel {
                            level: s.level,
                            cost_gold: s.cost_gold,
                            cost_lumber: s.cost_lumber,
                            research_time: s.research_time,
                            bonus: research_bonus(k, level),
                        })
                    })
                    .collect(),
                description: k.description(),
            })
            .collect(),
        items: ALL_ITEMS
            .iter()
            .map(|&id| {
                let d = item_def(id);
                CatalogItem {
                    id: d.name,
                    cost_gold: d.cost_gold,
                    sold_at: building_name(BuildingKind::Shop),
                    tier: d.tier.level(),
                    destination: item_chooses_destination(id).then_some("choosable"),
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
        stances: ALL_STANCES
            .iter()
            .map(|&s| {
                let d = s.def();
                CatalogStance {
                    id: s.word(),
                    label: d.label.clone(),
                    description: d.description.clone(),
                    posture: match d.posture {
                        StancePosture::Defend => "defend",
                        StancePosture::Push => "push",
                        StancePosture::Forage => "forage",
                    },
                    radius: d.radius,
                    leash: d.leash,
                    retreat_below: d.retreat_below,
                    rally: match d.rally {
                        StanceRally::Anchor => "anchor",
                        StanceRally::Base => "base",
                    },
                    priority: d.priority.iter().map(|c| c.name()).collect(),
                }
            })
            .collect(),
        // Split off the consts rather than re-listed, so the domain a form
        // serves and the refusal a bad phrase earns are the same words.
        selectors: CatalogSelectors {
            units: SELECTOR_UNIT_NAMES.split(", ").collect(),
            nodes: SELECTOR_NODE_NAMES.split(", ").collect(),
            sites: SELECTOR_SITE_NAMES.split(", ").collect(),
        },
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
    /// The cache entity's `to_bits()` — the same key the event feed's own
    /// bounty memo is indexed by, so the claiming team's feed can suppress the
    /// unattributed `bounty gone` line it would otherwise ALSO be shown for a
    /// cache it just took itself.
    pub id: u64,
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize)]
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

/// The team-wide half of the stat law's input: what this entity's OWNER has
/// researched. Zeroes for anything research does not touch (buildings, towers),
/// which is how "research equips the army, not the masonry" is spelled at the
/// call site rather than buried in a branch inside the law.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResearchBonus {
    /// Flat damage added to each attack this entity makes.
    pub bonus_damage: f32,
    /// Flat damage subtracted from each hit this entity takes.
    pub flat_armor: f32,
}

impl ResearchBonus {
    /// Nothing researched, or research does not apply here.
    pub const NONE: ResearchBonus = ResearchBonus {
        bonus_damage: 0.0,
        flat_armor: 0.0,
    };
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
    /// Flat damage ADDED to each attack, from attack research. Added AFTER
    /// every multiplier — `damage_mult`, a hero's level bonus, a Catapult's
    /// 6x vs buildings — precisely so that none of them can amplify it. +3
    /// attack is +3 damage on every swing in the game, whoever swings it and
    /// whatever they swing at. Multiplying it instead would make the upgrade
    /// worth +18 to a Catapult hitting a wall and +3 to a Footman hitting a
    /// man, which is a siege upgrade wearing an army upgrade's label.
    pub bonus_damage: f32,
    /// Flat damage SUBTRACTED from each incoming hit, from armor research.
    /// Applied after `damage_taken_mult`, at the single point health is
    /// subtracted, and floored at `MIN_DAMAGE_PER_HIT` so no stack of armour
    /// ever makes a unit immune.
    pub flat_armor: f32,
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
/// Statuses only — for everything research does not touch (buildings taking a
/// hit, tower weapons). Exactly `effective_stats_with(base, status,
/// ResearchBonus::NONE)`, named so that "no research applies here" is a
/// deliberate statement at the call site rather than a forgotten argument.
pub fn effective_stats(base: BaseStats, status: Option<&StatusEffects>) -> EffectiveStats {
    effective_stats_with(base, status, ResearchBonus::NONE)
}

/// **THE modifier function**, in full: the one place a base stat, a status
/// effect and a team's research meet. Everything downstream — move speed,
/// attack cooldown, damage dealt, damage taken — is read off the struct this
/// returns, never off `unit_stats(kind)` and never off `TeamResearch` directly.
///
/// The two inputs are deliberately different shapes because they are different
/// kinds of fact: a `StatusEffects` is a component ON the entity, `ResearchBonus`
/// is a property of the team that OWNS it. Multiplicative modifiers compose in
/// the multipliers; research composes as the two flat terms, which is what
/// keeps a percentage buff from ever scaling a flat upgrade.
pub fn effective_stats_with(
    base: BaseStats,
    status: Option<&StatusEffects>,
    research: ResearchBonus,
) -> EffectiveStats {
    let mut out = EffectiveStats {
        speed: base.speed,
        attack_cooldown: base.attack_cooldown,
        damage_mult: 1.0,
        damage_taken_mult: 1.0,
        heal_per_second: 0.0,
        bonus_damage: research.bonus_damage,
        flat_armor: research.flat_armor,
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

/// The same, for a unit whose owning team has researched something. This is
/// what combat.rs uses for every unit swing; the research-free wrapper above
/// remains for callers that only want speed (units.rs) or that are asking on
/// behalf of a structure.
pub fn effective_unit_stats_with(
    kind: UnitKind,
    status: Option<&StatusEffects>,
    research: ResearchBonus,
) -> EffectiveStats {
    effective_stats_with(BaseStats::of_unit(kind), status, research)
}

/// Damage actually subtracted from a victim's health, given the raw amount,
/// the victim's incoming-damage multiplier and its team's armor research.
///
/// One function because the floor is easy to forget and expensive to get wrong:
/// without it three levels of armor would take a Worker's 5-damage swing to 2
/// and a Spearman's 6 to 3, and a fourth level (or a stacking armor buff on top)
/// would reach zero and make the victim literally unkillable by that attacker.
/// The floor is `MIN_DAMAGE_PER_HIT` *or the unarmoured amount, whichever is
/// smaller* — armor must never round a hit UP. A 0.5-damage tick stays 0.5
/// against three levels of plate; only a hit that started above the floor can
/// be pushed down to it. That subtlety is why this is a function and not an
/// expression at the call site.
pub fn damage_after_armor(raw: f32, damage_taken_mult: f32, flat_armor: f32) -> f32 {
    let full = raw * damage_taken_mult;
    let floor = MIN_DAMAGE_PER_HIT.min(full.max(0.0));
    (full - flat_armor).max(floor)
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
///
/// Functions rather than constants because the numbers live in
/// `assets/data/abilities.ron` now, and a mirror copy in code is exactly the
/// two-sources-of-truth the data files exist to remove. `hero_slam()` is the
/// row; the accessors are what ai.rs and the tests ask for.
pub fn hero_slam() -> AbilityDef {
    *abilities_of_unit(UnitKind::Hero)
        .first()
        .expect("abilities.ron: the Champion must have at least one ability")
}
#[allow(dead_code)]
pub fn hero_ability_cost() -> f32 {
    hero_slam().mana_cost
}
#[allow(dead_code)]
pub fn hero_ability_cooldown() -> f32 {
    hero_slam().cooldown
}
pub fn hero_ability_radius() -> f32 {
    hero_slam().radius
}
#[allow(dead_code)]
pub fn hero_ability_damage() -> f32 {
    hero_slam().power()
}
/// Reviving a fallen hero (level preserved) is FASTER than the first training
/// and — since v3 — the only time a hero costs anything at all. The price
/// itself lives per class in `revive_gold`/`revive_lumber`; this is just the
/// clock. See `hero_train_cost`.
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum AbilityTargets {
    Enemies,
    /// Own units (buildings are never "allies" for buff purposes).
    Allies,
    OwnWorkers,
}

impl AbilityTargets {
    /// Wire name for the catalog's per-atom export.
    pub fn name(self) -> &'static str {
        match self {
            AbilityTargets::Enemies => "enemies",
            AbilityTargets::Allies => "allies",
            AbilityTargets::OwnWorkers => "own_workers",
        }
    }
}

/// **Where an ability's effect lands** — the geometry half of an `AbilityDef`,
/// answering the question `AbilityTargets` does not: not *who* the radius
/// catches, but *where the radius is centred*.
///
/// v2 had exactly one answer, `Caster`, and it was baked into combat.rs rather
/// than written down. That made every AoE a bubble the caster stands in the
/// middle of, which is a fine shape for a Champion who wants to be in the
/// middle and a fatal one for a Sorcerer who does not: the fog/arena finding
/// was that Sorcerers die on the front line because the only way to land Slow
/// on the enemy was to walk into them. Geometry as data fixes that without
/// touching the effect, the status framework or the cooldown store.
///
/// `range` is measured from the caster to the CENTRE of the effect; the
/// effect's own `radius` then spreads from there, so a `Point { range: 9 }`
/// ability with `radius: 4.5` reaches a body 13.5 away — the caster's reach is
/// range, the spell's reach is range + radius.
// `Unit` has no shipping ability behind it yet — Slow is the first targeted
// row and a point is the right shape for an AoE debuff. The variant, its wire
// name, its UI click and its tests exist so that the first single-target
// spell (a nuke, a polymorph, a targeted heal) is a table row, exactly as
// `AbilityTargets::Allies` waited for the ultimates.
///
/// `Default` is `Caster`, and that is load-bearing rather than decorative:
/// `assets/data/abilities.ron` defaults this field, so a row that has no
/// opinion about geometry says nothing at all and reads exactly as it did
/// before this field existed. Only a row that is genuinely thrown spells it
/// out.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Debug, Default, Deserialize)]
pub enum AbilityTarget {
    /// Centred on the caster. The v2 behaviour and the default for every row
    /// that does not say otherwise: no range, no target, nothing to click.
    #[default]
    Caster,
    /// Cast at a ground point within `range` of the caster.
    Point { range: f32 },
    /// Cast on a specific unit within `range`; the effect centres on wherever
    /// that unit is standing when the cast resolves.
    Unit { range: f32 },
}

impl AbilityTarget {
    /// Wire name for the catalog and the snapshot.
    pub fn name(self) -> &'static str {
        match self {
            AbilityTarget::Caster => "caster",
            AbilityTarget::Point { .. } => "point",
            AbilityTarget::Unit { .. } => "unit",
        }
    }
    /// How far from the caster the centre may be. `None` for `Caster`, whose
    /// centre is the caster and therefore always at range zero.
    pub fn range(self) -> Option<f32> {
        match self {
            AbilityTarget::Caster => None,
            AbilityTarget::Point { range } | AbilityTarget::Unit { range } => Some(range),
        }
    }
    /// Does this ability need a target chosen — by the player's click, the
    /// commander's payload, or the auto-pick?
    pub fn is_targeted(self) -> bool {
        !matches!(self, AbilityTarget::Caster)
    }
    /// Does the target have to be a UNIT (rather than bare ground)?
    pub fn wants_unit(self) -> bool {
        matches!(self, AbilityTarget::Unit { .. })
    }
}

/// The target actually chosen for one cast — the payload half of
/// [`AbilityTarget`]. `None` on a `CastAbility` means "you pick", which for a
/// `Caster` ability is the only possible answer and for a targeted one invokes
/// the auto-pick ([`best_cast_focus`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CastTarget {
    /// A ground point. Y is ignored — everything here is measured in XZ.
    Point(Vec3),
    /// A unit; the effect centres on where it is standing when the cast fires.
    Unit(Entity),
}

/// **The auto-pick.** Given the positions of everything this cast would
/// actually affect, choose the centre that catches the most of them.
///
/// This is the one rule behind three doors, so that a Sorcerer left on
/// auto-cast, a commander who sends `{"type":"cast","ability":"Slow"}` with no
/// coordinates, and a player who has not clicked yet all get the same answer:
///
///   * each body proposes a centre: itself if the caster can reach it, and
///     otherwise **the furthest point towards it the caster can reach**. That
///     clamp is what makes the auto-pick as long-armed as a human's click —
///     a clump 11 away is still catchable by a spell with 9 of range and 4.5
///     of radius, and an aimer that only ever centred ON a body would have
///     refused a shot the player can obviously take;
///   * the centre that catches the most bodies within `radius` wins, so a
///     debuff lands on the clump and a heal lands on the knot of wounded;
///   * ties go to the centre NEAREST the caster, which keeps the choice stable
///     frame to frame and biases a caster towards the fight in front of it
///     rather than the identical one further away.
///
/// `candidates` are already filtered by the caller to the entities the effect
/// would affect (right team, right kind, air-legal, and — for a heal — hurt),
/// because that predicate lives with the effect and differs per caller. A
/// caller that needs the aim to be an actual BODY rather than a point (an
/// `AbilityTarget::Unit` ability) must additionally pre-filter to candidates
/// within `range`, which makes the clamp a no-op and the returned index a
/// unit it may legally name.
///
/// Returns the index of the body the aim was derived from, the centre itself,
/// and how many bodies that centre catches — or `None` when the spell would
/// catch nobody, in which case it is not worth making and no cooldown is
/// spent.
pub fn best_cast_focus(
    caster: Vec3,
    range: f32,
    radius: f32,
    candidates: &[Vec3],
) -> Option<(usize, Vec3, u32)> {
    /// Distance on the ground plane — height never counts towards a spell's
    /// reach, exactly as it never counts in combat.rs's own `xz_dist`.
    fn xz_dist(a: Vec3, b: Vec3) -> f32 {
        Vec2::new(a.x - b.x, a.z - b.z).length()
    }
    let mut best: Option<(usize, Vec3, u32, f32)> = None;
    for (i, &body) in candidates.iter().enumerate() {
        let reach = xz_dist(caster, body);
        // As far towards it as the arm goes. The caster's own height is kept
        // so the centre sits on the caster's plane, exactly as an explicit
        // point does.
        let focus = if reach <= range {
            body
        } else {
            let dir = Vec2::new(body.x - caster.x, body.z - caster.z).normalize_or_zero();
            Vec3::new(caster.x + dir.x * range, caster.y, caster.z + dir.y * range)
        };
        let caught = candidates
            .iter()
            .filter(|&&other| xz_dist(focus, other) <= radius)
            .count() as u32;
        // A centre that catches nobody is not an aim. This is also what keeps
        // a body beyond `range + radius` from proposing anything: the clamped
        // point towards it is too far short to touch even that body.
        if caught == 0 {
            continue;
        }
        let distance = xz_dist(caster, focus);
        let better = match best {
            None => true,
            // More bodies wins; equal bodies, the nearer centre wins.
            Some((_, _, best_caught, best_dist)) => {
                caught > best_caught || (caught == best_caught && distance < best_dist)
            }
        };
        if better {
            best = Some((i, focus, caught, distance));
        }
    }
    best.map(|(i, focus, caught, _)| (i, focus, caught))
}

// ---------------------------------------------------------------------------
// v3: COMPOSABLE EFFECTS — an ability is a sentence, not a variant
// ---------------------------------------------------------------------------
//
// v2's `AbilityEffect` was a closed enum: `Damage | Heal | Militia |
// ApplyStatus{also}`. Every genuinely new mechanic needed a new variant, a new
// arm in combat.rs, a new arm in doctrine.rs and a new arm in the catalog — and
// the seams of that were already visible in `ApplyStatus::also`, which existed
// for exactly one ability (Sanctuary) because "two statuses on one button" had
// nowhere else to live.
//
// v3 splits the question in three, each answered independently by data:
//
//   * WHAT      — an `EffectAtom`: the smallest thing a cast can do to the
//                 world. An ability carries a LIST of them, applied in order
//                 from one resolved centre. `also` retires into a second atom.
//   * WHEN      — an `EffectSchedule` per atom: now, or spread over time.
//   * WHO/WHERE — `AbilityTarget` (the centre, v3-b1x) and each atom's own
//                 `AbilityTargets` (whose bodies inside the radius it looks
//                 for). One cast can damage enemies AND heal allies.
//
// The promise this makes to content: a mechanic that is a combination of
// things the engine already does is a ROW, not a patch. The demo ability at
// the bottom of abilities.ron is the proof — a damage-and-slow nuke that no
// line of Rust knows the name of.

/// **Where a `Teleport` atom sends its passengers.** One variant today, named
/// rather than numeric so the next destination (a beacon, the caster's own
/// starting position) is a row rather than a schema change.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum TeleportDestination {
    /// The nearest rung of the caster's own hall ladder — the same search the
    /// Town Portal and the Scroll of Mass Teleport already make.
    NearestHall,
}

impl TeleportDestination {
    pub fn name(self) -> &'static str {
        match self {
            TeleportDestination::NearestHall => "nearest_hall",
        }
    }
}

fn targets_enemies() -> AbilityTargets {
    AbilityTargets::Enemies
}
fn targets_allies() -> AbilityTargets {
    AbilityTargets::Allies
}
fn targets_own_workers() -> AbilityTargets {
    AbilityTargets::OwnWorkers
}

/// **One indivisible thing a cast does.** The vocabulary content writes in.
///
/// Every atom carries its OWN numbers — there is no shared `power` field an
/// atom has to reinterpret, which is what made the v2 row read "power: 40" and
/// mean "40 seconds" for one ability and "40 damage" for the next.
///
/// Two shapes live here, and the difference is structural rather than
/// stylistic: most atoms are PER-BODY (they ask `effect_hits` about everyone
/// inside the radius), while `Summon` and `Teleport` happen ONCE at the centre.
/// `per_target()` is the seam.
//
// `Eq` is out: magnitudes are f32. Nothing compares atoms for hashing or set
// membership.
#[derive(Clone, Copy, PartialEq, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum EffectAtom {
    /// Hurt everyone the `targets` predicate catches. The one atom that counts
    /// BUILDINGS as victims — a shockwave cracks a wall, a heal does not mend
    /// one and a hex does not confuse one.
    ///
    /// Scales with the caster's hero level (`Hero::damage_mult`), as v2's
    /// `Damage` always did.
    Damage {
        amount: f32,
        #[serde(default = "targets_enemies")]
        targets: AbilityTargets,
    },
    /// Restore HP. Scales with hero level, exactly as v2's `Heal` did.
    Heal {
        amount: f32,
        #[serde(default = "targets_allies")]
        targets: AbilityTargets,
    },
    /// Lay a timed status through the status framework's one public door,
    /// `StatusEffects::apply` — so stacking policy, caps and central expiry are
    /// the same as every other producer's.
    ///
    /// Magnitude is NOT hero-scaled: a 40% slow is 40% from a level 1 caster
    /// and from a level 10 one. That was v2's behaviour and it is deliberate —
    /// crowd control that sharpens with level is how a hero stops being a unit
    /// and starts being a win condition.
    ApplyStatus {
        status: StatusKind,
        magnitude: f32,
        duration: f32,
        targets: AbilityTargets,
    },
    /// Own workers pick up arms for `duration` seconds.
    ///
    /// **Why this is its own atom and not `Summon`** (the v3 bead asked): a
    /// summon CREATES a body, militia TRANSFORMS one. The worker keeps its
    /// entity, its gold, its harvest order and its identity, and goes back to
    /// mining when the timer runs out. Expressing it as `Summon` would mean
    /// killing five workers and spawning five fighters — different food, a
    /// different economy, five deaths on the ledger and five workers who never
    /// come back. The mechanic is a modifier on an existing body, so it stays
    /// an atom of its own.
    Militia {
        duration: f32,
        #[serde(default = "targets_own_workers")]
        targets: AbilityTargets,
    },
    /// Put `count` new units of `unit_kind` on the caster's team at the cast
    /// centre. `lifetime` is seconds before they vanish — `None` is permanent.
    ///
    /// A summon is not TRAINED: it costs no gold, occupies no production queue
    /// and answers to the same orders any other unit does while it lives.
    Summon {
        unit_kind: UnitKind,
        count: u32,
        #[serde(default)]
        lifetime: Option<f32>,
    },
    /// Recall the caster and its neighbours, through the same
    /// `TeleportRequest` the Town Portal has always used.
    ///
    /// The request gathers around the CASTER (that is what a recall is), so the
    /// validator refuses this atom on a thrown row — see `check_values`.
    Teleport {
        destination: TeleportDestination,
        /// Leave workers where they stand, as the Scroll of Mass Teleport does.
        #[serde(default)]
        army_only: bool,
    },
}

impl EffectAtom {
    /// Wire name for the catalog. The v2 names are kept verbatim (`"status"`
    /// for `ApplyStatus`) so a commander's parser does not have to learn two
    /// spellings of the same word.
    pub fn name(self) -> &'static str {
        match self {
            EffectAtom::Damage { .. } => "damage",
            EffectAtom::Heal { .. } => "heal",
            EffectAtom::ApplyStatus { .. } => "status",
            EffectAtom::Militia { .. } => "militia",
            EffectAtom::Summon { .. } => "summon",
            EffectAtom::Teleport { .. } => "teleport",
        }
    }
    /// Whose bodies inside the radius this atom looks for — `None` for the
    /// atoms that happen once at the centre rather than to a crowd.
    pub fn targets(self) -> Option<AbilityTargets> {
        match self {
            EffectAtom::Damage { targets, .. }
            | EffectAtom::Heal { targets, .. }
            | EffectAtom::ApplyStatus { targets, .. }
            | EffectAtom::Militia { targets, .. } => Some(targets),
            EffectAtom::Summon { .. } | EffectAtom::Teleport { .. } => None,
        }
    }
    /// Does this atom visit every body in the radius (rather than firing once
    /// at the centre)?
    pub fn per_target(self) -> bool {
        self.targets().is_some()
    }
    /// The status this atom lays down, if it lays one: `(kind, magnitude,
    /// duration)`.
    pub fn status(self) -> Option<(StatusKind, f32, f32)> {
        match self {
            EffectAtom::ApplyStatus { status, magnitude, duration, .. } => {
                Some((status, magnitude, duration))
            }
            _ => None,
        }
    }
    /// Does this atom restore HP (instantly or over time)? Auto-cast asks, so
    /// that a healing ability waits for someone who is actually hurt instead of
    /// firing at a column of full-health allies.
    pub fn heals(self) -> bool {
        matches!(self, EffectAtom::Heal { .. })
            || self.status().map(|(k, _, _)| k) == Some(StatusKind::HealOverTime)
    }
    /// Does a hero's level multiply this atom's headline number? Damage and
    /// healing grow with the hero; durations, magnitudes and body counts do
    /// not.
    pub fn scales_with_level(self) -> bool {
        matches!(self, EffectAtom::Damage { .. } | EffectAtom::Heal { .. })
    }
    /// The one number a UI or an AI would print for this atom — damage, HP,
    /// status magnitude, militia seconds, summon count. `duration()` is the
    /// other half; between them they reproduce v2's `power`/`duration` pair.
    pub fn power(self) -> f32 {
        match self {
            EffectAtom::Damage { amount, .. } | EffectAtom::Heal { amount, .. } => amount,
            EffectAtom::ApplyStatus { magnitude, .. } => magnitude,
            EffectAtom::Militia { duration, .. } => duration,
            EffectAtom::Summon { count, .. } => count as f32,
            EffectAtom::Teleport { .. } => 0.0,
        }
    }
}

/// **When an atom happens.** The timing half of the grammar.
///
/// `Instant` and `OverTime` are implemented end to end. `OnHit` and `OnDeath`
/// are the schema, and the validator refuses them with "not yet supported":
/// both need a hook the damage pipeline does not have (a per-unit charge store
/// consulted by every attacker, and a death callback that survives the
/// despawn), and inventing one on the way past would have been a second combat
/// system smuggled in under a data change. They are written down here so the
/// bead that builds those hooks is a wiring job with a name already agreed.
#[derive(Clone, Copy, PartialEq, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum EffectSchedule {
    /// Everything at the moment of the cast. What every v2 ability meant.
    #[default]
    Instant,
    /// `ticks` applications, `interval` seconds apart. **The first tick lands
    /// at the cast**, so `ticks: 3, interval: 1.0` is "now, +1s, +2s" — a
    /// 1-tick `OverTime` is exactly an `Instant`.
    ///
    /// The field is re-evaluated at the recorded CENTRE on every tick rather
    /// than snapshotted at the cast: a lingering blizzard is a place, so
    /// walking out of it works and walking into it is a mistake.
    OverTime { interval: f32, ticks: u32 },
    /// The caster's next `attacks` attacks carry this atom. NOT IMPLEMENTED.
    OnHit { attacks: u32 },
    /// Fires when the affected body dies. NOT IMPLEMENTED.
    OnDeath,
}

impl EffectSchedule {
    pub fn name(self) -> &'static str {
        match self {
            EffectSchedule::Instant => "instant",
            EffectSchedule::OverTime { .. } => "over_time",
            EffectSchedule::OnHit { .. } => "on_hit",
            EffectSchedule::OnDeath => "on_death",
        }
    }
    /// Is this schedule wired to anything yet? The validator's gate, kept here
    /// so the answer lives beside the variants rather than in data.rs.
    pub fn supported(self) -> bool {
        matches!(self, EffectSchedule::Instant | EffectSchedule::OverTime { .. })
    }
}

/// One clause of an ability: an atom and its schedule.
///
/// `schedule` is defaulted, so a row that means "now" — which is nearly every
/// row — says nothing at all and reads as `(atom: Damage(amount: 45.0))`.
#[derive(Clone, Copy, PartialEq, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    pub atom: EffectAtom,
    #[serde(default)]
    pub schedule: EffectSchedule,
}

/// When an ability becomes castable.
// `HeroLevel` has no shipping ability behind it yet — the hero ultimates bead
// is what fills it in; the predicate and its tests exist so that bead is data.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
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
    /// **What this ability does, in order.** One atom is the common case; two
    /// is Sanctuary (heal-over-time plus armour), and so is the demo nuke in
    /// abilities.ron (damage plus slow). The list is never empty — the loader refuses that.
    ///
    /// Order matters in exactly one place beyond execution order: the FIRST
    /// atom is the ability's AIM. `cast_center`'s auto-pick and doctrine's
    /// trigger count both ask atom 0 who it is looking for, because a spell
    /// has one centre and something has to decide it. Put the atom the ability
    /// is *about* first — for a nuke that also slows, that is the damage.
    pub effects: &'static [Effect],
    /// **Where the radius is centred.** `AbilityTarget::Caster` is the default
    /// in every sense that matters — it is what every row said implicitly
    /// before this field existed, and what a row means when its author has no
    /// opinion. A `Point`/`Unit` row additionally carries how far the caster
    /// may throw it.
    pub target: AbilityTarget,
    pub mana_cost: f32,
    pub cooldown: f32,
    /// How far the effect spreads from its centre — which is the caster for a
    /// `Caster` row and the chosen point/unit for a targeted one.
    pub radius: f32,
    /// Does the effect reach AIRBORNE units in its radius? A shockwave that
    /// travels along the ground does not; healing light does. combat.rs
    /// filters by this, and doctrine.rs will not auto-cast at targets the
    /// ability cannot affect.
    pub hits_air: bool,
    pub unlock: AbilityUnlock,
    pub description: &'static str,
}

impl AbilityDef {
    /// **The aim atom.** Atom 0 decides where a targeted cast points and whom
    /// doctrine counts before pulling the trigger, because a cast has one
    /// centre and one of the atoms has to own it. Documented on `effects`.
    ///
    /// The loader refuses an empty effect list precisely so this cannot fail;
    /// the fallback is a harmless zero-damage atom rather than a panic in a
    /// system that runs every frame.
    pub fn aim(&self) -> EffectAtom {
        self.effects.first().map(|e| e.atom).unwrap_or(EffectAtom::Damage {
            amount: 0.0,
            targets: AbilityTargets::Enemies,
        })
    }
    /// Wire name of the ability's headline effect — the v2 `effect` field,
    /// still emitted by the catalog so a commander written against v2 keeps
    /// reading. `effects[]` is the full sentence.
    pub fn effect_name(&self) -> &'static str {
        self.aim().name()
    }
    /// The v2 `power` field: the headline number of the FIRST atom. Byte-for-
    /// byte what the old field held for every shipped row — 45 damage for
    /// Slam, 60 HP for Heal, 40 seconds for CallToArms, 0.4 for Slow.
    pub fn power(&self) -> f32 {
        self.aim().power()
    }
    /// The v2 `duration` field: seconds the applied STATUS lasts, 0 for
    /// everything else. Militia's seconds live in `power()`, exactly as they
    /// did in v2 — the pair is reproduced, oddity included, because the
    /// catalog is a wire format and a wire format's job is not to improve.
    pub fn duration(&self) -> f32 {
        self.effects
            .iter()
            .find_map(|e| e.atom.status().map(|(_, _, d)| d))
            .unwrap_or(0.0)
    }
    /// First status this ability lays down (the v2 `status` catalog field).
    pub fn status(&self) -> Option<StatusKind> {
        self.effects.iter().find_map(|e| e.atom.status().map(|(k, _, _)| k))
    }
    /// SECOND status and its magnitude (the v2 `status2` catalog field, which
    /// was `ApplyStatus::also`). It is now simply the second status atom —
    /// which is what `also` always was, spelled honestly.
    pub fn extra_status(&self) -> Option<(StatusKind, f32)> {
        self.effects
            .iter()
            .filter_map(|e| e.atom.status())
            .nth(1)
            .map(|(k, m, _)| (k, m))
    }
    /// Does ANY atom restore HP? Auto-cast asks before firing a heal at a
    /// healthy army.
    pub fn heals(&self) -> bool {
        self.effects.iter().any(|e| e.atom.heals())
    }
    /// Does any atom lay `kind` on `who`? Doctrine's Warcry rule asks this
    /// rather than pattern-matching a whole effect, so a row that buffs damage
    /// as its SECOND clause is still recognised as an offensive buff.
    pub fn applies(&self, kind: StatusKind, who: AbilityTargets) -> bool {
        self.effects.iter().any(|e| {
            e.atom.status().map(|(k, _, _)| k) == Some(kind) && e.atom.targets() == Some(who)
        })
    }
}

// The ability table itself lives in `assets/data/abilities.ron`, along with
// the tuning notes for every number in it. What used to be a block of `const
// AbilityDef` literals plus five hand-maintained `[AbilityDef; N]` arrays is
// now one file the loader assembles into per-caster slot lists.

/// `BH_STATUS_PROBE=1`: dev instrumentation for the status + ability-v2
/// frameworks. Read once per process so the ability tables stay constant for
/// the whole run.
pub fn status_probe_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("BH_STATUS_PROBE").is_ok_and(|v| v != "0"))
}

/// Every ability this unit kind can ever cast, unlocked or not, in slot order.
pub fn abilities_of_unit(kind: UnitKind) -> &'static [AbilityDef] {
    crate::data::unit_abilities(kind)
}

/// Auto-cast doctrine a freshly trained unit is BORN with, when its kind has
/// one. units.rs applies it at spawn.
///
/// This exists because a caster whose entire value is one debuff is worthless
/// as a statue, and "the player must remember to turn the Sorcerer on" is
/// exactly the kind of mechanical bookkeeping THESIS.md says the engine should
/// be doing for whichever side set it. Heroes deliberately have NO default:
/// their mana is a resource a player budgets, and a hero that spends it the
/// instant one enemy wanders past is a hero with none left for the fight.
/// The Sorcerer pays only a cooldown, so there is nothing to hoard.
///
/// It is a `(slot, min_targets)` pair — the same shape `AutoCastPolicy::set`
/// takes — rather than a field on `AbilityDef`, so a kind can default one of
/// its abilities on and leave the rest silent, and so adding a row here never
/// disturbs the ability tables above.
pub fn default_autocast(kind: UnitKind) -> Option<(usize, u32)> {
    crate::data::unit_autocast(kind)
}

/// Every ability this building kind can ever cast, in slot order.
pub fn abilities_of_building(kind: BuildingKind) -> &'static [AbilityDef] {
    crate::data::building_abilities(kind)
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

/// Loose form of a name on the wire: case, spaces, dashes and underscores are
/// all noise, so `"town_hall"`, `"Town Hall"` and `"townhall"` are one name.
///
/// **This is the one name matcher.** Every player-facing name — unit kinds,
/// building kinds, research ladders, items, abilities, target classes — is
/// compared through here, so a commander who spells a name the way the catalog
/// prints it is never told it does not exist. It lives in shared.rs rather
/// than intent.rs because the catalog it folds names *of* lives here, and
/// because `ability_index_by_id` below is the shared.rs consumer that used to
/// have its own, stricter rule.
pub fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Slot of the ability with this id, unlocked or not.
///
/// Matched through `normalize_name`, exactly like every other name on the
/// wire. This used to be `eq_ignore_ascii_case`, which made `"CallToArms"` and
/// `"calltoarms"` work while `"Call to Arms"` — the spelling a person actually
/// types — did not. The old forms are a strict subset of what normalising
/// accepts, so nothing that parsed before stopped parsing.
pub fn ability_index_by_id(list: &[AbilityDef], id: &str) -> Option<usize> {
    let wanted = normalize_name(id);
    list.iter()
        .position(|def| normalize_name(def.name) == wanted)
}

/// Enemies inside Warcry's radius (and allies to buff) before the scripted
/// commander thinks the shout is worth its 45s.
pub const WARCRY_MIN_TARGETS: u32 = 4;
/// Hurt allies inside Sanctuary's radius before it is worth its 60s.
pub const SANCTUARY_MIN_TARGETS: u32 = 3;

/// Standing auto-cast doctrine a MACHINE-DRIVEN team gets for free, per hero
/// class: the ultimate slots and their trigger counts.
///
/// Ultimates are the one part of the kit a scripted commander cannot be
/// trusted to spend by hand — they are long-cooldown, situational, and worth
/// nothing cast early. So they are doctrine, not script: ai.rs installs these
/// rules on the heroes of any team it is actually driving, and doctrine.rs's
/// auto-caster fires them under the same unlock/cooldown/mana gate a player's
/// button obeys. A human or bridge-driven team gets nothing here and keeps
/// full manual control.
///
/// Slots are resolved BY NAME, so a row inserted ahead of an ultimate moves
/// the rule with it. Slot 0 (Slam / Heal) is deliberately absent: that is the
/// player's `T` toggle and the scripted AI's own explicit cast.
pub fn machine_autocast_rules(kind: UnitKind) -> Vec<(usize, u32)> {
    let list = abilities_of_unit(kind);
    // By NAME across both rosters: a hero class's ultimate is its slot-1 row
    // whichever race trained it, and naming them keeps the rule where the
    // content is. `filter_map` means a name that is not on this kind's list
    // simply does not apply, so the table is a union rather than a match.
    //
    // The FarSeer's `AncestralCall` is deliberately ABSENT, and the reason is
    // a real limit of the effect vocabulary rather than an oversight: a
    // `Summon` atom has no crowd to count, so doctrine's auto-cast trigger
    // (which counts whoever atom 0 is aimed at) can never fire it. A summon is
    // a hand-cast ultimate for both seats until an atom exists that a
    // `min_targets` rule can mean something about.
    [
        ("Warcry", WARCRY_MIN_TARGETS),
        ("Sanctuary", SANCTUARY_MIN_TARGETS),
        ("Bloodfury", WARCRY_MIN_TARGETS),
    ]
    .iter()
    .filter_map(|(id, min)| ability_index_by_id(list, id).map(|index| (index, *min)))
    .collect()
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
///
/// This is also the wire form: `Intent::Cast`/`Intent::Autocast` carry it
/// directly, and it serializes *untagged* — a bare `2` or a bare `"Slam"`,
/// which is what a commander already reads out of the snapshot. One type for
/// the event, the intent and the protocol, so a slot cannot be named three
/// slightly different ways.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(untagged)]
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

/// **A body that was called, not trained** — the mark an `EffectAtom::Summon`
/// leaves on the units it creates.
///
/// `until` is when it goes home; `None` is a permanent summon (a row may want
/// one, and "permanent" should not have to be spelled as a very large number).
/// `tick_militia_and_cooldowns` owns the expiry, beside the militia timer it
/// most resembles: both are "this body is temporarily something else".
#[derive(Component, Clone, Copy, Debug)]
pub struct Summoned {
    pub until: Option<f32>,
}

// ---------------------------------------------------------------------------
// Hero items: bought at a Shop, carried in a small hero inventory,
// consumed on use. economy.rs handles buying (money!), combat.rs executes
// potion effects, units.rs executes teleports (it owns Transforms).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum ItemId {
    HealingPotion,
    TownPortal,
    BootsOfSpeed,
    BannerOfCommand,
    ScrollOfMassTeleport,
}

/// The shop shelf, in shelf order: tier 1 first, then the gated rungs. The
/// command card, the catalog and the bridge all walk this array, so the order
/// here IS the order a player and a commander see.
pub const ALL_ITEMS: [ItemId; 5] = [
    ItemId::HealingPotion,
    ItemId::TownPortal,
    ItemId::BootsOfSpeed,
    ItemId::BannerOfCommand,
    ItemId::ScrollOfMassTeleport,
];

#[derive(Clone, Copy, Debug)]
pub struct ItemDef {
    pub name: &'static str,
    pub cost_gold: u32,
    /// Team tech tier required to BUY this. The shelf is tiered for the same
    /// reason the ability list is: a Shop built in the first two minutes must
    /// not sell the late-game map-control scroll. `item_unlocked` is the one
    /// place the comparison happens.
    pub tier: TechTier,
    pub description: &'static str,
}

pub fn item_def(id: ItemId) -> ItemDef {
    *crate::data::item_row(id)
}

/// May a team at `tier` buy `id`? Asked by economy.rs (which pays), by the
/// command card (which greys the button), by the bridge validator (which
/// explains the refusal) and by the snapshot (which reports `locked`), so the
/// four can never disagree about what is on the shelf.
pub fn item_unlocked(id: ItemId, tier: TechTier) -> bool {
    tier >= item_def(id).tier
}

pub const POTION_HEAL: f32 = 150.0;
pub const PORTAL_RADIUS: f32 = 8.0;

/// Boots of Speed: a Haste status on the hero alone, through the ordinary
/// status framework — so it stacks with, and expires like, every other buff.
pub const BOOTS_HASTE: f32 = 0.40;
pub const BOOTS_DURATION: f32 = 15.0;

/// Banner of Command: an ArmorBuff on own units around the hero. Shorter than
/// it is wide — it is a "hold this fight" button, not a march buff.
pub const BANNER_ARMOR: f32 = 0.30;
pub const BANNER_DURATION: f32 = 10.0;
pub const BANNER_RADIUS: f32 = 8.0;

/// Scroll of Mass Teleport's radius. Deliberately larger than the map's
/// diagonal (`MAP_HALF * 2 * sqrt(2)`), so the single radius test in units.rs
/// includes every own unit wherever it stands: "map-wide" is expressed as a
/// number in the existing mechanism, not as a second code path.
pub const MASS_TELEPORT_RADIUS: f32 = MAP_HALF * 4.0;

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
    /// For the two teleport items: WHICH own standing hall to arrive at.
    /// `None` means the hall nearest the hero — the original behaviour, and
    /// what every non-teleport item ignores entirely. Already validated by
    /// intent.rs (own, finished, a hall) at the moment the order was given;
    /// combat.rs re-checks on the frame it fires, because a hall can fall
    /// between the two.
    pub destination: Option<Entity>,
}

/// Does this item let its user choose where the teleport lands? The one place
/// the question is answered: the UI asks it to decide whether a keypress arms
/// a hall-pick or fires immediately, and the catalog asks it to tell a
/// commander the `destination` field is worth sending.
pub fn item_chooses_destination(id: ItemId) -> bool {
    matches!(id, ItemId::TownPortal | ItemId::ScrollOfMassTeleport)
}

/// Move `center` and own units within `radius` of it to `dest` instantly.
/// Handled by units.rs (the only Transform mover); it also clears MoveTo/paths.
#[derive(Event, Debug)]
pub struct TeleportRequest {
    pub center: Entity,
    pub radius: f32,
    pub dest: Vec3,
    /// Leave workers where they stand. A Town Portal at radius 8 sweeps up
    /// whatever is beside the hero and that is fine; a MAP-WIDE recall that
    /// also emptied every gold mine would be an economy wipe disguised as a
    /// map-control item. `center` itself always rides, worker or not.
    pub army_only: bool,
}

/// Resources bought per 5 XP: one XP "nickel" per 20 gold-equivalent spent.
///
/// Picked to reproduce the three hand-written rows this rule replaced —
/// Worker 75→15, Footman 135→30, Archer 120→30 — so the anchor points a match
/// was balanced around are the formula's own output rather than exceptions to
/// it. Everything else follows.
const XP_PER_STEP: f32 = 5.0;
const XP_COST_PER_STEP: u32 = 20;

/// XP granted to nearby enemy heroes when this thing dies.
///
/// **One rule: XP is a quarter of what the thing cost, in 5-XP steps.**
/// `5 * floor(cost / 20)`, where `cost` is gold + lumber weighted equally —
/// the same "material worth" `asset_score` uses, so the two places the game
/// puts a number on a corpse agree about what a corpse is worth.
///
/// This exists as a formula rather than a table because a table was the bug.
/// Six of the eleven unit kinds (Raider, Catapult, Spearman, Sorcerer, Knight,
/// Gryphon Rider) fell through the old `match` and granted **zero** XP: every
/// kind added after the hero system shipped was invisible to it, so the
/// tier-2/3 army a hero actually fights through was worth nothing to level on,
/// and killing a Worker paid better than killing a Knight. A formula cannot
/// have that gap — a new `UnitKind` is priced the day it is priced.
///
/// Buildings go through `building_value`, not `building_stats`, so a hall
/// counts everything paid to raise it. That matters twice: it stops the ladder
/// going *backwards* (a Keep's own row costs less than the TownHall it
/// replaces, so the rung would have been worth less than the thing it was an
/// upgrade of), and it is the same accounting `asset_score` already does.
/// It also ends the other half of the old bug's absurdity, where a 35-resource
/// Wall and a 590-resource TownHall were both flat 60 — wall spam was a hero
/// XP faucet.
///
/// The scalars stay in code (tables-move-scalars-stay): the *inputs* are the
/// data tables, and this is the one line of arithmetic over them.
pub fn xp_for_kill(unit: Option<UnitKind>, building: Option<BuildingKind>) -> f32 {
    let cost = match (unit, building) {
        (Some(kind), _) => {
            // `unit_value`: a hero trains for free, so its cost fields are 0
            // and its worth is its revival price. Reading the row directly
            // would make the enemy Champion — the most valuable kill on the
            // map — worth no experience whatsoever.
            let (gold, lumber) = unit_value(kind);
            gold + lumber
        }
        (_, Some(kind)) => {
            let (gold, lumber) = building_value(kind);
            gold + lumber
        }
        _ => return 0.0,
    };
    XP_PER_STEP * (cost / XP_COST_PER_STEP) as f32
}

/// ONE hero's progression, kept up to date while it lives and preserved when
/// it dies so revival restores the level.
#[derive(Clone, Copy, Debug)]
pub struct HeroRecord {
    pub level: u32,
    pub xp: f32,
    /// Which hero class this record belongs to — the key of the record, since
    /// a team holds at most one per class (`HeroRecords`).
    pub kind: UnitKind,
}

/// Every hero record a team has ever opened, **one per class**.
///
/// v1 was `Option<HeroRecord>` per team, because a team was allowed exactly
/// one hero for the whole match. Hero slots now scale with the hall ladder
/// (`hero_slots`), so a team can field a Champion *and* a Priestess at tier 2
/// — and each of them needs its own preserved level, XP and revival price. A
/// list keyed by class is therefore the shape, and "the class lock" changed
/// meaning with it: it no longer stops a SECOND hero, it stops a DUPLICATE
/// class (see `hero_slots`).
#[derive(Resource, Default)]
pub struct HeroRecords {
    pub human: Vec<HeroRecord>,
    pub claude: Vec<HeroRecord>,
}

impl HeroRecords {
    pub fn list(&self, team: Team) -> &[HeroRecord] {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }
    fn list_mut(&mut self, team: Team) -> &mut Vec<HeroRecord> {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
    /// This team's record for one hero CLASS, if it has ever fielded one.
    pub fn get(&self, team: Team, kind: UnitKind) -> Option<HeroRecord> {
        self.list(team).iter().copied().find(|r| r.kind == kind)
    }
    /// Insert or update the record for `record.kind`. Upsert by class, so
    /// `hero_progression` writing every frame stays idempotent.
    pub fn set(&mut self, team: Team, record: HeroRecord) {
        let list = self.list_mut(team);
        match list.iter_mut().find(|r| r.kind == record.kind) {
            Some(existing) => *existing = record,
            None => list.push(record),
        }
    }
}

/// How many heroes a team at this tech tier may field at once: **1 at
/// TownHall, 2 at Keep, 3 at Castle**. One rung, one hero — the tier number
/// *is* the answer, so there is no second table to keep in step with the
/// ladder, and a fourth rung would need no edit here.
///
/// Two rules travel with the count, and both are enforced at economy.rs's
/// pay-point (with a matching pre-check in intent.rs so a seat gets an error
/// string instead of a silent drop):
///
///   1. **Distinct classes only.** A team may hold a Champion and a Priestess,
///      never two Champions. Duplicate heroes would make the hero a *unit*
///      rather than a *character* — the whole reason revival preserves a level
///      is that the thing coming back is the same person — and it would turn
///      the tier-2 reward into "buy a second copy of your best unit", which is
///      a power spike rather than a widened decision.
///   2. **Slots are a ceiling that can fall.** The count is read from the
///      team's LIVE tier, exactly like every other tier-gated thing: lose the
///      Keep and you may not train a second hero, though the second hero you
///      already have is not confiscated. Nothing is ever retroactive.
///
/// Consequence worth stating plainly: only TWO hero classes exist today, so
/// tier 3's third slot is **currently unreachable** — a Castle team can field
/// Champion + Priestess and nothing more. That is deliberate future-proofing
/// rather than an oversight; the number comes from the ladder, and the third
/// slot fills itself the day a third class ships. Asserted by
/// `hero_slots_climb_the_hall_ladder_one_per_rung`.
pub fn hero_slots(tier: TechTier) -> u32 {
    tier.level()
}

/// Why a hero may (not) be queued right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeroSlotVerdict {
    Ok,
    /// The team already holds this class — alive on the map, or in flight in
    /// somebody's training queue.
    DuplicateClass,
    /// Every slot the team's tier grants is spoken for.
    NoSlot { used: u32, slots: u32 },
}

/// **THE hero-slot rule**, in one function, so the four places that ask cannot
/// disagree: economy.rs at the pay-point (authoritative), intent.rs as the
/// pre-check that turns a refusal into an error string, ui.rs to grey the
/// button, and ai.rs to decide what to queue.
///
/// `held` is every hero class this team is holding — **living heroes plus
/// every hero already sitting in any of its training queues**. The queued half
/// is the whole reason this takes a list rather than a count of bodies: two
/// halls each queuing a Priestess, or one hall queuing three Champions, are
/// all in flight and none of them is alive yet, and a rule that counted only
/// the map would let every one of them through.
pub fn hero_slot_check(held: &[UnitKind], kind: UnitKind, tier: TechTier) -> HeroSlotVerdict {
    if held.contains(&kind) {
        return HeroSlotVerdict::DuplicateClass;
    }
    let slots = hero_slots(tier);
    let used = held.len() as u32;
    if used >= slots {
        return HeroSlotVerdict::NoSlot { used, slots };
    }
    HeroSlotVerdict::Ok
}

/// How long a hero of `kind` sits in the queue: a fresh one takes the row's
/// `train_time`, a revival takes `HERO_REVIVE_TIME`. Split out of
/// `hero_train_cost` because the clock is the half of the answer that depends
/// on nothing but the record — the progress bar in ui.rs wants only this, and
/// asking it through the money function would mean handing that call site a
/// `held` list it has no reason to compute (and every reason to get wrong).
pub fn hero_train_time(records: &HeroRecords, team: Team, kind: UnitKind) -> f32 {
    match records.get(team, kind) {
        Some(_) => HERO_REVIVE_TIME,
        None => unit_stats(kind).train_time,
    }
}

/// Gold/lumber/time to put a hero of `kind` in a training queue right now.
/// **Your FIRST hero is free — once. Every hero after it is 400g/100l.**
///
/// `held` is the same slice `hero_slot_check` takes: every hero class this
/// team is holding, alive on the map *or* sitting in any of its queues. Ask
/// both functions with the same list — the slot rule and the price rule read
/// the same fact about the team, and a caller that computed "held" one way for
/// one and another way for the other is the divergence this parameter exists
/// to prevent.
///
///   * this team has NEVER had a hero — no record of any class, nothing in
///     flight — `(0, 0, train_time)`. Free, not instant: the 25 seconds and
///     the 5 supply are unchanged, and they are paid at the hall, which is
///     also the building that makes workers. The opening cost of a hero is a
///     worker line that stalls, not a treasury that empties.
///   * a class with a record (i.e. one that has died) — the row's
///     `revive_gold`/`revive_lumber` at `HERO_REVIVE_TIME`.
///   * any other hero — a second class, fielded fresh by a team that already
///     has one — the row's `revive_gold`/`revive_lumber` at the row's
///     `train_time`. Full fare, on the same clock as any fresh hero.
///
/// **Why the second class is priced off `revive_*`.** The fielding price and
/// the revival price are the same number by design — 400g/100l, one price for
/// "have a hero standing that you did not have a moment ago", whether it is
/// new or bought back. Rather than resurrect the `cost_gold`/`cost_lumber`
/// fields (which the loader pins at 0, and which `unit_value` deliberately
/// ignores for heroes so that killing one is worth something), the rule reads
/// `revive_gold`/`revive_lumber` for both. One number per class in
/// `units.ron`, which is also what a commander reads on the wire: the price
/// printed under "what it costs when it dies" is the same price they will pay
/// to field the second one.
///
/// **Why only the first is free.** Free-to-field exists to make the hero
/// guaranteed content rather than a 400g gamble nobody took (see the arena
/// history below). One free hero does that. A second free hero did something
/// else: with hero slots scaling on the hall ladder, reaching a Keep silently
/// posted a fully-levelling Priestess to your army for nothing, so the tier-2
/// reward stopped being "a decision widens" and became "a free power spike
/// arrives". The opening is the moment that needed de-risking; a mid-game
/// second hero is a purchase like any other, and it should compete with the
/// three Footmen it costs.
///
/// **The queue edge.** A first hero still IN TRAINING counts as held, so the
/// second class prices at full fare the instant the first is queued — you
/// cannot queue both in the same breath and have both come out free. This is
/// `hero_slot_check`'s precedent applied to money: that function counts queued
/// heroes against the slot ceiling for exactly the same reason (two halls each
/// queuing a hero are both in flight and neither is alive yet), and a price
/// rule that counted only bodies would be the one hole left in a wall that is
/// otherwise built. Asserted by
/// `a_first_hero_still_in_the_queue_already_prices_the_second`.
///
/// **Why this way round.** v2 charged 400g/100l up front and discounted the
/// revival to 250g. Arena rounds 17 through 19 produced five straight winners
/// who never fielded a hero, and in r19 BOTH commanders skipped deliberately:
/// 400 gold is three Footmen or a third Barracks *now*, against experience
/// that pays off past the ten-minute mark in a game that ends around seven.
/// The skip was correct, which made the hero optional content nobody opted
/// into. Free-to-field makes the hero guaranteed content; a costly death makes
/// keeping it alive the actual decision.
///
/// **Flat, not level-scaled.** A level-5 death costs exactly what a level-1
/// death costs, and that is deliberate:
///
///   * scaling the bill with level punishes precisely the play this change
///     rewards. The commander who nursed a hero to level 5 would get the
///     largest invoice, and the one who let it idle in the base would pay
///     least — an anti-incentive aimed at hero preservation, which is the
///     strategic event we are trying to create.
///   * the "a better hero should cost more" intuition is already satisfied on
///     the other side of the ledger: revival PRESERVES the level. A flat price
///     therefore buys back more the higher the hero was, so preserving levels
///     makes each revival a *better* deal, not a worse one. The gradient
///     exists; it just lives in the benefit rather than the fee.
///   * it keeps one number on the wire. `hero_costs` can say "400g/100l if it
///     dies" instead of teaching every commander a formula it would then have
///     to re-derive at every level-up.
///
/// The counter-argument, for the record: a flat fee means a snowballing hero
/// gets cheaper in relative terms as the match goes on, so a team that is
/// already ahead re-fields its level-6 Champion for the price of a level-1
/// one. If arena rounds show hero-snowball dominance rather than hero-loss
/// drama, level scaling is the lever to reach for — and this comment is where
/// the argument for it starts.
///
/// Revival stays per class, not per team: a team that lost a Champion and a
/// Priestess owes 400g/100l for each of them, and buying one back does not
/// make the other cheaper. It is only the *freebie* that is per team, because
/// it is not a discount on a class — it is the one-time waiver that gets your
/// first hero onto the field.
pub fn hero_train_cost(
    records: &HeroRecords,
    team: Team,
    kind: UnitKind,
    held: &[UnitKind],
) -> (u32, u32, f32) {
    let base = unit_stats(kind);
    let time = hero_train_time(records, team, kind);
    if records.get(team, kind).is_some() {
        // This class has stood before and is being bought back.
        return (base.revive_gold, base.revive_lumber, time);
    }
    // The waiver: no record of ANY class, and nothing in flight. Read out of
    // the table rather than written as a literal `0` — the validator
    // guarantees hero cost fields ARE zero, and going through the row keeps
    // units.ron the single place the number lives.
    if records.list(team).is_empty() && held.is_empty() {
        return (base.cost_gold, base.cost_lumber, time);
    }
    // A fresh hero for a team that already has (or has had) one.
    (base.revive_gold, base.revive_lumber, time)
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
///
/// `Deserialize` because `stances.ron` names the focus list of each stance
/// preset in these words. The wire still speaks strings and resolves them
/// through `parse_target_class`, which folds spellings; RON is a data file
/// written by hand once and validated at load, so it may name the variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
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
        // Everything below is derived from the row's ROLE rather than from a
        // list of kinds. That is what makes the counter-triangle hold ACROSS
        // races without a single cross-race line of code: `vs_cavalry_mult`
        // is keyed off the CLASS, the class is keyed off the role, and both
        // races' cavalry declare `role: Cavalry` — so a Kingdom Spearman's 5x
        // lands on a Horde Wolfrider exactly as hard as on a Raider, and an
        // Impaler's lands on a Knight.
        match (unit, building) {
            (Some(kind), _) => match unit_role(kind) {
                // Every hero class is "Hero" for targeting purposes.
                UnitRole::HeroMelee | UnitRole::HeroSupport => Some(TargetClass::Hero),
                // Casters answer to "Archer": the class is the fragile ranged
                // BACK RANK, and a doctrine that says "kill their archers"
                // means "get behind the line and kill the soft things", which
                // is exactly the order you want pointed at a caster. Naming a
                // separate Caster class would let a priority list that already
                // says Archer silently miss the most valuable target on the
                // field.
                UnitRole::Ranged | UnitRole::Caster => Some(TargetClass::Archer),
                // Anti-cavalry answers to "Footman": the class is the melee
                // line, and a doctrine that says "focus the front rank" means
                // the front rank, whatever it is holding.
                UnitRole::Line | UnitRole::AntiCavalry => Some(TargetClass::Footman),
                UnitRole::Worker => Some(TargetClass::Worker),
                UnitRole::Siege => Some(TargetClass::Siege),
                // The tier-3 shock unit rides in under the same class as the
                // light cavalry, which is the whole counter-triangle in one
                // line: a tech advantage buys tempo and reach, never immunity
                // to a counter.
                UnitRole::Cavalry | UnitRole::Shock => Some(TargetClass::Cavalry),
                // Unreachable in practice — a flyer was classified as `Air`
                // above — but a grounded flyer-role row would still classify.
                UnitRole::Flyer => Some(TargetClass::Air),
            },
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

impl SquadPosture {
    /// The bare posture word, matching the `posture` intent's `type` tag.
    /// What a unit names when it says which posture is moving it.
    pub fn word(&self) -> &'static str {
        match self {
            SquadPosture::Defend { .. } => "defend",
            SquadPosture::Push { .. } => "push",
            SquadPosture::Escort { .. } => "escort",
            SquadPosture::Forage { .. } => "forage",
        }
    }
}

/// The default squad every army unit belongs to unless assigned elsewhere.
/// doctrine.rs auto-enrolls postureless army units here and seeds a Defend
/// posture at the team's base, so "commander does nothing" still yields a
/// pooled, reactive army — never scattered statues.
pub const DEFAULT_SQUAD: u8 = 0;

/// Posture per (team, squad id). Bridge/AI writes; doctrine.rs executes.
///
/// A `BTreeMap`, not a `HashMap`, and that is a determinism decision rather
/// than a performance one: `run_squad_postures` snapshots this map and walks
/// it, and std's `HashMap` reseeds its hasher every process, so a hash map
/// here means the squads execute in a different order in every run of the
/// same binary. Sorted keys make the walk reproducible.
#[derive(Resource, Default)]
pub struct SquadOrders(pub std::collections::BTreeMap<(Team, u8), SquadPosture>);

// ---------------------------------------------------------------------------
// Stances: five fixed doctrine presets, one word each
// ---------------------------------------------------------------------------
//
// A stance is not a new capability. It is a NAME for a bundle of the doctrine
// this file already declares — a squad posture, a leash, a retreat threshold
// and a focus-fire list — installed together, through the same appliers, by one
// sentence. docs/AFFORDANCES.md is the argument for it: the bridge exposes ~25
// verbs and a small commander drowns choosing among them, while the sub-second
// half of the match is already played well by doctrine.rs between polls. Five
// words that each set five things is a decision surface a weak model can hold
// in its head, and the full vocabulary stays open beside it for one that cannot
// be served by a preset.
//
// **The vocabulary is fixed and engine-defined**, deliberately. Commander-
// defined bundles would make the affordance graph as large as the vocabulary
// they were meant to shrink, and would make two arena rounds incomparable.
//
// **The default is persistence.** Nothing in the engine clears a stance; a
// commander that says nothing for ninety seconds keeps the stance it set, and
// doctrine.rs keeps executing it. Silence is a policy, not a gap. (arena/r21:
// a Barracks idle for 98 seconds because nothing forced a re-evaluation and
// nothing continued the last decision either.)

/// The five stance words. Fixed vocabulary — see the section comment above.
/// `Default` is `Turtle` — the first word of `ALL_STANCES`, and the one the
/// human's cycle tile starts on for a squad that has never been stanced. It is
/// a starting point for a cycle, never an implicit stance: nothing is in a
/// stance until a `stance` intent puts it in one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Deserialize)]
pub enum StanceKind {
    /// Hold home, tight. Small ring, short leash, break off early.
    #[default]
    Turtle,
    /// Gather at a forward point and wait. A staging post, not a fight.
    Stage,
    /// Commit to an objective. No leash, low retreat threshold.
    Push,
    /// Hold ground away from home — an expansion, a ford. Wide ring.
    Secure,
    /// Hunt the soft targets and leave before the trade. Long leash, high
    /// retreat threshold.
    Harass,
}

pub const ALL_STANCES: [StanceKind; 5] = [
    StanceKind::Turtle,
    StanceKind::Stage,
    StanceKind::Push,
    StanceKind::Secure,
    StanceKind::Harass,
];

impl StanceKind {
    /// The word, as it travels on the wire and appears in the snapshot.
    pub fn word(self) -> &'static str {
        match self {
            StanceKind::Turtle => "turtle",
            StanceKind::Stage => "stage",
            StanceKind::Push => "push",
            StanceKind::Secure => "secure",
            StanceKind::Harass => "harass",
        }
    }

    /// The next stance in `ALL_STANCES`, wrapping. The human's doctrine tile is
    /// a cycle for the same reason `[P]` Priority is one.
    pub fn next(self) -> StanceKind {
        let i = ALL_STANCES.iter().position(|s| *s == self).unwrap_or(0);
        ALL_STANCES[(i + 1) % ALL_STANCES.len()]
    }

    /// This stance's row of `stances.ron`.
    pub fn def(self) -> &'static StanceDef {
        crate::data::stance_row(self)
    }
}

/// Parse a stance word off the wire. Folded through `normalize_name`, exactly
/// like every other name a commander may type, so `"Push"` and `"push"` are one
/// word. `None` is a refusal the caller turns into a message naming all five.
pub fn parse_stance(name: &str) -> Option<StanceKind> {
    let wanted = normalize_name(name);
    ALL_STANCES
        .iter()
        .copied()
        .find(|s| normalize_name(s.word()) == wanted)
}

/// Every stance word, comma-joined, for the refusal that teaches.
pub fn stance_words() -> String {
    ALL_STANCES
        .iter()
        .map(|s| s.word())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which ground-anchored posture a stance installs.
///
/// Three of the four postures, and the missing one is principled rather than an
/// omission: `Escort` names a *unit*, and a stance names ground. A preset that
/// followed a hero would need a second kind of target and would stop being one
/// word about one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum StancePosture {
    Defend,
    Push,
    Forage,
}

/// Where a stance's retreat threshold falls back TO.
///
/// A flag rather than a coordinate because the two answers are the only ones
/// that survive the anchor moving: `Anchor` keeps a defensive stance's wounded
/// inside the ring it is holding, `Base` sends an offensive stance's wounded
/// out of the fight entirely. Anything else would be a number the preset had no
/// way to know.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum StanceRally {
    /// The stance's own anchor point.
    Anchor,
    /// The team's base. `Team::base_pos`, the same point doctrine.rs seeds the
    /// default squad's home `Defend` on.
    Base,
}

/// One row of `stances.ron`: the numbers behind one stance word.
///
/// Every field here is a number or a flag, which is why the table is data and
/// not a `match` (DESIGN.md § "Content data files", and the merge argument in
/// data.rs's header). The five WORDS are identity and stay in the enum above.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StanceDef {
    pub kind: StanceKind,
    /// Tile caption on the human's doctrine card.
    pub label: String,
    /// One line, for the card's cost slot and the catalog.
    pub description: String,
    pub posture: StancePosture,
    /// Ring radius for a `Defend` posture. Must be > 0 for `Defend`; ignored
    /// (and conventionally 0) for the other two, which have no ring.
    pub radius: f32,
    /// Leash radius around the anchor. `0` installs no leash — which is a real
    /// setting, not a missing one: a push that could be recalled is not a push.
    pub leash: f32,
    /// Retreat threshold as a fraction of max HP. `0` installs no retreat
    /// policy; anything else must be in the open range (0,1).
    pub retreat_below: f32,
    pub rally: StanceRally,
    /// Focus-fire list, in order. Empty installs no `TargetPriority`, which
    /// leaves combat.rs's ordinary acquisition alone.
    pub priority: Vec<TargetClass>,
}

/// The stance word each squad is holding, per `(team, squad)` — the readout
/// half of the feature, and the thing "the default is persistence" is about.
///
/// It is a RECORD, never a second source of truth: the components and the
/// `SquadOrders` entry the stance installed are what doctrine.rs executes, and
/// this map only remembers which word produced them. That is why the `posture`
/// verb clears the entry — a squad hand-tasked out of its stance is no longer in
/// it, and a snapshot that still said `"push"` would be lying to the commander
/// about its own army.
///
/// A `BTreeMap` for the reason `SquadOrders` beside it is one: iteration order
/// is randomness, and `HashMap` reseeds per process.
#[derive(Resource, Default)]
pub struct SquadStances(pub std::collections::BTreeMap<(Team, u8), StanceKind>);

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

/// `BH_FOG=0` restores the pre-v2 omniscient baseline: every cell permanently
/// `Visible`, no memory, nothing filtered anywhere. It exists so old AARs and
/// balance tooling have something to compare against — not as a gameplay
/// option. Default is on.
pub const FOG_ENV: &str = "BH_FOG";

/// Game-seconds between recomputes (~4 Hz). Deliberately GAME time and not
/// real time, unlike the event feed's cadence: the feed keeps a *watcher*
/// current and a watcher's attention runs at one second per second, but fog is
/// a gameplay input that the scripted AI and the doctrine layer both read, so
/// a `BH_SPEED=16` run has to resolve the same number of fog updates per
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
    /// Dense index, for anything keyed by state — `FogTinted::shades` is the
    /// one caller. Spelled out rather than `as usize` so a reordering of the
    /// variants cannot silently reshuffle a lookup table.
    pub fn index(self) -> usize {
        match self {
            CellVis::Unexplored => 0,
            CellVis::Explored => 1,
            CellVis::Visible => 2,
        }
    }
}

/// **How much of a thing's own colour survives at each fog state.**
///
/// The one shading rule, and it is deliberately here rather than in a
/// renderer, because as of the scenery fix there are *two* renderers of it and
/// docs/FOG.md's whole promise is that two renderings of one rule cannot drift:
///
/// | renderer | how it uses this |
/// |---|---|
/// | the ground quad (`ui::fog_alpha`) | lays `1.0 - shade` of black over the ground |
/// | scenery tint (`FogTinted`) | multiplies a doodad's own base colour by it |
///
/// Those are the same darkening arrived at from opposite directions, which is
/// exactly what makes a tree standing in remembered ground read as the *same*
/// remembered as the ground under it. Before this, the quad dimmed the floor
/// and the forest above it stayed at full brightness — a lit canopy hanging
/// over dark earth, because a flat quad at `y = 0.16` cannot dim anything
/// taller than 0.16.
///
/// The three numbers are 100% / 56% / 12%, which is 0.0 / 0.44 / 0.88 of black
/// — the exact alphas the overlay already used, so the legibility this was
/// tuned for is preserved by construction rather than re-tuned.
pub fn fog_shade(cell: CellVis) -> f32 {
    match cell {
        CellVis::Visible => 1.0,
        CellVis::Explored => 0.56,
        CellVis::Unexplored => 0.12,
    }
}

/// A doodad that wears its cell's fog state, as one pre-built material per
/// state (indexed by `CellVis::index`).
///
/// Three materials rather than one material repainted per frame, and that
/// choice is the direct lesson of the bug documented in `ui::update_fog_overlay`:
/// a `StandardMaterial`'s bind group is built once and rebuilt only when the
/// *material asset* changes, so anything that repaints in place has to
/// republish or it silently goes on rendering last-time's data. Swapping which
/// **handle** an entity wears has no such failure mode — the material assets
/// are written once at setup and never touched again, and `MeshMaterial3d`
/// pointing somewhere new is the whole update. The trap is designed out rather
/// than defended against.
#[derive(Component, Clone, Debug)]
pub struct FogTinted {
    pub shades: [Handle<StandardMaterial>; 3],
}

impl FogTinted {
    pub fn at(&self, cell: CellVis) -> &Handle<StandardMaterial> {
        &self.shades[cell.index()]
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

// ---------------------------------------------------------------------------
// Intel — a sighting as durable, queryable knowledge
// ---------------------------------------------------------------------------
//
// The memory model above stops at structures, and `CellVis::Explored` gives
// the reason: an army is not furniture, and a remembered army is a lie that
// gets people killed. That is still true of `units`, which reports what a team
// can see THIS INSTANT and is left exactly as it was.
//
// What it was never an argument for is throwing the observation away. A player
// who watches six footmen cross the centre ford does not forget it a quarter
// of a second later. They remember a stale fact *as* a stale fact and discount
// it accordingly — and the discount is the whole skill. The lie was never the
// memory; it was reporting a memory in the same shape as a sighting, so that
// nothing downstream could tell them apart.
//
// So intel is a SEPARATE section with a timestamp welded to every entry.
// Nothing in it can be mistaken for something standing on the board right now,
// because there is no field-compatible way to read it as one: `units[]` has no
// `t_seen` and every sighting has nothing else.
//
// FOG-HONEST BY CONSTRUCTION, and in the same way everything else here is: the
// ledger is written by `update_fog`, in the same pass, off the same cells that
// decide what the snapshot and the screen show. The only line that inserts a
// sighting is guarded by the identical `vis_at(..).sees()` the building ghosts
// are guarded by. A unit that has never stood in a visible cell cannot appear
// in this ledger, and there is no second code path that could put it there.

/// How long an unrefreshed unit sighting survives before it is dropped.
///
/// Buildings are remembered forever because they do not move; a unit's
/// position decays into fiction at walking pace. Ninety seconds is about the
/// time it takes an army to cross this map twice, so a sighting that old has
/// no remaining power to say where anything *is* — only that it existed and
/// once passed through. Past the horizon it is a rumour, and the ledger drops
/// it rather than let a commander plan against it.
///
/// This is the one place unit intel and building ghosts genuinely differ, and
/// the difference is exactly the thing that moves.
pub const SIGHTING_TTL_S: f32 = 90.0;

/// How close two concurrent sightings must be to read as one body of troops.
pub const GROUP_RADIUS: f32 = 18.0;

/// ...and how close in observation time. See `FogGrid::army_groups`.
const GROUP_WINDOW_S: f32 = 10.0;

/// Below this much observed movement between two consecutive fog ticks, a unit
/// is standing still and HAS no heading. Well above float jitter, well below
/// what the slowest unit in the game covers in one `FOG_INTERVAL`.
const HEADING_MIN_MOVE: f32 = 0.2;

/// Two observations further apart than this in game time are not a track.
/// Re-sighting a raider a minute later on the other side of the map says
/// nothing about which way it is walking now, and the straight line between
/// the two observations is a line nobody watched it travel.
const HEADING_GAP_S: f32 = 1.0;

/// A coarse eight-point heading, as an observer reads it off a moving body of
/// troops. Eight and not sixty-four because "they are heading northeast" is
/// the whole of what somebody watching from across a valley can honestly
/// claim, and a finer number would be precision the observation does not have.
///
/// `N` is `+Z`, which is the minimap's up (`ui::world_to_minimap`), so the word
/// in the snapshot and the arrow on the screen point the same way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heading8 {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Heading8 {
    /// The heading implied by observed movement, or `None` if the body did not
    /// visibly move. `None` is a real answer here, not a failure: a unit
    /// standing still has no heading, and inventing one from float noise would
    /// be the smallest possible lie told the most often.
    pub fn of(delta: Vec3) -> Option<Heading8> {
        if Vec2::new(delta.x, delta.z).length() < HEADING_MIN_MOVE {
            return None;
        }
        // atan2(x, z): zero is +Z (north), increasing toward +X (east).
        let deg = delta.x.atan2(delta.z).to_degrees();
        Some(match ((deg / 45.0).round() as i32).rem_euclid(8) {
            0 => Heading8::N,
            1 => Heading8::NE,
            2 => Heading8::E,
            3 => Heading8::SE,
            4 => Heading8::S,
            5 => Heading8::SW,
            6 => Heading8::W,
            _ => Heading8::NW,
        })
    }

    /// The wire word. Short because it rides on every sighting in the
    /// snapshot and reads next to a coordinate pair.
    pub fn as_str(self) -> &'static str {
        match self {
            Heading8::N => "N",
            Heading8::NE => "NE",
            Heading8::E => "E",
            Heading8::SE => "SE",
            Heading8::S => "S",
            Heading8::SW => "SW",
            Heading8::W => "W",
            Heading8::NW => "NW",
        }
    }
}

/// An enemy unit as this team last observed it.
///
/// Every field is something a human at the keyboard could have read off the
/// screen while that unit stood in their vision, and **nothing else**:
///
/// | field | how a human knows it |
/// |---|---|
/// | `kind` | the model is on screen |
/// | `pos` | it is standing there |
/// | `hp_frac` | health bars are children of their owner, so an enemy that renders renders its bar (`ui::apply_fog_visibility`) |
/// | `heading` | they watched it move, across two consecutive fog ticks |
/// | `t_seen` | the clock |
///
/// Deliberately **absent**: level, xp, mana, inventory, squad, orders,
/// abilities. ui.rs's pickers select own-team entities only — both the
/// rubber-band and the plain click `continue` on `*team != Team::Human` — so
/// no enemy is ever loaded into the panel that prints `Lv 4`. A commander
/// given a level here would hold an information right no human has a gesture
/// to exercise, which is the exact asymmetry docs/FOG.md exists to close.
#[derive(Clone, Copy, Debug)]
pub struct Sighting {
    /// The real entity's `to_bits()` — the same identity `RememberedBuilding`
    /// uses, so a renderer can match a marker against the live unit and
    /// suppress one when the other is on screen.
    pub id: u64,
    /// The observed unit's team. Always the ledger owner's enemy: the only
    /// insert is guarded on `*t != team`.
    pub team: Team,
    pub kind: UnitKind,
    pub pos: Vec3,
    /// Health fraction when last observed, `0..=1`.
    pub hp_frac: f32,
    /// Which way it was walking when last observed. `None` when it was
    /// standing still, and `None` on a first glimpse — a single glance carries
    /// no heading, because a heading is a difference between two of them.
    pub heading: Option<Heading8>,
    /// Game time of the observation. The field that makes this a memory rather
    /// than a report.
    pub t_seen: f32,
}

impl Sighting {
    /// Game-seconds since the observation.
    pub fn age(&self, now: f32) -> f32 {
        (now - self.t_seen).max(0.0)
    }
}

/// What one team believes about one enemy hero class.
///
/// Three states, and there is no fourth, because there is no fourth thing an
/// observer can honestly be in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeroStatus {
    /// Never laid eyes on it. **Not** "they have no hero": those are the same
    /// empty observation, and conflating them is precisely the mistake the
    /// `fog` block exists to prevent for terrain — "I have no information" and
    /// "there is nothing there" must not be the same answer.
    Unknown,
    /// Seen alive, and nothing observed since has said otherwise. The honest
    /// reading is *alive as far as you know*: it may have died two minutes ago
    /// somewhere nobody was looking, and this will go on saying `Alive`. That
    /// is not a bug in the belief, it is the belief.
    Alive,
    /// We **watched it die** — it stood in our vision on the previous fog tick
    /// and is not in the world on this one. Witnessed, not inferred from an
    /// absence.
    SeenDying,
}

impl HeroStatus {
    /// The wire word, and the same word the HUD line uses.
    pub fn as_str(self) -> &'static str {
        match self {
            HeroStatus::Unknown => "unknown",
            HeroStatus::Alive => "alive",
            HeroStatus::SeenDying => "seen-dying",
        }
    }
}

/// One team's belief about one enemy hero class.
///
/// Keyed by *class* rather than by entity, because a class is what survives a
/// death: heroes revive through `HeroRecords`, and the question a commander
/// asks is "is their Champion on the field", not "is entity 4294968150 alive".
/// A revive is observed the ordinary way — see the hero again and `status`
/// goes back to `Alive`, so the belief is revocable rather than sticky.
#[derive(Clone, Copy, Debug)]
pub struct HeroIntel {
    pub kind: UnitKind,
    pub status: HeroStatus,
    /// Where it was when last observed. `None` iff `status` is `Unknown`.
    pub pos: Option<Vec3>,
    /// Game time of that observation. `None` iff `status` is `Unknown`.
    pub t_seen: Option<f32>,
    /// Health fraction when last observed — the bar the watcher could see.
    /// `0.0` alongside `SeenDying`, which is what they watched happen.
    pub hp_frac: Option<f32>,
}

/// The hero classes, in one canonical order.
///
/// The ledger keeps one entry per class in this order — a fixed `Vec` rather
/// than a map — so hero intel iterates deterministically without `UnitKind`
/// having to grow an `Ord` that nothing else in the codebase wants.
pub const HERO_CLASSES: [UnitKind; 2] = [UnitKind::Hero, UnitKind::Priestess];

fn blank_hero_intel() -> Vec<HeroIntel> {
    HERO_CLASSES
        .iter()
        .map(|kind| HeroIntel {
            kind: *kind,
            status: HeroStatus::Unknown,
            pos: None,
            t_seen: None,
            hp_frac: None,
        })
        .collect()
}

/// Write one class's entry. A no-op for a non-hero kind, which cannot happen —
/// every caller is already behind `is_hero_kind`.
fn set_hero_intel(
    heroes: &mut [HeroIntel],
    kind: UnitKind,
    status: HeroStatus,
    pos: Vec3,
    t: f32,
    hp_frac: f32,
) {
    if let Some(h) = heroes.iter_mut().find(|h| h.kind == kind) {
        h.status = status;
        h.pos = Some(pos);
        h.t_seen = Some(t);
        h.hp_frac = Some(hp_frac);
    }
}

/// Concurrent sightings read as one body of troops.
///
/// The aggregate view exists because the ledger's grain is wrong for the
/// question actually being asked. Nobody wants eleven rows; they want "there
/// is an army of eleven at the ford". Clustering is done where it is *read*
/// rather than stored, so the ledger keeps one truth and the summary is always
/// derived from the current one.
#[derive(Clone, Debug)]
pub struct ArmyGroup {
    pub size: usize,
    /// `(kind, count)`, most numerous first, ties broken by the kind's name so
    /// the summary string is byte-identical on every run.
    pub composition: Vec<(UnitKind, usize)>,
    pub centroid: Vec3,
    /// The **freshest** observation in the group — "the group was seen at
    /// least this recently". Individual members may be up to `GROUP_WINDOW_S`
    /// older, which is what makes them one concurrent picture at all.
    pub t_seen: f32,
}

impl ArmyGroup {
    /// `"5 Footman, 3 Archer"` — the composition as the event feed, the
    /// snapshot and the HUD all say it. One producer, so the three cannot
    /// drift into three different descriptions of one army.
    pub fn summary(&self) -> String {
        self.composition
            .iter()
            .map(|(kind, n)| format!("{n} {}", kind_name(*kind)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// How near a named chokepoint a spot must be to be called by its name.
const PLACE_CHOKE_RADIUS: f32 = 30.0;
/// ...and how near a base to be called "our"/"their" ground.
const PLACE_BASE_RADIUS: f32 = 50.0;

/// Name a spot the way a player would say it out loud.
///
/// The nearest named chokepoint wins, then whose base it is near, then the
/// bare coordinate. All three are **public geography** — chokepoint names and
/// positions ship in every snapshot's `map` block and both bases are known to
/// everyone from the first frame — so naming a place reveals nothing about who
/// is standing in it. What the event says about the army is fog-gated; where
/// the ground is was never secret.
pub fn place_name(pos: Vec3, me: Team) -> String {
    let mut best: Option<(f32, &'static str)> = None;
    for choke in crate::terrain::active_map().chokepoints() {
        let d = Vec2::new(pos.x - choke.pos.x, pos.z - choke.pos.z).length();
        if d <= PLACE_CHOKE_RADIUS && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, choke.name));
        }
    }
    if let Some((_, name)) = best {
        return format!("near the {name}");
    }
    for (team, label) in [(me, "our base"), (me.enemy(), "their base")] {
        let home = team.base_pos();
        if Vec2::new(pos.x - home.x, pos.z - home.z).length() <= PLACE_BASE_RADIUS {
            return format!("near {label}");
        }
    }
    format!("at ({:.0}, {:.0})", pos.x, pos.z)
}

/// One team's knowledge of the map: what it can see now, what it has ever
/// seen, and what it remembers standing there.
pub struct FogGrid {
    /// `GRID_DIM * GRID_DIM`, indexed exactly like `NavGrid` — fog reuses the
    /// nav grid's cell geometry rather than inventing a second one, so "the
    /// cell a unit stands in" means one thing in this codebase.
    cells: Vec<CellVis>,
    /// `BTreeMap` for determinism: `ghosts()` feeds ai.rs's wave targeting,
    /// which picks the nearest remembered structure with a first-minimum
    /// `min_by`. Under a `HashMap` two equidistant ghosts would pick a
    /// different target in every process.
    ghosts: std::collections::BTreeMap<u64, RememberedBuilding>,
    /// Enemy **units** this team has seen, keyed by entity id. `BTreeMap` for
    /// the same reason `ghosts` is one, and with a second reason on top: this
    /// is the input to `army_groups`, whose cluster order and float centroids
    /// would both differ per process under std's hash order.
    sightings: std::collections::BTreeMap<u64, Sighting>,
    /// One entry per hero class, in `HERO_CLASSES` order. Never empty.
    heroes: Vec<HeroIntel>,
    /// Game time of the previous recompute. A unit counts as *watched dying*
    /// only if it was visible at that instant and is gone at this one — see
    /// `update_fog`, which is the only writer.
    last_update: f32,
    explored: usize,
    visible: usize,
}

impl FogGrid {
    fn dark() -> Self {
        FogGrid {
            cells: vec![CellVis::Unexplored; GRID_DIM * GRID_DIM],
            ghosts: std::collections::BTreeMap::new(),
            sightings: std::collections::BTreeMap::new(),
            heroes: blank_hero_intel(),
            last_update: 0.0,
            explored: 0,
            visible: 0,
        }
    }

    /// The `BH_FOG=0` grid: permanently and entirely lit. Every reader works
    /// unchanged against it, which is why the escape hatch needs no `if` at
    /// any call site.
    fn revealed() -> Self {
        let n = GRID_DIM * GRID_DIM;
        FogGrid {
            cells: vec![CellVis::Visible; n],
            ghosts: std::collections::BTreeMap::new(),
            // Empty, and stays empty: `update_fog` returns before it writes
            // anything under `BH_FOG=0`, exactly as it already does for
            // `ghosts`. Under the omniscient baseline live sight supersedes
            // memory entirely — every enemy is in `units[]` every tick — so an
            // intel section would be a second, staler copy of the same board.
            sightings: std::collections::BTreeMap::new(),
            heroes: blank_hero_intel(),
            last_update: 0.0,
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

    /// Enemy units this team remembers seeing, in id order.
    ///
    /// Unlike `ghosts()` there is **no visible-now filter**, and the asymmetry
    /// is deliberate. A ghost stands in for a building that is still there, so
    /// showing one beside the live article would draw the same base twice. A
    /// sighting is a record of an *instant* — "here at T" — and the live unit
    /// that refreshed it is somewhere else by definition of having moved.
    /// Renderers that want to suppress the marker under a live unit have the
    /// `id` to do it with; see `ui::sync_intel_markers`, which does exactly
    /// that for the one tick where they coincide.
    pub fn sightings(&self) -> impl Iterator<Item = &Sighting> + '_ {
        self.sightings.values()
    }

    /// This team's belief about each enemy hero class, in `HERO_CLASSES`
    /// order. Always `HERO_CLASSES.len()` entries — a class never observed
    /// reports `HeroStatus::Unknown` rather than going missing, because a
    /// missing row and a row saying "no idea" are different claims and only
    /// one of them is true.
    pub fn hero_intel(&self) -> &[HeroIntel] {
        &self.heroes
    }

    /// This team's belief about one enemy hero class.
    pub fn hero_intel_of(&self, kind: UnitKind) -> Option<&HeroIntel> {
        self.heroes.iter().find(|h| h.kind == kind)
    }

    /// Cluster concurrent sightings into bodies of troops.
    ///
    /// Single-link agglomeration: two sightings join the same group when they
    /// are within `GROUP_RADIUS` of each other **and** were observed within
    /// `GROUP_WINDOW_S` of each other. Both halves are load-bearing. Without
    /// the distance test the whole map is one army. Without the **time** test
    /// a footman glimpsed at the ford eighty seconds ago merges with an archer
    /// standing there now, and the ledger reports a two-unit force that
    /// existed at no instant — an aggregate that is a lie assembled out of
    /// two honest facts, which is the failure mode this whole section is
    /// arranged to avoid.
    ///
    /// **Workers are excluded.** A mining crew is not an army, and
    /// `enemy_army_seen` firing on one would be the same false alarm
    /// `base_under_attack` refuses to raise for a skirmish in midfield. They
    /// stay in the ledger — seeing five workers on a hillside is exactly how
    /// you find an expansion — they just do not constitute a force.
    ///
    /// Deliberately not clever: O(n²) over a ledger bounded by how many enemy
    /// units a team has recently seen, recomputed where it is read rather than
    /// cached anywhere it could go stale.
    pub fn army_groups(&self) -> Vec<ArmyGroup> {
        // `sightings` is a BTreeMap, so this vector is in id order and every
        // tie broken below resolves the same way on every run.
        let seen: Vec<&Sighting> = self
            .sightings
            .values()
            .filter(|s| s.kind != UnitKind::Worker)
            .collect();
        let n = seen.len();
        if n == 0 {
            return Vec::new();
        }

        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let mut parent: Vec<usize> = (0..n).collect();
        for a in 0..n {
            for b in (a + 1)..n {
                let d = seen[a].pos - seen[b].pos;
                let close = Vec2::new(d.x, d.z).length() <= GROUP_RADIUS;
                let concurrent = (seen[a].t_seen - seen[b].t_seen).abs() <= GROUP_WINDOW_S;
                if close && concurrent {
                    let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                    if ra != rb {
                        parent[ra] = rb;
                    }
                }
            }
        }

        let mut buckets: std::collections::BTreeMap<usize, Vec<&Sighting>> =
            std::collections::BTreeMap::new();
        for i in 0..n {
            let r = root(&mut parent, i);
            buckets.entry(r).or_default().push(seen[i]);
        }

        buckets
            .into_values()
            .map(|members| {
                // Count by NAME so the ordering key and the printed word are
                // the same string — a composition that sorts by one and prints
                // the other is a summary that can reorder without changing.
                let mut counts: std::collections::BTreeMap<&'static str, (UnitKind, usize)> =
                    std::collections::BTreeMap::new();
                let (mut sx, mut sz) = (0.0f32, 0.0f32);
                let mut t_seen = f32::MIN;
                for s in &members {
                    counts.entry(kind_name(s.kind)).or_insert((s.kind, 0)).1 += 1;
                    sx += s.pos.x;
                    sz += s.pos.z;
                    t_seen = t_seen.max(s.t_seen);
                }
                let mut composition: Vec<(UnitKind, usize)> = counts.into_values().collect();
                composition
                    .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| kind_name(a.0).cmp(kind_name(b.0))));
                let k = members.len() as f32;
                ArmyGroup {
                    size: members.len(),
                    composition,
                    centroid: Vec3::new(ev_r1(sx / k), 0.0, ev_r1(sz / k)),
                    t_seen,
                }
            })
            .collect()
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
    /// False under `BH_FOG=0`. Readers do not need this to be *correct* — the
    /// revealed grid makes every query answer "yes" — but renderers use it to
    /// skip painting an overlay that would be entirely transparent.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Test-only: a fully dark, fog-enabled pair, so a test in another module
    /// can pin the mode instead of inheriting whatever `BH_FOG` happens to
    /// say. Deliberately `#[cfg(test)]` rather than a widened public API —
    /// nothing outside a test may seed a team's knowledge, and the compiler
    /// should be the thing enforcing that rather than a comment.
    #[cfg(test)]
    pub fn test_dark() -> Self {
        FogGrids {
            enabled: true,
            human: FogGrid::dark(),
            claude: FogGrid::dark(),
            ..Default::default()
        }
    }

    /// Test-only twin of `test_dark`: both grids fully lit, fog still nominally
    /// ON. For a test whose subject is what a team DOES about something it can
    /// see, rather than whether it can see it — a scripted-AI reaction, say.
    /// Pinning it here keeps the ambient `BH_FOG` out of the outcome in both
    /// directions.
    #[cfg(test)]
    pub fn test_revealed() -> Self {
        FogGrids {
            enabled: true,
            human: FogGrid::revealed(),
            claude: FogGrid::revealed(),
            ..Default::default()
        }
    }

    /// Test-only: set one cell's state directly, so a renderer test can pin a
    /// grid that holds all three states without standing up an army to look at
    /// the map. Same `#[cfg(test)]` reasoning as `test_dark`.
    #[cfg(test)]
    pub fn test_set_cell(&mut self, team: Team, cx: usize, cz: usize, vis: CellVis) {
        let grid = self.get_mut(team);
        grid.cells[NavGrid::idx(cx, cz)] = vis;
        grid.recount();
    }

    /// Test-only: plant a memory in `team`'s grid, exactly as `update_fog`
    /// does when the structure is in sight, without a scout having to walk
    /// there and back.
    #[cfg(test)]
    pub fn test_remember(&mut self, team: Team, record: RememberedBuilding) {
        let grid = match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        };
        grid.ghosts.insert(record.id, record);
    }

    /// Test-only: plant a unit sighting in `team`'s ledger, exactly as
    /// `update_fog` does when the unit is in sight. Same `#[cfg(test)]`
    /// reasoning as `test_remember` — nothing outside a test may seed a team's
    /// knowledge, and the compiler rather than a comment should be what says
    /// so.
    #[cfg(test)]
    pub fn test_sight(&mut self, team: Team, record: Sighting) {
        self.get_mut(team).sightings.insert(record.id, record);
    }

    /// Test-only: set one team's belief about one enemy hero class directly,
    /// so a trigger test can assert on the predicate without staging a death.
    #[cfg(test)]
    pub fn test_hero_intel(&mut self, team: Team, kind: UnitKind, status: HeroStatus, pos: Vec3) {
        set_hero_intel(&mut self.get_mut(team).heroes, kind, status, pos, 0.0, 0.0);
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
    // `Health` is OPTIONAL, and deliberately so. It is read only to stamp a
    // sighting's `hp_frac`; requiring it would make a unit's ability to SEE
    // conditional on it having a health bar, which is a coupling nothing in
    // the fog rule wants and which would fail silently — the unit would simply
    // stop lighting cells, and the map would go dark for no stated reason.
    // Vision comes from the stat table and the transform, full stop.
    units: Query<(Entity, &Unit, &Team, &Transform, Option<&Health>)>,
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
        for (_, unit, t, tf, _) in &units {
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
        let FogGrid {
            cells,
            ghosts,
            sightings,
            heroes,
            last_update,
            ..
        } = grid;
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

        // --- unit sightings (intel) -----------------------------------------
        // Written from the identical cells the ghosts above are written from.
        // Two rules of knowability would be two rules, and this is the same
        // one.
        let mut live_units: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for (e, unit, t, tf, health) in &units {
            if *t == team {
                continue;
            }
            let id = e.to_bits();
            live_units.insert(id);
            let pos = tf.translation;
            if !vis_at(pos).sees() {
                continue;
            }
            // A heading is a TRACK, not a glance: it needs a previous
            // observation close enough in time that we actually watched the
            // ground get covered. Re-acquiring the same raider a minute later
            // yields `None`, which is the honest answer.
            let heading = sightings.get(&id).and_then(|prev| {
                if now - prev.t_seen <= HEADING_GAP_S {
                    Heading8::of(pos - prev.pos)
                } else {
                    None
                }
            });
            // No bar to read means nothing to report; full is the honest
            // default for a unit that has no health to be missing.
            let frac = health.map_or(1.0, |h| {
                if h.max > 0.0 {
                    (h.current / h.max).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            });
            sightings.insert(
                id,
                Sighting {
                    id,
                    team: *t,
                    kind: unit.kind,
                    pos,
                    hp_frac: frac,
                    heading,
                    t_seen: now,
                },
            );
            if is_hero_kind(unit.kind) {
                // Seeing it alive OVERWRITES a `SeenDying` belief, which is
                // how a revive is learned: heroes come back through
                // `HeroRecords`, so the belief has to be revocable by the
                // ordinary act of looking rather than latched forever.
                set_hero_intel(heroes, unit.kind, HeroStatus::Alive, pos, now, frac);
            }
        }

        // A unit is "seen dying" only if it stood in our vision at the
        // PREVIOUS recompute and is absent from the world at this one.
        //
        // The weaker test the building ghosts use — "gone, and we can see the
        // spot where it was" — is *wrong* for something that moves. A hero
        // that walked out of our sight and died half a map away would be
        // reported as watched-dying by a scout still staring at the empty
        // grass it left behind, which is intelligence nobody observed.
        // Requiring that it was visible one tick ago closes that: a quarter of
        // a second is not enough to leave our vision AND die somewhere we
        // cannot see. Entities are despawned centrally by `apply_death` and by
        // nothing else, so "absent from the world" means dead and not
        // bookkeeping.
        let witnessed: Vec<Sighting> = sightings
            .values()
            .filter(|s| s.t_seen >= *last_update && !live_units.contains(&s.id))
            .copied()
            .collect();
        for dead in &witnessed {
            if is_hero_kind(dead.kind) {
                set_hero_intel(heroes, dead.kind, HeroStatus::SeenDying, dead.pos, now, 0.0);
            }
            sightings.remove(&dead.id);
        }

        // The rumour horizon, and the ONLY other way a sighting leaves.
        //
        // Note what is deliberately absent: there is no "we looked at the spot
        // and it was empty" removal. That rule is right for a building, whose
        // record claims the thing is standing there; a sighting claims only
        // that a unit was there AT `t_seen`, and walking onto the spot
        // confirms nothing the timestamp had not already said. Watching an
        // army march off is not amnesia — the marker stays where you last saw
        // it, wearing the heading you watched it leave on, which is precisely
        // the fact worth keeping.
        sightings.retain(|_, s| s.age(now) <= SIGHTING_TTL_S);

        *last_update = now;
    }
}

/// Announce the mode once, so a log or an AAR says which rules it was played
/// under without anyone having to guess from the environment.
fn log_fog_mode(fog: Res<FogGrids>) {
    if fog.enabled {
        info!("fog of war: ON (BH_FOG=0 to disable)");
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

    /// `BH_FOG=0` must make every question answer "yes" so that no consumer
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
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>();
        let mut grids = FogGrids::default();
        // Pin the mode: the ambient BH_FOG must not decide a test's outcome.
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
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
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

    // -- intel: the sightings ledger --------------------------------------
    //
    // The subject of every test below is KNOWABILITY, so each one pins the fog
    // mode rather than inheriting `BH_FOG` — same discipline as the ghost
    // tests above, and for a sharper reason: a ledger tested under an
    // omniscient grid would pass while proving nothing.

    /// A scouting app: one Human unit whose position the test drives, and the
    /// real `update_fog` on a hand-driven clock.
    fn intel_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>();
        let mut grids = FogGrids::default();
        grids.enabled = true;
        grids.human = FogGrid::dark();
        grids.claude = FogGrid::dark();
        app.insert_resource(grids);
        app.add_systems(Update, update_fog);
        app
    }

    fn tick(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    fn human_grid(app: &App) -> &FogGrid {
        app.world().resource::<FogGrids>().get(Team::Human)
    }

    fn spawn_scout(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Human,
                Transform::from_translation(at),
                Health::new(100.0),
            ))
            .id()
    }

    fn spawn_enemy(app: &mut App, kind: UnitKind, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind },
                Team::Claude,
                Transform::from_translation(at),
                Health::new(100.0),
            ))
            .id()
    }

    /// **The headline honesty property.** An enemy that has never stood in a
    /// visible cell is absent from the ledger, and no amount of time passing
    /// puts it there. If this ever fails, `intel` has become the omniscient
    /// snapshot docs/FOG.md was written to delete.
    #[test]
    fn a_unit_never_seen_never_enters_the_ledger() {
        let mut app = intel_app();
        spawn_scout(&mut app, Vec3::new(-90.0, 0.0, -90.0));
        spawn_enemy(&mut app, UnitKind::Footman, Vec3::new(80.0, 0.0, 80.0));

        for _ in 0..8 {
            tick(&mut app, 0.3);
        }
        let grid = human_grid(&app);
        assert_eq!(
            grid.sightings().count(),
            0,
            "an army nobody looked at is an army nobody knows about"
        );
        assert!(grid.army_groups().is_empty());
        // And the enemy's OWN ledger is equally empty of our scout's kind —
        // the rule is symmetric, which is the whole point of one producer.
        let claude = app.world().resource::<FogGrids>().get(Team::Claude);
        assert_eq!(claude.sightings().count(), 0);
    }

    /// Seen, then left: the record persists at the place and time it was
    /// observed, which is the entire difference between this and `units[]`.
    #[test]
    fn a_unit_seen_then_left_is_remembered_where_it_was() {
        let mut app = intel_app();
        let spot = Vec3::new(6.0, 0.0, 0.0);
        let scout = spawn_scout(&mut app, Vec3::ZERO);
        let enemy = spawn_enemy(&mut app, UnitKind::Raider, spot);
        tick(&mut app, 0.3);
        assert_eq!(human_grid(&app).sightings().count(), 1, "eyes on it");

        // The scout leaves; the enemy stays put and is now unobserved.
        app.world_mut()
            .entity_mut(scout)
            .insert(Transform::from_translation(Vec3::new(-90.0, 0.0, -90.0)));
        tick(&mut app, 0.3);
        let grid = human_grid(&app);
        let seen: Vec<&Sighting> = grid.sightings().collect();
        assert_eq!(seen.len(), 1, "memory outlives sight");
        assert_eq!(seen[0].id, enemy.to_bits());
        assert_eq!(seen[0].pos, spot, "recorded where it was, not where it is");
        assert!(!grid.sees(spot), "and we can no longer see that ground");
    }

    /// The rumour horizon. A sighting older than `SIGHTING_TTL_S` is dropped
    /// rather than reported, because a ninety-second-old unit position is not
    /// a stale fact, it is a wrong one.
    #[test]
    fn a_sighting_expires_at_the_staleness_horizon() {
        let mut app = intel_app();
        let spot = Vec3::new(6.0, 0.0, 0.0);
        let scout = spawn_scout(&mut app, Vec3::ZERO);
        spawn_enemy(&mut app, UnitKind::Footman, spot);
        tick(&mut app, 0.3);
        assert_eq!(human_grid(&app).sightings().count(), 1);

        app.world_mut()
            .entity_mut(scout)
            .insert(Transform::from_translation(Vec3::new(-90.0, 0.0, -90.0)));

        // Just inside the horizon: still believed.
        tick(&mut app, SIGHTING_TTL_S - 1.0);
        assert_eq!(
            human_grid(&app).sightings().count(),
            1,
            "a stale sighting is still a sighting, right up to the horizon"
        );
        // Past it: dropped, silently. We did not witness anything; we simply
        // stopped being entitled to the claim.
        tick(&mut app, 2.0);
        assert_eq!(
            human_grid(&app).sightings().count(),
            0,
            "past the horizon it is a rumour, and the ledger does not keep rumours"
        );
    }

    /// Watched dying: the record goes immediately, because we saw it happen.
    #[test]
    fn a_unit_that_dies_while_watched_is_removed_at_once() {
        let mut app = intel_app();
        let spot = Vec3::new(6.0, 0.0, 0.0);
        spawn_scout(&mut app, Vec3::ZERO);
        let enemy = spawn_enemy(&mut app, UnitKind::Footman, spot);
        tick(&mut app, 0.3);
        assert_eq!(human_grid(&app).sightings().count(), 1);

        // `apply_death` despawns centrally; from fog's point of view the unit
        // is simply no longer in the world while we are looking at its cell.
        app.world_mut().entity_mut(enemy).despawn();
        tick(&mut app, 0.3);
        assert_eq!(
            human_grid(&app).sightings().count(),
            0,
            "we watched it die; there is nothing left to remember"
        );
    }

    /// The honesty case that the building rule would have got wrong.
    ///
    /// A unit walks out of our vision and dies somewhere we cannot see. The
    /// ghost rule — "gone, and we can see the spot it was" — would call that
    /// witnessed, because our scout is still staring at the grass it left.
    /// It was not witnessed, and the sighting must survive as the stale fact
    /// it is.
    #[test]
    fn a_unit_that_leaves_our_sight_and_dies_elsewhere_is_not_seen_dying() {
        let mut app = intel_app();
        let watched = Vec3::new(6.0, 0.0, 0.0);
        let away = Vec3::new(80.0, 0.0, 80.0);
        spawn_scout(&mut app, Vec3::ZERO);
        let enemy = spawn_enemy(&mut app, UnitKind::Hero, watched);
        tick(&mut app, 0.3);
        assert_eq!(
            human_grid(&app).hero_intel_of(UnitKind::Hero).unwrap().status,
            HeroStatus::Alive
        );

        // It walks far away — out of our vision, while we go on watching the
        // ground it stood on.
        app.world_mut()
            .entity_mut(enemy)
            .insert(Transform::from_translation(away));
        tick(&mut app, 0.3);
        tick(&mut app, 0.3);
        assert!(human_grid(&app).sees(watched), "we ARE still watching that spot");

        // And now it dies, unobserved.
        app.world_mut().entity_mut(enemy).despawn();
        tick(&mut app, 0.3);
        let grid = human_grid(&app);
        assert_eq!(
            grid.hero_intel_of(UnitKind::Hero).unwrap().status,
            HeroStatus::Alive,
            "a death nobody watched must not be reported as watched"
        );
        assert_eq!(
            grid.sightings().count(),
            1,
            "and the last honest observation survives as the stale fact it is"
        );
    }

    /// The full hero belief cycle: unknown -> alive -> seen-dying -> alive
    /// again on a revive. The last leg is what keeps the belief a belief.
    #[test]
    fn hero_intel_moves_unknown_to_alive_to_seen_dying_and_back() {
        let mut app = intel_app();
        let spot = Vec3::new(6.0, 0.0, 0.0);
        spawn_scout(&mut app, Vec3::ZERO);

        // Unknown: never met. Note the entry EXISTS and says so, rather than
        // being absent — "no idea" is a claim.
        tick(&mut app, 0.3);
        let intel = *human_grid(&app).hero_intel_of(UnitKind::Hero).unwrap();
        assert_eq!(intel.status, HeroStatus::Unknown);
        assert!(intel.pos.is_none() && intel.t_seen.is_none());
        assert_eq!(
            human_grid(&app).hero_intel().len(),
            HERO_CLASSES.len(),
            "every class is listed, met or not"
        );

        // Alive.
        let hero = spawn_enemy(&mut app, UnitKind::Hero, spot);
        tick(&mut app, 0.3);
        let intel = *human_grid(&app).hero_intel_of(UnitKind::Hero).unwrap();
        assert_eq!(intel.status, HeroStatus::Alive);
        assert_eq!(intel.pos, Some(spot));
        assert!(intel.hp_frac.is_some(), "a watcher can read the health bar");

        // Seen dying, while we are looking at it.
        app.world_mut().entity_mut(hero).despawn();
        tick(&mut app, 0.3);
        assert_eq!(
            human_grid(&app).hero_intel_of(UnitKind::Hero).unwrap().status,
            HeroStatus::SeenDying
        );
        // The OTHER class is untouched — beliefs are per class, not per team.
        assert_eq!(
            human_grid(&app)
                .hero_intel_of(UnitKind::Priestess)
                .unwrap()
                .status,
            HeroStatus::Unknown
        );

        // Revived and seen again: the belief is revocable by the ordinary act
        // of looking. Heroes come back through `HeroRecords`, so a latched
        // "dead forever" would be the interface lying on the enemy's behalf.
        spawn_enemy(&mut app, UnitKind::Hero, spot);
        tick(&mut app, 0.3);
        assert_eq!(
            human_grid(&app).hero_intel_of(UnitKind::Hero).unwrap().status,
            HeroStatus::Alive,
            "seeing it alive again corrects the belief"
        );
    }

    /// A heading is a difference between two observations, so the first glance
    /// has none and a stationary unit has none.
    #[test]
    fn heading_needs_two_observations_and_actual_movement() {
        let mut app = intel_app();
        spawn_scout(&mut app, Vec3::ZERO);
        let enemy = spawn_enemy(&mut app, UnitKind::Raider, Vec3::new(4.0, 0.0, 0.0));

        tick(&mut app, 0.3);
        assert!(
            human_grid(&app).sightings().next().unwrap().heading.is_none(),
            "a single glance carries no heading"
        );

        // Standing still through a second observation: still none.
        tick(&mut app, 0.3);
        assert!(
            human_grid(&app).sightings().next().unwrap().heading.is_none(),
            "a unit that has not moved is not going anywhere"
        );

        // Now it walks north (+Z is north, matching the minimap's up).
        app.world_mut()
            .entity_mut(enemy)
            .insert(Transform::from_translation(Vec3::new(4.0, 0.0, 6.0)));
        tick(&mut app, 0.3);
        assert_eq!(
            human_grid(&app).sightings().next().unwrap().heading,
            Some(Heading8::N)
        );
    }

    #[test]
    fn the_eight_headings_point_where_they_say() {
        for (delta, want) in [
            (Vec3::new(0.0, 0.0, 5.0), Heading8::N),
            (Vec3::new(5.0, 0.0, 5.0), Heading8::NE),
            (Vec3::new(5.0, 0.0, 0.0), Heading8::E),
            (Vec3::new(5.0, 0.0, -5.0), Heading8::SE),
            (Vec3::new(0.0, 0.0, -5.0), Heading8::S),
            (Vec3::new(-5.0, 0.0, -5.0), Heading8::SW),
            (Vec3::new(-5.0, 0.0, 0.0), Heading8::W),
            (Vec3::new(-5.0, 0.0, 5.0), Heading8::NW),
        ] {
            assert_eq!(Heading8::of(delta), Some(want), "delta {delta:?}");
        }
        // Altitude is not a direction: fog is XZ only, so a flyer climbing
        // straight up is standing still as far as any observer is concerned.
        assert_eq!(Heading8::of(Vec3::new(0.0, 99.0, 0.0)), None);
    }

    /// Clustering needs BOTH tests. Two units close in space but far apart in
    /// observation time are not one force, and reporting them as one would be
    /// an aggregate assembled out of two honest facts into a lie.
    #[test]
    fn army_groups_cluster_by_distance_and_by_concurrency() {
        let mut grids = FogGrids::test_dark();
        let sighting = |id: u64, kind: UnitKind, x: f32, t: f32| Sighting {
            id,
            team: Team::Claude,
            kind,
            pos: Vec3::new(x, 0.0, 0.0),
            hp_frac: 1.0,
            heading: None,
            t_seen: t,
        };
        // Three together at the same instant...
        grids.test_sight(Team::Human, sighting(1, UnitKind::Footman, 0.0, 100.0));
        grids.test_sight(Team::Human, sighting(2, UnitKind::Footman, 5.0, 100.0));
        grids.test_sight(Team::Human, sighting(3, UnitKind::Archer, 9.0, 100.0));
        // ...one far away at the same instant...
        grids.test_sight(Team::Human, sighting(4, UnitKind::Footman, 90.0, 100.0));
        // ...and one standing where the first three are, but seen a minute
        // earlier. Close in space, not concurrent.
        grids.test_sight(Team::Human, sighting(5, UnitKind::Knight, 4.0, 40.0));

        let groups = grids.get(Team::Human).army_groups();
        assert_eq!(groups.len(), 3, "two forces and one straggler in time");
        let big = groups.iter().find(|g| g.size == 3).expect("the body of three");
        assert_eq!(big.summary(), "2 Footman, 1 Archer", "most numerous first");
        assert_eq!(big.t_seen, 100.0);
        assert!(
            groups.iter().any(|g| g.size == 1 && g.composition[0].0 == UnitKind::Knight),
            "the stale knight is its own group, not a fourth member of theirs"
        );
    }

    /// Workers are in the ledger and out of the armies. Finding five workers
    /// on a hillside is how you find an expansion; it is not a force, and a
    /// rule that fired on one would be an alarm nobody can act on.
    #[test]
    fn workers_are_remembered_but_are_not_an_army() {
        let mut grids = FogGrids::test_dark();
        for id in 0..5u64 {
            grids.test_sight(
                Team::Human,
                Sighting {
                    id,
                    team: Team::Claude,
                    kind: UnitKind::Worker,
                    pos: Vec3::new(id as f32, 0.0, 0.0),
                    hp_frac: 1.0,
                    heading: None,
                    t_seen: 10.0,
                },
            );
        }
        let grid = grids.get(Team::Human);
        assert_eq!(grid.sightings().count(), 5, "you did see them");
        assert!(grid.army_groups().is_empty(), "a mining crew is not an army");
    }

    /// The omniscient baseline keeps its old shape: live sight supersedes
    /// memory entirely, so the ledger stays empty exactly as `ghosts` does.
    #[test]
    fn the_disabled_fog_baseline_keeps_an_empty_ledger() {
        let mut app = App::new();
        app.init_resource::<Time>();
        let mut grids = FogGrids::default();
        grids.enabled = false;
        grids.human = FogGrid::revealed();
        grids.claude = FogGrid::revealed();
        app.insert_resource(grids);
        app.add_systems(Update, update_fog);
        spawn_scout(&mut app, Vec3::ZERO);
        spawn_enemy(&mut app, UnitKind::Footman, Vec3::new(6.0, 0.0, 0.0));
        tick(&mut app, 0.3);

        let grid = human_grid(&app);
        assert_eq!(grid.sightings().count(), 0);
        assert_eq!(grid.ghosts().count(), 0);
        assert!(grid.sees(Vec3::new(6.0, 0.0, 0.0)), "and everything is visible");
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
    /// Hand a payment back. The counterpart to `pay`, and used in exactly one
    /// place: economy.rs returning the price of a queue item that was
    /// cancelled after it had already been charged for.
    ///
    /// Deliberately NOT capped at any ceiling — a refund is money the team
    /// already had, so refusing it would be a way to destroy resources by
    /// buying and cancelling. `saturating_add` only guards the arithmetic.
    pub fn refund(&mut self, gold: u32, lumber: u32) {
        self.gold = self.gold.saturating_add(gold);
        self.lumber = self.lumber.saturating_add(lumber);
    }
}

/// **Free supply, pessimistically** — what is left of the cap once everything
/// living AND everything already queued is paid for.
///
/// `queued` is the supply cost of every unit standing in one of this team's
/// production queues, which is the half of the number that is easy to leave
/// out and expensive to leave out. economy.rs will not pay for a front item
/// whose supply does not fit (`supply_used + stats.supply > supply_cap`), so a
/// team with four Footmen queued into two free supply is *already* stalled —
/// its ledger simply has not been told yet. Anything that asks "may I train?"
/// has to ask the pessimistic question or it will keep saying yes right up to
/// the moment production silently stops.
///
/// One definition, three callers: the scripted commander's build logic
/// (ai.rs), the `supply_capped` predicate (trigger.rs), and the HUD. Saturating,
/// so an over-committed queue reads as zero headroom rather than wrapping into
/// four billion.
pub fn supply_headroom(economy: &Economy, queued: u32) -> u32 {
    economy
        .supply_cap
        .saturating_sub(economy.supply_used + queued)
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

// ---------------------------------------------------------------------------
// Territory: named places and regions
// ---------------------------------------------------------------------------
//
// Every verb in this language that touches ground has spoken it as a pair of
// floats. `{"type":"posture","id":2,"posture":{"type":"defend","x":-60,"z":60,
// "radius":18}}` is a legal sentence and an unreadable one: three months of
// replay logs say "defend (-60.0, 60.0)" and nobody, human or model, can tell
// from that whether the commander meant the northwest ford or a patch of grass.
// intent_compile.py already knew this and had a private answer — a table of
// fords, "mid", and the two bases that it resolved on the *read* side, in
// Python, invisible to the engine. This section makes that vocabulary
// first-class and gives it to both seats.
//
// Two kinds of name, and the difference is the whole design:
//
//   * **Built-in places** are map facts. Derived per map, read-only, IDENTICAL
//     for both teams (modulo the two per-seat aliases), and they exist without
//     anybody arming anything — you can say "hold the center ford" in the first
//     second of a match. They are shared vocabulary: when one seat says
//     "northwest ford" and the other reads "northwest ford" in the map summary,
//     the two are talking about the same ground, and that is checkable.
//
//   * **Regions** are what a commander names. `region_set` gives a circle a
//     name; from then on every verb that takes x/z takes `"region":"<name>"`
//     instead. They are DOCTRINE, not information: a region appears only in its
//     owner's snapshot, and naming ground is never a way to tell the enemy
//     something. The cap and the replace-by-name rule are copied verbatim from
//     `MAX_TRIGGERS_PER_TEAM` and for the identical reason — eight named places
//     is a map a commander can hold in their head; eighty is a database.
//
// **Circles only.** A polygon region is more expressive and there is no
// evidence anybody needs it: every shape the game already speaks — leash,
// defend, `MINE_HOME_RADIUS`, ability areas, the fog grid's reveal — is a point
// and a radius, `contains` is one distance test the frame can afford at 4 Hz,
// and a circle is drawable on a 100px minimap in a way a polygon is not. If a
// match is ever lost because a ford was square, the shape enum is one variant
// away; until then the extra vocabulary is cost without a buyer.

/// Longest region name the language accepts, in bytes. Same bound as
/// [`TRIGGER_NAME_MAX`] and for the same reason: a name is a label, not a
/// sentence, and both are echoed back in teaching errors that have to fit on a
/// HUD line.
pub const REGION_NAME_MAX: usize = 24;

/// The most regions one team may have named at once. See
/// [`MAX_TRIGGERS_PER_TEAM`] — this is that argument, applied to geography.
/// Replacing by name is free, so tuning a region never costs a slot.
pub const MAX_REGIONS_PER_TEAM: usize = 8;

/// Smallest legal region radius, in world units. Below this a "region" is a
/// coordinate wearing a name: `CELL` is 2.0, so a 3-unit circle cannot even
/// hold a formation, and `defend`ing it would jitter.
pub const REGION_RADIUS_MIN: f32 = 4.0;

/// Largest legal region radius. `MAP_HALF` is 100, so 60 is a circle covering
/// most of one half of the board — past that "the region" stops distinguishing
/// anything and a rule keyed on it is a rule that is always true.
pub const REGION_RADIUS_MAX: f32 = 60.0;

/// A named circle of ground.
///
/// `name` is stored exactly as the commander spelled it — that is the string
/// echoed in errors, drawn on the map and printed in sentences. Lookup folds
/// case and punctuation ([`normalize_place`]), so `The Perimeter`,
/// `the-perimeter` and `the perimeter` are one region, but the label keeps the
/// commander's capitals.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: String,
    /// Centre on the ground plane (y is always 0).
    pub center: Vec3,
    pub radius: f32,
}

impl Region {
    pub fn new(name: impl Into<String>, center: Vec3, radius: f32) -> Self {
        Region {
            name: name.into(),
            center: Vec3::new(center.x, 0.0, center.z),
            radius,
        }
    }

    /// Is this world point inside the circle? Measured on XZ only — the game
    /// is flat, and a y component here would be a bug waiting for a flying
    /// unit.
    pub fn contains(&self, p: Vec3) -> bool {
        let d = p - self.center;
        Vec2::new(d.x, d.z).length() <= self.radius
    }
}

/// Fold a place name to its comparison form: lowercase, `-` and `_` become
/// spaces, runs of whitespace collapse.
///
/// Deliberately does NOT drop articles or possessives. `our base` and `their
/// base` are two different places whose only difference is a possessive, and a
/// normalizer that threw those away would make the two built-ins collide — the
/// exact bug intent_compile.py's `NOISE` list has to special-case around.
pub fn normalize_place(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        let ch = if ch == '-' || ch == '_' { ' ' } else { ch };
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// `Ok` with the trimmed name, or `Err` saying why it is not a name.
///
/// Rejects rather than truncates, exactly like [`TriggerName::new`]: a
/// truncated name is a name `region_clear` cannot spell.
pub fn validate_region_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("a region needs a name".to_string());
    }
    if name.len() > REGION_NAME_MAX {
        return Err(format!(
            "region name '{name}' is longer than {REGION_NAME_MAX} characters"
        ));
    }
    if !name.bytes().all(|b| (0x21..=0x7e).contains(&b) || b == b' ') {
        return Err(format!(
            "region name '{name}' must be printable ASCII"
        ));
    }
    Ok(name.to_string())
}

/// Radii the built-in places are given. Not arbitrary: each one is the radius
/// at which "a unit is at that place" is the answer a person would give.
const BUILTIN_BASE_RADIUS: f32 = 28.0;
const BUILTIN_MINE_RADIUS: f32 = 14.0;
const BUILTIN_MID_RADIUS: f32 = 22.0;

/// The map's own vocabulary, from this seat's point of view.
///
/// Read-only and derived — there is no resource holding these, because they are
/// not state: they are what the map *is*, and re-deriving them is four
/// distances and a `Vec`. Both teams get the same list except for the two
/// aliases whose whole job is to be seat-relative.
///
/// The list, in the order it is offered:
///
///   * `our base` / `their base` — the two starting halls, per-seat.
///   * `mid` — the map centre, where bounties spawn and the centre ford is.
///   * `<compass> mine` — one per [`GOLD_MINE_POSITIONS`] entry, named for the
///     compass anchor it is nearest. These are exactly the names
///     intent_compile.py's `pick_mine` already resolves, so the NL front end
///     and the wire protocol name the same four holes in the ground.
///   * `<name> ford` — one per [`crate::terrain::ChokePoint`] on maps that have
///     any, named by the map itself. Empty on `open`.
pub fn builtin_places(team: Team) -> Vec<Region> {
    let (ours, theirs) = match team {
        Team::Human => (HUMAN_BASE, CLAUDE_BASE),
        Team::Claude => (CLAUDE_BASE, HUMAN_BASE),
    };
    let mut out = vec![
        Region::new("our base", ours, BUILTIN_BASE_RADIUS),
        Region::new("their base", theirs, BUILTIN_BASE_RADIUS),
        Region::new("mid", Vec3::ZERO, BUILTIN_MID_RADIUS),
    ];
    for pos in GOLD_MINE_POSITIONS {
        out.push(Region::new(mine_place_name(pos), pos, BUILTIN_MINE_RADIUS));
    }
    for choke in crate::terrain::active_map().chokepoints() {
        // The ford's own opening is the honest radius: `width` is how wide the
        // gap is, so half of it is "inside the ford". Floored at
        // `REGION_RADIUS_MIN` so a narrow gap is still a place you can stand.
        out.push(Region::new(
            choke.name,
            choke.pos,
            (choke.width * 0.5).max(REGION_RADIUS_MIN),
        ));
    }
    out
}

/// Which of the eight compass anchors a point is nearest.
///
/// The anchors are intent_compile.py's `COMPASS` table, byte for byte, and that
/// is the point: this function is the inverse of `pick_mine`, so a mine this
/// names `northwest mine` is the mine that tool hands back for the words
/// "northwest mine". The map's own convention (bases on the SW→NE diagonal,
/// west is -x, north is +z) is read off terrain.rs's ford names.
/// What a gold mine is CALLED — `"north mine"`.
///
/// The one producer of the four mine names. [`builtin_places`] offers them as
/// regions a commander may type, `intent_compile.py`'s `pick_mine` resolves
/// the same words, and economy.rs's exhaustion event names the hole in the
/// ground it is about with them. A second spelling of "the mine at (-60, -60)"
/// would be a second vocabulary for one place, which is the drift docs/FOG.md
/// and docs/INTENT.md both spend their length preventing.
///
/// Positional, not a lookup: it takes the nearest compass anchor, so a node
/// that is not one of [`GOLD_MINE_POSITIONS`] still gets a name rather than a
/// panic or a coordinate.
pub fn mine_place_name(p: Vec3) -> String {
    format!("{} mine", compass_word(p))
}

fn compass_word(p: Vec3) -> &'static str {
    const ANCHORS: [(&str, f32, f32); 8] = [
        ("west", -65.0, 0.0),
        ("east", 65.0, 0.0),
        ("north", 0.0, 65.0),
        ("south", 0.0, -65.0),
        ("northwest", -60.0, 60.0),
        ("northeast", 60.0, 60.0),
        ("southwest", -60.0, -60.0),
        ("southeast", 60.0, -60.0),
    ];
    ANCHORS
        .iter()
        .min_by(|a, b| {
            let da = Vec2::new(a.1 - p.x, a.2 - p.z).length();
            let db = Vec2::new(b.1 - p.x, b.2 - p.z).length();
            da.total_cmp(&db)
        })
        .map(|a| a.0)
        .unwrap_or("center")
}

/// Every team's named regions, in the order they were set.
///
/// A `Vec` for the same determinism reason as [`Triggers`]: the snapshot walks
/// it, the renderer draws it, and both have to produce the same order on every
/// run of the same binary. `set` replaces **in place** by name.
///
/// Built-ins are NOT stored here. They are derived by [`builtin_places`] at the
/// three places that need them (resolution, snapshot, renderer), which is what
/// makes "a built-in exists without arming anything" true rather than a startup
/// system somebody could forget to run.
#[derive(Resource, Default)]
pub struct Regions {
    human: Vec<Region>,
    claude: Vec<Region>,
}

impl Regions {
    pub fn get(&self, team: Team) -> &Vec<Region> {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }

    pub fn get_mut(&mut self, team: Team) -> &mut Vec<Region> {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }

    /// Create or replace by name. `Err` names why it was refused.
    ///
    /// Refuses a name a built-in already owns. Shadowing would be the worse
    /// answer: `our base` would silently mean different ground for the two
    /// seats depending on whether either had redefined it, and the shared
    /// vocabulary is only shared if nobody can quietly repoint a word in it.
    pub fn set(&mut self, team: Team, region: Region) -> Result<(), String> {
        let key = normalize_place(&region.name);
        if let Some(builtin) = builtin_places(team)
            .into_iter()
            .find(|b| normalize_place(&b.name) == key)
        {
            return Err(format!(
                "'{}' is a built-in place on this map ({} at ({:.0}, {:.0})) — \
                 pick another name",
                region.name, builtin.name, builtin.center.x, builtin.center.z
            ));
        }
        let list = self.get_mut(team);
        if let Some(slot) = list.iter_mut().find(|r| normalize_place(&r.name) == key) {
            *slot = region;
            return Ok(());
        }
        if list.len() >= MAX_REGIONS_PER_TEAM {
            return Err(format!(
                "you already have {MAX_REGIONS_PER_TEAM} regions ({}) — \
                 clear one first, or re-use its name to move it",
                list.iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        list.push(region);
        Ok(())
    }

    /// Remove one by name; `true` if anything was there.
    pub fn clear(&mut self, team: Team, name: &str) -> bool {
        let key = normalize_place(name);
        let list = self.get_mut(team);
        let before = list.len();
        list.retain(|r| normalize_place(&r.name) != key);
        list.len() != before
    }

    /// Remove every region of a team. Returns how many went.
    pub fn clear_all(&mut self, team: Team) -> usize {
        let list = self.get_mut(team);
        let n = list.len();
        list.clear();
        n
    }

    /// Resolve a name to a shape, for this seat.
    ///
    /// Own regions first, then the map's built-ins — an order that cannot
    /// matter, because `set` refuses to create the collision. Stated anyway so
    /// the invariant is visible from the lookup as well as the writer.
    pub fn find(&self, team: Team, name: &str) -> Option<Region> {
        let key = normalize_place(name);
        self.get(team)
            .iter()
            .find(|r| normalize_place(&r.name) == key)
            .cloned()
            .or_else(|| {
                builtin_places(team)
                    .into_iter()
                    .find(|b| normalize_place(&b.name) == key)
            })
    }

    /// Every name this seat may speak, own regions first. The teaching half of
    /// an unknown-name refusal: a commander who mistyped gets the menu rather
    /// than a "no".
    pub fn known_names(&self, team: Team) -> Vec<String> {
        let mut out: Vec<String> = self.get(team).iter().map(|r| r.name.clone()).collect();
        out.extend(builtin_places(team).into_iter().map(|b| b.name));
        out
    }

    /// The refusal an unknown name earns, with the menu attached.
    pub fn unknown(&self, team: Team, name: &str) -> String {
        format!(
            "no region named '{name}' — known places: {}",
            self.known_names(team).join(", ")
        )
    }
}

// ---------------------------------------------------------------------------
// Triggers: 'when' as a first-class word
// ---------------------------------------------------------------------------
//
// Doctrine is CONTINUOUS standing policy — retreat below 35%, hold this ring,
// focus the siege — and the engine runs it at machine speed for whoever set it.
// A trigger is the CONTINGENT half of the same idea: a condition the engine
// watches and an intent it submits the instant the condition holds. Both exist
// for the same reason (THESIS.md principle 3, "the engine does what is fast"),
// and the gap between them was the one thing a commander could only do by
// polling: read `events`, notice the base is burning, and answer 13 seconds
// later. A trigger prices that reaction at the engine's 250ms instead.
//
// The ACTION is any `Intent`. That is the design: a trigger adds no second
// vocabulary, it defers the one that already exists, and a fired trigger goes
// through the ordinary compiler with the ordinary validation, the ordinary
// error channel and the ordinary replay log.

/// Longest trigger name the language accepts, in bytes.
///
/// Short on purpose, and the reason is [`Cause`]: a trigger's name is part of
/// the answer a unit gives to "why are you doing that?", and that answer is a
/// `Copy` enum of scalars with no allocation in it. A bounded inline name keeps
/// that property. It is also plenty — a name is a label on a piece of doctrine,
/// not a sentence.
pub const TRIGGER_NAME_MAX: usize = 24;

/// The most triggers one team may have armed at once.
///
/// **Eight is doctrine, not programming.** The cap is the whole difference
/// between "standing policy a commander can hold in their head" and "a
/// scripting language with no debugger". Every trigger is a rule that fires
/// without anybody watching; a player who cannot recite their own rules has
/// stopped commanding and started debugging, and the losing AAR would blame the
/// engine. Eight also fits: the human's readout is one HUD line, the snapshot's
/// `triggers` array is something a model re-reads every poll, and the evaluator
/// sweeps the whole set at 4 Hz without anyone having to think about cost.
///
/// Replacing a trigger by name is free — the cap counts distinct names, so
/// tuning one rule never costs a slot.
pub const MAX_TRIGGERS_PER_TEAM: usize = 8;

/// A trigger's name as a `Copy` scalar, so [`Cause`] stays allocation-free.
///
/// ASCII only, non-empty, at most [`TRIGGER_NAME_MAX`] bytes. Names are
/// compared and stored exactly as given: they are labels a commander chose, and
/// silently folding `Home Guard` into `home-guard` would make `trigger_clear`
/// guess which rule was meant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TriggerName {
    bytes: [u8; TRIGGER_NAME_MAX],
    len: u8,
}

impl TriggerName {
    /// `None` for empty, over-long, or non-ASCII-printable names. Rejecting
    /// rather than truncating: a truncated name is a name `trigger_clear`
    /// cannot spell.
    pub fn new(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > TRIGGER_NAME_MAX {
            return None;
        }
        if !raw.bytes().all(|b| (0x21..=0x7e).contains(&b) || b == b' ') {
            return None;
        }
        let mut bytes = [0u8; TRIGGER_NAME_MAX];
        bytes[..raw.len()].copy_from_slice(raw.as_bytes());
        Some(TriggerName {
            bytes,
            len: raw.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        // Safe by construction: `new` is the only constructor and it accepts
        // printable ASCII only.
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }
}

impl std::fmt::Display for TriggerName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// **The predicate vocabulary.** Every arm is answerable from state the engine
/// already keeps, for the team that armed it, at any instant — no new
/// bookkeeping, no event subscriptions, no history beyond what a component
/// already carries. That constraint is what keeps the set small enough to be
/// doctrine rather than a query language.
///
/// Exact semantics are in `trigger.rs`, next to the code that evaluates them,
/// and restated in docs/INTENT.md.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerWhen {
    /// Any of our own **buildings** took damage within
    /// `BASE_ATTACK_WINDOW_S` (`LastDamaged`). Buildings only: a skirmish in
    /// midfield is not the base being attacked.
    BaseUnderAttack,
    /// Any of our own living heroes is below `frac` of max health.
    HeroBelow { frac: f32 },
    /// **Every** living hero we own is at or above `frac` of max health, and we
    /// own at least one. "My hero is healed", as a wait-condition.
    ///
    /// This is NOT `not hero_below`, and the difference is the whole reason it
    /// is a named predicate rather than a negation operator (docs/INTENT.md
    /// refuses `and`/`or`/`not` — a boolean algebra is where doctrine stops
    /// being doctrine). Two departures, both deliberate:
    ///
    ///   * **A dead hero is not a healed hero.** With no living hero this is
    ///     false, exactly as `hero_below` is. A chain that said "turtle until
    ///     the hero is healed, then commit" must not commit the moment the hero
    ///     dies — which is precisely what a negation would do, at the worst
    ///     available instant.
    ///   * **All, not any.** `hero_below` asks about "whichever one is dying"
    ///     because that is the useful reading of *save my hero*; the useful
    ///     reading of *wait until my heroes are ready* is the whole roster, or
    ///     a fresh second hero would release a chain over the corpse-adjacent
    ///     first one.
    ///
    /// So with at least one hero alive the two really are complements, and with
    /// none alive both are false — which is the honest answer to both questions.
    HeroAbove { frac: f32 },
    /// The living members of our squad `id` hold, in total, less than `frac` of
    /// their combined max health. False for a squad with no living members —
    /// a squad that is gone cannot be "hurt", and firing a rescue at a corpse
    /// pile is worse than firing nothing.
    SquadBelow { id: u8, frac: f32 },
    /// We can SEE at least `count` enemy units right now, optionally of one
    /// `class` (`TargetClass`, the same words `priority` takes).
    ///
    /// **Fog-honest by construction**: it counts against this team's own
    /// `FogGrid::sees`, so a trigger can never react to something its owner
    /// could not have been told about. Remembered buildings do NOT count —
    /// "sighted" means eyes on it now.
    EnemySighted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        /// Defaults to 1.
        #[serde(default = "one_u32")]
        count: u32,
    },
    /// We can SEE at least `count` enemy units **inside a named region** right
    /// now, optionally of one `class`.
    ///
    /// The territorial half of `enemy_sighted`, and the reason regions are
    /// worth having: "any enemy is sighted" fires on a lone scout wandering
    /// past a tower, whereas "five enemies are in north-pass" is the sentence a
    /// commander actually wants to sleep behind. The region may be one this
    /// team named or one the map named, so `{"type":"enemy_in","region":
    /// "center ford","count":5}` is legal from the first second of a match.
    ///
    /// **Fog-honest by construction**, through the identical call
    /// `enemy_sighted` uses: it counts bodies this team's own `FogGrid::sees`
    /// admits AND that are inside the circle. Both filters, always — a region
    /// is a place you are watching, not a place you are told about, and a
    /// region nobody has eyes on stays quiet no matter what walks into it.
    ///
    /// The **live** half of the pair it forms with [`TriggerWhen::EnemyArmySeen`]:
    /// this one asks "is there an enemy in that place *now*", the other asks
    /// "do we know of an army at all". Eyes-on versus remembered, and a
    /// commander wants both words.
    ///
    /// An unknown region name is refused **at arm time** by the compiler, so
    /// this predicate never has to have an opinion about a name that is not a
    /// place. If its region is cleared after arming, it goes quiet rather than
    /// firing on the whole map — see `trigger.rs`.
    EnemyIn {
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        /// Defaults to 1.
        #[serde(default = "one_u32")]
        count: u32,
    },
    /// Our intel ledger holds a body of at least `size` enemy units that was
    /// observed as one concurrent force (`FogGrid::army_groups`).
    ///
    /// Unlike `enemy_sighted` this reads MEMORY, and that is the whole point:
    /// an army does not stop existing because your scout died, and a rule that
    /// only fires while eyes are on the enemy is a rule that disarms itself at
    /// exactly the moment the enemy wants it to. Workers do not count toward
    /// the size — a mining crew is not a force.
    ///
    /// `within_s` bounds how stale the observation may be. Absent, any
    /// surviving sighting counts, which means up to `SIGHTING_TTL_S` old; set
    /// it when the rule wants a *current* army rather than a known one.
    ///
    /// Deliberately carries no region, and that stayed true when territory
    /// landed: [`TriggerWhen::EnemyIn`] is the arm that asks *where*, against
    /// the one vocabulary of places this game has. Growing a second notion of
    /// "where" in here — one that answered from memory rather than from sight
    /// — would be the second implementation this project keeps refusing to
    /// write, and it would also be a lie about knowability, because a
    /// remembered position is not a position.
    EnemyArmySeen {
        /// Defaults to 1.
        #[serde(default = "one_u32")]
        size: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        within_s: Option<f32>,
    },
    /// An enemy hero class is **currently believed dead** — we watched one die
    /// and have not seen it alive since (`HeroStatus::SeenDying`). `class`
    /// names a hero kind (`Hero`, `Priestess`); absent means any of them.
    ///
    /// This is a **level** predicate, like every other arm here, and the
    /// wording above is exact: it is not "a hero died this tick" but "as far
    /// as we know, their hero is down". That distinction decides how it
    /// behaves.
    ///
    /// * A **once** trigger — the normal case, and the one the recipes use —
    ///   fires on the first sweep where the belief holds, then disarms. That
    ///   is the edge behaviour a commander wants from "when their hero falls,
    ///   strike", and they get it without the engine having to keep an
    ///   edge-detection latch nobody can inspect.
    /// * A **repeating** trigger re-fires every cooldown for as long as the
    ///   belief stands. That is a coherent thing to want — "keep pressing
    ///   while they have no hero" — and it is why the level form was kept
    ///   rather than special-cased into an edge.
    ///
    /// The belief is revocable: see the hero alive again after a revive and
    /// the status returns to `Alive`, so a re-armed once-trigger will fire
    /// again on the next death actually witnessed.
    EnemyHeroDown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<String>,
    },
    /// At least one neutral bounty cache is on the map AND visible to us. Also
    /// fog-honest — the snapshot's `bounties` array is filtered the same way,
    /// so the trigger sees exactly the caches its owner is shown.
    BountySpawned,
    /// A gold mine within `MINE_HOME_RADIUS` of one of our completed halls has
    /// run dry (`remaining == 0`). This is what "our mine" means in a game
    /// where mines are neutral: the one your hall was placed to work.
    MineDry,
    /// We have no free supply left: [`supply_headroom`] is zero, counting
    /// everything already standing in a production queue.
    ///
    /// **The alarm for a commander who has stopped playing.** Arena round 17
    /// was lost at 28/28 supply with 2280 gold banked — three plain numbers in
    /// a snapshot full of plain numbers, none of which was going to reach out
    /// and say *you cannot train anything*. `mine_dry` is the same shape of
    /// fact about income; this is the one about spending it.
    ///
    /// Counting the QUEUE is what makes it armable rather than merely true:
    /// the engine's own production gate (economy.rs) refuses to pay for a
    /// front item whose supply will not fit, so a team with four Footmen
    /// queued into two free supply is already blocked and a predicate that
    /// waited for the ledger to catch up would fire after the stall instead of
    /// at it. Same fold, same reason, as `hero_slot_check` counting queued
    /// classes rather than only living heroes.
    ///
    /// A cap of zero is NOT capped — that is "no completed supply building
    /// yet", which is where every team stands on frame one, and a rule that
    /// fired there would fire before the match began.
    SupplyCapped,
    /// Our tech tier has reached `tier` (1, 2 or 3).
    TierReached { tier: u8 },
    /// We field at least `count` living units of `kind`.
    UnitCount { kind: String, count: u32 },
    /// The match clock has passed `at` game-seconds. The one predicate about
    /// nothing in the world — it is here because "expand at 6 minutes" is a
    /// plan every commander already writes, and writing it as a trigger is how
    /// it stops depending on remembering.
    GameTime { at: f32 },
}

fn one_u32() -> u32 {
    1
}

/// Seconds after a hit that `base_under_attack` still counts as "under attack".
///
/// Generous enough to survive the evaluator's own 250ms cadence plus a lull
/// between two volleys, short enough that a raid repelled a minute ago stops
/// arming the rule.
pub const BASE_ATTACK_WINDOW_S: f32 = 8.0;

/// How close a gold mine must be to one of our completed halls to be "ours"
/// for `mine_dry`. Mines are neutral and unowned; a hall is placed to work one,
/// so proximity to your own hall is the only honest definition of the mine you
/// are losing.
pub const MINE_HOME_RADIUS: f32 = 40.0;

impl TriggerWhen {
    /// The predicate as an English clause, for `Intent::sentence()` and the
    /// event feed. Reads after the word "when".
    pub fn phrase(&self) -> String {
        fn pct(frac: f32) -> String {
            format!("{}%", (frac * 100.0).round() as i32)
        }
        match self {
            TriggerWhen::BaseUnderAttack => "the base is attacked".to_string(),
            TriggerWhen::HeroBelow { frac } => {
                format!("a hero drops below {} health", pct(*frac))
            }
            TriggerWhen::HeroAbove { frac } => {
                format!("every living hero is back above {} health", pct(*frac))
            }
            TriggerWhen::SquadBelow { id, frac } => {
                format!("squad {id} drops below {} health", pct(*frac))
            }
            TriggerWhen::EnemySighted { class, count } => {
                let what = match class {
                    Some(class) => format!("enemy {class}"),
                    None => "enemies".to_string(),
                };
                if *count <= 1 {
                    format!("any {what} are sighted")
                } else {
                    format!("{count} or more {what} are sighted")
                }
            }
            TriggerWhen::EnemyIn {
                region,
                class,
                count,
            } => {
                let what = match class {
                    Some(class) => format!("enemy {class}"),
                    None => "enemies".to_string(),
                };
                if *count <= 1 {
                    format!("any {what} are seen in {region}")
                } else {
                    format!("{count} or more {what} are seen in {region}")
                }
            }
            TriggerWhen::EnemyArmySeen { size, within_s } => {
                let fresh = match within_s {
                    Some(w) => format!(" seen in the last {w:.0}s"),
                    None => String::new(),
                };
                format!("we know of {size} or more enemy troops together{fresh}")
            }
            TriggerWhen::EnemyHeroDown { class } => match class {
                Some(class) => format!("their {class} is believed dead"),
                None => "an enemy hero is believed dead".to_string(),
            },
            TriggerWhen::BountySpawned => "a bounty cache is sighted".to_string(),
            TriggerWhen::MineDry => "a mine at our base runs dry".to_string(),
            TriggerWhen::SupplyCapped => "we are supply capped".to_string(),
            TriggerWhen::TierReached { tier } => format!("we reach tier {tier}"),
            TriggerWhen::UnitCount { kind, count } => {
                format!("we field {count} or more {kind}")
            }
            TriggerWhen::GameTime { at } => format!("the clock passes {at:.0}s"),
        }
    }
}

/// One armed trigger, as the engine holds it.
///
/// `armed` and `last_fired` are the *runtime* half; everything above them is
/// what the commander said. A once-trigger disarms on firing and stays in the
/// list, spent — deleting it would make "did my rule ever fire?" unanswerable
/// from the snapshot, which is the first question anybody asks.
#[derive(Clone, Debug)]
pub struct TriggerRule {
    pub name: TriggerName,
    pub when: TriggerWhen,
    pub then: Intent,
    /// `None` ⇒ fires **once** and disarms. `Some(secs)` ⇒ **repeating**, with
    /// that many game-seconds of cooldown between fires.
    pub repeat: Option<f32>,
    /// The seat that armed it. Preserved so a fired intent is attributed to the
    /// player who authored it, never to the engine — the engine is the
    /// executor, not the author.
    pub source: IntentSource,
    /// False once a `once` trigger has spent itself.
    pub armed: bool,
    /// Game time of the last fire, `None` if it never has.
    pub last_fired: Option<f32>,
}

impl TriggerRule {
    /// May this trigger fire at `now`? Pure, so the once/cooldown rule is
    /// testable without a world.
    pub fn ready(&self, now: f32) -> bool {
        if !self.armed {
            return false;
        }
        match (self.repeat, self.last_fired) {
            // A once-trigger is disarmed on firing, so `armed` already said no;
            // this arm only matters if something re-armed it.
            (None, _) => true,
            (Some(_), None) => true,
            (Some(cooldown), Some(last)) => now - last >= cooldown,
        }
    }

    /// What the snapshot and the HUD call this trigger's state.
    pub fn status(&self, now: f32) -> &'static str {
        if !self.armed {
            return "spent";
        }
        if self.ready(now) {
            "armed"
        } else {
            "cooling"
        }
    }
}

/// Every team's armed triggers, in the order they were set.
///
/// A `Vec` rather than a map, and that is a determinism decision of the same
/// family as `SquadOrders`' `BTreeMap`: the evaluator walks this list every
/// tick and two triggers can fire on the same tick, so the order they are
/// submitted in has to be the order the commander wrote them in, identically on
/// every run. `trigger_set` replaces **in place** by name for the same reason.
#[derive(Resource, Default)]
pub struct Triggers {
    human: Vec<TriggerRule>,
    claude: Vec<TriggerRule>,
}

impl Triggers {
    pub fn get(&self, team: Team) -> &Vec<TriggerRule> {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }

    pub fn get_mut(&mut self, team: Team) -> &mut Vec<TriggerRule> {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }

    /// Create or replace by name. `Err` names why it was refused; the cap is
    /// the only way this fails once the compiler has validated the pieces.
    pub fn set(&mut self, team: Team, trigger: TriggerRule) -> Result<(), String> {
        let list = self.get_mut(team);
        if let Some(slot) = list.iter_mut().find(|t| t.name == trigger.name) {
            // Replace in place: same slot, same order, fresh runtime state.
            // Re-stating a rule re-arms it, which is what "set" means and is
            // also how a commander revives a spent once-trigger.
            *slot = trigger;
            return Ok(());
        }
        if list.len() >= MAX_TRIGGERS_PER_TEAM {
            return Err(format!(
                "you already have {MAX_TRIGGERS_PER_TEAM} triggers ({}) — \
                 clear one first, or re-use its name to replace it",
                list.iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        list.push(trigger);
        Ok(())
    }

    /// Remove one by name; `true` if anything was there.
    pub fn clear(&mut self, team: Team, name: &str) -> bool {
        let list = self.get_mut(team);
        let before = list.len();
        list.retain(|t| t.name.as_str() != name);
        list.len() != before
    }

    /// Remove every trigger of a team. Returns how many went.
    pub fn clear_all(&mut self, team: Team) -> usize {
        let list = self.get_mut(team);
        let n = list.len();
        list.clear();
        n
    }
}

// ---------------------------------------------------------------------------
// Plans: 'then' as a first-class word
// ---------------------------------------------------------------------------
//
// Doctrine is CONTINUOUS standing policy; a trigger is the CONTINGENT half.
// Neither of them can say ORDER. "Build the barracks, then when we hit tier 2
// put up the sanctum, then train sorcerers" is the single most common thing a
// commander thinks, and until now it could only be spelled by *remembering* to
// send the next command — which for a language model is one ten-to-fifteen
// second poll per step of a build order, spent on a sequence that was fully
// decided before the match started.
//
// A plan is that sequence, handed to the engine: named ordered steps, each an
// ordinary `Intent`, each with a condition for moving on to the next one. The
// engine walks it at the same 4 Hz the trigger evaluator uses and submits each
// step through the same compiler.
//
// ## Why plans are not compiled into triggers
//
// The obvious implementation is "a plan is N chained triggers". It is wrong for
// a reason worth writing down: `MAX_TRIGGERS_PER_TEAM` is 8 and it is doctrine,
// not a budget — it is the number of rules a commander can hold in their head.
// A five-step plan that ate five of those slots would make the two features
// compete for the same scarce thing while being about different halves of the
// same sentence. Plans get their own storage, their own (smaller) cap and their
// own evaluator, so arming a plan never costs a trigger and reading your
// triggers never means reading a plan's internals.
//
// ## What a plan deliberately cannot do
//
// **Loop.** A plan runs once through and stops. Repetition is the trigger's
// job (`repeat`), and a construct with both sequencing *and* iteration is a
// programming language with no debugger — the exact thing `MAX_TRIGGERS_PER_TEAM`
// exists to refuse.
//
// **Name units it does not have yet.** A step's intent is frozen at plan_set
// time, exactly like a trigger's `then`, so a step cannot say "the eight
// footmen I will have by then" — there is no id to write. The idiom, and it is
// the same one triggers already use, is the LATE-BINDING SELECTOR the language
// already has: **squads and doctrine templates**. `template` stamps every unit
// a building trains into squad 2; `posture` addresses squad 2 by number and
// resolves its membership at execution time. So a plan says "the barracks
// stamps squad 2" as step 1 and "squad 2 pushes their base" as step 4, and the
// units that obey step 4 are whoever exists when it runs. See docs/INTENT.md
// § "Plans" for the worked example.

/// A plan's name. The same scalar a trigger's name is, and for the same reason:
/// it rides inside [`Cause`], which is `Copy` and allocation-free.
///
/// An alias rather than a second type because the constraint, the parser and
/// the failure mode are identical — two structurally identical names would be
/// two chances to fix a bug in one of them.
pub type PlanName = TriggerName;

/// The most plans one team may run at once.
///
/// **Two is doctrine, not programming**, the same argument as
/// [`MAX_TRIGGERS_PER_TEAM`] one notch tighter. A plan is a *sequence* running
/// unattended, and the failure it can produce — two plans stepping over each
/// other's build sites and squad ids — is much harder to read out of a snapshot
/// than two triggers that each fire once. Two is also what commanders actually
/// want: an economic opening and a military follow-up. A third would be the
/// point where "what is my base doing" stops being answerable.
///
/// Replacing a plan by name is free and restarts it from step 1, so iterating
/// on an opening never costs a slot.
pub const MAX_PLANS_PER_TEAM: usize = 2;

/// The most steps one plan may have.
///
/// Eight, matching the trigger cap, and for the identical reason: it is the
/// length of a sequence a person can recite. A build order longer than eight
/// steps is two plans, or a plan and some triggers.
pub const MAX_PLAN_STEPS: usize = 8;

/// **When the engine moves off a step and onto the next one.**
///
/// Three arms, deliberately, and the middle one is the whole seam: it is a
/// [`TriggerWhen`], the *same* predicate vocabulary triggers use. That is not
/// code reuse for its own sake — it means a plan and a trigger agree on what
/// "we reached tier 2" means, one evaluator answers both, and any predicate a
/// later bead adds (territory held, intel gathered) becomes a plan
/// advance-condition the moment it becomes a trigger condition, with no work in
/// this file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanAdvance {
    /// The instant the step's intent is ACCEPTED by the compiler. The default,
    /// and what a bare "X, then Y" means: do this, then do that.
    ///
    /// Accepted, not *completed* — the engine does not wait for the barracks to
    /// finish, it waits for the order to be legal and taken. Waiting on
    /// completion is what the `when` form is for (`unit_count`, `tier_reached`),
    /// and conflating the two would make "then" mean something different for
    /// every verb.
    #[default]
    OnApplied,
    /// When a predicate holds. Level-triggered, exactly like a trigger: the
    /// engine checks whether it is true *now*, not whether it became true.
    When { when: TriggerWhen },
    /// After a fixed wait, measured from the moment the step's intent was
    /// accepted. The one arm about nothing in the world, present for the same
    /// reason `game_time` is a trigger predicate: "give it thirty seconds" is a
    /// thing commanders really say.
    #[serde(rename = "after")]
    AfterSeconds { secs: f32 },
}

impl PlanAdvance {
    /// The clause that introduces the NEXT step, read after "then". Empty for
    /// the default, because "then" already says it.
    pub fn phrase(&self) -> String {
        match self {
            PlanAdvance::OnApplied => String::new(),
            PlanAdvance::When { when } => format!("when {}: ", when.phrase()),
            PlanAdvance::AfterSeconds { secs } => format!("after {secs:.0}s: "),
        }
    }

    /// The same condition read as a trailing clause — what the LAST step's
    /// advance means, since it introduces no next step and instead decides when
    /// the plan is finished. `None` for the default, which needs no words.
    pub fn trailing_phrase(&self) -> Option<String> {
        match self {
            PlanAdvance::OnApplied => None,
            PlanAdvance::When { when } => Some(format!("when {}", when.phrase())),
            PlanAdvance::AfterSeconds { secs } => Some(format!("{secs:.0}s later")),
        }
    }
}

/// One step of a plan: something to do, and how to know it is time for the
/// next thing.
///
/// `advance` defaults to [`PlanAdvance::OnApplied`], so the terse form
/// `{"intent":{…}}` is a legal step and a plan of nothing but terse steps is a
/// straight-through build order.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlanStep {
    pub intent: Intent,
    #[serde(default)]
    pub advance: PlanAdvance,
}

/// Where a plan is, as one word plus (when it is bad news) the reason.
///
/// `Blocked` and `Halted` carry the compiler's own error string verbatim. A
/// status of "blocked" with no reason would send its owner to the error channel
/// to go looking, and the whole point of a plan is that nobody is watching.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanState {
    /// Walking its steps.
    Running,
    /// The current step's intent was REFUSED. The plan is retrying it at
    /// [`PLAN_RETRY_S`]; it will halt if it is still refused after
    /// [`PLAN_BLOCK_GRACE_S`].
    Blocked(String),
    /// The current step was still refused after the grace window. The plan
    /// stops here, on this step, forever — it never skips ahead.
    Halted(String),
    /// Every step ran and the last one's advance-condition came true.
    Done,
}

/// How long a blocked plan keeps retrying its refused step before giving up.
///
/// **A minute, and the number was raised from ten seconds by watching a plan
/// die.** The canonical opening in tools/COMMANDER_BRIEF.md reached its
/// `upgrade` step with 180 of the 320 gold a Keep costs, blocked correctly,
/// and then *halted* — while the income that would have paid for it was
/// twenty seconds away. Ten seconds is the right window for the reason it was
/// picked (a worker mid-walk, a building one tick from done) and the wrong one
/// for the reason plans actually exist: **economic sequencing, where the
/// dominant refusal is "not yet affordable" and money arrives on a scale of
/// tens of seconds.**
///
/// Halting late costs nothing, which is what makes the long window safe. The
/// owner is told at the *first* bounce — the status becomes
/// `blocked: <why>` within a tick and an event line goes out — so this constant
/// governs only how long the engine keeps *trying* before it stops. A commander
/// who wants it dead sooner sends `plan_clear`.
pub const PLAN_BLOCK_GRACE_S: f32 = 60.0;

/// How often a blocked plan re-submits its refused step.
///
/// Not every sweep: retrying at 4 Hz would put 240 copies of the same refusal
/// into the error channel and the replay log inside the grace window, and a
/// channel that floods is a channel nobody reads. Five seconds gives twelve
/// attempts across the window — enough that a step waiting on income retries
/// several times per mining trip, few enough to read.
pub const PLAN_RETRY_S: f32 = 5.0;

/// One plan, as the engine holds it.
///
/// Everything above `state` is what the commander said; everything from `state`
/// down is runtime. A finished plan stays in the list, `Done`, for the same
/// reason a spent trigger does: "did my opening actually run?" has to be
/// answerable from the snapshot, and an absence cannot answer it.
#[derive(Clone, Debug)]
pub struct PlanRun {
    pub name: PlanName,
    pub steps: Vec<PlanStep>,
    /// The seat that set it. The AUTHOR — the engine only executes.
    pub source: IntentSource,
    pub state: PlanState,
    /// Index of the step being worked. Stays pinned at the last index when the
    /// plan is `Done` or `Halted`, so `step k/n` always names a real step.
    pub at: usize,
    /// Has the current step's intent been submitted yet this visit?
    pub submitted: bool,
    /// Has the current step's intent been ACCEPTED? Gates
    /// [`PlanAdvance::OnApplied`] and starts the `after` clock.
    pub applied: bool,
    /// Game time the current step's intent was accepted.
    pub applied_at: f32,
    /// Game time of the last submit attempt, so a blocked step retries on
    /// [`PLAN_RETRY_S`] rather than every sweep.
    pub last_try: f32,
    /// Game time the current step first bounced, `None` if it has not.
    pub blocked_since: Option<f32>,
    /// Has the owner been told about the current block?
    ///
    /// One notice per block, not one per retry — the human's alert stack is six
    /// rows and twelve copies of "not enough gold" would push the match's
    /// actual news off it, and a bridge commander's `bridge_wait` treats a
    /// fresh line as a reason to stop sleeping.
    ///
    /// It doubles as **"an unblock line is owed"**, which is why
    /// [`Plans::report`] does not clear it on acceptance. A plan that announced
    /// a block and is now `Running` again has exactly one thing left to say,
    /// and the flag that remembers the announcement is the same flag that
    /// remembers the debt. The alternative — a second bool that is always the
    /// first one's shadow — would let the two disagree.
    pub told_blocked: bool,
}

/// What [`Plans::report`] did with a verdict, so the compiler knows whether
/// the refusal it is holding is **news**.
///
/// This exists because "the step bounced" and "the step is still bouncing" are
/// different events that produce an identical error string, and every channel
/// downstream — the seat's `errors` array, the replay log, the alert stack —
/// wants only the first. Arena round 17 is the cost of not distinguishing them:
/// a step blocked on `cannot afford Footman` re-emitted its refusal every
/// [`PLAN_RETRY_S`], `bridge_wait` woke on each one, and the commander escaped
/// the resulting fire hose by chaining waits and lost the match in the gap.
///
/// The plan's [`PlanRun::status`] carries the condition continuously in the
/// snapshot, so nothing is hidden by staying quiet between transitions — the
/// reader can always see `blocked: <why>`; it just is not *told* twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanVerdict {
    /// No live plan is waiting on this stamp — a plan replaced mid-flight, a
    /// verdict for a step already left behind, or a halted plan's late news.
    Ignored,
    /// Accepted, and it was not blocked. The ordinary case, and silent.
    Accepted,
    /// Accepted, and it HAD been blocked. Worth one line: the commander was
    /// told the plan stopped and is owed the news that it started again.
    Unblocked,
    /// Refused, and this is news — the first bounce on this step, or a
    /// *different* reason from the one already announced. "cannot afford it"
    /// becoming "the site is blocked" is a different problem with a different
    /// answer.
    Blocked,
    /// Refused again, with the identical reason. **Not news**, and the one
    /// verdict every emission channel drops on the floor.
    BlockedAgain,
}

impl PlanRun {
    /// `k` of `step k/n`, one-based, as a person counts.
    pub fn step_no(&self) -> usize {
        (self.at + 1).min(self.steps.len())
    }

    /// The step the plan is on, or the last one once it is finished.
    pub fn current(&self) -> Option<&PlanStep> {
        self.steps.get(self.at.min(self.steps.len().saturating_sub(1)))
    }

    /// What the snapshot and the HUD call this plan's state. Blocked and halted
    /// carry their reason — the word alone would be a status you have to go
    /// research.
    pub fn status(&self) -> String {
        match &self.state {
            PlanState::Running => "running".to_string(),
            PlanState::Blocked(why) => format!("blocked: {why}"),
            PlanState::Halted(why) => format!("halted: {why}"),
            PlanState::Done => "done".to_string(),
        }
    }

    /// Is this plan still going to do anything?
    pub fn live(&self) -> bool {
        matches!(self.state, PlanState::Running | PlanState::Blocked(_))
    }
}

/// Every team's plans, in the order they were set.
///
/// A `Vec` for the same determinism reason [`Triggers`] is one: two plans can
/// step on the same tick and the order they submit in has to be the order they
/// were written, on every run.
#[derive(Resource, Default)]
pub struct Plans {
    human: Vec<PlanRun>,
    claude: Vec<PlanRun>,
}

impl Plans {
    pub fn get(&self, team: Team) -> &Vec<PlanRun> {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }

    pub fn get_mut(&mut self, team: Team) -> &mut Vec<PlanRun> {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }

    /// Create or replace by name. Replacing restarts from step 1 — a re-stated
    /// plan is a new plan with the same label, which is what "set" means
    /// everywhere else in this language and is how a commander iterates on an
    /// opening without spending the other slot.
    ///
    /// The cap counts plans that are still LIVE. A `done` or `halted` plan is
    /// history rather than policy, and holding a slot open for a sequence that
    /// finished four minutes ago would make the cap about bookkeeping instead
    /// of about how much is running unattended. Finished plans stay readable in
    /// the list; the oldest one is evicted to make room.
    pub fn set(&mut self, team: Team, plan: PlanRun) -> Result<(), String> {
        // A plan with no steps can never finish — nothing to submit, nothing to
        // advance — so it would sit `running` and hold one of the two slots for
        // the rest of the match. The compiler refuses this with a better
        // message; this is the same refusal at the door of the store, so no
        // caller can install one by construction.
        if plan.steps.is_empty() {
            return Err("a plan needs at least one step".to_string());
        }
        let list = self.get_mut(team);
        if let Some(slot) = list.iter_mut().find(|p| p.name == plan.name) {
            *slot = plan;
            return Ok(());
        }
        if list.iter().filter(|p| p.live()).count() >= MAX_PLANS_PER_TEAM {
            return Err(format!(
                "you already have {MAX_PLANS_PER_TEAM} plans running ({}) — \
                 clear one first, or re-use its name to replace it",
                list.iter()
                    .filter(|p| p.live())
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // Room for the new one; drop the oldest finished plan if the list has
        // grown past what a reader can hold.
        while list.len() >= MAX_PLANS_PER_TEAM * 2 {
            let Some(oldest) = list.iter().position(|p| !p.live()) else {
                break;
            };
            list.remove(oldest);
        }
        list.push(plan);
        Ok(())
    }

    /// **The compiler's verdict on a step it just compiled**, routed back to
    /// the plan that submitted it.
    ///
    /// This is the whole of plans' failure semantics and it is why
    /// [`SubmitIntent::plan`] exists rather than plan.rs scraping the error
    /// channel for its own tag. A plan that could not tell "accepted" from
    /// "refused" would have exactly two options, and both are bad: advance
    /// anyway (silently skipping the step, which is the failure mode this whole
    /// design is here to refuse) or never advance (wedging on the first hiccup).
    ///
    /// `error` is the compiler's own first message for that step, `None` if it
    /// was accepted. Verdicts for a step the plan has already moved off are
    /// ignored — a plan replaced mid-flight must not inherit the old one's news.
    ///
    /// Returns [`PlanVerdict`] — whether this verdict was **news**. The caller
    /// is the intent compiler, and what it does with the answer is decide
    /// whether the error it is holding reaches the seat's channels at all. A
    /// step refused for the twelfth time with the same words has already been
    /// reported; saying it again is not honesty, it is a fire hose (arena r17).
    pub fn report(
        &mut self,
        team: Team,
        stamp: PlanStamp,
        error: Option<String>,
        now: f32,
    ) -> PlanVerdict {
        let Some(plan) = self
            .get_mut(team)
            .iter_mut()
            .find(|p| p.name == stamp.name)
        else {
            return PlanVerdict::Ignored;
        };
        // Three conditions, and the third is the one that makes "replaced
        // mid-flight" safe. `plan_set` is compiled in `SimSet::Intent`, the
        // same set as this verdict and ahead of it in the batch whenever the
        // replacement came from a seat — so a fresh `PlanRun` can be sitting
        // under the old one's name when the old step's verdict arrives. Name
        // and step number do NOT tell them apart (both are "opening", both are
        // on step 1). `submitted` does, and unconditionally: a plan that has
        // sent nothing cannot be the addressee of a verdict, and `Plans::set`
        // always installs a replacement with `submitted: false`.
        if !plan.submitted || plan.step_no() != stamp.step as usize || !plan.live() {
            return PlanVerdict::Ignored;
        }
        match error {
            None => {
                let was_blocked = matches!(plan.state, PlanState::Blocked(_));
                plan.applied = true;
                plan.applied_at = now;
                plan.blocked_since = None;
                plan.state = PlanState::Running;
                // `told_blocked` is deliberately NOT cleared here — see its
                // doc. It is now the plan's memory that it owes an "unblocked"
                // line, and plan.rs clears it when that line goes out.
                if was_blocked {
                    PlanVerdict::Unblocked
                } else {
                    PlanVerdict::Accepted
                }
            }
            Some(why) => {
                // The clock starts at the FIRST bounce, not this one, so the
                // grace window measures how long the step has been impossible
                // rather than how long since the last retry.
                plan.blocked_since.get_or_insert(now);
                plan.applied = false;
                // A refusal that CHANGES is news. "cannot afford it" becoming
                // "the site is blocked" is a different problem with a different
                // answer, and staying quiet about the second would leave the
                // owner acting on the first right up until the plan halted on
                // the other one.
                //
                // The same comparison now answers the caller's question too:
                // an identical refusal is the retry cadence talking, not the
                // world, and it stops here.
                let same = plan.state == PlanState::Blocked(why.clone());
                if !same {
                    plan.told_blocked = false;
                }
                plan.state = PlanState::Blocked(why);
                if same {
                    PlanVerdict::BlockedAgain
                } else {
                    PlanVerdict::Blocked
                }
            }
        }
    }

    /// Remove one by name; `true` if anything was there.
    pub fn clear(&mut self, team: Team, name: &str) -> bool {
        let list = self.get_mut(team);
        let before = list.len();
        list.retain(|p| p.name.as_str() != name);
        list.len() != before
    }

    /// Remove every plan of a team. Returns how many went.
    pub fn clear_all(&mut self, team: Team) -> usize {
        let list = self.get_mut(team);
        let n = list.len();
        list.clear();
        n
    }
}

/// Which plan and which step an intent came from, as a `Copy` scalar so it can
/// ride in [`Cause`] and in [`SubmitIntent`] without an allocation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlanStamp {
    pub name: PlanName,
    /// One-based, as `step k/n` reads.
    pub step: u8,
    pub of: u8,
}

impl std::fmt::Display for PlanStamp {
    /// `opening step 2/5` — the phrase every channel uses, defined once.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} step {}/{}", self.name, self.step, self.of)
    }
}

// ---------------------------------------------------------------------------
// Late-bound references: saying WHO and WHERE without freezing an entity id
// ---------------------------------------------------------------------------
//
// An entity id is a fact about one instant. `{"units":[4294967297]}` names the
// hero that was alive when the sentence was written, and a sentence the engine
// stores and runs later — a trigger's `then`, a plan step — is exactly the
// place where that instant has already passed. Arena rounds r21–r23 traced five
// distinct failure classes to it: a hero-save trigger armed with `"units":[]`
// because the hero had not been trained yet; dead hero ids in triggers; stale
// unit lists in `priority`; a memorized tree chopped out from under a harvest
// order; and the wrong worker frozen into a repeating trigger. Every one of
// them is the same bug — an author-time id used at fire time.
//
// A [`Selector`] is the fix and it is deliberately the same shape as a region
// name. `"region":"north-pass"` has always been a late-bound *place*: the name
// travels in the stored intent and becomes coordinates when the intent is
// compiled, so moving the region re-aims every rule that names it. A selector
// is a late-bound *role*, on the identical footing, resolved at the identical
// moment: [`intent::resolve_places`], at the top of the one compiler, which a
// trigger reaches only when it FIRES.
//
// The vocabulary is small on purpose. Roles a commander already thinks in
// (my hero, all army, workers, squad N), and the two "nearest X" phrases that
// answer the questions ids answered badly (which tree, which build site).

/// A role, a squad, or a nearest-thing — written as a phrase, resolved against
/// the world at the moment the intent is compiled.
///
/// Parsed by [`parse_selector`]; the channels each verb accepts it on are
/// listed on [`Intent`]. Which selectors are legal in which channel is decided
/// by the resolver, so `{"units": …, "select":"nearest tree"}` earns a sentence
/// explaining that a tree is not an army rather than a silent miss.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selector {
    /// Every living hero of the seat. Plural because hero slots scale with the
    /// hall ladder — "my hero" said by a Keep team with two of them means both,
    /// and the channels that can only take one (a cast's caster) say so.
    Heroes,
    /// Every living non-worker of the seat, heroes included. What a commander
    /// means by "all army": the things that fight.
    Army,
    /// Every living unit of the seat, workers included.
    AllUnits,
    /// Every living worker of the seat.
    Workers,
    /// The CURRENT members of squad `n` — the whole point of the selector, and
    /// the answer to red-r23's stale rosters. A unit trained into the squad
    /// after the trigger was armed is in it; a unit that died is not.
    ///
    /// One caveat, and it is the standing one about `Commands`: the `squad`
    /// verb enrols members through deferred commands, so a `squad` and a
    /// `select: "squad 1"` in the SAME batch do not see each other. The second
    /// sentence earns the ordinary "matches none of your units right now"
    /// refusal rather than acting on a stale roster, which is the safe half of
    /// the trade; the fix is to send the two in successive batches, or to
    /// address the squad by number with `posture`, which never needed the
    /// roster at all.
    Squad(u8),
    /// The nearest living tree to the units being ordered.
    NearestTree,
    /// The nearest un-exhausted gold mine to the units being ordered.
    NearestMine,
    /// The nearest legal footprint to the point the sentence names. Not a unit
    /// and not a node — the one selector that answers "where", and the answer
    /// to blue-r23's farm trigger that reported `site blocked` all match
    /// without ever moving the site.
    NearestLegalSite,
}

impl Selector {
    /// The canonical phrase, as the replay log and every error print it.
    pub fn phrase(&self) -> String {
        match self {
            Selector::Heroes => "my hero".to_string(),
            Selector::Army => "all army".to_string(),
            Selector::AllUnits => "all units".to_string(),
            Selector::Workers => "workers".to_string(),
            Selector::Squad(n) => format!("squad {n}"),
            Selector::NearestTree => "nearest tree".to_string(),
            Selector::NearestMine => "nearest mine".to_string(),
            Selector::NearestLegalSite => "nearest legal site".to_string(),
        }
    }

    /// Does this selector name a set of the seat's own units?
    pub fn is_unit_selector(&self) -> bool {
        matches!(
            self,
            Selector::Heroes
                | Selector::Army
                | Selector::AllUnits
                | Selector::Workers
                | Selector::Squad(_)
        )
    }

    /// Does this selector name a resource node?
    pub fn is_node_selector(&self) -> bool {
        matches!(self, Selector::NearestTree | Selector::NearestMine)
    }
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.phrase())
    }
}

/// The unit-naming half of the vocabulary, for error messages that teach.
pub const SELECTOR_UNIT_NAMES: &str = "my hero, all army, all units, workers, squad <n>";
/// The node-naming half.
pub const SELECTOR_NODE_NAMES: &str = "nearest tree, nearest mine";
/// The site-naming half — `build`'s `"site"`, and nowhere else. Third const
/// rather than a literal at the one call site so `Catalog::selectors` can serve
/// all three channels as splits of the same words the refusals print.
pub const SELECTOR_SITE_NAMES: &str = "nearest legal site";
/// Everything a selector phrase may be. Printed by the unknown-phrase refusal,
/// on the same rule as `Regions::unknown`: a refusal that names no alternative
/// is a refusal to help.
pub const SELECTOR_NAMES: &str =
    "my hero, all army, all units, workers, squad <n>, nearest tree, nearest mine, \
     nearest legal site";

/// Parse a selector phrase, or `None` if it is not one.
///
/// Folded through [`normalize_place`] rather than [`normalize_name`]: a
/// selector is a phrase with word boundaries that matter (`squad 1`), and
/// stripping the space would leave `squad1` needing its own parser. Case,
/// dashes and underscores are noise exactly as they are everywhere else on this
/// wire, so `"My Hero"`, `"my_hero"` and `"my hero"` are one phrase.
pub fn parse_selector(raw: &str) -> Option<Selector> {
    let folded = normalize_place(raw);
    // `squad 3`, and the two spellings a commander reaches for when it forgets
    // the space. Bounded by u8 like every other squad id on this wire.
    if let Some(rest) = folded
        .strip_prefix("squad ")
        .or_else(|| folded.strip_prefix("squad:"))
        .or_else(|| folded.strip_prefix("squad"))
    {
        if let Ok(n) = rest.trim().parse::<u8>() {
            return Some(Selector::Squad(n));
        }
    }
    Some(match folded.as_str() {
        "my hero" | "hero" | "heroes" | "my heroes" | "our hero" | "our heroes" => Selector::Heroes,
        "all army" | "army" | "my army" | "our army" | "all my army" | "the army" => Selector::Army,
        "all units" | "all" | "everything" | "my units" | "our units" | "everyone" => {
            Selector::AllUnits
        }
        "workers" | "worker" | "all workers" | "my workers" | "our workers" => Selector::Workers,
        "nearest tree" | "nearest trees" | "nearest lumber" | "nearest wood" | "tree" => {
            Selector::NearestTree
        }
        "nearest mine" | "nearest gold" | "nearest gold mine" | "mine" => Selector::NearestMine,
        "nearest legal site" | "nearest legal" | "nearest free site" | "nearest free"
        | "nearest site" | "auto" => Selector::NearestLegalSite,
        _ => return None,
    })
}

/// The refusal an unrecognised selector phrase earns. One wording, so every
/// channel teaches the same lesson — the [`Regions::unknown`] rule applied to
/// roles instead of places.
pub fn unknown_selector(raw: &str) -> String {
    format!("unknown selector '{raw}' — known selectors: {SELECTOR_NAMES}")
}

/// Everything a player can mean.
///
/// Grouped by what it is for: unit orders, production, the doctrine layer that
/// runs at machine speed for whoever set it, abilities and items, and the three
/// match-level statements. Adding a verb here is adding it to *both* seats at
/// once, which is the point.
///
/// **Two late-binding channels run through this vocabulary**, and both are
/// optional keys that outrank the frozen form beside them:
///
///   * `"region": "<name>"` wherever `x`/`z` appear — the place channel, which
///     has always been here.
///   * `"select": "<phrase>"` wherever `units`, `worker` or a cast's `hero`
///     appear — the role channel. See [`Selector`]. Plus `"target_select"` for
///     the node a `harvest` gathers and the unit a `follow` follows, and
///     `"site"` for `build`'s footprint.
///
/// Both resolve in `intent::resolve_places`, which runs at the top of the one
/// compiler — so a trigger's `then` and a plan's step resolve when they FIRE,
/// against the world that fired them.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Intent {
    // --- unit orders ---
    /// Walk to a point. **Every verb below that takes `x`/`z` also takes
    /// `"region": "<name>"` instead** — see [`Regions`] for what a name is, and
    /// `intent::resolve_places` for the single point at which one becomes a
    /// coordinate. `x`/`z` are `Option` for exactly that reason: "no place at
    /// all" is now a thing a sentence can say, and it earns a refusal that
    /// names both spellings rather than serde's "missing field x".
    Move {
        /// Frozen ids. `#[serde(default)]` so a sentence may name its units by
        /// `select` alone and omit this key entirely — the array still
        /// serializes exactly as it always did, so nothing on the wire moved.
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        /// The role channel. See [`Selector`]. Given, it OUTRANKS `units`, on
        /// the same rule that makes a region outrank the coordinates beside it.
        /// Last in the struct because new wire keys are appended, never
        /// interleaved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    AttackMove {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    Attack {
        #[serde(default)]
        units: Vec<IntentId>,
        target: IntentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// Gold mines and trees alike; workers only.
    ///
    /// `target_select` — `"nearest tree"` or `"nearest mine"` — is the answer to
    /// the memorized-tree bug: the node is chosen when the order is COMPILED,
    /// measured from the workers being sent, so a repeating trigger that says
    /// "idle workers, nearest tree" keeps working after the tree it would have
    /// frozen is felled.
    Harvest {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_select: Option<String>,
    },
    Return {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    Follow {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
        /// A unit selector naming the leader. Resolves to ONE unit — the lowest
        /// entity id among the matches — so `"my hero"` escorts the hero this
        /// team has *now*, not the one it had when the trigger was armed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_select: Option<String>,
    },
    /// Halt in place and drop any attack target.
    Stop {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },

    // --- production ---
    /// `select` names the worker by role instead of by id and resolves to ONE —
    /// the lowest entity id among the matches — which is what kills the "wrong
    /// worker frozen into a repeating trigger" class.
    ///
    /// `site: "nearest legal site"` lets the engine move the footprint to the
    /// nearest legal one within [`crate::intent::PLACEMENT_HINT_RADIUS`] of the
    /// point named, instead of refusing. Blue-r23 armed a farm trigger on fixed
    /// coordinates, watched it report `site blocked` every retry for the whole
    /// match, and never got the farm; the hint the refusal already computed was
    /// right there and nothing could accept it.
    Build {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker: Option<IntentId>,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        site: Option<String>,
    },
    Train {
        building: IntentId,
        unit: String,
    },
    /// Convert one of our own finished buildings into its next tier IN PLACE
    /// (`catalog.buildings[].upgrades_to`). Paid in full the moment it is
    /// accepted; the building keeps its position, footprint, rally and
    /// training queue, but trains nothing until the conversion finishes.
    Upgrade {
        building: IntentId,
    },
    Cancel {
        building: IntentId,
        index: usize,
    },
    /// Start the next rung of a team-wide research ladder at one of our own
    /// finished Blacksmiths (`catalog.research`). `upgrade` is a ladder id —
    /// `"attack"` or `"armor"`, parsed like every other name in the protocol
    /// (case, spaces, dashes and underscores are noise).
    ///
    /// Paid in full the moment it is accepted, exactly like `upgrade`. The
    /// level it produces is always *current + 1*: a commander cannot name a
    /// level, because skipping rungs is not a thing the game can do and
    /// accepting a number that has only one legal value is a way to spell it
    /// wrong. Refused if the forge is already researching something — one job
    /// per Blacksmith, and requests are rejected rather than queued (see
    /// `Researching`).
    Research {
        building: IntentId,
        upgrade: String,
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
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentId>,
    },

    // --- abilities & items ---
    /// Cast one of the caster's abilities. The caster is a hero or one of our
    /// own finished ability buildings (the TownHall's Call to Arms). `hero` is
    /// the historical field name; `caster` says what it really means now.
    ///
    /// `ability` picks a slot — the integer index or the ability id (`"Slam"`,
    /// case-insensitive). OMIT IT for the caster's first unlocked ability, so
    /// `{"type":"cast","hero":123}` means exactly what it always meant. The UI
    /// is index-native (each hotkey is a slot); the bridge speaks both.
    ///
    /// **Where it lands** depends on the ability's `catalog.abilities[].target`:
    ///
    ///   * `"caster"` — centred on the caster. Send nothing else; `x`/`z`/
    ///     `target` are ignored, so an old-form `cast` is still exactly what it
    ///     always was.
    ///   * `"point"` — send `x` and `z` for a ground point within
    ///     `target_range` of the caster.
    ///   * `"unit"` — send `target`, the id of the unit to cast it on, again
    ///     within `target_range`.
    ///
    /// **Omit the payload on a targeted ability and the engine aims it**: the
    /// centre that catches the most bodies the ability would affect, among
    /// those the caster can reach, nearest one winning ties. That is the same
    /// rule an auto-cast Sorcerer uses, so `{"type":"cast","caster":7,
    /// "ability":"Slow"}` is a legal and sensible sentence — you are saying
    /// "slow them", not "slow nothing".
    ///
    /// Out of range is REFUSED with an error naming both distances, not walked
    /// into: a caster that closed the gap by itself would undo the reason
    /// targeted casting exists.
    Cast {
        #[serde(alias = "caster", default, skip_serializing_if = "Option::is_none")]
        hero: Option<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ability: Option<AbilitySelector>,
        /// Ground point for a `"point"` ability. Both `x` and `z` or neither.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        /// Victim/beneficiary for a `"unit"` ability.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<IntentId>,
        /// A unit selector naming the CASTER, resolved when the cast compiles
        /// and narrowed to one — the lowest entity id among the matches. This
        /// is the one that kills red-r23's dead-hero-id trigger: a hero-save
        /// rule armed as `"select":"my hero"` still finds the hero after it has
        /// died and been revived into a new entity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// Buy a consumable at one of our own finished Shops.
    ///
    /// `hero` names WHICH of your heroes is buying. It used to be implied,
    /// because a team had at most one — but hero slots scale with the hall
    /// ladder now (`hero_slots`), so a Keep team fielding a Champion AND a
    /// Priestess had a coin-flip about which inventory a potion landed in.
    /// Omit it and the tie-break is documented and stable: **the living hero
    /// with the lowest entity id**, which is the same hero every existing
    /// one-hero call site already got. Back-compatible by construction.
    Buy {
        shop: IntentId,
        item: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hero: Option<IntentId>,
    },
    /// Consume the item in one of a hero's inventory slots. `hero` picks which
    /// hero's bag, with the same default as `Buy`.
    ///
    /// `destination` is for the two teleport items and names WHICH of your own
    /// standing halls to arrive at. Omit it and both scrolls fall back to the
    /// hall nearest the hero, which is what they always did — so every
    /// `use_item` written before this field means exactly what it used to.
    /// Naming a building that is not your own finished hall is an error, not a
    /// quiet fall-back: "the scroll went somewhere else" is the whole bug the
    /// field exists to prevent.
    #[serde(rename = "use_item")]
    UseItem {
        slot: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hero: Option<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination: Option<IntentId>,
    },

    // --- doctrine: standing policy, executed by the engine at machine speed ---
    /// Focus-fire order. An empty/omitted `classes` clears the policy.
    Priority {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default)]
        classes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// Break off below `below` (a fraction in the open range 0..1) and fall
    /// back to x/z. `below` omitted, null, or 0 clears the policy.
    Retreat {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        below: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// Anchor to x/z within `radius`. `radius <= 0` clears the policy.
    ///
    /// Given a `region` and no `radius`, the REGION'S OWN radius is the leash —
    /// which is the whole point of naming a circle: "hold the perimeter" should
    /// not also require remembering how big you said the perimeter was. An
    /// explicit `radius` still wins.
    Leash {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// Heroes only. `min_enemies` omitted, null, or 0 clears the rule.
    /// `ability` names the slot the rule governs; omitted, it means the first
    /// slot, which is what it always meant. Rules are per-slot: a hero told to
    /// auto-heal does not thereby stop auto-slamming.
    Autocast {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_enemies: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ability: Option<AbilitySelector>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// Squad membership. `id` omitted or null removes the units from any squad.
    Squad {
        #[serde(default)]
        units: Vec<IntentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select: Option<String>,
    },
    /// What a squad is for. `posture` omitted or null clears the entry, which
    /// leaves the members where they are without disbanding the squad.
    Posture {
        id: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        posture: Option<PostureIntent>,
    },
    /// **One word for a whole doctrine.** Put a squad into one of the five
    /// engine-defined stances — `turtle`, `stage`, `push`, `secure`, `harass` —
    /// each a fixed bundle of posture + anchor + leash + retreat threshold +
    /// focus priority, taken from `assets/data/stances.ron`.
    ///
    /// It installs exactly what the four verbs above install, through the same
    /// appliers: there is no stance machinery in the executor, and doctrine.rs
    /// cannot tell a stanced squad from a hand-tuned one. What the stance adds
    /// is a *name* — one that persists in the snapshot, so silence continues it
    /// rather than dissolving it, and one sentence that replaces the bundle
    /// atomically rather than four that can half-land.
    ///
    /// `stance` is a `String` rather than a typed tag so an unknown word earns
    /// a refusal naming all five, instead of serde's "unknown variant" killing
    /// the whole command. Same reason `priority` takes class names as strings.
    ///
    /// The anchor is `x`/`z`, or a place name in `region` — spelled `target` on
    /// the wire as well, because "which stance, where" reads better with a
    /// target than with a region and both spellings resolve identically. Omit
    /// the anchor entirely and it is the team's own base, which is what
    /// `turtle` means anyway.
    ///
    /// **A region names WHERE, never HOW BIG.** Unlike `posture defend`, a named
    /// region's own radius does not become the ring: the ring is the stance's,
    /// because a preset whose numbers moved with its target would not be a
    /// preset. A commander who wants a custom ring still has `posture`.
    Stance {
        #[serde(alias = "id")]
        squad: u8,
        stance: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, alias = "target", skip_serializing_if = "Option::is_none")]
        region: Option<String>,
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

    // --- triggers: contingent standing policy ---
    /// **Arm a trigger.** Create it, or replace an existing one of the same
    /// name in place.
    ///
    /// `then` is any other intent, and that is the whole design: a trigger adds
    /// no second vocabulary, it defers the one that already exists. When the
    /// predicate holds, the engine submits `then` through this same compiler,
    /// attributed to the seat that armed it, exempt from the command link
    /// (docs/TEMPO.md) because it is engine-executed standing policy — the same
    /// row `posture` and `retreat` are on, and for the same reason.
    ///
    /// `repeat` omitted or null ⇒ fires ONCE and disarms. A number ⇒ repeating,
    /// with that many game-seconds of cooldown between fires.
    ///
    /// A trigger may not arm another trigger: `then` must not itself be
    /// `trigger_set` or `trigger_clear`. That refusal is what keeps this
    /// doctrine rather than a programming language, and it is what makes
    /// `MAX_TRIGGERS_PER_TEAM` an actual bound.
    #[serde(rename = "trigger_set")]
    TriggerSet {
        name: String,
        when: TriggerWhen,
        then: Box<Intent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat: Option<f32>,
    },
    /// **Disarm a trigger.** `name` omitted clears every trigger this team has
    /// — the whole-slate form the human's one-key gesture needs, and cheap to
    /// undo since re-arming is one sentence.
    #[serde(rename = "trigger_clear")]
    TriggerClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },

    // --- territory: the ground, given names ---
    /// **Name a circle of ground.** Create it, or move an existing one of the
    /// same name in place.
    ///
    /// From then on every verb that takes `x`/`z` takes `"region":"<name>"`
    /// instead, and the compiler resolves it at submit time. A region is
    /// private doctrine: it appears in its owner's snapshot only, and naming
    /// ground tells the enemy nothing.
    ///
    /// Refused if the name is a built-in place, if the team already holds
    /// [`MAX_REGIONS_PER_TEAM`] under other names, or if the radius is outside
    /// [`REGION_RADIUS_MIN`]..[`REGION_RADIUS_MAX`].
    #[serde(rename = "region_set")]
    RegionSet {
        name: String,
        x: f32,
        z: f32,
        radius: f32,
    },
    /// **Forget a region.** `name` omitted clears every region this team named
    /// — the whole-slate form, matching `trigger_clear`. Built-ins are map
    /// facts and are never cleared by this.
    #[serde(rename = "region_clear")]
    RegionClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    // --- plans: sequenced standing policy ---
    /// **Set a plan.** Named ordered steps the engine walks for you, one at a
    /// time, submitting each step's intent through this same compiler when its
    /// turn comes. Create it, or replace an existing one of the same name —
    /// which restarts it from step 1.
    ///
    /// Each step is `{"intent": <any intent>, "advance": <how to move on>}`,
    /// and `advance` may be omitted for the ordinary meaning of "then": as soon
    /// as this step is accepted, do the next one. The other two forms are
    /// `{"type":"when","when":{…}}` — any [`TriggerWhen`] predicate — and
    /// `{"type":"after","secs":30}`.
    ///
    /// Bounded at [`MAX_PLANS_PER_TEAM`] live plans of [`MAX_PLAN_STEPS`] steps
    /// each, on the same argument as the trigger cap: a sequence running
    /// unattended has to be one its owner can recite.
    ///
    /// **Once through, never looping.** Repetition is what a trigger's `repeat`
    /// is for. A step may arm a `trigger_set` — that is a real idiom ("build
    /// the barracks, then arm the home guard") and stays bounded because
    /// `trigger_set` still cannot arm anything — but a step may not be a
    /// `plan_set` or `plan_clear`, and neither may a trigger's `then`. Those
    /// two refusals are what make the caps actual bounds rather than starting
    /// balances.
    ///
    /// Every step's intent is frozen at set time, exactly like a trigger's
    /// `then`. To act on units that do not exist yet, name a SQUAD: see the
    /// module comment above [`PlanAdvance`] and docs/INTENT.md § "Plans".
    #[serde(rename = "plan_set")]
    PlanSet { name: String, steps: Vec<PlanStep> },
    /// **Drop a plan.** `name` omitted drops every plan this team has, finished
    /// ones included.
    #[serde(rename = "plan_clear")]
    PlanClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },

    // --- match level ---
    /// Hand this faction to the scripted AI (or take it back).
    Autopilot {
        on: bool,
    },
    /// Concede: the opponent wins immediately.
    Surrender,
    /// **I have read the map and I am ready to begin.** The one verb that is
    /// legal *before* the match exists — see [`ReadyGate`]. Idempotent: saying
    /// it twice is saying it once, and saying it after the clock has started is
    /// a no-op rather than an error, because a commander that reconnects and
    /// re-sends its opening should not be punished for a stale first line.
    Ready,
}

/// The `retreat` piece of a `template` intent.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct RetreatIntent {
    pub below: f32,
    pub x: f32,
    pub z: f32,
}

/// The inner object of a `posture` intent.
///
/// **Three of the four take a region**, and each mapping is stated here rather
/// than left to be discovered from the compiler:
///
///   * `defend` — the region IS the ring. Centre becomes the anchor, and the
///     region's own radius becomes the defend radius unless `radius` is given
///     explicitly. "Squad 2 defends north-pass" is then one sentence with no
///     numbers in it, which is the entire feature.
///   * `push` — push to the region's CENTRE. A push is a direction, not an
///     area; the radius is deliberately dropped, and dropping it silently is
///     fine because `push` has never had a radius to confuse it with.
///   * `forage` — the region's centre is the muster point held while no bounty
///     is up. Radius dropped, same reasoning: foraging is bounded by where the
///     caches are, not by where you were standing.
///   * `escort` — names a unit, not ground. No region form, and there should
///     not be one: a region that followed a hero would be a second, moving
///     vocabulary for the same word.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PostureIntent {
    Defend {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
        /// Optional ONLY when a region supplies it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
    },
    Push {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
    Escort {
        unit: IntentId,
    },
    /// Hunt bounty caches; x/z (or a region's centre) is the muster point held
    /// while none exist.
    Forage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        z: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
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
            Intent::Upgrade { .. } => "upgrade",
            Intent::Cancel { .. } => "cancel",
            Intent::Research { .. } => "research",
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
            Intent::Stance { .. } => "stance",
            Intent::Template { .. } => "template",
            Intent::TriggerSet { .. } => "trigger_set",
            Intent::TriggerClear { .. } => "trigger_clear",
            Intent::RegionSet { .. } => "region_set",
            Intent::RegionClear { .. } => "region_clear",
            Intent::PlanSet { .. } => "plan_set",
            Intent::PlanClear { .. } => "plan_clear",
            Intent::Autopilot { .. } => "autopilot",
            Intent::Surrender => "surrender",
            Intent::Ready => "ready",
        }
    }

    /// The verb this intent stamps into its targets' [`Provenance`], or `None`
    /// for the intents that install policy, spend money or end the match
    /// rather than changing what a unit is doing *right now*.
    ///
    /// The split is exactly the one a unit's "why" answer needs. `move` is a
    /// reason to be walking somewhere; `retreat` is not a reason to be doing
    /// anything yet — it becomes one only when doctrine.rs fires it, and
    /// doctrine.rs stamps `policy:retreat` at that moment instead.
    pub fn provenance_verb(&self) -> Option<&'static str> {
        match self {
            Intent::Move { .. }
            | Intent::AttackMove { .. }
            | Intent::Attack { .. }
            | Intent::Harvest { .. }
            | Intent::Return { .. }
            | Intent::Follow { .. }
            | Intent::Stop { .. }
            | Intent::Build { .. } => Some(self.verb()),
            _ => None,
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
        /// How a sentence names ground. **A named place is spoken as its
        /// name**, and that is most of why regions exist: the replay line for a
        /// defended ford should read "defends north-pass", not "defends
        /// (-60.0, 60.0)". Falls back to the coordinates when no name was used,
        /// and says so plainly when a sentence names no ground at all — that
        /// last case is a refusal being described, and describing it as
        /// "(0.0, 0.0)" would put the map centre in the log.
        fn place(x: &Option<f32>, z: &Option<f32>, region: &Option<String>) -> String {
            match (region, x, z) {
                (Some(name), _, _) => name.clone(),
                (None, Some(x), Some(z)) => at(*x, *z),
                _ => "(unspecified)".to_string(),
            }
        }
        /// The same, for the **one** place-taking field in the vocabulary that
        /// has a default: a stance's anchor. Omit it and `compile_intent`
        /// anchors the stance on the issuing team's own base, which is what
        /// `turtle` means with no argument — so `(unspecified)` was a sentence
        /// describing a refusal for an intent that was not refused, and it
        /// travelled to three readers (the intent log, `plans[].current` and
        /// the seat journal) saying "defends (unspecified) within 14" about a
        /// squad that was, in fact, defending home.
        ///
        /// **"our base" is the right words and not a paraphrase.** It is a
        /// built-in region name — `builtin_places` mints it per seat, pointing
        /// at that seat's own hall — so the defaulted sentence is byte-identical
        /// to the sentence a commander who spelled `"target": "our base"` gets,
        /// which is what makes the log comparable across the two spellings.
        /// Rendering the coordinates instead would have frozen a hall position
        /// into the record; tools/intent_compile.py's `stance_anchor` declines
        /// to emit an anchor key for exactly that reason.
        ///
        /// The fix is here rather than in the compiler because the sentence is
        /// rendered from the intent **as it was written** — apply_intents logs
        /// `submission.intent` and hands `compile_intent` a *clone*, and a plan
        /// step is stored as authored — so a default filled in downstream
        /// reaches none of the three readers.
        fn anchor(x: &Option<f32>, z: &Option<f32>, region: &Option<String>) -> String {
            match (region, x, z) {
                // Mirrors `intent::compile_intent`'s `resolved_point`, which
                // takes the anchor only when BOTH coordinates arrived and
                // falls back to `me.base_pos()` otherwise. A half-given anchor
                // is a defaulted anchor there, so it is one here too.
                (None, Some(_), Some(_)) | (Some(_), _, _) => place(x, z, region),
                _ => "our base".to_string(),
            }
        }
        /// How a sentence names the units it is about. **A selector is spoken
        /// as its phrase**, for the same reason a region is spoken as its name:
        /// the replay line for a hero-save rule should read "my hero falls back
        /// to home", not "unit 4294967297 falls back to (0.0, 0.0)" — and after
        /// the hero has died and been revived, the id in that second line would
        /// be a lie besides. An unparseable phrase is quoted verbatim so the log
        /// shows the typo the commander actually wrote.
        fn group(units: &[IntentId], select: &Option<String>) -> String {
            if let Some(raw) = select {
                return match parse_selector(raw) {
                    Some(sel) => sel.phrase(),
                    None => format!("'{raw}'"),
                };
            }
            match units.len() {
                1 => format!("unit {}", units[0]),
                n => format!("{n} units"),
            }
        }
        /// The same rule for the single-referent channels — a `build`'s worker,
        /// a `cast`'s caster, a `follow`'s leader.
        fn one(id: &Option<IntentId>, select: &Option<String>) -> String {
            match (select, id) {
                (Some(raw), _) => match parse_selector(raw) {
                    Some(sel) => sel.phrase(),
                    None => format!("'{raw}'"),
                },
                (None, Some(id)) => format!("{id}"),
                (None, None) => "(unspecified)".to_string(),
            }
        }
        /// How a sentence names an ability slot. The id reads as itself; a
        /// bare index has no name to give, so it says which slot it is.
        fn ability_name(sel: &Option<AbilitySelector>) -> String {
            match sel {
                None => "its ability".to_string(),
                Some(AbilitySelector::Id(id)) => id.clone(),
                Some(AbilitySelector::Index(i)) => format!("ability slot {i}"),
            }
        }
        match self {
            Intent::Move {
                units,
                x,
                z,
                region,
                select,
            } => {
                format!("move {} to {}", group(units, select), place(x, z, region))
            }
            Intent::AttackMove {
                units,
                x,
                z,
                region,
                select,
            } => {
                format!(
                    "attack-move {} to {}",
                    group(units, select),
                    place(x, z, region)
                )
            }
            Intent::Attack {
                units,
                target,
                select,
            } => {
                format!("{} attack {target}", group(units, select))
            }
            Intent::Harvest {
                units,
                target,
                select,
                target_select,
            } => {
                format!(
                    "{} harvest node {}",
                    group(units, select),
                    one(target, target_select)
                )
            }
            Intent::Return { units, select } => {
                format!("{} return cargo", group(units, select))
            }
            Intent::Follow {
                units,
                target,
                select,
                target_select,
            } => {
                format!(
                    "{} follow {}",
                    group(units, select),
                    one(target, target_select)
                )
            }
            Intent::Stop { units, select } => {
                format!("{} hold position", group(units, select))
            }
            Intent::Build {
                worker,
                kind,
                x,
                z,
                region,
                select,
                site,
            } => {
                // The site selector is part of the sentence because "build the
                // farm at (10, 20)" and "build the farm near (10, 20), wherever
                // it fits" are different instructions, and a log that spelled
                // them identically would hide which one was given.
                let where_ = match site {
                    Some(_) => format!("the nearest legal site to {}", place(x, z, region)),
                    None => place(x, z, region),
                };
                format!("worker {} builds {kind} at {where_}", one(worker, select))
            }
            Intent::Train { building, unit } => {
                format!("building {building} trains {unit}")
            }
            Intent::Upgrade { building } => {
                format!("building {building} upgrades to its next tier")
            }
            Intent::Cancel { building, index } => {
                format!("building {building} cancels queue slot {index}")
            }
            Intent::Research { building, upgrade } => {
                format!("building {building} researches {upgrade}")
            }
            Intent::Rally {
                building,
                x,
                z,
                region,
                target,
            } => match (x, z, region, target) {
                (_, _, Some(name), _) => {
                    format!("building {building} rallies to {name}")
                }
                (Some(x), Some(z), _, _) => {
                    format!("building {building} rallies to {}", at(*x, *z))
                }
                (_, _, _, Some(t)) => format!("building {building} rallies onto {t}"),
                _ => format!("building {building} rally (unspecified)"),
            },
            // The sentence carries the AIM, because "who cast what" stopped
            // being the whole story the moment a cast could miss by being
            // pointed somewhere else. A log line that read `7 casts Slow` for
            // both a clump-shattering hit and a shot at empty ground would
            // hide the only decision the caster made.
            Intent::Cast {
                hero,
                ability,
                x,
                z,
                target,
                select,
            } => {
                let aim = match (x, z, target) {
                    (Some(x), Some(z), _) => format!(" at {}", at(*x, *z)),
                    (_, _, Some(t)) => format!(" on {t}"),
                    _ => String::new(),
                };
                format!("{} casts {}{aim}", one(hero, select), ability_name(ability))
            }
            Intent::Buy { shop, item, hero } => match hero {
                Some(hero) => format!("hero {hero} buys {item} at shop {shop}"),
                None => format!("buy {item} at shop {shop}"),
            },
            // The DESTINATION is part of the sentence for the same reason a
            // cast's aim is: with two halls standing, "used the scroll" and
            // "used the scroll at the hall that is not the one being hit" are
            // different decisions, and a log that spelled them identically
            // would hide the only one the commander made.
            Intent::UseItem {
                slot,
                hero,
                destination,
            } => {
                let to = match destination {
                    Some(d) => format!(", bound for hall {d}"),
                    None => String::new(),
                };
                match hero {
                    Some(hero) => format!("hero {hero} uses item in slot {slot}{to}"),
                    None => format!("hero uses item in slot {slot}{to}"),
                }
            }
            Intent::Priority {
                units,
                classes,
                select,
            } => {
                if classes.is_empty() {
                    format!("{} clear focus-fire priority", group(units, select))
                } else {
                    format!("{} focus {}", group(units, select), classes.join(" > "))
                }
            }
            Intent::Retreat {
                units,
                below,
                x,
                z,
                region,
                select,
            } => {
                let has_place = region.is_some() || (x.is_some() && z.is_some());
                match below {
                    Some(b) if *b > 0.0 && has_place => format!(
                        "{} fall back to {} below {:.0}% health",
                        group(units, select),
                        place(x, z, region),
                        b * 100.0
                    ),
                    _ => format!("{} clear retreat policy", group(units, select)),
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
                let has_place = region.is_some() || (x.is_some() && z.is_some());
                // A leash whose radius came from the region has no number to
                // print, so it names the shape instead — and "hold the
                // perimeter" is the more honest rendering of what was said.
                match (radius, has_place) {
                    (Some(r), true) if *r > 0.0 => format!(
                        "{} hold within {r:.0} of {}",
                        group(units, select),
                        place(x, z, region)
                    ),
                    (None, true) => match region {
                        Some(name) => format!("{} hold {name}", group(units, select)),
                        None => format!("{} clear leash", group(units, select)),
                    },
                    _ => format!("{} clear leash", group(units, select)),
                }
            }
            Intent::Autocast {
                units,
                min_enemies,
                ability,
                select,
            } => match min_enemies {
                Some(n) if *n > 0 => format!(
                    "{} auto-cast {} at {n}+ enemies",
                    group(units, select),
                    ability_name(ability)
                ),
                _ => format!(
                    "{} clear auto-cast for {}",
                    group(units, select),
                    ability_name(ability)
                ),
            },
            Intent::Squad { units, id, select } => match id {
                Some(id) => format!("{} join squad {id}", group(units, select)),
                None => format!("{} leave their squad", group(units, select)),
            },
            Intent::Posture { id, posture } => match posture {
                None => format!("squad {id} stands down (posture cleared)"),
                Some(PostureIntent::Defend { x, z, region, radius }) => match radius {
                    Some(r) => {
                        format!("squad {id} defends {} within {r:.0}", place(x, z, region))
                    }
                    // The region is the ring: no number was said, so none is
                    // printed. This is the sentence the feature exists for.
                    None => format!("squad {id} defends {}", place(x, z, region)),
                },
                Some(PostureIntent::Push { x, z, region }) => {
                    format!("squad {id} pushes to {}", place(x, z, region))
                }
                Some(PostureIntent::Escort { unit }) => {
                    format!("squad {id} escorts {unit}")
                }
                Some(PostureIntent::Forage { x, z, region }) => {
                    format!("squad {id} forages, mustering at {}", place(x, z, region))
                }
            },
            // One word in, one sentence out — and the sentence spells out the
            // WHOLE bundle rather than echoing the word back. A replay log that
            // said only "squad 1 takes the push stance" would make the log
            // depend on a data file to be readable a year later, and the point
            // of the journal is that it is the record of what happened.
            Intent::Stance { squad, stance, x, z, region } => {
                match parse_stance(stance) {
                    Some(kind) => {
                        let def = kind.def();
                        // `anchor`, not `place`: this is the field with a
                        // default, and it is the only one in the vocabulary.
                        let where_ = anchor(x, z, region);
                        let mut parts: Vec<String> = vec![match def.posture {
                            StancePosture::Defend => {
                                format!("defends {where_} within {:.0}", def.radius)
                            }
                            StancePosture::Push => format!("pushes to {where_}"),
                            StancePosture::Forage => {
                                format!("forages, mustering at {where_}")
                            }
                        }];
                        if def.leash > 0.0 {
                            parts.push(format!("leashed to {:.0}", def.leash));
                        }
                        if def.retreat_below > 0.0 {
                            parts.push(format!(
                                "retreat below {:.0}% to its {}",
                                def.retreat_below * 100.0,
                                match def.rally {
                                    StanceRally::Anchor => "anchor",
                                    StanceRally::Base => "base",
                                }
                            ));
                        }
                        if !def.priority.is_empty() {
                            parts.push(format!(
                                "focus {}",
                                def.priority
                                    .iter()
                                    .map(|c| c.name())
                                    .collect::<Vec<_>>()
                                    .join(" > ")
                            ));
                        }
                        format!(
                            "squad {squad} takes the {} stance: {}",
                            kind.word(),
                            parts.join(", ")
                        )
                    }
                    // Unparseable words never reach the appliers — the compiler
                    // refuses them with the list of five. This arm exists only
                    // because the journal renders intents that were merely
                    // *submitted*, and it says what was asked for.
                    None => format!("squad {squad} is asked for an unknown stance '{stance}'"),
                }
            }
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
            // A trigger's sentence carries BOTH halves — the condition and the
            // action it defers — because the whole thing is one statement and a
            // log line naming only the condition would leave the reader unable
            // to tell what is about to happen to their army. The action half is
            // `then.sentence()` verbatim, so the line a trigger writes when it
            // is armed and the line it writes when it fires are the same words.
            Intent::TriggerSet {
                name,
                when,
                then,
                repeat,
            } => {
                let cadence = match repeat {
                    Some(secs) => format!(", repeating every {secs:.0}s"),
                    None => String::new(),
                };
                format!(
                    "when {}: {} (trigger: {name}{cadence})",
                    when.phrase(),
                    then.sentence()
                )
            }
            Intent::TriggerClear { name } => match name {
                Some(name) => format!("clear trigger {name}"),
                None => "clear every trigger".to_string(),
            },
            Intent::RegionSet {
                name,
                x,
                z,
                radius,
            } => format!(
                "'{name}' is the ground within {radius:.0} of {}",
                at(*x, *z)
            ),
            Intent::RegionClear { name } => match name {
                Some(name) => format!("forget the region {name}"),
                None => "forget every region".to_string(),
            },
            // A plan's sentence carries EVERY step, joined by the word the verb
            // is named after. That is deliberate and it is what makes a plan
            // reviewable: a co-commander proposing an opening is proposing ONE
            // thing, and the human answering it needs the whole sequence on the
            // line they are answering — not a step count they would have to go
            // expand. Bounded by `MAX_PLAN_STEPS`, so "the whole thing" is at
            // most eight clauses.
            //
            // The advance-condition of step k introduces step k+1, because that
            // is what it governs: `A, then when we reach tier 2: B`. The last
            // step's advance decides when the plan reports itself finished, so
            // it is rendered as a trailing clause when it is not the plain
            // "as soon as it lands" default.
            Intent::PlanSet { name, steps } => {
                if steps.is_empty() {
                    return format!("plan {name}: no steps");
                }
                let mut out = format!("plan {name} ({} steps): ", steps.len());
                for (i, step) in steps.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", then ");
                        out.push_str(&steps[i - 1].advance.phrase());
                    }
                    out.push_str(&step.intent.sentence());
                }
                if let Some(clause) = steps
                    .last()
                    .and_then(|last| last.advance.trailing_phrase())
                {
                    out.push_str(&format!(" (done {clause})"));
                }
                out
            }
            Intent::PlanClear { name } => match name {
                Some(name) => format!("clear plan {name}"),
                None => "clear every plan".to_string(),
            },
            Intent::Autopilot { on } => {
                if *on {
                    "hand the faction to the scripted AI".to_string()
                } else {
                    "take the faction back from the scripted AI".to_string()
                }
            }
            Intent::Surrender => "surrender the match".to_string(),
            Intent::Ready => "declare ready to begin".to_string(),
        }
    }
}

/// Who spelled the intent. The compiler treats every source identically when
/// deciding whether it is *legal* — this is never consulted for authority.
///
/// It is consulted for one thing: which renderer gets told when an intent is
/// refused. A bridge seat reads its errors out of the next snapshot; a human
/// has no snapshot, so a `Ui` rejection is also raised on that team's
/// `GameEvents` feed for the alert stack. Same verdict, same string, delivered
/// down the channel the seat is actually reading.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum IntentSource {
    /// A human gesture in ui.rs (mouse, hotkey, command card).
    Ui,
    /// A command batch through the file bridge, from the seat that *is* the
    /// faction — the opponent model in an LLM-vs-LLM or human-vs-LLM match.
    Bridge,
    /// The CO-COMMANDER seat: a second author on the *same* team as the human
    /// at the keyboard (`BH_BRIDGE=copilot`, copilot.rs).
    ///
    /// A seat, not a script — which is why it gets its own rung here rather
    /// than borrowing `Cause::Script`'s. Everything a co-commander mints is
    /// attributed with zero extra plumbing: `Cause::Order { source }` already
    /// answers "which of us moved this unit", the snapshot's `units[].why`
    /// already renders it, and ui.rs's `why_line` already tallies mixed
    /// answers across a selection — which is exactly the "did my partner
    /// re-task my push?" readout.
    ///
    /// Still descriptive, never authoritative: the compiler reaches the same
    /// verdict for all three. What *is* different — which of this seat's
    /// intents need the human's approval first — is decided in copilot.rs,
    /// upstream of submission, and is a matter of etiquette rather than of
    /// legality (see docs/INTENT.md § co-command).
    Copilot,
    /// The SCRIPTED COMMANDER (ai.rs), which drives the Claude faction always
    /// and either faction under `autopilot`.
    ///
    /// It used to be the last thing in the game that changed state without
    /// saying anything: it wrote `Order`s, pushed `TrainingQueue`s and sent
    /// `UpgradeBuilding` straight through, which meant the fairness invariant
    /// had to be stated with a footnote. It is a seat now. Every action it
    /// takes is a `SubmitIntent` like anybody else's — validated by the same
    /// compiler, refused by the same rules, priced by the same link, and
    /// written to the same replay log, so a replay finally names all three
    /// authors instead of two and a silence.
    ///
    /// Descriptive, never authoritative, exactly like the three above: the
    /// compiler reaches its verdict without consulting this field. What the
    /// variant *does* decide is where a refusal is delivered — a script has no
    /// snapshot and no alert stack, so its errors go to the debug log (see
    /// `apply_intents`) rather than into a seat's error channel it does not
    /// read and a human never caused.
    Script,
}

impl IntentSource {
    pub fn name(self) -> &'static str {
        match self {
            IntentSource::Ui => "ui",
            IntentSource::Bridge => "bridge",
            IntentSource::Copilot => "copilot",
            IntentSource::Script => "script",
        }
    }
}

// ---------------------------------------------------------------------------
// Provenance — every unit's answer to "why are you doing that?"
// ---------------------------------------------------------------------------

/// Why this unit is doing what it is doing, stamped at the moment its current
/// behaviour was minted.
///
/// The chain THESIS.md promises is *posture <- squad <- template <- directive*,
/// and each rung of it mints an `Order` somewhere different: the intent
/// compiler for a direct order, doctrine.rs for a squad posture or a retreat
/// trigger, units.rs for the template a producing building stamps, and the
/// engine's own instincts for everything left over. Whoever writes the order
/// writes the reason next to it, in the same `Commands` call — so the answer
/// cannot drift from the behaviour, because there is no second place that
/// could disagree.
///
/// Deliberately cheap: an enum of `Copy` scalars, no allocation, rendered to a
/// string only when someone asks (`why()`), which is once a second per unit in
/// the snapshot and once a frame for the handful of selected units.
#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Provenance {
    pub cause: Cause,
    /// Game seconds at which this behaviour was minted.
    pub at: f32,
}

/// The rungs of the chain of command, as a flat enum.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cause {
    /// A direct order from a player — which verb, spelled by which interface.
    /// Outranks everything below until doctrine re-tasks the unit.
    Order {
        verb: &'static str,
        source: IntentSource,
    },
    /// A **trigger** the player armed in advance fired, and the engine
    /// submitted its intent. Its own rung rather than `Order`'s, because the
    /// two answer different questions: `order:move by bridge` means somebody
    /// decided to move this unit *just now*, and that is exactly what did not
    /// happen here. The seat is still named — a trigger has an author, and the
    /// engine is only its executor.
    Trigger {
        name: TriggerName,
        verb: &'static str,
        source: IntentSource,
    },
    /// A **plan** the player set reached this step, and the engine submitted
    /// its intent. Its own rung beside `Trigger` and for the same reason: the
    /// honest answer to "why are you doing that" is not "somebody said so just
    /// now", it is "you wrote this down as step 4 of five", and the step number
    /// is the part that makes the answer usable — it tells the reader where in
    /// the sequence they are without opening the plan.
    Plan {
        plan: PlanStamp,
        verb: &'static str,
        source: IntentSource,
    },
    /// The unit's squad has a standing posture and the engine is executing it.
    Posture { squad: u8, posture: &'static str },
    /// A standing policy fired: a retreat threshold, a leash snapping back.
    Policy { policy: &'static str },
    /// Inherited at spawn from the building that produced it — a doctrine
    /// `template`, or just the building's `rally`.
    Stamp {
        /// `"template"` or `"rally"` — which of the building's stamps this was.
        how: &'static str,
        kind: &'static str,
        building: Entity,
    },
    // NOTE: there used to be a `Script { what }` rung here, for the scripted
    // `ai.rs` baseline, "not a seat, so it gets its own rung rather than
    // borrowing `Order`'s". ai.rs is a seat now (`IntentSource::Script`), and
    // its orders come out of the compiler stamped `order:<verb> by script`
    // like everybody else's — which is the whole of the answer, and one rung
    // fewer to explain.
    /// Engine default: nothing above applies. `"idle"` renders bare.
    Instinct { what: &'static str },
}

impl Provenance {
    pub fn new(cause: Cause, at: f32) -> Self {
        Provenance { cause, at }
    }

    /// Auto-enrolment, idle instinct and the like.
    pub fn instinct(what: &'static str, at: f32) -> Self {
        Provenance::new(Cause::Instinct { what }, at)
    }

    /// The compact one-line answer, shared verbatim by the snapshot's
    /// `units[].why` and the human's selection panel. Same question, same
    /// string, both seats — the equity claim applied to introspection.
    ///
    /// ```text
    /// order:move by bridge t=123     a player said so, and when
    /// trigger:home-guard move by ui t=41   a rule they armed earlier fired
    /// posture:push sq1               squad 1's standing posture
    /// policy:retreat t=210           a retreat threshold fired
    /// template:Barracks#4294968163   stamped at spawn by that building
    /// order:attackmove by script t=5 the scripted AI, speaking as a seat
    /// instinct:flee                  an engine reflex
    /// idle                           nothing to do
    /// ```
    pub fn why(&self) -> String {
        match self.cause {
            Cause::Order { verb, source } => {
                format!("order:{verb} by {} t={:.0}", source.name(), self.at)
            }
            Cause::Trigger {
                name,
                verb,
                source,
            } => format!(
                "trigger:{name} {verb} by {} t={:.0}",
                source.name(),
                self.at
            ),
            Cause::Plan {
                plan,
                verb,
                source,
            } => format!("plan:{plan} {verb} by {} t={:.0}", source.name(), self.at),
            Cause::Posture { squad, posture } => format!("posture:{posture} sq{squad}"),
            Cause::Policy { policy } => format!("policy:{policy} t={:.0}", self.at),
            Cause::Stamp {
                how,
                kind,
                building,
            } => format!("{how}:{kind}#{}", building.to_bits()),
            // The one bare word: "idle" is the absence of a reason, and
            // dressing it up as `instinct:idle` would imply there was one.
            Cause::Instinct { what } if what == "idle" => "idle".to_string(),
            Cause::Instinct { what } => format!("instinct:{what}"),
        }
    }
}

/// What a freshly trained unit answers when asked who sent it.
///
/// A pure function so the rule is testable without standing up a renderer:
/// `spawn_units` needs mesh assets to run at all, and "which rung of the chain
/// does a new unit start on" is a decision worth checking on its own.
///
/// A template is the stronger claim — it stamped standing doctrine, not merely
/// a destination — but a bare rally still decided this unit's first order, and
/// "the barracks sent me" is the true answer either way. With neither, a fresh
/// unit genuinely has no reason yet.
pub fn spawn_provenance(
    producer: Option<(Entity, BuildingKind)>,
    has_template: bool,
    rallied: bool,
    now: f32,
) -> Provenance {
    match producer {
        Some((building, kind)) if has_template || rallied => Provenance::new(
            Cause::Stamp {
                how: if has_template { "template" } else { "rally" },
                kind: building_name(kind),
                building,
            },
            now,
        ),
        _ => Provenance::instinct("idle", now),
    }
}

/// What a unit with no `Provenance` component at all answers. Reached by units
/// that have never been given a reason — the opening workers before anyone has
/// spoken — and by anything that outlives its stamp.
pub const NO_PROVENANCE: &str = "idle";

/// Who is speaking and when, threaded through the intent compiler so every
/// order it mints can stamp its own reason. One `Copy` parameter instead of
/// two, because `compile_intent`'s signature is long enough already.
#[derive(Clone, Copy, Debug)]
pub struct IntentMark {
    pub source: IntentSource,
    pub at: f32,
    /// Set when a **trigger** submitted this intent rather than a player
    /// speaking it now. Carried here rather than checked at each of the eight
    /// order arms so that the rung a unit ends up on is decided in one place.
    pub trigger: Option<TriggerName>,
    /// Set when a **plan step** submitted this intent. Same role as `trigger`
    /// one rung along: the two are mutually exclusive by construction (a plan
    /// step's intent is submitted by plan.rs, a trigger's by trigger.rs) and
    /// `order` prefers the trigger arm if both are somehow set.
    pub plan: Option<PlanStamp>,
}

impl IntentMark {
    /// The provenance a direct order of `verb` stamps on its targets.
    pub fn order(&self, verb: &'static str) -> Provenance {
        let cause = match (self.trigger, self.plan) {
            (Some(name), _) => Cause::Trigger {
                name,
                verb,
                source: self.source,
            },
            (None, Some(plan)) => Cause::Plan {
                plan,
                verb,
                source: self.source,
            },
            (None, None) => Cause::Order {
                verb,
                source: self.source,
            },
        };
        Provenance::new(cause, self.at)
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
    /// Set when a **trigger** fired this intent. Three things read it, and each
    /// would otherwise have to guess:
    ///
    /// * the link (docs/TEMPO.md) — trigger-fired intents are engine-executed
    ///   standing policy and pay nothing, exactly like a posture;
    /// * [`Provenance`] — the unit answers `trigger:<name>`, not `order:`;
    /// * the human's refusal notice, which names the rule that failed rather
    ///   than a gesture the player never made.
    pub trigger: Option<TriggerName>,
    /// Set when a **plan step** submitted this intent. Read by the same three
    /// consumers as `trigger`, for the same three reasons — the link exemption
    /// (a plan is standing policy the engine executes), [`Provenance`], and the
    /// human's refusal notice — plus a fourth that is plans' own: the compiler
    /// reports the verdict back to `Plans` through it, which is how a plan
    /// learns that its step bounced instead of silently walking past it.
    pub plan: Option<PlanStamp>,
}

impl SubmitIntent {
    /// A gesture from the human at the keyboard.
    pub fn ui(team: Team, intent: Intent) -> Self {
        SubmitIntent {
            team,
            source: IntentSource::Ui,
            tag: "ui".to_string(),
            intent,
            trigger: None,
            plan: None,
        }
    }

    /// An intent a **trigger** fired, on behalf of the seat that armed it.
    ///
    /// The tag is the trigger's own name, so every channel that already
    /// prefixes by tag — the wire's `errors`, the replay log, the human's
    /// alert stack — says which rule spoke without any of them learning a new
    /// concept.
    pub fn fired(team: Team, source: IntentSource, name: TriggerName, intent: Intent) -> Self {
        SubmitIntent {
            team,
            source,
            tag: format!("trigger:{name}"),
            intent,
            trigger: Some(name),
            plan: None,
        }
    }

    /// An intent a **plan step** submitted, on behalf of the seat that set it.
    ///
    /// The tag names the plan AND the step (`plan:opening#2`), so every channel
    /// that already prefixes by tag says which step spoke. The `#k` is what a
    /// commander needs that a trigger never did: with eight steps a bare plan
    /// name would not say which of them bounced.
    pub fn plan_step(team: Team, source: IntentSource, plan: PlanStamp, intent: Intent) -> Self {
        SubmitIntent {
            team,
            source,
            tag: format!("plan:{}#{}", plan.name, plan.step),
            intent,
            trigger: None,
            plan: Some(plan),
        }
    }

    /// A decision from the scripted commander (ai.rs).
    ///
    /// One flat tag rather than the bridge's `cmd 3`: a batch is a document
    /// whose commands need distinguishing, a think tick is a stream of
    /// independent sentences and nobody is going to look one of them up by
    /// number. What the tag does buy is a debug line that says *who* was
    /// refused without having to read the verb.
    pub fn script(team: Team, intent: Intent) -> Self {
        SubmitIntent {
            team,
            source: IntentSource::Script,
            tag: "script".to_string(),
            intent,
            // **The script pays.** `None` is load-bearing here, not a default
            // filled in to satisfy the compiler: `apply_intents` hands a
            // trigger-fired intent the `exempt_issuer`, and a scripted think
            // tick is not a trigger firing — it is a commander deciding, at
            // the moment of deciding, which is the thing the link prices.
            // docs/TEMPO.md §3's "the scripted AI pays latency too, or
            // autopilot becomes a cheat at the third seat" is enforced by this
            // field being what it is.
            trigger: None,
            // And `plan` is `None` on the identical argument, which is the same
            // sentence one verb further along: a plan step is *also* handed the
            // `exempt_issuer`, so a script that could stamp this field could
            // claim the exemption without ever having written a plan down. The
            // scripted seat decides at the moment of deciding; it never defers,
            // so it never earns the discount for having deferred.
            plan: None,
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

/// One command that was applied, and what reaching the units it named actually
/// cost — the realised worst link across everything the sentence moved, in
/// seconds (`OrderIssuer::max_delay`).
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedCommand {
    /// The submitting seat's own name for the command — `"cmd 3"` for the
    /// fourth command of a bridge batch. The same string the error channel
    /// prefixes its messages with, so the two join without a second identity
    /// scheme: `errors: ["cmd 3: ..."]` and `applied: [{cmd: "cmd 3", ...}]`
    /// are the negative and positive halves of one acknowledgement.
    pub cmd: String,
    pub delay: f32,
}

/// **The acknowledgement channel** (docs/TEMPO.md §4, issue 6): what the last
/// batch each team submitted actually cost in link latency.
///
/// The sibling of [`IntentErrors`], and deliberately shaped like it — same
/// per-team split, same "cleared when a batch is accepted, appended to by the
/// compiler, copied into the next snapshot" lifecycle. Errors tell a commander
/// what it may not do; this tells it what its orders cost, which is the other
/// half of learning a mechanic instead of inferring it from failure.
///
/// Only commands that actually **paid** are recorded. A command that landed in
/// the frame it was spoken says nothing here, so with `BH_COMMAND_LATENCY` off
/// this resource is permanently empty and the wire is byte-identical to v1.
#[derive(Resource, Default)]
pub struct IntentApplied {
    pub human: Vec<AppliedCommand>,
    pub claude: Vec<AppliedCommand>,
}

impl IntentApplied {
    pub fn get(&self, team: Team) -> &Vec<AppliedCommand> {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }
    pub fn get_mut(&mut self, team: Team) -> &mut Vec<AppliedCommand> {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
}

/// How many recent intents `IntentJournal` keeps per team.
pub const JOURNAL_MAX: usize = 40;

/// One remembered intent: what was said, by whom, and when.
///
/// Deliberately the *same four fields* `intent_log.jsonl` writes — this is the
/// in-memory tail of that file, not a second record with its own vocabulary.
#[derive(Clone, Debug)]
pub struct JournalEntry {
    pub t: f32,
    pub source: IntentSource,
    pub verb: &'static str,
    /// `Intent::sentence()` — the half a person (or a partner) reads.
    pub sentence: String,
    pub ok: bool,
}

/// The recent intent history of each team, in memory, oldest first.
///
/// `intent_log.jsonl` is the *match record* and lives on disk; this is the
/// tail of it a running seat can be shown. It exists for exactly one reason:
/// **co-command needs the legibility to run both ways.** The human already
/// sees the co-commander's directives (they arrive as proposals with a note
/// and compiled sentences). Without this, the co-commander could not see the
/// human's — it would be commanding half-blind, next to a partner it cannot
/// read, which is the asymmetry THESIS.md's wager is against.
///
/// No new vocabulary: the entries are the same sentences `Intent::sentence()`
/// renders for the log and the same `source` tags `units[].why` carries. A
/// copilot seat serializes them as `partner_log`.
#[derive(Resource, Default)]
pub struct IntentJournal {
    human: VecDeque<JournalEntry>,
    claude: VecDeque<JournalEntry>,
}

impl IntentJournal {
    pub fn get(&self, team: Team) -> &VecDeque<JournalEntry> {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }

    /// Append one entry, dropping the oldest past `JOURNAL_MAX`.
    pub fn push(&mut self, team: Team, entry: JournalEntry) {
        let ring = match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        };
        ring.push_back(entry);
        while ring.len() > JOURNAL_MAX {
            ring.pop_front();
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
    /// Set by an `EffectAtom::Summon`: this body was CALLED, and units.rs
    /// stamps it with the component that says when it goes home. `None` for
    /// every trained or scripted spawn, which is all of them but one.
    pub summoned: Option<Summoned>,
}

/// A team concedes the match. Written by ui.rs (player) or bridge.rs
/// (commander); CorePlugin resolves it into a GameOver for the opponent.
#[derive(Event, Debug)]
pub struct Surrender {
    pub team: Team,
}

/// A seat says it has read the map and is ready to begin. Written by the
/// intent compiler from [`Intent::Ready`]; CorePlugin's `ready_gate` resolves
/// it against [`ReadyGate`] and, once every gating seat has spoken, starts the
/// match clock. Modelled on [`Surrender`] deliberately: both are match-level
/// statements a seat makes about the match rather than about its units, and
/// both reach the engine as an event so the compiler arm stays two lines.
#[derive(Event, Debug)]
pub struct MatchReady {
    pub team: Team,
}

/// Ask a caster (hero or ability building) to cast. Written by ui.rs
/// (hotkey/button), ai.rs, doctrine.rs and bridge.rs; combat.rs validates
/// (alive, unlocked, mana, cooldown) and executes the AoE.
///
/// `ability: None` means "the first ability this caster has unlocked" — the v1
/// meaning of the event — so nothing that only knows about one ability per
/// caster had to change.
/// `target: None` is the same kind of promise: for a `AbilityTarget::Caster`
/// ability it is the only answer there is, and for a targeted one it means
/// "aim it for me" — combat.rs runs [`best_cast_focus`] over everything the
/// effect could affect within range. Every pre-existing writer of this event
/// therefore keeps working unchanged, and an auto-cast Sorcerer aims itself.
#[derive(Event, Clone, Debug)]
pub struct CastAbility {
    pub caster: Entity,
    pub ability: Option<AbilitySelector>,
    /// Where to centre it. `None` = the ability's own default (the caster, or
    /// the auto-pick for a targeted ability). Ignored by a `Caster` ability,
    /// which has nowhere else to be.
    pub target: Option<CastTarget>,
}

#[allow(dead_code)]
impl CastAbility {
    /// Backward-compatible cast: the caster's first unlocked ability, aimed by
    /// the engine.
    pub fn new(caster: Entity) -> Self {
        CastAbility { caster, ability: None, target: None }
    }
    /// Cast a specific slot of the caster's ability list.
    pub fn index(caster: Entity, index: usize) -> Self {
        CastAbility { caster, ability: Some(AbilitySelector::Index(index)), target: None }
    }
    /// Cast an ability by `AbilityDef::name` (what the bridge sends).
    pub fn id(caster: Entity, id: impl Into<String>) -> Self {
        CastAbility { caster, ability: Some(AbilitySelector::Id(id.into())), target: None }
    }
    /// Aim this cast at a chosen point or unit.
    pub fn at(mut self, target: CastTarget) -> Self {
        self.target = Some(target);
        self
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

/// How a match ended. Round-9 AAR (`wc3clone-azo`): the winner could not tell
/// which win they had got. The engine recognises exactly two *wins* and has
/// always known which one it took — it just never said.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameOverReason {
    /// The loser has no production buildings left — see `check_game_over`.
    Razed,
    /// The loser conceded.
    Surrender,
    /// **The time cap ran out and the referee counted the assets.**
    /// `BH_MAX_GAME_SECS` stops an unattended run; `asset_score` says who was
    /// ahead when it did (`main::headless_exit`).
    ///
    /// Named apart from the two wins on purpose, and docs/ARENA.md's ledger
    /// vocabulary has said so since round 10: this is a referee's opinion, not
    /// a win the game recognises, so a round decided this way is recorded
    /// `decisive: false` and can never be quoted as a conquest. It is a
    /// *verdict* nonetheless — a decided match, delivered down the same
    /// channel as the other two, because the alternative is what
    /// `wc3clone-j84` was filed for: a capped match that ends with
    /// `game_over: null` and a commander polling a file that will never
    /// change again.
    Score,
}

impl GameOverReason {
    /// Wire text. Snapshot, HUD and headless log all print this one string.
    pub fn name(self) -> &'static str {
        match self {
            GameOverReason::Razed => "razed",
            GameOverReason::Surrender => "surrender",
            GameOverReason::Score => "score",
        }
    }
}

/// The verdict, once there is one. Named fields rather than the old
/// `GameOver(Option<Team>)` tuple so the reason cannot be added at one call
/// site and forgotten at the other: `decide` and `decide_draw` are the only
/// ways to set any of it, and each takes everything it sets.
#[derive(Resource, Default)]
pub struct GameOver {
    /// `Some(winner)` once somebody has won. **`None` is not "still playing"**
    /// — a drawn match is decided and names nobody. Ask `decided()` whether
    /// the match is over; ask this only who won it.
    pub winner: Option<Team>,
    /// Always `Some` exactly when the match is `decided()`.
    pub reason: Option<GameOverReason>,
    /// **Decided, with no winner.** Only `GameOverReason::Score` can produce
    /// one today: two teams dead even on assets when the cap expired.
    ///
    /// A separate flag rather than a sentinel team, for the reason
    /// docs/ARENA.md gives the ledger ("a draw is an absent winner, not a
    /// sentinel team. Round 2 is the reason") — a fake winner is a lie every
    /// downstream reader inherits, and `winner` is read by the HUD banner, the
    /// snapshot and the arena record.
    pub drawn: bool,
}

impl GameOver {
    /// Record the verdict. Both halves, in one statement, or neither.
    pub fn decide(&mut self, winner: Team, reason: GameOverReason) {
        self.winner = Some(winner);
        self.reason = Some(reason);
    }

    /// Record a decided match that nobody won.
    pub fn decide_draw(&mut self, reason: GameOverReason) {
        self.drawn = true;
        self.reason = Some(reason);
    }

    /// **Is the match over?** The question every gate in the game is actually
    /// asking when it stops taking input, stops the scripted commander, or
    /// draws a banner. It was spelled `winner.is_some()` everywhere until a
    /// draw became possible, at which point that spelling quietly meant
    /// "somebody won" instead — one choke point, so a draw cannot end the
    /// match in the snapshot and leave the UI still accepting orders.
    pub fn decided(&self) -> bool {
        self.winner.is_some() || self.drawn
    }
}

/// Which teams the scripted AI drives. Claude is always AI; the Human side
/// can be AI too (AI-vs-AI spectating): `BH_AI_BOTH=1` at launch or F9 at
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
// The ready handshake — bridged seats start simultaneously
// ---------------------------------------------------------------------------

/// `BH_READY=0` turns the handshake off entirely and restores the pre-t0d
/// behaviour: the clock runs from process start and a commander plays whatever
/// is left of the opening by the time it connects.
pub const READY_ENV: &str = "BH_READY";

/// `BH_READY_TIMEOUT=180` — how long, in WALL seconds, the engine will hold
/// the match waiting for a seat that may never speak.
///
/// Measured on `Time<Real>`, which is the same clock bridge.rs paces its
/// snapshot writes and command polls off. That pairing is the point: the
/// timeout runs on the clock the commander's own experience runs on, so "two
/// minutes" means two minutes' worth of the snapshots it is actually reading.
///
/// **Under `BH_FIXED_DT` that is not the wall clock.** The fixed tick drives
/// `Time<Real>` as well as `Time<Virtual>` (see [`fixed_time_strategy`]), so a
/// headless run stepping as fast as it can burns "real" seconds far faster
/// than a person does, and a bridged fixed-dt run will time out long before a
/// human-scale agent could answer. Set the timeout in TICKS' worth of seconds
/// there, or leave the fixed tick to the determinism harness, which has no
/// bridge seat and so never holds at all.
pub const READY_TIMEOUT_ENV: &str = "BH_READY_TIMEOUT";

/// Two minutes: long enough to spawn a commander agent and let it read the map
/// and the brief, short enough that a dead seat costs one coffee rather than
/// one afternoon.
pub const READY_TIMEOUT_DEFAULT: f32 = 120.0;

/// Is the handshake mechanic switched on at all? Off only for an explicit
/// `BH_READY=0`/`false`. Note this is *not* the same question as "is the match
/// being held" — a run with no bridge seat has the mechanic enabled and nothing
/// to gate on, which is exactly why pure-scripted sims are byte-untouched.
pub fn ready_handshake_enabled() -> bool {
    !matches!(
        std::env::var(READY_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// The configured hold timeout in wall seconds, or the default. A value of 0
/// (or a negative one) means "do not wait at all", which is a legitimate thing
/// to ask for in a smoke test.
pub fn ready_timeout_from_env() -> f32 {
    std::env::var(READY_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|t| t.is_finite() && *t >= 0.0)
        .unwrap_or(READY_TIMEOUT_DEFAULT)
}

/// One gating seat and whether it has spoken yet.
#[derive(Debug, Clone)]
pub struct ReadySeat {
    /// The seat's directory name — `red`, `blue`, `copilot`. This is the name
    /// that appears in the snapshot's `waiting_for` and in the log, because it
    /// is the name the operator typed into `BH_BRIDGE` and the name on the
    /// commander's own seat directory. Teams are an engine-side noun.
    pub name: &'static str,
    pub team: Team,
    pub ready: bool,
}

/// **The match does not start until every externally-commanded seat says it is
/// ready.**
///
/// The problem this exists for: the sim clock used to run from process start,
/// but commander agents connect seconds to minutes apart — in arena round 9
/// Red's first order landed at t=41s, against an opponent that had been playing
/// since t=0. The opening is part of the game, and a game whose opening one
/// side simply misses is not a fair one. So the engine holds at t=0, keeps
/// writing snapshots (both sides get to read the map and plan), and starts the
/// clock for everyone at the same instant.
///
/// **Which seats gate.** Every seat in `BH_BRIDGE`, including a `copilot`
/// seat. A copilot is a commanding seat — it writes orders that reach units,
/// its trust policy only decides whether they go direct or via a proposal — so
/// starting the match before it has read the map reproduces the exact unfairness
/// this mechanism removes, one rung down. Seats that are NOT in `BH_BRIDGE`
/// are not in this list at all, which is what "scripted AI seats are born
/// ready" means concretely: a scripted opponent has nothing to wait for.
///
/// **Autopilot is born ready too**, and that check is live rather than
/// startup-only (`ready_gate` re-derives it every frame): a seat whose faction
/// is currently in the scripted AI's hands cannot meaningfully read a map, and
/// letting it gate would mean a commander could hang a match by autopiloting
/// and disconnecting.
///
/// **Nothing gameplay-relevant runs while held.** The hold is a paused
/// `Time<Virtual>`, so every accumulator in the sim integrates zero: harvest
/// timers, build progress, projectile flight, `on_timer` gates, the headless
/// time cap. The bridge's own snapshot and poll timers run on `Time<Real>` and
/// are unaffected, which is the property that makes the whole design work —
/// the world is photographed and orders are read while the world itself is
/// still. Pinned by `a_held_match_is_inert`.
///
/// **Orders sent before `ready` are legal**, for both sides equally. They
/// compile at t=0 and their units act on the first live frame. That is not a
/// loophole, it is the planning time the hold exists to give: the etiquette is
/// symmetric because the hold is.
#[derive(Resource, Default)]
pub struct ReadyGate {
    /// The gating seats, in `BH_BRIDGE` order. Empty means nothing gates and
    /// the match runs exactly as it always did.
    pub seats: Vec<ReadySeat>,
    /// Has the clock been started? Once true this resource is inert and the
    /// snapshot keys disappear again.
    pub started: bool,
    /// Wall seconds to wait before starting anyway.
    pub timeout: f32,
    /// Wall seconds waited so far.
    pub waited: f32,
    /// Did the timeout start the match rather than the last seat? Recorded so
    /// the log line, the feed entry and any after-action report all say the
    /// same thing about how this match began.
    pub started_by_timeout: bool,
}

impl ReadyGate {
    /// Is the match being held right now?
    pub fn holding(&self) -> bool {
        !self.started && !self.seats.is_empty()
    }

    /// The seats still owed a `ready`, by name. Empty while not holding.
    pub fn waiting_for(&self) -> Vec<&'static str> {
        if !self.holding() {
            return Vec::new();
        }
        self.seats
            .iter()
            .filter(|s| !s.ready)
            .map(|s| s.name)
            .collect()
    }
}

/// Stop the clock before the first frame runs.
///
/// A `Startup` system rather than a run condition or a first-frame check,
/// because "held at t=0" has to mean t=0 exactly: by the time an `Update`
/// system could notice, the frame's movement, economy and combat have already
/// integrated one delta. bridge.rs fills `ReadyGate` in its own `Startup`
/// system and orders itself `.before(ReadyGateHold)`.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ReadyGateHold;

fn hold_for_ready(gate: Res<ReadyGate>, mut vtime: ResMut<Time<Virtual>>) {
    if !gate.holding() {
        return;
    }
    vtime.pause();
    info!(
        "{READY_ENV}: match HELD at t=0 — waiting for {} to send {{\"type\":\"ready\"}} \
         (auto-start after {:.0}s wall)",
        gate.waiting_for().join(" "),
        gate.timeout
    );
}

/// Collect `ready` statements, and start the match when they are all in — or
/// when the timeout says a missing seat has had long enough.
///
/// `SimSet::Upkeep`, alongside the win check, and for the same reason: both are
/// per-frame questions about the *match* rather than about any unit, and both
/// have to run after `SimSet::Intent` so a statement submitted this frame is
/// resolved this frame rather than next.
fn ready_gate(
    real: Res<Time<Real>>,
    mut vtime: ResMut<Time<Virtual>>,
    mut gate: ResMut<ReadyGate>,
    mut readies: EventReader<MatchReady>,
    ai: Res<AiControlled>,
    mut feed: ResMut<GameEvents>,
) {
    if !gate.holding() {
        // Idempotent past the start line: a reconnecting commander that
        // re-sends its opening batch says `ready` again and is not punished.
        readies.clear();
        return;
    }

    for ev in readies.read() {
        // `bypass_change_detection` is not needed; we already hold `ResMut`.
        let Some(seat) = gate.seats.iter_mut().find(|s| s.team == ev.team) else {
            continue;
        };
        if seat.ready {
            continue;
        }
        seat.ready = true;
        let name = seat.name;
        let still = gate.waiting_for();
        if still.is_empty() {
            info!("{READY_ENV}: {name} is ready — every seat is in");
        } else {
            info!(
                "{READY_ENV}: {name} is ready — still waiting for {}",
                still.join(" ")
            );
        }
    }

    // A faction in the scripted AI's hands is ready by construction, checked
    // live so a mid-hold `autopilot` releases the match rather than freezing it.
    for seat in &mut gate.seats {
        let autopiloted = match seat.team {
            Team::Human => ai.human,
            Team::Claude => ai.claude,
        };
        if autopiloted && !seat.ready {
            seat.ready = true;
            info!(
                "{READY_ENV}: {} is on autopilot — born ready, not gating",
                seat.name
            );
        }
    }

    gate.waited += real.delta_secs();
    let all_in = gate.seats.iter().all(|s| s.ready);
    let timed_out = gate.waited >= gate.timeout;
    if !all_in && !timed_out {
        return;
    }

    gate.started = true;
    gate.started_by_timeout = !all_in;
    // The missing seats have to be named while `holding()` is still true;
    // afterwards `waiting_for()` is empty by definition.
    let absent: Vec<&'static str> = gate
        .seats
        .iter()
        .filter(|s| !s.ready)
        .map(|s| s.name)
        .collect();
    vtime.unpause();

    let note = if all_in {
        "match start — every seat readied, the clock runs from t=0".to_string()
    } else {
        format!(
            "match start — {:.0}s ready timeout expired without {}; starting anyway",
            gate.timeout,
            absent.join(" ")
        )
    };
    info!("{READY_ENV}: {note}");
    // Pushed to BOTH feeds, which is not the asymmetry `GameEvents::push`
    // warns about: the start of the match is a global fact that both sides
    // know by construction, not intel about the opponent. Both sides must see
    // the same line or the handshake is not observably fair.
    for team in [Team::Human, Team::Claude] {
        feed.push(team, 0.0, note.clone(), EventSeverity::Info, None);
    }
    // Flush the feed now rather than up to a second later, so `match start` is
    // in the very next snapshot each commander reads.
    feed.force = true;
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
// The sim frame: one explicit, total order
// ---------------------------------------------------------------------------

/// The canonical order of a simulation frame. Every gameplay system in the
/// project lands in exactly one of these, and `CorePlugin` chains them, so
/// "what runs before what" is one list in one file instead of a scatter of
/// `.after()` clauses that only constrained a fraction of the schedule.
///
/// Before this existed the schedule had exactly two ordering handles —
/// `FogSet` and `IntentApply` — and everything else (movement vs combat vs
/// economy vs bounty) was left to Bevy's multi-threaded executor, which
/// resolves conflicts against whatever happens to be running on another
/// thread. Two runs of the same binary could therefore step units, resolve
/// damage and spend gold in different orders. That is the bug this enum
/// closes; see DESIGN.md § Determinism.
///
/// Three edges were already in the code and are merely re-encoded here:
///   * `Deaths` → `Fog` (`update_fog.after(apply_death)`: the dead stop seeing)
///   * `Fog` → `Intent` (`IntentApply.after(FogSet)`: an order is judged
///     against the visibility its issuer has right now)
///   * `Input`/`CoCommand` → `Intent` (bridge poll, co-command negotiation and
///     the latency dispatcher all declared `.before(IntentApply)`)
///
/// The rest was ambiguous and is CHOSEN here. The choices, and why:
///   * `Deaths`/`Fog` lead the frame. They are forced there: fog must follow
///     death and intent must follow fog, so both sit upstream of everything
///     the commander does. The consequence is that damage dealt in `Combat`
///     becomes a despawn at the top of the NEXT frame — a one-tick lag that
///     the old schedule already had roughly half the time, now had always.
///   * `Think` (doctrine) before `Intent`, matching command.rs's existing rule
///     that "a fresh direct order issued in the same frame still wins":
///     standing orders execute first so an explicit order can overwrite them
///     in the same frame, never the other way round.
///   * `AiThink` before `Think`, because the scripted commander writes the
///     `SquadOrders` that doctrine then executes — same frame, not next.
///   * `Movement` before `Combat` so a unit shoots from where it now stands.
///   * `Bounty` before `Economy` so a cache claimed this frame is banked this
///     frame rather than next.
///   * `Upkeep`/`Feed` last, so the recounts, the win check and the event feed
///     all describe the frame that just finished.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum SimSet {
    /// `apply_death` — reap anything at zero HP and free its nav footprint.
    Deaths,
    /// `update_fog` — the one producer of knowability. Wraps `FogSet`.
    Fog,
    /// Everything that reads the outside world: hotkeys, the bridge poll, the
    /// chain-of-command dispatcher, the whole ui.rs gesture chain.
    Input,
    /// Co-command negotiation, between reading a partner's wire and compiling.
    CoCommand,
    /// The scripted commander's macro decisions.
    AiThink,
    /// Standing orders: postures, retreats, leashes, auto-cast — and the
    /// trigger evaluator, which is the contingent member of the same family.
    Think,
    /// `apply_intents` — the one path from a stated intent to game state.
    Intent,
    /// Spawning, pathing, steering, separation.
    Movement,
    /// Target acquisition, projectiles, abilities, damage application.
    Combat,
    /// Treasure caches: spawn, claim, expiry.
    Bounty,
    /// Construction, research, harvesting, training, purchases.
    Economy,
    /// Per-tick bookkeeping: xp, regen, cooldowns, supply, tech, win check.
    Upkeep,
    /// Everything that only *describes* the frame: event feed, snapshot,
    /// logging. Nothing here may change game state.
    Feed,
    /// Purely visual: health bars, status rings, shockwaves, orb pulses.
    /// Excluded from the determinism contract on purpose — it cannot affect
    /// the sim, so it is free to run wherever the executor likes.
    Cosmetic,
}

/// The canonical frame order, as data, so tests and DESIGN.md can't drift from
/// the schedule.
pub const SIM_ORDER: [SimSet; 14] = [
    SimSet::Deaths,
    SimSet::Fog,
    SimSet::Input,
    SimSet::CoCommand,
    SimSet::AiThink,
    SimSet::Think,
    SimSet::Intent,
    SimSet::Movement,
    SimSet::Combat,
    SimSet::Bounty,
    SimSet::Economy,
    SimSet::Upkeep,
    SimSet::Feed,
    SimSet::Cosmetic,
];

/// The alarm evaluator's slot inside `SimSet::Feed`.
///
/// A named handle for the same reason `FogSet` is one: two other systems in
/// `Feed` need a hard edge against it and would otherwise be scheduled against
/// it by the executor. `alarm.rs` files itself here; `bridge.rs` declares
/// `.after(AlarmSet)` so a snapshot carries the alarms computed in ITS frame
/// rather than the previous one, and `shared.rs`'s own event diff runs before
/// it so the two writers of `GameEvents` cannot interleave differently between
/// two runs of one seed.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AlarmSet;

// ---------------------------------------------------------------------------
// Seeded randomness
// ---------------------------------------------------------------------------

/// Environment override for the match seed.
pub const SEED_ENV: &str = "BH_SEED";

/// The one source of gameplay randomness in the running sim.
///
/// Terrain has always been deterministic (`terrain.rs` seeds `StdRng` from the
/// fixed `MAP_SEED`), but bounty placement used `rand::thread_rng()` — OS
/// entropy, reproducible by nothing. Any system that wants a random number now
/// draws from here, in sim order, so the whole draw sequence is a function of
/// the seed alone.
///
/// Default is a fresh random seed, logged at startup, so a normal match is
/// still unpredictable; set `BH_SEED` to replay one.
#[derive(Resource)]
pub struct SimRng {
    /// The seed this match was started with. Logged at startup, which is the
    /// point: a run that turns out to be worth reproducing can be reproduced
    /// from its own log, without having decided in advance to record it.
    pub seed: u64,
    rng: rand::rngs::StdRng,
}

impl SimRng {
    pub fn from_env() -> Self {
        let seed = std::env::var(SEED_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or_else(rand::random::<u64>);
        SimRng::from_seed(seed)
    }

    pub fn from_seed(seed: u64) -> Self {
        use rand::SeedableRng;
        SimRng {
            seed,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
        }
    }

    /// Draw from the match stream. Callers take the whole `Rng` rather than a
    /// single value so a system that needs several numbers advances the stream
    /// once, in one place.
    pub fn rng(&mut self) -> &mut impl rand::Rng {
        &mut self.rng
    }
}

impl Default for SimRng {
    fn default() -> Self {
        SimRng::from_env()
    }
}

fn log_seed(rng: Res<SimRng>) {
    info!(
        "{SEED_ENV}: match seed {} (set {SEED_ENV}={} to replay this match)",
        rng.seed, rng.seed
    );
}

// ---------------------------------------------------------------------------
// Core plugin: initial spawns, death, supply recount, win condition
// ---------------------------------------------------------------------------

pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        // Content data first, before a single system, resource or spawn can
        // read a stat. The tables are `LazyLock`s so this is belt-and-braces
        // — but it is what turns a bad data file into a startup panic that
        // names the offending row, instead of a surprise mid-match.
        crate::data::ensure_loaded();

        // Then the frame order, installed pairwise straight out of
        // `SIM_ORDER` rather than retyped as a `.chain()` tuple — the constant
        // IS the schedule, so the list in DESIGN.md and the list Bevy enforces
        // can never drift apart.
        for pair in SIM_ORDER.windows(2) {
            app.configure_sets(Update, pair[0].before(pair[1]));
        }

        // Races: read once, here, before any spawn. `Races::from_env`
        // defaults both sides to `Kingdom`, so a run with neither variable set
        // is the pre-race game exactly. Inserted rather than `init_resource`d
        // because the environment is the source and `Default` is only the
        // fallback a test wants.
        let races = Races::from_env();
        if races.human != Race::Kingdom || races.claude != Race::Kingdom {
            info!(
                "races: blue={} red={}",
                races.human.name(),
                races.claude.name()
            );
        }

        app.insert_resource(races)
            .init_resource::<NavGrid>()
            .init_resource::<Economies>()
            .init_resource::<GameOver>()
            .init_resource::<HeroRecords>()
            .init_resource::<AiControlled>()
            .init_resource::<ExternallyCommanded>()
            .init_resource::<SquadOrders>()
            .init_resource::<SquadStances>()
            .init_resource::<TechTiers>()
            .init_resource::<TeamResearch>()
            .init_resource::<GameEvents>()
            // Here rather than in `AlarmPlugin` so the resource exists
            // wherever `CorePlugin` does: `bridge.rs`'s snapshot reads it, and
            // a hand-built test app that composes the two without alarm.rs
            // should get an empty list rather than a panic inside Bevy's
            // worker pool (which HANGS the suite rather than failing it).
            .init_resource::<Alarms>()
            .init_resource::<FogGrids>()
            .add_event::<SpawnUnitEvent>()
            .add_event::<SpawnBuildingEvent>()
            .add_event::<CameraFocus>()
            .add_event::<CastAbility>()
            .add_event::<XpDrop>()
            .add_event::<Surrender>()
            .add_event::<MatchReady>()
            .init_resource::<ReadyGate>()
            .add_event::<BountyClaim>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .add_event::<TeleportRequest>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .init_resource::<SimRng>()
            // `FogSet` predates `SimSet` and stays: it is the handle four
            // modules already declare `.after()`. It now lives *inside*
            // `SimSet::Fog`, so both spellings mean the same edge.
            .configure_sets(Update, FogSet.in_set(SimSet::Fog))
            // The alarm evaluator only describes the frame, so it lives in the
            // frame's reporting phase — downstream of `Think` and `Intent`,
            // which is the structural half of "an alarm fires only after the
            // reflex has". Configured here rather than in `AlarmPlugin` so the
            // edge exists even in an app that reads `Alarms` without owning
            // the evaluator.
            .configure_sets(Update, AlarmSet.in_set(SimSet::Feed))
            .add_systems(
                Startup,
                (initial_spawns, apply_env_speed, log_fog_mode, log_seed),
            )
            // Last thing at startup, after bridge.rs has filled `ReadyGate`
            // (it orders itself `.before(ReadyGateHold)`): stopping the clock
            // has to happen before the first `Update` frame integrates a delta.
            .add_systems(Startup, hold_for_ready.in_set(ReadyGateHold))
            .add_systems(
                Update,
                (
                    apply_death.in_set(SimSet::Deaths),
                    // The one producer of knowability. After `apply_death` so
                    // the dead have stopped seeing; ahead of every consumer in
                    // every other module via `FogSet`. Both edges are now also
                    // implied by `SimSet::Deaths` -> `SimSet::Fog`; the
                    // explicit `.after` stays because it is the statement of
                    // intent the set order was derived from.
                    update_fog.in_set(FogSet).after(apply_death),
                    // Reads the keyboard, so: input.
                    speed_hotkeys.in_set(SimSet::Input),
                    // Dev-only synthetic commander. In `Input` because it
                    // emits `CastAbility`, which `SimSet::Combat` consumes
                    // later in the same frame.
                    status_probe.in_set(SimSet::Input),
                ),
            )
            .add_systems(
                Update,
                // Chained, not merely co-located: `regen_health`,
                // `tick_status_effects` and `tick_militia_and_cooldowns` all
                // take `&mut Health`/`&mut StatusEffects`, so leaving them
                // unordered inside one set would move the race rather than
                // remove it. The order is the old declaration order.
                (
                    // First in the phase: whether the match has started at all
                    // is prior to every other question asked here, and the
                    // frame it releases the clock on is the frame the rest of
                    // this chain should already be reading as live.
                    ready_gate,
                    award_xp,
                    hero_progression,
                    regen_health,
                    tick_militia_and_cooldowns,
                    tick_status_effects,
                    recount_supply,
                    recount_tech_tiers,
                    check_game_over,
                )
                    .chain()
                    .in_set(SimSet::Upkeep),
            )
            .add_systems(
                Update,
                (
                    debug_log,
                    // After `apply_death`, so a unit that died this frame is
                    // already gone from the picture the diff walks — the feed
                    // reports losses on the tick they happen, not the next one.
                    // After `FogSet` because the feed is now vision-filtered:
                    // a team is told about hostiles and treasure it can see.
                    // Before the diff, so a claim's id is registered in the
                    // same tick the diff would otherwise report the cache
                    // vanishing anonymously to the team that took it.
                    announce_bounty_claims,
                    produce_game_events,
                    fingerprint_log,
                )
                    .chain()
                    .in_set(SimSet::Feed),
            );
    }
}

/// The opening position: one tier-1 hall and five workers per team — of
/// **that team's race**. The two kinds are the only thing a race changes here;
/// the geometry, the count and the starting bank are identical, because a race
/// is a roster and not a different game.
fn initial_spawns(
    races: Res<Races>,
    mut unit_events: EventWriter<SpawnUnitEvent>,
    mut building_events: EventWriter<SpawnBuildingEvent>,
) {
    for team in [Team::Human, Team::Claude] {
        let race = races.get(team);
        let base = team.base_pos();
        building_events.write(SpawnBuildingEvent {
            kind: race_hall(race),
            team,
            pos: base,
            completed: true,
        });
        for i in 0..5 {
            let toward_center = -base.normalize();
            let side = Vec3::new(-toward_center.z, 0.0, toward_center.x);
            let pos = base + toward_center * 8.0 + side * (i as f32 - 2.0) * 2.5;
            unit_events.write(SpawnUnitEvent {
                kind: race_worker(race),
                team,
                pos,
                rally: None,
                source: None,
                summoned: None,
            });
        }
    }
}

/// Despawn anything whose Health reached zero; free building footprints,
/// snapshot dying heroes for revival, and drop XP for nearby enemy heroes.
///
/// `pub(crate)` only so combat.rs's fixed-clock test harness can register the
/// real one: a simulated duel where the dead keep swinging measures nothing.
/// It is still `CorePlugin`'s system and nobody else's to schedule.
pub(crate) fn apply_death(
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

/// Expire Call-to-Arms militia and temporary summons, and tick every caster's
/// per-ability cooldowns. One system for heroes and buildings alike —
/// `AbilityCooldowns` is the only cooldown store there is.
fn tick_militia_and_cooldowns(
    time: Res<Time>,
    mut commands: Commands,
    militia: Query<(Entity, &Militia)>,
    summons: Query<(Entity, &Summoned)>,
    mut cooldowns: Query<(Entity, &mut AbilityCooldowns)>,
) {
    let now = time.elapsed_secs();
    for (entity, m) in &militia {
        if now >= m.until {
            commands.entity(entity).try_remove::<Militia>();
        }
    }
    // A summon whose time is up simply LEAVES. It is not killed: no death, no
    // bounty, no XP, no corpse for the enemy to have earned — the body was
    // never theirs to take. (Everything derived from live units — supply, the
    // snapshot's army counts, fog — recounts itself every frame, so the
    // departure needs no bookkeeping of its own.)
    for (entity, s) in &summons {
        if let Some(until) = s.until {
            if now >= until {
                commands.entity(entity).try_despawn();
            }
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
/// `pub(crate)` for the same reason as `apply_death`: combat.rs's fixed-clock
/// harness needs the real expiry pass, or a Slow in a simulated duel would
/// last forever.
pub(crate) fn tick_status_effects(
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

/// `BH_STATUS_PROBE=1`: dev instrumentation, off in every normal run.
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
    mut next_sorcerer_report: Local<f32>,
    mut sorcerer_slow_seen: Local<bool>,
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

    // Keep asking the Champion for its probe ability by explicit slot. The
    // executor refuses while it is locked, on cooldown, or short of mana; every
    // cast that does land slows whatever is standing around it.
    if now >= *next_cast {
        *next_cast = now + 5.0;
        for (entity, unit, team, hero) in &heroes {
            if unit.kind != UnitKind::Hero {
                continue;
            }
            let list = abilities_of_unit(unit.kind);
            // By NAME, not by slot: the ultimates bead put Warcry in slot 1,
            // and the probe still means the probe.
            if let Some(index) = ability_index_by_id(list, "ProbeChill") {
                let ctx = UnlockCtx::new(hero.level, tiers.get(*team));
                if ability_unlocked(&list[index], ctx) {
                    casts.write(CastAbility::index(entity, index));
                }
            }
        }
    }

    // Sorcerer watch: the SHIPPING half of the same chain, and the one the
    // dev probe cannot fake. Nothing here casts anything — it only reports who
    // is standing under an ability-sourced Slow, which is a thing that can
    // only be true if a Sorcerer auto-cast at a real enemy in a real fight.
    // Filtered on `StatusSource::Ability` precisely so the probe's own
    // `Debug`-sourced Slow on a worker cannot be mistaken for the evidence.
    // Sampled EVERY tick rather than on the report cadence: a Slow lasts 5
    // seconds, so a 15-second sampler would routinely look between two of them
    // and report a working caster as an idle one.
    let sorcerers = units
        .iter()
        .filter(|(_, u, _, _)| u.kind == UnitKind::Sorcerer)
        .count();
    let slowed: Vec<&'static str> = units
        .iter()
        .filter(|(_, _, _, status)| {
            status.is_some_and(|s| {
                s.iter()
                    .any(|e| e.kind == StatusKind::Slow && e.source == StatusSource::Ability)
            })
        })
        .map(|(_, u, _, _)| kind_name(u.kind))
        .collect();
    // Gated on a Sorcerer being ALIVE, because the Champion's probe-only
    // `ProbeChill` also writes an ability-sourced Slow and would otherwise
    // claim this latch — the evidence has to be about the shipping unit.
    if sorcerers > 0 && !slowed.is_empty() && !*sorcerer_slow_seen {
        *sorcerer_slow_seen = true;
        info!(
            "[{now:>6.1}s] STATUS PROBE: FIRST Sorcerer Slow has landed in combat — \
             {sorcerers} Sorcerer(s) alive, victims [{}]",
            slowed.join(" ")
        );
    }
    if sorcerers > 0 && now >= *next_sorcerer_report {
        *next_sorcerer_report = now + 15.0;
        info!(
            "[{now:>6.1}s] STATUS PROBE: {sorcerers} Sorcerer(s) alive, {} unit(s) under an \
             ability Slow right now [{}] (ever landed: {})",
            slowed.len(),
            slowed.join(" "),
            *sorcerer_slow_seen,
        );
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

// ---------------------------------------------------------------------------
// Determinism fingerprint
// ---------------------------------------------------------------------------

/// `BH_FINGERPRINT=<game seconds>`: log a hash of the whole simulation state
/// at fixed game-time intervals. Off by default and pure output — it never
/// touches game state, which is why it sits in `SimSet::Feed`.
///
/// The point is falsifiability. "Deterministic" is a claim about every float
/// in the world, so the check hashes every float in the world: raw IEEE bits
/// of each unit's, building's and cache's position and health, plus the entity
/// id that owns them and both economies. Two runs that agree on this line at
/// every interval agree on the match; the first interval where they differ is
/// the first sample after they diverged.
pub const FINGERPRINT_ENV: &str = "BH_FINGERPRINT";

fn fingerprint_interval() -> Option<f32> {
    std::env::var(FINGERPRINT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|s| *s > 0.0)
}

/// FNV-1a, hand-rolled on purpose: `DefaultHasher` is `RandomState`-seeded and
/// would produce a different number every process, which is the exact failure
/// this function exists to detect.
fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Neutral entities — treasure caches — belong to no team, so they get their
/// own tag rather than being squeezed into `Team`.
const FP_NEUTRAL: u8 = 2;

fn fp_team(team: Team) -> u8 {
    match team {
        Team::Human => 0,
        Team::Claude => 1,
    }
}

/// One entity's contribution, as a canonical byte string.
///
/// `aux` is the second float worth watching, and what it means depends on the
/// entity: a cache's expiry deadline, and nothing (0.0) for a unit or
/// building, whose max HP is a constant of their kind and would add no
/// information.
fn fingerprint_record(id: u64, team: u8, name: &str, pos: Vec3, hp: f32, aux: f32) -> Vec<u8> {
    let mut v = Vec::with_capacity(44 + name.len());
    v.extend_from_slice(&id.to_le_bytes());
    v.push(team);
    v.extend_from_slice(name.as_bytes());
    v.extend_from_slice(&pos.x.to_bits().to_le_bytes());
    v.extend_from_slice(&pos.y.to_bits().to_le_bytes());
    v.extend_from_slice(&pos.z.to_bits().to_le_bytes());
    v.extend_from_slice(&hp.to_bits().to_le_bytes());
    v.extend_from_slice(&aux.to_bits().to_le_bytes());
    v
}

#[allow(clippy::type_complexity)]
fn fingerprint_log(
    time: Res<Time>,
    mut due_at: Local<f32>,
    economies: Res<Economies>,
    units: Query<(Entity, &Unit, &Team, &Health, &Transform)>,
    buildings: Query<(Entity, &Building, &Team, &Health, &Transform)>,
    // Caches are in here because they are the ONE thing `BH_SEED` actually
    // steers. A fingerprint that skipped them could not tell two seeds apart
    // until an army happened to walk onto one, which is a check that passes
    // for the wrong reason.
    bounties: Query<(Entity, &Bounty, &Transform)>,
) {
    let Some(step) = fingerprint_interval() else {
        return;
    };
    let now = time.elapsed_secs();
    if now < *due_at {
        return;
    }
    *due_at = now + step;

    // Sorted, so the hash describes the WORLD and not the order Bevy happened
    // to hand us its archetypes. An archetype-order change is still visible —
    // it moves entity ids, which are inside the records.
    let mut records: Vec<Vec<u8>> = Vec::new();
    for (e, unit, team, hp, tf) in &units {
        records.push(fingerprint_record(
            e.to_bits(),
            fp_team(*team),
            kind_name(unit.kind),
            tf.translation,
            hp.current,
            0.0,
        ));
    }
    for (e, b, team, hp, tf) in &buildings {
        records.push(fingerprint_record(
            e.to_bits(),
            fp_team(*team),
            building_name(b.kind),
            tf.translation,
            hp.current,
            0.0,
        ));
    }
    for (e, bounty, tf) in &bounties {
        records.push(fingerprint_record(
            e.to_bits(),
            FP_NEUTRAL,
            "Bounty",
            tf.translation,
            bounty.gold as f32,
            bounty.expires_at,
        ));
    }
    records.sort_unstable();

    let mut h = FNV_OFFSET;
    for r in &records {
        h = fnv1a(r, h);
    }
    for team in [Team::Human, Team::Claude] {
        let e = economies.get(team);
        h = fnv1a(&e.gold.to_le_bytes(), h);
        h = fnv1a(&e.lumber.to_le_bytes(), h);
        h = fnv1a(&e.supply_used.to_le_bytes(), h);
        h = fnv1a(&e.supply_cap.to_le_bytes(), h);
    }

    let (hu, cl) = (economies.get(Team::Human), economies.get(Team::Claude));
    info!(
        "FINGERPRINT t={:.2} n={} human={}g/{}l/{}s claude={}g/{}l/{}s hash={h:016x}",
        now,
        records.len(),
        hu.gold,
        hu.lumber,
        hu.supply_used,
        cl.gold,
        cl.lumber,
        cl.supply_used,
    );
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

/// `BH_SPEED=4 cargo run` — accelerated headless-ish testing.
///
/// Ignored under `BH_FIXED_DT`: there the step size IS the tick, and
/// multiplying the two would silently turn a 0.05s tick into a 0.4s one, which
/// is not "the same match faster" but a different, coarser match.
fn apply_env_speed(mut time: ResMut<Time<Virtual>>) {
    if let Ok(raw) = std::env::var("BH_SPEED") {
        if let Ok(speed) = raw.parse::<f32>() {
            if let Some(dt) = fixed_step_from_env() {
                info!("BH_SPEED={raw} ignored: {FIXED_DT_ENV}={dt} sets the tick");
                return;
            }
            let speed = speed.clamp(0.1, 16.0);
            time.set_relative_speed(speed);
            info!("BH_SPEED: game speed set to {speed}x");
        }
    }
}

// ---------------------------------------------------------------------------
// Fixed-tick clock
// ---------------------------------------------------------------------------

/// `BH_FIXED_DT=0.05`: advance the clock by exactly this many seconds per
/// frame instead of by however long the frame took.
pub const FIXED_DT_ENV: &str = "BH_FIXED_DT";

/// The smallest step worth allowing, and the largest. A tick below a
/// millisecond makes a match take forever to simulate; a tick above a quarter
/// second is coarser than the fog recompute and the projectile hit test, both
/// of which would start skipping.
const FIXED_DT_MIN: f64 = 0.001;
const FIXED_DT_MAX: f64 = 0.25;

/// The configured tick, in seconds, if `BH_FIXED_DT` names a sane one.
pub fn fixed_step_from_env() -> Option<f64> {
    std::env::var(FIXED_DT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|d| (FIXED_DT_MIN..=FIXED_DT_MAX).contains(d))
}

/// Bevy's own hook for a hand-driven clock, configured from the environment.
///
/// This is what makes a headless replay independent of the machine it runs on.
/// Normally `TimePlugin` derives the frame delta from the wall clock, so every
/// accumulator in the sim — attack cooldowns, construction progress,
/// projectile flight, the `on_timer` gates — integrates a number that depends
/// on how fast the CPU happened to be that second. `ManualDuration` replaces
/// that with a constant, and two runs of the same seed then see the same
/// numbers in the same frames.
///
/// It drives `Time<Real>` as well as `Time<Virtual>`, which matters more than
/// it looks: bridge.rs paces its snapshot writes and command polls off the
/// real clock, so on the wall clock the frame an external order lands on would
/// be a property of the host rather than of the match.
pub fn fixed_time_strategy() -> Option<bevy::time::TimeUpdateStrategy> {
    fixed_step_from_env().map(|dt| {
        bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(dt))
    })
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
    if game_over.decided() {
        surrenders.clear();
        return;
    }
    if let Some(surrender) = surrenders.read().next() {
        info!(
            "{:?} surrenders at t={:.0}s — {:?} wins",
            surrender.team,
            time.elapsed_secs(),
            surrender.team.enemy()
        );
        game_over.decide(surrender.team.enemy(), GameOverReason::Surrender);
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
            game_over.decide(team.enemy(), GameOverReason::Razed);
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
pub const BUILDING_HURT_FRAC: f32 = 0.5;
/// A tick that loses this many units of one kind is reported as one line.
const LOSS_AGGREGATE: usize = 3;
/// Smallest body of enemy troops worth an event line. Below this it is a
/// patrol, and the feed's job is attention rather than transcription.
const ARMY_EVENT_MIN: usize = 4;
/// How recently a group must have been observed to be "spotted" rather than
/// merely remembered. Generous in GAME seconds because the feed's cadence is
/// REAL seconds: at `BH_SPEED=16` a whole diff interval is sixteen game
/// seconds, and a window narrower than that would silently stop announcing
/// armies at speed — the exact class of bug the two clocks invite.
///
/// `pub` because alarm.rs asks the identical question of the identical ledger.
/// Two definitions of "recently enough to be news" would be two languages, and
/// the alarm layer's whole claim is that it renders the same facts the feed
/// does with the running default attached.
pub const ARMY_EVENT_FRESH_S: f32 = 20.0;
/// Same ground, same news: an army near a place already reported stays quiet
/// this long. Rate-limiting re-sightings is not politeness, it is the
/// difference between a feed and a stream.
const ARMY_REANNOUNCE_S: f32 = 60.0;
/// How near counts as the same place for that suppression. Wider than
/// `GROUP_RADIUS` so an army that shuffles across its own frontage does not
/// re-announce itself as a second army.
const ARMY_SAME_PLACE: f32 = GROUP_RADIUS * 1.5;
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
/// at one second per second no matter what `BH_SPEED` is doing.
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

impl EventSeverity {
    /// The wire word. Added with alarms, which are the first thing the bridge
    /// reports a severity for — `events` never carried one, because a reader
    /// with the message text and all the time in the world can grade its own
    /// news, and an `alarms` entry is a triage list where it cannot.
    pub fn as_str(self) -> &'static str {
        match self {
            EventSeverity::Info => "info",
            EventSeverity::Warning => "warning",
            EventSeverity::Critical => "critical",
        }
    }
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
    /// Caches claimed since the last diff, as `(cache id, claiming team)`.
    /// Written by `announce_bounty_claims`, read and cleared by the diff.
    ///
    /// It accumulates rather than being cleared per frame because the two run
    /// on different clocks: claims are swept every few game-seconds by
    /// bounty.rs, the diff every real second. An id has to survive the gap or
    /// the claimer gets the anonymous `bounty gone` line for its own cache.
    claims: Vec<(u64, Team)>,
}

impl Default for GameEvents {
    fn default() -> Self {
        GameEvents {
            human: TeamFeed::default(),
            claude: TeamFeed::default(),
            timer: Timer::from_seconds(EVENT_INTERVAL, TimerMode::Repeating),
            force: true,
            next_seq: 1,
            claims: Vec::new(),
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
    ///
    /// Every map in this memo is a `BTreeMap`. The diff walks them to build
    /// event text and to average positions into a centroid; hash order would
    /// make both the line order AND the float sum differ between runs.
    units: std::collections::BTreeMap<u64, (UnitKind, [f32; 2])>,
    /// own building id -> (kind, position, hp, max_hp)
    buildings: std::collections::BTreeMap<u64, (BuildingKind, [f32; 2], f32, f32)>,
    hero_alive: bool,
    hero_level: u32,
    /// Last place the hero was seen, so "hero died" still has somewhere to
    /// point a camera after the entity is gone.
    hero_pos: [f32; 2],
    /// Latched so "hero low" fires once per crossing rather than every tick.
    hero_low: bool,
    threat: usize,
    squad_members: std::collections::BTreeMap<u8, usize>,
    /// Largest membership seen since each squad was last empty. A squad that
    /// bleeds out one member per tick is still a squad that got wiped, so the
    /// report keys off this rather than the previous tick's count.
    squad_peak: std::collections::BTreeMap<u8, usize>,
    /// Last known centre of mass per squad — where to look when one is wiped.
    squad_pos: std::collections::BTreeMap<u8, [f32; 2]>,
    /// bounty entity id -> (position, gold, expiry deadline). Bounties are the
    /// one thing in this memo that isn't own-team: treasure is neutral.
    bounties: std::collections::BTreeMap<u64, ([f32; 2], u32, f32)>,
    /// `(centroid, game time)` of each enemy army already announced.
    ///
    /// Keyed by PLACE rather than by group identity, and that is the whole
    /// trick. A group has no stable id — it is re-clustered from scratch every
    /// time anyone asks, and one casualty or one straggler renumbers it — so
    /// suppressing repeats by identity would suppress nothing. What a reader
    /// actually means by "I already know about that army" is "I already know
    /// about an army *there*", and ground holds still.
    ///
    /// A `Vec` and not a map: it is pruned to the last `ARMY_REANNOUNCE_S` and
    /// bounded by how many distinct places an army can be at once, and it must
    /// iterate in insertion order for the event lines to be reproducible.
    announced_armies: Vec<([f32; 2], f32)>,
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
/// Tell the claiming team it claimed. Round-9 AAR (`wc3clone-azo`): the feed's
/// only word on a cache was the unattributed `bounty gone`, which is what a
/// watcher *observes* — and the team standing on the cache observed exactly
/// that too, then had to diff its own gold against harvest income arriving in
/// the same second to work out whether it had won the race or lost it.
///
/// A claim is a discrete act with no lasting trace, so it goes through
/// `GameEvents::push` rather than the once-a-second diff: there is nothing in
/// two consecutive pictures of the world that says who took the gold. That is
/// also why the diff could never have answered this — the fix is not a better
/// diff, it is the one fact only the claim event carries.
///
/// Pushed to `claim.team` and to nobody else. **The asymmetry is deliberate
/// and is the fog rule, not an oversight** — see docs/FOG.md: the enemy still
/// gets only `bounty gone`, and only if they were looking at the spot. Who
/// took a cache is not visible in any snapshot, so telling them would hand out
/// intel the map does not contain.
fn announce_bounty_claims(
    time: Res<Time>,
    mut claims: EventReader<BountyClaim>,
    mut feed: ResMut<GameEvents>,
) {
    let now = time.elapsed_secs();
    for claim in claims.read() {
        feed.claims.push((claim.id, claim.team));
        feed.push(
            claim.team,
            now,
            format!("we claimed the cache (+{}g)", claim.gold),
            EventSeverity::Info,
            Some(claim.pos),
        );
    }
}

/// `pub` so alarm.rs can declare a hard ordering edge against it. Both
/// systems live in `SimSet::Feed` and both write `GameEvents`, and Bevy would
/// otherwise leave two `ResMut` writers in one set to the executor — which
/// would make `seq` numbering a property of the thread pool rather than of the
/// seed.
pub fn produce_game_events(
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

    // Cloned out before the per-team loop takes a mutable borrow of the memo.
    // Two entries at the very most; a Vec is the honest size here.
    let claims = feed.claims.clone();
    feed.claims.clear();

    for team in [Team::Human, Team::Claude] {
        let mine: Vec<u64> = claims
            .iter()
            .filter(|(_, t)| *t == team)
            .map(|(id, _)| *id)
            .collect();
        let produced = diff_team(
            team,
            now,
            &mut feed.team_mut(team).memo,
            &units,
            &buildings,
            &bounties,
            &squad_orders,
            fog.get(team),
            &mine,
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
    // Caches THIS team claimed since the last tick. `announce_bounty_claims`
    // has already told it so by name; the anonymous `bounty gone` line below
    // would only be the same news told worse.
    my_claims: &[u64],
) -> Vec<(String, EventSeverity, Option<Vec3>)> {
    // `BTreeMap`, not `HashMap`, for every scratch map below: each one is
    // walked to produce ordered output (event lines) or to sum floats
    // (centroids), so all of them must iterate in key order rather than in
    // std's per-process hash order.
    use std::collections::BTreeMap;

    let home = me.base_pos();

    // --- gather the current picture -------------------------------------
    let mut cur_units: BTreeMap<u64, (UnitKind, [f32; 2])> = BTreeMap::new();
    let mut hero_alive = false;
    let mut hero_level = memo.hero_level;
    let mut hero_frac = 1.0f32;
    let mut hero_pos = memo.hero_pos;
    let mut hostiles: Vec<[f32; 2]> = Vec::new();
    let mut members: BTreeMap<u8, usize> = BTreeMap::new();
    let mut squad_points: BTreeMap<u8, Vec<[f32; 2]>> = BTreeMap::new();
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

    let mut cur_buildings: BTreeMap<u64, (BuildingKind, [f32; 2], f32, f32)> = BTreeMap::new();
    for b in buildings {
        if b.team == me {
            cur_buildings.insert(b.id, (b.kind, b.pos, b.hp, b.max_hp));
        }
    }

    let threat = hostiles.len();

    // What this team can actually see of the treasure on the map right now.
    let seen_bounties: BTreeMap<u64, ([f32; 2], u32, f32)> = bounties
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
    let mut lost: BTreeMap<&'static str, Vec<[f32; 2]>> = BTreeMap::new();
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
            // ...unless it was OURS, in which case we have already been told
            // so, with the gold attached.
            if now + BOUNTY_EXPIRY_EPS < expires_at && !my_claims.contains(&id) {
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

    // --- enemy armies spotted --------------------------------------------
    // The aggregate view, pushed rather than polled. Both renderers get the
    // same line for the same reason the rest of the feed exists: a commander
    // that has to notice an army by diffing two intel sections is paying the
    // polling latency trigger.rs was written to delete, and a human who has to
    // spot one by watching the minimap is paying it too.
    //
    // Fog-honest for free — `army_groups()` clusters THIS team's ledger, which
    // cannot contain a unit this team never saw.
    memo.announced_armies
        .retain(|(_, t)| now - *t <= ARMY_REANNOUNCE_S);
    for group in fog.army_groups() {
        if group.size < ARMY_EVENT_MIN {
            continue;
        }
        // Remembered is not spotted. Without this gate a ninety-second-old
        // rumour would be announced as news the moment the suppression window
        // on its patch of ground expired, and the feed would report armies
        // that were reported an hour ago.
        if now - group.t_seen > ARMY_EVENT_FRESH_S {
            continue;
        }
        let here = [group.centroid.x, group.centroid.z];
        if memo
            .announced_armies
            .iter()
            .any(|(p, _)| (p[0] - here[0]).hypot(p[1] - here[1]) <= ARMY_SAME_PLACE)
        {
            continue;
        }
        memo.announced_armies.push((here, now));
        out.push((
            format!(
                "enemy army spotted: ~{} ({}) {}",
                group.size,
                group.summary(),
                place_name(group.centroid, me)
            ),
            EventSeverity::Warning,
            Some(group.centroid),
        ));
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
// Alarms — forced re-decisions that default to continue and never act
// ---------------------------------------------------------------------------
//
// docs/AFFORDANCES.md § Alarms, plan item 4. The contract half lives here for
// the reason `TriggerWhen`/`Triggers` do: alarm.rs computes, bridge.rs renders,
// and a type two modules speak through is an integrator type. The evaluator —
// every predicate, every sentence — is alarm.rs and nothing here evaluates.
//
// Three properties are the whole design, and each one is structural rather
// than a matter of anybody remembering it:
//
//   * **An alarm never acts.** There is no `Intent` in this section, no
//     `SubmitIntent`, no `Order`. `Alarms` is a `Vec<Alarm>` of strings and
//     the evaluator's only other write is a line on the team's own feed. The
//     one path from a player to the world is still `Intent` → `apply_intents`,
//     and an alarm is not on it.
//   * **An alarm fires only AFTER the reflex has.** Two mechanisms, both
//     load-bearing. The evaluator sits in `SimSet::Feed`, downstream of
//     `Think` (doctrine and the trigger evaluator) and `Intent` (where what
//     they submitted is compiled) in the SAME frame — so by the time an alarm
//     is computed, the fast tier has already answered. And each kind carries
//     a `grace_s` from `assets/data/alarms.ron`: the condition must hold that
//     long before the alarm is raised, which is the reflex's window expressed
//     as a number a balance-tuner can move. The payoff of an alarm is
//     attention, not speed (AFFORDANCES.md), and a mechanism that could beat
//     the reflex would be claiming the wrong one.
//   * **Every alarm names its running default.** `Alarm::running_default` is
//     mandatory, not optional, because the line the design asks for is not
//     "base under attack — respond!" but "base under attack — home-guard is
//     recalling squad 1 (ETA 22s)". A silent commander gets that outcome; the
//     alarm exists so the choice to stay silent is a choice.
//
// Levels and edges, the distinction docs/BUILDER_BRIEF.md §6.11 lost an arena
// match to: the `Alarms` list is a **status**, present in every snapshot for
// exactly as long as the condition holds and refreshed each sweep so its ETA
// and its counts stay current. The feed line is an **edge** — one on the way
// in, one on the way out, and nothing in between. Told once that something is
// standing and never told it stopped, a reader has to poll, which is the
// polling this whole layer deletes.

/// The four conditions worth interrupting a commander for.
///
/// A closed set on purpose. AFFORDANCES.md's argument is that a small
/// commander drowns in an open-ended question every cycle, so the alarm layer
/// is valuable in proportion to how little it says: four events force a fresh
/// choice and *everything else defaults to continue*. A fifth kind should have
/// to argue that a round was lost for want of it, the way each of these four
/// can (r21's unflagged income collapse, r23's stale-contact reads).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Deserialize)]
pub enum AlarmKind {
    /// A body of enemy troops at or above the threshold is in this team's
    /// **sightings ledger**. Memory, not sight — see `FogGrid::army_groups`.
    EnemyArmySighted,
    /// One of this team's own squads has fallen below the threshold fraction
    /// of its pooled health.
    SquadBelowHalf,
    /// The gold has stopped: every mine this team's halls work is dry, or
    /// there are fewer workers on gold than the threshold.
    IncomeCollapse,
    /// Buildings are being hit in two or more distinct **places** at once —
    /// the alarm r23's blue seat asked for, and the one that carries a recall
    /// ETA because "full recall or sacrifice the expansion?" is the question.
    PlacesUnderAttack,
}

/// Every alarm kind, in the order they are evaluated and reported. The
/// `alarms.ron` loader refuses to start if any of them has no row.
pub const ALL_ALARM_KINDS: [AlarmKind; 4] = [
    AlarmKind::EnemyArmySighted,
    AlarmKind::SquadBelowHalf,
    AlarmKind::IncomeCollapse,
    AlarmKind::PlacesUnderAttack,
];

impl AlarmKind {
    /// The wire id. Stable forever: a commander that learns to key off
    /// `"income_collapse"` must keep working.
    pub fn id(self) -> &'static str {
        match self {
            AlarmKind::EnemyArmySighted => "enemy_army_sighted",
            AlarmKind::SquadBelowHalf => "squad_below_half",
            AlarmKind::IncomeCollapse => "income_collapse",
            AlarmKind::PlacesUnderAttack => "places_under_attack",
        }
    }

    /// The short English name, for the clearing edge on the feed. The firing
    /// edge does not use it — it carries the whole fact instead, because
    /// "enemy army sighted" is a category and "enemy army of 9 near the east
    /// ford" is news.
    pub fn label(self) -> &'static str {
        match self {
            AlarmKind::EnemyArmySighted => "enemy army sighted",
            AlarmKind::SquadBelowHalf => "squad below half strength",
            AlarmKind::IncomeCollapse => "income collapse",
            AlarmKind::PlacesUnderAttack => "multiple places under attack",
        }
    }
}

/// One alarm kind's two numbers, from `assets/data/alarms.ron`.
///
/// In data rather than in code because these are the knobs the arena will
/// actually turn: `threshold` is "how big is big" and `grace_s` is "how long
/// does the fast tier get". A round that wants a jumpier or a calmer harness
/// should be a `BH_DATA_DIR` away, not a rebuild — and the scaffold version a
/// round was played under is meant to be recorded in its `ruleset`
/// (AFFORDANCES.md constraint 3), which is easier when it is a file.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlarmTuning {
    /// What "enough" means, per kind:
    ///
    /// | kind | `threshold` |
    /// |---|---|
    /// | `EnemyArmySighted` | smallest body of troops worth the interruption, in units |
    /// | `SquadBelowHalf` | pooled-health fraction, `0..=1` |
    /// | `IncomeCollapse` | fewest workers on gold that still counts as income |
    /// | `PlacesUnderAttack` | how many distinct places at once |
    pub threshold: f32,
    /// **The reflex's window.** Game seconds the condition must hold before
    /// the alarm is raised. Not a debounce: it is the statement that the
    /// fastest tier that can hold this decision gets it first, expressed as
    /// the only thing a schedule cannot express — a duration.
    pub grace_s: f32,
}

/// One standing alarm, as both seats' renderers read it.
///
/// Everything here is derived from what its OWNING team may know. There is no
/// field an omniscient reader could fill that a fog-honest one could not: the
/// enemy half comes from the sightings ledger, and every other half is the
/// team's own units, buildings and doctrine.
#[derive(Clone, Debug)]
pub struct Alarm {
    pub kind: AlarmKind,
    /// **The triggering fact**, in the words the feed and the HUD use for the
    /// same thing. Refreshed every sweep while the alarm stands.
    pub fact: String,
    /// **What is already being done about it** — the reflex tier's answer,
    /// named. Never empty: when nothing is standing, this says so in those
    /// words, because "nothing is happening automatically" is the single most
    /// useful thing a commander can be told and the thing an alarm that only
    /// shouted would hide.
    pub running_default: String,
    /// Game time the alarm STARTED STANDING — when it fired, not when the
    /// condition first held. (The condition held `grace_s` earlier; the two
    /// are one subtraction apart and this is the one that matches the feed.)
    pub since_t: f32,
    pub severity: EventSeverity,
    /// Seconds until the running default arrives, when the running default is
    /// a movement. `None` when nothing is in transit — and `None` is a real
    /// answer, not a missing one: it means the default is to stand still.
    pub eta_s: Option<f32>,
    /// Where it is happening, so the HUD can ping it and a camera can go
    /// there. `None` for an alarm with no one place (income collapse).
    pub pos: Option<Vec3>,
}

/// What one team's one alarm kind is doing between sweeps.
#[derive(Default, Clone, Copy)]
struct AlarmState {
    /// When the condition began holding continuously, or `None` while it does
    /// not hold. Reset the moment it lapses, so a condition that flickers
    /// never accumulates its way past the grace window.
    held_since: Option<f32>,
    /// Is the alarm currently standing (i.e. in the team's list)?
    firing: bool,
}

/// A transition, and the only thing that earns a line on the feed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlarmEdge {
    Fired,
    Cleared,
}

/// Per-team standing alarms, plus the little state machine that decides when
/// one starts and stops standing.
///
/// `alarm.rs` is the only writer, through `observe` alone. Readers get a slice
/// and must name a team to get anything at all — the same shape `GameEvents`
/// uses, and for the same reason: a renderer for one seat has no way to ask
/// for the other's.
#[derive(Resource, Default)]
pub struct Alarms {
    human: Vec<Alarm>,
    claude: Vec<Alarm>,
    /// `BTreeMap` and not a `HashMap`, on the rule that governs every
    /// collection on a gameplay path here: std's hasher reseeds per process.
    /// This one only decides *event ordering*, which is exactly the kind of
    /// thing that makes two runs of one seed produce two different logs.
    states: std::collections::BTreeMap<(Team, AlarmKind), AlarmState>,
}

impl Alarms {
    /// That team's standing alarms, in `ALL_ALARM_KINDS` order.
    pub fn get(&self, team: Team) -> &[Alarm] {
        match team {
            Team::Human => &self.human,
            Team::Claude => &self.claude,
        }
    }

    /// One kind's verdict for one team, this sweep.
    ///
    /// `raised` is `Some` when the condition holds — carrying the fact and the
    /// running default the caller has already worded — and `None` when it does
    /// not. The return value is the EDGE, if there was one, so the caller
    /// writes exactly one feed line per transition and none for the long
    /// middle where the alarm is merely still true.
    ///
    /// The grace window is applied HERE rather than in each predicate, so no
    /// alarm can be added later that forgets to let the reflex go first.
    pub fn observe(
        &mut self,
        team: Team,
        kind: AlarmKind,
        now: f32,
        grace_s: f32,
        raised: Option<Alarm>,
    ) -> Option<AlarmEdge> {
        let state = self.states.entry((team, kind)).or_default();
        let (edge, standing) = match raised {
            // The condition has lapsed. Clear, and owe the reader the exit
            // edge if we ever told them about the entry one.
            None => {
                state.held_since = None;
                let was = std::mem::replace(&mut state.firing, false);
                (was.then_some(AlarmEdge::Cleared), None)
            }
            Some(mut alarm) => {
                let held = *state.held_since.get_or_insert(now);
                if state.firing {
                    // Still true: refresh the wording, keep the clock.
                    (None, Some((alarm, None)))
                } else if now - held >= grace_s {
                    state.firing = true;
                    alarm.since_t = ev_r1(now);
                    (Some(AlarmEdge::Fired), Some((alarm, Some(ev_r1(now)))))
                } else {
                    // Inside the reflex's window. The condition is true and
                    // the commander is deliberately not being told yet.
                    (None, None)
                }
            }
        };
        let list = match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        };
        match standing {
            None => list.retain(|a| a.kind != kind),
            Some((mut alarm, fired_at)) => {
                match list.iter_mut().find(|a| a.kind == kind) {
                    Some(slot) => {
                        // A standing alarm keeps the time it started standing;
                        // everything else about it is this sweep's truth.
                        alarm.since_t = fired_at.unwrap_or(slot.since_t);
                        *slot = alarm;
                    }
                    None => {
                        list.push(alarm);
                        // Sorted by kind rather than by arrival, so the array
                        // a commander diffs between two polls depends on WHICH
                        // alarms stand and never on the order they arrived in.
                        list.sort_by_key(|a| a.kind);
                    }
                }
            }
        }
        edge
    }
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
mod determinism_tests {
    use super::*;

    /// The claim `BH_SEED` makes: a seed IS the match's randomness. If two
    /// generators built from one seed ever disagree, replay is a fiction and
    /// every downstream guarantee in DESIGN.md § Determinism goes with it.
    #[test]
    fn a_seed_replays_the_same_number_stream() {
        use rand::Rng;
        let draw = |seed: u64| {
            let mut sim = SimRng::from_seed(seed);
            let rng = sim.rng();
            (0..64)
                .map(|_| rng.gen_range(0.0f32..1.0))
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(42), draw(42), "one seed, two different streams");
        assert_ne!(
            draw(42),
            draw(43),
            "two seeds produced the same stream — the seed is being ignored"
        );
    }

    /// `SIM_ORDER` is not documentation, it is what `CorePlugin` feeds to
    /// `configure_sets` pairwise. A duplicate would silently install a
    /// contradictory edge and a missing phase would leave its systems floating
    /// free again, which is the exact bug the enum was added to close.
    #[test]
    fn the_frame_order_names_every_phase_exactly_once() {
        let mut seen: Vec<SimSet> = SIM_ORDER.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            SIM_ORDER.len(),
            "a phase appears twice in SIM_ORDER"
        );
        // The edges that were load-bearing before `SimSet` existed, and that
        // the set order had to preserve rather than invent.
        let at = |s: SimSet| SIM_ORDER.iter().position(|x| *x == s).unwrap();
        assert!(at(SimSet::Deaths) < at(SimSet::Fog), "the dead still see");
        assert!(
            at(SimSet::Fog) < at(SimSet::Intent),
            "an order would be judged against last frame's visibility"
        );
        assert!(
            at(SimSet::Input) < at(SimSet::Intent),
            "input would compile a frame late"
        );
        assert!(
            at(SimSet::CoCommand) < at(SimSet::Intent),
            "an approved proposal would compile a frame late"
        );
        // And the choices this bead made, asserted so a later reshuffle has to
        // argue with a test rather than quietly change what a match is.
        assert!(
            at(SimSet::AiThink) < at(SimSet::Think),
            "doctrine would execute postures the commander has not written yet"
        );
        assert!(
            at(SimSet::Think) < at(SimSet::Intent),
            "a standing order would overrule a direct one issued the same frame"
        );
        assert!(
            at(SimSet::Movement) < at(SimSet::Combat),
            "units would shoot from where they used to stand"
        );
        assert!(
            at(SimSet::Bounty) < at(SimSet::Economy),
            "treasure claimed this frame would bank next frame"
        );
        assert!(
            at(SimSet::Economy) < at(SimSet::Feed),
            "the snapshot would report a stale bank balance"
        );
    }

    /// The fingerprint has to be a function of the WORLD. Bevy hands entities
    /// back in archetype order, which is stable within a run but is not what
    /// we are measuring, so the records are sorted before hashing — otherwise
    /// a harmless archetype reshuffle would read as a divergence and the check
    /// would cry wolf until nobody trusted it.
    #[test]
    fn the_fingerprint_describes_the_world_not_the_visit_order() {
        let a = fingerprint_record(1, 0, "Spearman", Vec3::new(1.0, 0.0, 2.0), 40.0, 0.0);
        let b = fingerprint_record(2, 1, "Raider", Vec3::new(-3.0, 0.0, 4.0), 55.0, 0.0);

        let hash = |mut recs: Vec<Vec<u8>>| {
            recs.sort_unstable();
            recs.iter().fold(FNV_OFFSET, |h, r| fnv1a(r, h))
        };
        assert_eq!(
            hash(vec![a.clone(), b.clone()]),
            hash(vec![b.clone(), a.clone()]),
            "visiting the same two entities in the other order changed the hash"
        );

        // ...and it must still be sensitive to the thing it is watching. One
        // unit one hit point lighter is a different match.
        let a_hurt = fingerprint_record(1, 0, "Spearman", Vec3::new(1.0, 0.0, 2.0), 39.0, 0.0);
        assert_ne!(
            hash(vec![a.clone(), b.clone()]),
            hash(vec![a_hurt, b.clone()]),
            "a health change left the fingerprint unmoved"
        );

        // The neutral slot has to be live too: a treasure cache two world
        // units to the left is the difference between two seeds, and it was
        // invisible to this hash until caches were added to it.
        let cache = |x: f32| {
            fingerprint_record(
                9,
                FP_NEUTRAL,
                "Bounty",
                Vec3::new(x, 0.0, 0.0),
                300.0,
                225.0,
            )
        };
        assert_ne!(
            hash(vec![a.clone(), b.clone(), cache(10.0)]),
            hash(vec![a, b, cache(12.0)]),
            "a cache moving left the fingerprint unmoved"
        );
    }

    /// A hand-rolled FNV-1a rather than `DefaultHasher`, because
    /// `DefaultHasher` is `RandomState`-seeded and reseeds every process — it
    /// would report every pair of runs as divergent. This pins the constant so
    /// the fingerprints in one run's log can be compared against another's.
    #[test]
    fn the_hash_is_the_same_number_in_every_process() {
        assert_eq!(fnv1a(b"bridgehead", FNV_OFFSET), 0x104d_3b9f_fb8d_ced2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rung of the chain of command renders to one compact line, and
    /// those lines are the literal contract: the bridge snapshot's
    /// `units[].why`, the human selection panel and the intent log all print
    /// this exact string. A commander greps one against the other, so the
    /// format is data, not decoration.
    #[test]
    fn every_rung_of_the_chain_answers_in_one_line() {
        let building = Entity::from_raw(42);
        // Spelled from the entity rather than hardcoded: the point of the
        // format is that it JOINS against `buildings[].id` in the snapshot,
        // which is `Entity::to_bits` and nothing else.
        let stamped = format!("template:Barracks#{}", building.to_bits());
        let cases: [(Cause, f32, String); 8] = [
            (
                Cause::Order { verb: "move", source: IntentSource::Bridge },
                123.4,
                "order:move by bridge t=123".to_string(),
            ),
            (
                // The equity claim in one assertion: the same order spelled by
                // a mouse differs from the JSON one ONLY in the source word.
                Cause::Order { verb: "move", source: IntentSource::Ui },
                123.4,
                "order:move by ui t=123".to_string(),
            ),
            (
                Cause::Posture { squad: 1, posture: "push" },
                91.6,
                "posture:push sq1".to_string(),
            ),
            (
                Cause::Posture { squad: 0, posture: "defend" },
                0.0,
                "posture:defend sq0".to_string(),
            ),
            (
                Cause::Policy { policy: "retreat" },
                210.0,
                "policy:retreat t=210".to_string(),
            ),
            (
                Cause::Stamp { how: "template", kind: "Barracks", building },
                12.0,
                stamped,
            ),
            (
                // The third seat, since wc3clone-jem: the scripted commander
                // submits intents like the other two, so its wave reads as an
                // ORDER by `script` rather than as a rung of its own. Nothing
                // in the format is special-cased for it — that IS the claim.
                Cause::Order { verb: "attackmove", source: IntentSource::Script },
                5.0,
                "order:attackmove by script t=5".to_string(),
            ),
            (
                Cause::Instinct { what: "auto-enroll" },
                5.0,
                "instinct:auto-enroll".to_string(),
            ),
        ];
        for (cause, at, expected) in cases {
            let answer = Provenance::new(cause, at).why();
            assert_eq!(answer, expected, "{cause:?} rendered wrong");
            // It shares a line with the doctrine summary in the HUD and a JSON
            // field in the snapshot, so it stays one short line or it stops
            // being readable in either.
            assert!(!answer.contains('\n'), "{answer} is not one line");
            assert!(answer.len() <= 48, "{answer} is too long for the panel");
        }

        // "idle" is the absence of a reason and says so bare — dressing it up
        // as `instinct:idle` would imply the engine had one.
        assert_eq!(Provenance::instinct("idle", 77.0).why(), NO_PROVENANCE);
        assert_eq!(NO_PROVENANCE, "idle");
    }

    /// The split that decides what a unit can say about itself: verbs that
    /// change what it is DOING stamp a reason; verbs that install policy do
    /// not, because their reason only exists on the frame doctrine.rs acts on
    /// them (and then it reads `policy:...`, not `order:...`).
    #[test]
    fn only_behaviour_verbs_stamp_a_reason() {
        let behaviour = [
            r#"{"type":"move","units":[1],"x":1.0,"z":2.0}"#,
            r#"{"type":"attackmove","units":[1],"x":1.0,"z":2.0}"#,
            r#"{"type":"attack","units":[1],"target":9}"#,
            r#"{"type":"harvest","units":[1],"target":9}"#,
            r#"{"type":"return","units":[1]}"#,
            r#"{"type":"follow","units":[1],"target":2}"#,
            r#"{"type":"stop","units":[1]}"#,
            r#"{"type":"build","worker":1,"kind":"Farm","x":0.0,"z":0.0}"#,
        ];
        for case in behaviour {
            let intent: Intent = serde_json::from_str(case).unwrap();
            assert_eq!(
                intent.provenance_verb(),
                Some(intent.verb()),
                "{case} should stamp its own verb"
            );
        }

        let policy_or_spending = [
            r#"{"type":"retreat","units":[1],"below":0.35,"x":1.0,"z":2.0}"#,
            r#"{"type":"leash","units":[1],"x":1.0,"z":2.0,"radius":20.0}"#,
            r#"{"type":"priority","units":[1],"classes":["Hero"]}"#,
            r#"{"type":"autocast","units":[1],"min_enemies":3}"#,
            r#"{"type":"squad","units":[1],"id":1}"#,
            r#"{"type":"posture","id":1,"posture":{"type":"push","x":1.0,"z":2.0}}"#,
            r#"{"type":"template","building":1,"squad":1}"#,
            r#"{"type":"train","building":1,"unit":"Footman"}"#,
            r#"{"type":"rally","building":1,"x":1.0,"z":2.0}"#,
            r#"{"type":"research","building":1,"upgrade":"attack"}"#,
            r#"{"type":"buy","shop":1,"item":"HealingPotion"}"#,
            r#"{"type":"surrender"}"#,
        ];
        for case in policy_or_spending {
            let intent: Intent = serde_json::from_str(case).unwrap();
            assert_eq!(intent.provenance_verb(), None, "{case} should stamp nothing");
        }
    }

    /// The join between the two halves of the record: the log line for an
    /// order carries the same string the units it moved will report, so
    /// "why is that unit attacking?" and "who said so?" are one grep apart.
    #[test]
    fn the_log_tag_and_the_units_answer_are_the_same_string() {
        let intent: Intent =
            serde_json::from_str(r#"{"type":"move","units":[1,2],"x":40.0,"z":40.0}"#).unwrap();
        let mark = IntentMark { source: IntentSource::Bridge, at: 21.5, trigger: None, plan: None };
        let logged = mark.order(intent.provenance_verb().unwrap()).why();
        let on_the_unit = Provenance::new(
            Cause::Order { verb: "move", source: IntentSource::Bridge },
            21.5,
        )
        .why();
        assert_eq!(logged, on_the_unit);
        assert_eq!(logged, "order:move by bridge t=22");
    }

    /// **A stance with no anchor is anchored on home, and says so**
    /// (wc3clone-p91).
    ///
    /// `compile_intent` has always defaulted an omitted stance anchor to the
    /// issuing team's own base — it is what `turtle` means with no argument —
    /// but the sentence rendered `(unspecified)`, which is the wording reserved
    /// for a sentence that names no ground *and is about to be refused*. Three
    /// readers were told a legal order was incoherent: the intent log,
    /// `plans[].current`, and the seat journal a co-commander reads.
    ///
    /// The three spellings below are the whole claim. Anchor omitted and anchor
    /// spelled `"our base"` produce the **identical** sentence, because "our
    /// base" is a built-in region name and not a paraphrase of one; an explicit
    /// coordinate still reads as a coordinate.
    #[test]
    fn an_omitted_stance_anchor_reads_as_home_and_not_as_a_refusal() {
        let defaulted = Intent::Stance {
            squad: 1,
            stance: "turtle".to_string(),
            x: None,
            z: None,
            region: None,
        };
        let named = Intent::Stance {
            squad: 1,
            stance: "turtle".to_string(),
            x: None,
            z: None,
            region: Some("our base".to_string()),
        };
        let radius = StanceKind::Turtle.def().radius;
        assert_eq!(
            defaulted.sentence(),
            named.sentence(),
            "the default IS the built-in region, so the two spellings are one sentence"
        );
        assert!(
            defaulted
                .sentence()
                .contains(&format!("defends our base within {radius:.0}")),
            "{}",
            defaulted.sentence()
        );
        assert!(
            !defaulted.sentence().contains("(unspecified)"),
            "the order was not refused and the log must not read as though it was: {}",
            defaulted.sentence()
        );

        // A given anchor is untouched — this is a default, not an override.
        let placed = Intent::Stance {
            squad: 2,
            stance: "push".to_string(),
            x: Some(12.0),
            z: Some(-8.0),
            region: None,
        };
        assert!(
            placed.sentence().contains("pushes to (12.0, -8.0)"),
            "{}",
            placed.sentence()
        );

        // And a half-given one defaults, because `resolved_point` takes the
        // anchor only when both coordinates arrived. The sentence tracks the
        // compiler rather than guessing at the missing half.
        let half = Intent::Stance {
            squad: 3,
            stance: "harass".to_string(),
            x: Some(12.0),
            z: None,
            region: None,
        };
        assert!(
            half.sentence().contains("pushes to our base"),
            "{}",
            half.sentence()
        );
    }

    /// The template rung: a unit trained by a building that carries standing
    /// doctrine starts life naming that building, and keeps naming it until a
    /// posture or an order re-tasks it. This is the rung a commander uses to
    /// tell "my template is working" from "my template never applied".
    #[test]
    fn a_trained_unit_names_the_building_that_stamped_it() {
        let barracks = Entity::from_raw(7);
        let producer = Some((barracks, BuildingKind::Barracks));

        // A doctrine template is the strongest claim a building can make.
        let templated = spawn_provenance(producer, true, true, 30.0);
        assert_eq!(
            templated.why(),
            format!("template:Barracks#{}", barracks.to_bits())
        );
        // ...and it holds even when the building set no rally at all.
        assert_eq!(
            spawn_provenance(producer, true, false, 30.0).why(),
            templated.why()
        );

        // A bare rally still decided the unit's first order, so it still
        // answers with the building — just a weaker word for what it did.
        assert_eq!(
            spawn_provenance(producer, false, true, 30.0).why(),
            format!("rally:Barracks#{}", barracks.to_bits())
        );

        // Neither: the unit genuinely has no reason yet, and says so. A stale
        // rally (depleted node, dead followee) degrades to exactly this,
        // rather than claiming a destination that no longer exists.
        assert_eq!(spawn_provenance(producer, false, false, 30.0).why(), "idle");
        // The opening workers have no producer at all.
        assert_eq!(spawn_provenance(None, false, false, 0.0).why(), "idle");
    }

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
        assert!(matches!(champion[0].aim(), EffectAtom::Damage { amount, .. } if amount == 45.0));
        assert!(!champion[0].hits_air);
        assert_eq!(champion[0].mana_cost, hero_ability_cost());

        let priestess = abilities_of_unit(UnitKind::Priestess);
        assert_eq!(priestess[0].name, "Heal");
        assert!(priestess[0].hits_air);

        let hall = abilities_of_building(BuildingKind::TownHall);
        assert_eq!(hall[0].name, "CallToArms");
        assert!(abilities_of_unit(UnitKind::Footman).is_empty());
        assert!(abilities_of_building(BuildingKind::Barracks).is_empty());
    }

    /// The shipped ability rows the tests below build fixtures out of. They
    /// used to be `const SLAM` / `const HEAL` / `const CALL_TO_ARMS` in this
    /// file; the rows moved to `assets/data/abilities.ron`, so the tests read
    /// them back through the same accessors the game uses.
    fn slam() -> AbilityDef {
        abilities_of_unit(UnitKind::Hero)[0]
    }
    fn heal() -> AbilityDef {
        abilities_of_unit(UnitKind::Priestess)[0]
    }
    fn call_to_arms() -> AbilityDef {
        abilities_of_building(BuildingKind::TownHall)[0]
    }

    // --- ability framework v3: geometry ------------------------------------

    /// **The back-compat claim, as a table sweep rather than a promise.**
    /// Adding geometry to `AbilityDef` must not have moved a single existing
    /// ability off the caster — the whole v2 catalogue is `Caster` except the
    /// one row this bead deliberately re-aimed.
    #[test]
    fn every_ability_is_caster_centred_except_the_one_that_was_retargeted() {
        let mut targeted: Vec<(&str, &str, Option<f32>)> = Vec::new();
        for &k in &ALL_UNIT_KINDS {
            for a in abilities_of_unit(k) {
                if a.target.is_targeted() {
                    targeted.push((a.name, a.target.name(), a.target.range()));
                }
            }
        }
        for &k in &ALL_BUILDING_KINDS {
            for a in abilities_of_building(k) {
                assert_eq!(
                    a.target,
                    AbilityTarget::Caster,
                    "{} is cast by a building, which has nowhere to walk to",
                    a.name
                );
            }
        }
        targeted.sort_by(|a, b| a.0.cmp(b.0));
        targeted.dedup();
        assert_eq!(
            targeted,
            vec![("Bloodlust", "point", Some(8.0)), ("Slow", "point", Some(9.0))],
            "the two casters' spells are the only targeted abilities in the game; \
             every other row must still centre on its caster"
        );
        // And `Caster` rows carry no range, so nothing can accidentally
        // range-check an ability that has no reach to check.
        assert_eq!(AbilityTarget::Caster.range(), None);
        assert!(!AbilityTarget::Caster.is_targeted());
    }

    /// **The rebalance, in numbers.** Slow stopped being a bubble the Sorcerer
    /// stands in the middle of and became a spell it throws. The trade is the
    /// point: a smaller footprint, a longer reach, and — the number that
    /// matters — the caster no longer has to be anywhere near the victim.
    #[test]
    fn slow_reaches_further_than_it_ever_did_from_further_back() {
        let slow = abilities_of_unit(UnitKind::Sorcerer)[0];
        let AbilityTarget::Point { range } = slow.target else {
            panic!("Slow is cast at a point");
        };
        assert_eq!(range, 9.0);
        assert_eq!(slow.radius, 4.5);

        // v2: a radius-8 bubble on the caster. Total reach 8, and the caster
        // had to be within 8 of whatever it wanted to slow.
        const OLD_RADIUS: f32 = 8.0;
        assert!(
            range + slow.radius > OLD_RADIUS,
            "the targeted spell must reach FURTHER ({} vs {OLD_RADIUS}) or the \
             rebalance is a nerf wearing a feature's clothes",
            range + slow.radius
        );
        // The Sorcerer's own attack range is the honesty check: a spell that
        // asked it to walk closer than its gun does would be no improvement.
        let reach = unit_stats(UnitKind::Sorcerer).range;
        assert!(
            range <= reach,
            "Slow's {range} range must not exceed the Sorcerer's own {reach}"
        );
        // Smaller blanket: area scales with the square.
        assert!(slow.radius * slow.radius < OLD_RADIUS * OLD_RADIUS * 0.4);
        // Everything that made it Slow is untouched — this bead moved geometry
        // and nothing else.
        assert_eq!(slow.power(), 0.4);
        assert_eq!(slow.duration(), 5.0);
        assert_eq!(slow.cooldown, 9.0);
        assert_eq!(slow.mana_cost, 0.0);
        assert!(slow.hits_air);
    }

    /// **The auto-pick rule**, stated as the three things it promises: only a
    /// reachable body may be the centre, the biggest clump wins, and ties go
    /// to the nearest.
    #[test]
    fn the_auto_pick_takes_the_biggest_clump_it_can_reach() {
        let caster = Vec3::ZERO;
        // Two clumps. A pair at ~5 away, a trio at ~8 away.
        let pair = [Vec3::new(5.0, 0.0, 0.0), Vec3::new(6.0, 0.0, 0.0)];
        let trio = [
            Vec3::new(0.0, 0.0, 8.0),
            Vec3::new(1.0, 0.0, 8.0),
            Vec3::new(0.0, 0.0, 9.0),
        ];
        let all: Vec<Vec3> = pair.iter().chain(trio.iter()).copied().collect();

        // Range 9, radius 4.5 — Slow's numbers. The trio wins on count even
        // though the pair is nearer.
        let (index, focus, caught) =
            best_cast_focus(caster, 9.0, 4.5, &all).expect("something in range");
        assert_eq!(caught, 3, "the aim must be the clump, not the nearest body");
        assert!(trio.contains(&all[index]), "derived from the trio");
        assert_eq!(focus, all[index], "a reachable body IS the centre, unclamped");

        // Shorten the reach until the trio is past `range + radius` entirely:
        // the aim falls back to what the caster can still touch rather than to
        // nothing. (At range 3 the far edge is 7.5, short of the trio at 8.)
        let (index, _, caught) =
            best_cast_focus(caster, 3.0, 4.5, &all).expect("the pair is still in reach");
        assert_eq!(caught, 2);
        assert!(pair.contains(&all[index]));

        // Nothing in reach at all ⇒ no aim ⇒ (in combat.rs) no cast and no
        // cooldown spent.
        assert!(best_cast_focus(caster, 0.4, 4.5, &all).is_none());
        assert!(best_cast_focus(caster, 100.0, 4.5, &[]).is_none());

        // Ties break on distance: two equal clumps, the near one wins, and the
        // answer is stable when the slice order is reversed.
        let twins = [Vec3::new(4.0, 0.0, 0.0), Vec3::new(-4.0, 0.0, 0.0)];
        let (near, _, _) = best_cast_focus(Vec3::new(1.0, 0.0, 0.0), 9.0, 1.0, &twins).unwrap();
        assert_eq!(twins[near], Vec3::new(4.0, 0.0, 0.0));
        let flipped = [twins[1], twins[0]];
        let (near, _, _) = best_cast_focus(Vec3::new(1.0, 0.0, 0.0), 9.0, 1.0, &flipped).unwrap();
        assert_eq!(flipped[near], Vec3::new(4.0, 0.0, 0.0));
    }

    /// **The clamp**, which is the difference between an aimer as long-armed
    /// as the player and one that is not. A clump sitting just past `range`
    /// is still catchable — the aim slides up to the caster's reach along the
    /// line towards it — and a clump past `range + radius` genuinely is not.
    #[test]
    fn the_auto_pick_reaches_as_far_as_a_players_click_would() {
        let caster = Vec3::ZERO;
        // Slow's numbers: reach 9, bloom 4.5, so the far edge is 13.5.
        let (range, radius) = (9.0, 4.5);

        // A charge arriving at 11-13: no body is within 9, so a body-only
        // aimer finds nothing. The clamp aims at 9 and catches all three.
        let charge = [
            Vec3::new(0.0, 0.0, 11.0),
            Vec3::new(1.0, 0.0, 11.5),
            Vec3::new(0.0, 0.0, 13.0),
        ];
        let (_, focus, caught) =
            best_cast_focus(caster, range, radius, &charge).expect("a clamped aim exists");
        assert_eq!(caught, 3, "all three are inside 4.5 of the clamped centre");
        assert!(
            (focus.length() - range).abs() < 1e-3,
            "the centre sits exactly at the caster's reach, at {focus:?}"
        );

        // Past the far edge, and nothing is caught at all.
        let distant = [Vec3::new(0.0, 0.0, 14.0)];
        assert!(best_cast_focus(caster, range, radius, &distant).is_none());
        // Exactly on the far edge is still a hit.
        let edge = [Vec3::new(0.0, 0.0, range + radius)];
        assert_eq!(best_cast_focus(caster, range, radius, &edge).unwrap().2, 1);
    }


    /// **A commander reading the catalog must not think a hero costs 400.**
    /// The catalog is the only documentation either seat gets, so the cost
    /// fields have to say the true thing (zero) and the revival price has to
    /// be there beside them — a 0g row with no second number would read as an
    /// oversight, or worse, as a hero with no downside at all.
    ///
    /// `revive_gold`/`revive_lumber` are hero-only and omitted elsewhere: a
    /// Footman row carrying `revive_gold: 0` would invite the question of what
    /// reviving a Footman does.
    #[test]
    fn the_catalog_prices_a_hero_at_zero_and_publishes_what_losing_it_costs() {
        let catalog = game_catalog();
        let mut heroes = 0;
        for row in &catalog.units {
            let is_hero = row.role.starts_with("Hero");
            if !is_hero {
                assert_eq!(
                    (row.revive_gold, row.revive_lumber),
                    (None, None),
                    "{}: only heroes publish a revival price",
                    row.id
                );
                assert!(
                    row.cost_gold > 0 || row.cost_lumber > 0,
                    "{}: everything except a hero costs something to train",
                    row.id
                );
                continue;
            }
            heroes += 1;
            assert_eq!(
                (row.cost_gold, row.cost_lumber),
                (0, 0),
                "{}: the catalog must show training as free",
                row.id
            );
            assert_eq!(
                (row.revive_gold, row.revive_lumber),
                (Some(400), Some(100)),
                "{}: ...and must show what losing it costs",
                row.id
            );
            // Free is not instant, and the catalog is where a commander finds
            // out: 25 seconds at the hall that also makes your workers.
            assert!(row.train_time >= 25.0, "{}: {}s", row.id, row.train_time);
            assert!(row.supply > 0, "{}: a free hero still eats supply", row.id);
        }
        assert_eq!(heroes, 4, "two hero classes per roster, two rosters");
    }

    /// The catalog is the commander's only documentation, so geometry has to
    /// be *in* it — an ability whose target shape you cannot read is one you
    /// can only aim by trial and error.
    #[test]
    fn the_catalog_publishes_target_shape_and_range() {
        let catalog = game_catalog();
        let slow = catalog
            .abilities
            .iter()
            .find(|a| a.id == "Slow")
            .expect("the Sorcerer's Slow is in the catalog");
        assert_eq!(slow.target, "point");
        assert_eq!(slow.target_range, Some(9.0));
        assert_eq!(slow.radius, 4.5);

        let slam = catalog.abilities.iter().find(|a| a.id == "Slam").unwrap();
        assert_eq!(slam.target, "caster");
        assert_eq!(slam.target_range, None, "a caster-centred row publishes no range");
    }

    /// The affordance document (`tools/bridge_view.py --doc`) serves a form's
    /// field domains out of the catalog. Both vocabularies therefore have to be
    /// IN the catalog, and have to agree with the refusals a bad value earns —
    /// a domain list and an error message that disagree teach two languages.
    #[test]
    fn the_catalog_publishes_the_stance_and_selector_vocabularies() {
        let catalog = game_catalog();

        let words: Vec<&str> = catalog.stances.iter().map(|s| s.id).collect();
        assert_eq!(words, vec!["turtle", "stage", "push", "secure", "harass"]);
        assert_eq!(
            words.join(", "),
            stance_words(),
            "the catalog's order is `ALL_STANCES`, which is what the refusal lists"
        );
        let push = catalog.stances.iter().find(|s| s.id == "push").unwrap();
        assert_eq!(push.posture, "push");
        assert_eq!(push.leash, 0.0, "a push that could be recalled is not a push");
        assert_eq!(push.rally, "base");
        assert!(push.priority.contains(&"Siege"));

        // Every phrase served as a domain is a phrase the parser accepts —
        // except `squad <n>`, which is a shape rather than a word.
        let sel = &catalog.selectors;
        for phrase in sel.units.iter().chain(&sel.nodes).chain(&sel.sites) {
            if phrase.contains('<') {
                assert_eq!(*phrase, "squad <n>");
                assert!(parse_selector("squad 3").is_some());
                continue;
            }
            assert!(parse_selector(phrase).is_some(), "catalog serves '{phrase}' but nothing parses it");
        }
        assert!(sel.units.contains(&"my hero") && sel.units.contains(&"all army"));
        assert_eq!(sel.nodes, vec!["nearest tree", "nearest mine"]);
        assert_eq!(sel.sites, vec!["nearest legal site"]);
    }

    /// A synthetic two-ability caster: the shape every content bead will use.
    fn two_ability_list() -> [AbilityDef; 2] {
        [
            slam(),
            AbilityDef {
                name: "TestWarcry",
                effects: &[Effect {
                    atom: EffectAtom::ApplyStatus {
                        status: StatusKind::DamageBuff,
                        magnitude: 0.3,
                        duration: 12.0,
                        targets: AbilityTargets::Allies,
                    },
                    schedule: EffectSchedule::Instant,
                }],
                target: AbilityTarget::Caster,
                mana_cost: 50.0,
                cooldown: 30.0,
                radius: 10.0,
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

        let tiered = [AbilityDef { unlock: AbilityUnlock::TeamTier(TechTier::T2), ..slam() }];
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

        let gated = [AbilityDef { unlock: AbilityUnlock::TeamTier(TechTier::T2), ..slam() }];
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
        let locked = [AbilityDef { unlock: AbilityUnlock::HeroLevel(5), ..slam() }];
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
        assert!(ability_ready(&call_to_arms(), None, Some(&cds), 1));
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

    // --- hero ultimates (T3 content) -----------------------------------------

    #[test]
    fn each_hero_class_has_an_ultimate_in_slot_one_at_level_five() {
        for (kind, id) in [(UnitKind::Hero, "Warcry"), (UnitKind::Priestess, "Sanctuary")] {
            let list = abilities_of_unit(kind);
            assert_eq!(list[1].name, id, "{id} must be the second slot");
            assert_eq!(list[1].unlock, AbilityUnlock::HeroLevel(5));
            // The unlock is a CLIFF at 5, and the tier ladder has no say in it:
            // a T3 team with a level-4 hero still has no ultimate.
            for level in 0..5 {
                assert!(
                    !ability_unlocked(&list[1], UnlockCtx::new(level, TechTier::T3)),
                    "{id} must stay locked at hero level {level}"
                );
            }
            assert!(ability_unlocked(&list[1], UnlockCtx::new(5, TechTier::T1)));
            // Slot 0 is untouched — an ultimate never displaces the basic kit.
            assert!(ability_unlocked(&list[0], UnlockCtx::new(1, TechTier::T1)));
            // `None` selector still means "the first ability I can use", so a
            // level-5 hero's old one-button call sites did not silently move.
            assert_eq!(first_unlocked_ability(list, UnlockCtx::new(5, TechTier::T3)), Some(0));
            // The ultimate is reachable by id, which is what the bridge sends.
            assert_eq!(
                resolve_ability(list, Some(&AbilitySelector::Id(id.to_lowercase())), UnlockCtx::new(5, TechTier::T1)),
                Some(1)
            );
            assert_eq!(
                resolve_ability(list, Some(&AbilitySelector::Id(id.to_string())), UnlockCtx::new(4, TechTier::T3)),
                None,
                "{id} must be unreachable below level 5"
            );
            // Its own cooldown slot: firing the ultimate never blocks Slam/Heal.
            let mut cds = AbilityCooldowns::default();
            cds.start(1, list[1].cooldown);
            assert!(!ability_ready(&list[1], None, Some(&cds), 1));
            assert!(ability_ready(&list[0], None, Some(&cds), 0));
        }
    }

    #[test]
    fn sanctuary_is_one_cast_carrying_two_statuses() {
        let sanctuary = abilities_of_unit(UnitKind::Priestess)[1];
        assert_eq!(sanctuary.status(), Some(StatusKind::HealOverTime));
        assert_eq!(
            sanctuary.extra_status(),
            Some((StatusKind::ArmorBuff, 0.25))
        );
        assert!(sanctuary.heals());
        // Warcry is the single-status shape, and is NOT a heal — the auto-cast
        // trigger keys off exactly this.
        let warcry = abilities_of_unit(UnitKind::Hero)[1];
        assert_eq!(warcry.extra_status(), None);
        assert!(!warcry.heals());
        assert!(slam().extra_status().is_none() && !slam().heals());
        assert!(heal().heals());
    }

    #[test]
    fn ultimate_and_item_magnitudes_arrive_through_effective_stats() {
        let footman = BaseStats::of_unit(UnitKind::Footman);
        let base = unit_stats(UnitKind::Footman);

        // Warcry: +30% outgoing damage for 8s.
        let warcry = abilities_of_unit(UnitKind::Hero)[1];
        let mut buffed = StatusEffects::new();
        buffed.apply(StatusEffect::new(
            warcry.status().unwrap(),
            warcry.power(),
            0.0,
            warcry.duration(),
            StatusSource::Ability,
        ));
        assert!((effective_stats(footman, Some(&buffed)).damage_mult - 1.30).abs() < 1e-6);

        // Sanctuary: both statuses, one cast, one duration.
        let sanctuary = abilities_of_unit(UnitKind::Priestess)[1];
        let (extra, magnitude) = sanctuary.extra_status().unwrap();
        let mut warded = StatusEffects::new();
        warded.apply(StatusEffect::new(
            sanctuary.status().unwrap(),
            sanctuary.power(),
            0.0,
            sanctuary.duration(),
            StatusSource::Ability,
        ));
        warded.apply(StatusEffect::new(
            extra,
            magnitude,
            0.0,
            sanctuary.duration(),
            StatusSource::Ability,
        ));
        let eff = effective_stats(footman, Some(&warded));
        assert!((eff.heal_per_second - 15.0).abs() < 1e-6);
        assert!((eff.damage_taken_mult - 0.75).abs() < 1e-6);
        // Both instances die on the same tick — one cast, one expiry.
        assert!(warded.expire(sanctuary.duration() + 0.01));
        assert!(warded.is_empty());

        // Boots of Speed: +40% legs, and legs only.
        let mut hasted = StatusEffects::new();
        hasted.apply(StatusEffect::new(
            StatusKind::Haste,
            BOOTS_HASTE,
            0.0,
            BOOTS_DURATION,
            StatusSource::Item,
        ));
        let eff = effective_stats(footman, Some(&hasted));
        assert!((eff.speed - base.speed * 1.40).abs() < 1e-4);
        assert!((eff.attack_cooldown - base.attack_cooldown).abs() < 1e-6);

        // Banner of Command: 30% off incoming damage.
        let mut shielded = StatusEffects::new();
        shielded.apply(StatusEffect::new(
            StatusKind::ArmorBuff,
            BANNER_ARMOR,
            0.0,
            BANNER_DURATION,
            StatusSource::Item,
        ));
        assert!((effective_stats(footman, Some(&shielded)).damage_taken_mult - 0.70).abs() < 1e-6);
    }

    #[test]
    fn machine_autocast_covers_the_ultimates_and_nothing_else() {
        let champion = machine_autocast_rules(UnitKind::Hero);
        assert_eq!(champion, vec![(1, WARCRY_MIN_TARGETS)]);
        let priestess = machine_autocast_rules(UnitKind::Priestess);
        assert_eq!(priestess, vec![(1, SANCTUARY_MIN_TARGETS)]);
        // Slot 0 stays the player's `T` toggle / the script's own cast.
        assert!(champion.iter().all(|(index, _)| *index != 0));
        assert!(machine_autocast_rules(UnitKind::Footman).is_empty());
        // Installing them leaves any hand-set slot-0 rule alone.
        let mut policy = AutoCastPolicy::first(3);
        for (index, min) in machine_autocast_rules(UnitKind::Hero) {
            policy.set(index, min);
        }
        assert_eq!(policy.min_enemies_for(0), Some(3));
        assert_eq!(policy.min_enemies_for(1), Some(WARCRY_MIN_TARGETS));
    }

    // --- the tiered shop shelf -----------------------------------------------

    #[test]
    fn the_shop_shelf_is_tiered_and_gating_is_one_rule() {
        // The shelf, as designed: two starter consumables and the boots at T1,
        // the banner at the Keep, the mass-teleport scroll at the Castle.
        let expected = [
            (ItemId::HealingPotion, 100, TechTier::T1),
            (ItemId::TownPortal, 150, TechTier::T1),
            (ItemId::BootsOfSpeed, 50, TechTier::T1),
            (ItemId::BannerOfCommand, 125, TechTier::T2),
            (ItemId::ScrollOfMassTeleport, 250, TechTier::T3),
        ];
        assert_eq!(ALL_ITEMS.len(), expected.len());
        for (id, cost, tier) in expected {
            let def = item_def(id);
            assert_eq!(def.cost_gold, cost, "{} price", def.name);
            assert_eq!(def.tier, tier, "{} tier", def.name);
            // Gating is `tier >= required`, and nothing else: a team at or
            // above the rung may buy, a team below may not, at every rung.
            for team_tier in [TechTier::T1, TechTier::T2, TechTier::T3] {
                assert_eq!(
                    item_unlocked(id, team_tier),
                    team_tier >= tier,
                    "{} at {}",
                    def.name,
                    team_tier.name()
                );
            }
        }
        // The rungs a fresh team can reach are exactly the T1 ones — the
        // regression this bead exists to prevent is a Shop built at minute two
        // selling the late-game scroll.
        let at_start: Vec<&str> = ALL_ITEMS
            .iter()
            .filter(|id| item_unlocked(**id, TechTier::T1))
            .map(|id| item_def(*id).name)
            .collect();
        assert_eq!(at_start, ["HealingPotion", "TownPortal", "BootsOfSpeed"]);
        // Tier is a property of what is STANDING: the same team that could buy
        // the scroll with a Castle up cannot once it is rubble.
        let with_castle = tech_tier_for([BuildingKind::TownHall, BuildingKind::Castle].into_iter());
        let after_loss = tech_tier_for([BuildingKind::TownHall].into_iter());
        assert!(item_unlocked(ItemId::ScrollOfMassTeleport, with_castle));
        assert!(!item_unlocked(ItemId::ScrollOfMassTeleport, after_loss));
        assert!(item_unlocked(ItemId::HealingPotion, after_loss));
    }

    #[test]
    fn mass_teleport_spans_the_map_and_the_catalog_carries_the_shelf() {
        // "Map-wide" is a radius, not a special case: the scroll's radius has
        // to beat the longest possible distance between two units.
        let diagonal = (MAP_HALF * 2.0) * std::f32::consts::SQRT_2;
        assert!(MASS_TELEPORT_RADIUS > diagonal);
        assert!(PORTAL_RADIUS < diagonal, "the Town Portal stays local");

        let catalog = game_catalog();
        assert_eq!(catalog.items.len(), ALL_ITEMS.len());
        for id in ALL_ITEMS {
            let def = item_def(id);
            let row = catalog
                .items
                .iter()
                .find(|i| i.id == def.name)
                .unwrap_or_else(|| panic!("{} missing from the catalog", def.name));
            assert_eq!(row.tier, def.tier.level());
            assert_eq!(row.cost_gold, def.cost_gold);
            assert_eq!(row.sold_at, building_name(BuildingKind::Shop));
        }
        // The ultimates and their second status reach the catalog too, so a
        // commander reading catalog.json alone knows Sanctuary does two things.
        for id in ["Warcry", "Sanctuary"] {
            let row = catalog.abilities.iter().find(|a| a.id == id).expect(id);
            assert_eq!(row.index, 1);
            assert_eq!(row.unlock, "hero level 5");
            assert_eq!(row.effect, "status");
        }
        let sanctuary = catalog.abilities.iter().find(|a| a.id == "Sanctuary").unwrap();
        assert_eq!(sanctuary.status, Some("HealOverTime"));
        assert_eq!(sanctuary.status2, Some(("ArmorBuff", 0.25)));
    }

    /// **A called body goes home; it does not die.** The summon timer sits
    /// beside the militia timer because they are the same kind of promise, and
    /// the difference from a death is the point: no bounty, no XP, no kill on
    /// the enemy's ledger for a body that was never theirs to take.
    #[test]
    fn a_summon_leaves_when_its_time_is_up() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .add_systems(Update, tick_militia_and_cooldowns);
        let temporary = app
            .world_mut()
            .spawn((Unit { kind: UnitKind::Footman }, Summoned { until: Some(30.0) }))
            .id();
        let permanent = app
            .world_mut()
            .spawn((Unit { kind: UnitKind::Footman }, Summoned { until: None }))
            .id();

        app.update();
        assert!(app.world().get_entity(temporary).is_ok(), "not yet");

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(31.0));
        app.update();
        assert!(app.world().get_entity(temporary).is_err(), "its time was up");
        assert!(
            app.world().get_entity(permanent).is_ok(),
            "a summon with no lifetime stays until something kills it"
        );
    }

    /// **The catalog reads the composition.** A commander should not have to
    /// infer that Sanctuary does two things from a `status2` field bolted onto
    /// a one-effect schema: `effects[]` is the sentence, clause by clause,
    /// with each clause's own numbers and its own side.
    ///
    /// The v2 fields are asserted alongside, because they are still on the
    /// wire and a commander written against them must keep working.
    #[test]
    fn catalog_exports_abilities_atom_by_atom() {
        let catalog = game_catalog();
        let row = |id: &str| {
            catalog
                .abilities
                .iter()
                .find(|a| a.id == id)
                .unwrap_or_else(|| panic!("catalog lost {id}"))
                .clone()
        };

        // One clause: a shockwave.
        let slam = row("Slam");
        assert_eq!(slam.effects.len(), 1);
        assert_eq!(slam.effects[0].atom, "damage");
        assert_eq!(slam.effects[0].schedule, "instant");
        assert_eq!(slam.effects[0].amount, Some(45.0));
        assert_eq!(slam.effects[0].targets, Some("enemies"));
        assert_eq!(slam.effects[0].status, None);
        assert_eq!(slam.effect, "damage", "the v2 headline still says damage");
        assert_eq!(slam.power, 45.0);

        // Two clauses: the row `also` used to hide.
        let sanctuary = row("Sanctuary");
        assert_eq!(sanctuary.effects.len(), 2, "Sanctuary is two clauses, and says so");
        assert_eq!(
            sanctuary
                .effects
                .iter()
                .map(|e| (e.atom, e.status, e.magnitude, e.duration, e.targets))
                .collect::<Vec<_>>(),
            vec![
                ("status", Some("HealOverTime"), Some(15.0), Some(6.0), Some("allies")),
                ("status", Some("ArmorBuff"), Some(0.25), Some(6.0), Some("allies")),
            ],
        );

        // Militia's seconds are a duration in the composition, even though the
        // v2 `power` field has always carried them.
        let call = row("CallToArms");
        assert_eq!(call.effects[0].atom, "militia");
        assert_eq!(call.effects[0].duration, Some(40.0));
        assert_eq!(call.effects[0].targets, Some("own_workers"));
        assert_eq!(call.power, 40.0, "the v2 pair is reproduced exactly");
        assert_eq!(call.duration, 0.0);

        // Nothing on the wire carries a schedule the engine cannot RUN.
        // `over_time` joined `instant` as shipping content when the FarSeer's
        // Mend shipped — the first row to use the schedule the v3 vocabulary
        // had carried unused. The two unimplemented schedules (`on_hit`,
        // `on_death`) are refused by the loader, so reaching them here is
        // impossible; this asserts the *runnable* set rather than a
        // historical accident.
        for ability in &catalog.abilities {
            for clause in &ability.effects {
                assert!(
                    matches!(clause.schedule, "instant" | "over_time"),
                    "{}: schedule {:?} is not one the engine can run",
                    ability.id,
                    clause.schedule
                );
            }
        }
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

    /// The other half of "the catalog IS the tech tree": not just the hall
    /// ladder, but **every gated kind's full requirement chain**, reconstructed
    /// from `Catalog` and nothing else — no `unit_requires`, no `trainable`,
    /// no prose.
    ///
    /// The gap this pins shut: `units[].requires` used to carry only the
    /// requirements *beyond* owning the trainer, a caveat that lived in a Rust
    /// doc comment. Exported that way the catalog stated `Footman: requires
    /// []` — buildable from nothing — and `Catapult: requires []`, silently
    /// hiding a Barracks→Workshop chain. An agent could not tell a tier-1 unit
    /// from a tier-3 one without a join it was never told to make.
    #[test]
    fn the_catalog_alone_reconstructs_every_requirement_chain() {
        let catalog = game_catalog();
        let building = |id: &str| {
            catalog
                .buildings
                .iter()
                .find(|b| b.id == id)
                .unwrap_or_else(|| panic!("{id} missing from the catalog"))
        };
        let unit = |id: &str| {
            catalog
                .units
                .iter()
                .find(|u| u.id == id)
                .unwrap_or_else(|| panic!("{id} missing from the catalog"))
        };

        // --- reconstruct, using catalog fields only -----------------------
        // Everything a build order must contain to reach `id`: each named
        // building, whatever IT requires, and — for an upgrade-only rung —
        // every rung below it, walked down via `upgraded_from`.
        let full_chain = |seeds: &[&str]| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            let mut queue: Vec<String> = seeds.iter().map(|s| s.to_string()).collect();
            while let Some(id) = queue.pop() {
                if out.contains(&id) {
                    continue;
                }
                let b = building(&id);
                out.push(id);
                queue.extend(b.requires.iter().map(|r| r.to_string()));
                queue.extend(b.upgraded_from.map(|r| r.to_string()));
            }
            out.sort();
            out
        };
        let chain_of = |id: &str| {
            let u = unit(id);
            let seeds: Vec<&str> = u.requires.to_vec();
            full_chain(&seeds)
        };
        let expect = |got: Vec<String>, want: &[&str]| {
            let mut want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            want.sort();
            assert_eq!(got, want);
        };

        // --- every gated kind, bottom to top ------------------------------
        // A basic unit names its trainer. This is the entry that used to be
        // an empty list.
        expect(chain_of("Footman"), &["Barracks"]);
        expect(chain_of("Worker"), &["TownHall"]);
        // The Raider's Workshop gate drags in the Workshop's own Barracks
        // gate — the two-step chain that was entirely invisible before.
        expect(chain_of("Raider"), &["Barracks", "Workshop"]);
        expect(chain_of("Catapult"), &["Barracks", "Workshop"]);
        // The tier-3 pair: a Castle, which means a Keep, which means a hall.
        expect(chain_of("Knight"), &["Barracks", "TownHall", "Keep", "Castle"]);
        expect(
            chain_of("GryphonRider"),
            &["Barracks", "Workshop", "TownHall", "Keep", "Castle"],
        );

        // --- and the chains agree with the tiers --------------------------
        // `tier` is not an independent claim a reader has to trust: it is the
        // highest rung in the chain, and the catalog proves it against itself.
        for u in &catalog.units {
            let chain = chain_of(u.id);
            let reconstructed = chain
                .iter()
                .map(|id| building(id).tier)
                .max()
                .expect("every unit has a trainer");
            assert_eq!(
                u.tier, reconstructed,
                "{}: tier {} disagrees with its own requirement chain {chain:?}",
                u.id, u.tier
            );
            // The chain must bottom out somewhere a worker can actually
            // start: at least one placeable, ungated tier-1 building.
            assert!(
                chain
                    .iter()
                    .any(|id| { let b = building(id); b.placeable && b.tier == 1 && b.requires.is_empty() }),
                "{}: nothing in {chain:?} can be built from an empty base",
                u.id
            );
            // `trained_at` is in the chain, so the two fields cannot drift.
            assert!(
                u.requires.contains(&u.trained_at),
                "{}: trained_at {} is missing from requires",
                u.id,
                u.trained_at
            );
        }
        // Nothing is buildable above the top of the hall ladder.
        assert_eq!(catalog.units.iter().map(|u| u.tier).max(), Some(3));
        // The Catapult is honestly tier 1: a Workshop needs no hall upgrade.
        assert_eq!(unit("Catapult").tier, 1);
        assert_eq!(unit("Knight").tier, 3);

        // --- the counter triangle, as data rather than as English ----------
        // `class` plus the three multipliers is the whole triangle. Without
        // `class` even an exported `vs_cavalry_mult` would be unusable: no
        // field would say that a Knight is what it hits.
        let cavalry: Vec<&str> = catalog
            .units
            .iter()
            .filter(|u| u.class == Some("Cavalry"))
            .map(|u| u.id)
            .collect();
        // Both rosters' cavalry, in one list, because the class is one class.
        // That is the whole cross-race claim stated as catalog data: whatever
        // appears here takes 5x from whatever appears in `anti_cavalry` below,
        // and neither list is grouped by race.
        assert_eq!(cavalry, vec!["Raider", "Knight", "Wolfrider"]);
        let anti_cavalry: Vec<&str> = catalog
            .units
            .iter()
            .filter(|u| u.vs_cavalry_mult > 1.0)
            .map(|u| u.id)
            .collect();
        assert_eq!(anti_cavalry, vec!["Spearman", "Impaler"]);
        let anti_siege: Vec<&str> = catalog
            .units
            .iter()
            .filter(|u| u.vs_siege_mult > 1.0)
            .map(|u| u.id)
            .collect();
        assert_eq!(anti_siege, vec!["Raider", "Wolfrider"]);
        assert_eq!(unit("Catapult").class, Some("Siege"));
        // Siege is the answer to fortification, and it says so numerically.
        assert!(unit("Catapult").vs_building_mult > 1.0);
        // dps is computable, which is what every description quotes.
        assert!(catalog.units.iter().all(|u| u.attack_cooldown > 0.0));

        // --- the reverse indices agree with the forward ones ---------------
        for r in &catalog.research {
            assert!(
                building(r.researched_at).researches.contains(&r.id),
                "{} is researched at {} but that building does not list it",
                r.id,
                r.researched_at
            );
        }
        for i in &catalog.items {
            assert!(
                building(i.sold_at).sells.contains(&i.id),
                "{} is sold at {} but that building does not list it",
                i.id,
                i.sold_at
            );
        }
        // Ability gates are numbers, not sentences to parse.
        let ult = catalog
            .abilities
            .iter()
            .find(|a| a.id == "Warcry")
            .expect("the Champion's ultimate");
        assert_eq!(ult.unlock_hero_level, Some(5));
        assert_eq!(ult.unlock_tier, None);
        assert_eq!(ult.unlock, "hero level 5");
        for a in &catalog.abilities {
            assert_eq!(
                a.unlock == "always",
                a.unlock_hero_level.is_none() && a.unlock_tier.is_none(),
                "{}: the prose and the numbers disagree about the gate",
                a.id
            );
        }
    }

    // -----------------------------------------------------------------------
    // The Sorcerer's Slow: the first shipping crowd control, and the proof
    // that a status ability is a table row rather than a system.
    // -----------------------------------------------------------------------

    /// The one ability slot the Sorcerer has.
    fn slow_def() -> AbilityDef {
        let list = abilities_of_unit(UnitKind::Sorcerer);
        assert_eq!(list.len(), 1, "the Sorcerer ships exactly one ability");
        list[0]
    }

    /// The whole bead, end to end and without a World: take the Sorcerer's
    /// ability straight out of the table, build the status instance combat.rs
    /// would build from it, and read the result through `effective_stats` —
    /// the one function units.rs and combat.rs actually run on. If this holds,
    /// a Slow landing on a Raider really does what the card claims.
    #[test]
    fn sorcerer_slow_cripples_a_charge_through_effective_stats() {
        let def = slow_def();
        assert_eq!(def.name, "Slow");
        assert_eq!(
            def.effects.iter().map(|e| e.atom).collect::<Vec<_>>(),
            vec![EffectAtom::ApplyStatus {
                status: StatusKind::Slow,
                magnitude: 0.4,
                duration: 5.0,
                targets: AbilityTargets::Enemies,
            }],
            "Slow must reach the status framework through an ApplyStatus ATOM, not a new variant",
        );

        // Exactly what `cast_abilities` constructs for an ApplyStatus effect.
        let mut effects = StatusEffects::new();
        effects.apply(StatusEffect::new(
            StatusKind::Slow,
            def.power(),
            0.0,
            def.duration(),
            StatusSource::Ability,
        ));

        // The victim the ability was designed against.
        let charger = UnitKind::Raider;
        let base = effective_unit_stats(charger, None);
        let slowed = effective_unit_stats(charger, Some(&effects));

        // Legs: -40%, and now slower than the Footman it was diving past.
        assert!((slowed.speed - base.speed * 0.6).abs() < 1e-4);
        assert!(
            slowed.speed < unit_stats(UnitKind::Footman).speed,
            "a slowed Raider ({}) must be slower than a Footman ({})",
            slowed.speed,
            unit_stats(UnitKind::Footman).speed,
        );

        // Weapon: attack speed is the RECIPROCAL of cooldown, so -40% attack
        // speed is a cooldown DIVIDED by 0.6, not multiplied by it. This is
        // the arithmetic `effective_stats` exists to own.
        assert!((slowed.attack_cooldown - base.attack_cooldown / 0.6).abs() < 1e-4);
        assert!(slowed.attack_cooldown > base.attack_cooldown);

        // Nothing else moved: Slow is a tempo debuff, not a damage one.
        assert_eq!(slowed.damage_mult, 1.0);
        assert_eq!(slowed.damage_taken_mult, 1.0);

        // And it ends. `tick_status_effects` calls exactly this.
        let mut expired = effects.clone();
        assert!(expired.expire(def.duration() + 0.01));
        assert!(expired.is_empty());
        let recovered = effective_unit_stats(charger, Some(&expired));
        assert!((recovered.speed - base.speed).abs() < 1e-4);
    }

    /// Massing Sorcerers must widen the debuff, never deepen it. `Slow` is a
    /// debuff and debuffs REFRESH — three casters on one victim is still -40%,
    /// which is what keeps crowd control from becoming a stun.
    #[test]
    fn massed_sorcerers_refresh_a_slow_rather_than_stacking_it() {
        let def = slow_def();
        let mut effects = StatusEffects::new();
        for i in 0..3 {
            effects.apply(StatusEffect::new(
                StatusKind::Slow,
                def.power(),
                i as f32,
                def.duration(),
                StatusSource::Ability,
            ));
        }
        assert_eq!(effects.iter().count(), 1);
        assert!((effects.magnitude(StatusKind::Slow) - def.power()).abs() < 1e-4);
    }

    /// The Sorcerer is the first caster in the game that is not a hero: no
    /// `Hero` component, no mana, gated on its cooldown slot alone. The
    /// readiness rule has to answer that without special-casing it.
    #[test]
    fn the_sorcerer_is_a_mana_less_caster_gated_only_by_cooldown() {
        let def = slow_def();
        assert_eq!(def.mana_cost, 0.0, "a non-hero caster cannot pay mana");
        assert_eq!(def.unlock, AbilityUnlock::Always);

        // No hero, no cooldown store yet -> ready.
        assert!(ability_ready(&def, None, None, 0));

        let mut cds = AbilityCooldowns::default();
        cds.start(0, def.cooldown);
        assert!(!ability_ready(&def, None, Some(&cds), 0));
        cds.tick(def.cooldown + 0.01);
        assert!(ability_ready(&def, None, Some(&cds), 0));

        // And it is on by default — a caster whose only value is a spell it
        // never casts is a statue.
        assert_eq!(default_autocast(UnitKind::Sorcerer), Some((0, 1)));
        assert_eq!(default_autocast(UnitKind::Hero), None);
    }

    /// **The roster names its own gates** — `wc3clone-pbd`, round-9 AAR.
    ///
    /// `buildings[].trains` listed the Raider under the Barracks with nothing
    /// on that entry to say it waits on a Workshop. The answer did exist, in
    /// `units[].requires`, on the far side of the catalog behind a join
    /// nothing advertised — and a commander reading a *roster* has no reason
    /// to suspect a join is needed. It cost them their scout timing.
    ///
    /// So the gate is now legible without leaving the building entry, and the
    /// two roster fields are pinned together so they cannot drift.
    #[test]
    fn the_roster_shows_the_gate_where_the_roster_is_read() {
        let catalog = game_catalog();
        let building = |id: &str| {
            catalog
                .buildings
                .iter()
                .find(|b| b.id == id)
                .unwrap_or_else(|| panic!("{id} missing from the catalog"))
        };
        let entry = |b: &str, u: &str| {
            building(b)
                .trains_gated
                .iter()
                .find(|t| t.unit == u)
                .unwrap_or_else(|| panic!("{b} does not train {u}"))
        };

        // One roster, two readings — element for element, in the same order.
        for b in &catalog.buildings {
            let ids: Vec<&str> = b.trains_gated.iter().map(|t| t.unit).collect();
            assert_eq!(b.trains, ids, "{}: trains and trains_gated disagree", b.id);
        }

        // The three units gated AT their trainer. This is the fact round 9 had
        // to learn from a rejection.
        assert_eq!(entry("Barracks", "Raider").requires, vec!["Workshop"]);
        assert_eq!(entry("Barracks", "Knight").requires, vec!["Castle"]);
        assert_eq!(entry("Workshop", "GryphonRider").requires, vec!["Castle"]);
        // An ungated unit says so with an empty list rather than with silence.
        assert!(entry("Barracks", "Footman").requires.is_empty());
        // The Sorcerer's gate is on its TRAINER, not on itself — so its entry
        // is legitimately empty, and the Sanctum's own `requires` carries the
        // Keep. The building entry is complete either way, which is the
        // property that matters: one entry, whole answer.
        assert!(entry("Sanctum", "Sorcerer").requires.is_empty());
        assert_eq!(building("Sanctum").requires, vec!["Keep"]);

        // Tier travels with it, so "is this branch open to me yet" is one
        // lookup against `me.tier` instead of a walk up the chain.
        assert_eq!(entry("Barracks", "Raider").tier, 1);
        assert_eq!(entry("Barracks", "Knight").tier, 3);
        assert_eq!(entry("Sanctum", "Sorcerer").tier, 2);

        // The catalog cannot claim a gate the engine does not enforce.
        for b in &catalog.buildings {
            for t in &b.trains_gated {
                let kind = ALL_UNIT_KINDS
                    .into_iter()
                    .find(|k| kind_name(*k) == t.unit)
                    .expect("roster names a real unit");
                let want: Vec<&str> = unit_requires(kind).iter().map(|r| building_name(*r)).collect();
                assert_eq!(t.requires, want, "{}: {} gate is not the engine's", b.id, t.unit);
            }
        }
    }

    /// **A refused train order says where the unit actually trains** —
    /// `wc3clone-pbd`, the other half of the round-9 pair.
    ///
    /// The two strings a commander hit were individually true and collectively
    /// a dead end. `Raider requires Workshop`, read *at the Barracks*, reads as
    /// "wrong building" — so they moved the order to the Workshop and got
    /// `Workshop cannot train Raider`. Neither ever said: keep training it
    /// here, once a Workshop stands.
    #[test]
    fn a_refused_train_order_says_where_the_unit_actually_trains() {
        let opening = [BuildingKind::TownHall, BuildingKind::Barracks];
        let with_workshop = [
            BuildingKind::TownHall,
            BuildingKind::Barracks,
            BuildingKind::Workshop,
        ];

        // The Raider at the Barracks — the right building, so the string has
        // to name it anyway. The redundancy IS the fix.
        assert_eq!(
            train_gate_error(UnitKind::Raider, &opening).unwrap(),
            "Raider trains at the Barracks once a Workshop stands (you have none)"
        );
        // ...and at the Workshop, which is where the old error sent them.
        assert!(train_gate_error(UnitKind::Raider, &with_workshop).is_none());
        assert_eq!(
            wrong_trainer_error(BuildingKind::Workshop, UnitKind::Raider, &with_workshop),
            "Workshop cannot train Raider — Raider trains at the Barracks"
        );

        // The hall ladder: "you have none" is a lie to somebody looking
        // straight at their TownHall, so the clause names what they hold and
        // what to do to it.
        assert_eq!(
            train_gate_error(UnitKind::Knight, &opening).unwrap(),
            "Knight trains at the Barracks once a Castle stands \
             (yours is a TownHall — upgrade it)"
        );
        assert_eq!(
            train_gate_error(UnitKind::GryphonRider, &with_workshop).unwrap(),
            "GryphonRider trains at the Workshop once a Castle stands \
             (yours is a TownHall — upgrade it)"
        );

        // The Sorcerer. Its `unit_requires` is empty, so `train_gate_error`
        // has nothing to say and the wrong-building string is the ONLY one it
        // can ever produce — which is why that string has to carry the
        // Sanctum's own Keep requirement too.
        assert!(train_gate_error(UnitKind::Sorcerer, &opening).is_none());
        assert_eq!(
            wrong_trainer_error(BuildingKind::Barracks, UnitKind::Sorcerer, &opening),
            "Barracks cannot train Sorcerer — Sorcerer trains at the Sanctum \
             (you have no Sanctum; it needs a Keep)"
        );
        // Keep up, Sanctum not yet: the only instruction left is the building.
        let keep = [BuildingKind::Keep, BuildingKind::Barracks];
        assert_eq!(
            wrong_trainer_error(BuildingKind::Barracks, UnitKind::Sorcerer, &keep),
            "Barracks cannot train Sorcerer — Sorcerer trains at the Sanctum \
             (you have no Sanctum)"
        );
        // A Castle satisfies the Keep, so the gate clause must not reappear.
        let castle = [BuildingKind::Castle, BuildingKind::Barracks];
        assert_eq!(
            wrong_trainer_error(BuildingKind::Barracks, UnitKind::Sorcerer, &castle),
            "Barracks cannot train Sorcerer — Sorcerer trains at the Sanctum \
             (you have no Sanctum)"
        );

        // The general property, rather than the three cases: every gate error
        // names the building that will accept the order once the gate is met.
        // This is what the round-9 pair failed AS A PAIR.
        for kind in ALL_UNIT_KINDS {
            let Some(msg) = train_gate_error(kind, &opening) else {
                continue;
            };
            let trainer = building_name(unit_trainer(kind).expect("a gated unit has a trainer"));
            assert!(msg.contains(trainer), "{kind:?}: '{msg}' never names {trainer}");
        }
    }

    /// **The verdict says which win it was** — `wc3clone-azo`, round-9 AAR:
    /// the winner could not tell a razed base from a concession.
    #[test]
    fn the_verdict_says_which_win_it_was() {
        let verdict = |setup: &dyn Fn(&mut App)| -> (Option<Team>, Option<GameOverReason>) {
            let mut app = App::new();
            // Races: `CorePlugin` supplies this in a real match; a hand-built
            // test app must too, or any system reading it panics inside Bevy's
            // worker pool and the test HANGS rather than fails.
            app.init_resource::<Races>();
            app.init_resource::<Time>()
                .init_resource::<GameOver>()
                .add_event::<Surrender>()
                .add_systems(Update, check_game_over);
            setup(&mut app);
            app.update();
            let over = app.world().resource::<GameOver>();
            (over.winner, over.reason)
        };

        // Razed: Claude has no production building left. The 10s grace has to
        // be cleared first, or the opening frame decides the match.
        let (winner, reason) = verdict(&|app: &mut App| {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs(20));
            app.world_mut()
                .spawn((Building { kind: BuildingKind::Barracks }, Team::Human));
        });
        assert_eq!(winner, Some(Team::Human));
        assert_eq!(reason, Some(GameOverReason::Razed));

        // Surrender: decided before the grace period even matters, and the
        // conceding team is the one that loses.
        let (winner, reason) = verdict(&|app: &mut App| {
            app.world_mut().send_event(Surrender { team: Team::Human });
        });
        assert_eq!(winner, Some(Team::Claude));
        assert_eq!(reason, Some(GameOverReason::Surrender));

        // The two halves are set together or not at all — a winner with no
        // reason is the bug this shape exists to make unrepresentable.
        assert_eq!(GameOver::default().winner, None);
        assert_eq!(GameOver::default().reason, None);
        assert_eq!(GameOverReason::Razed.name(), "razed");
        assert_eq!(GameOverReason::Surrender.name(), "surrender");
    }

    /// **A drawn match is decided and names nobody** — `wc3clone-j84`. The cap
    /// can end a match dead even, and the two questions "is it over" and "who
    /// won" stopped having the same answer the moment it could: every gate in
    /// the game asks `decided()`, and only the things that print a name ask
    /// `winner`.
    #[test]
    fn a_draw_is_decided_and_names_nobody() {
        let mut over = GameOver::default();
        assert!(!over.decided(), "an undecided match is not over");

        over.decide_draw(GameOverReason::Score);
        assert!(over.decided(), "a draw ENDS the match");
        assert_eq!(over.winner, None, "and it names no winner (docs/ARENA.md)");
        assert_eq!(over.reason, Some(GameOverReason::Score));
        assert!(over.drawn);

        // A win is still a win, and still decided.
        let mut over = GameOver::default();
        over.decide(Team::Claude, GameOverReason::Score);
        assert!(over.decided());
        assert_eq!(over.winner, Some(Team::Claude));
        assert!(!over.drawn, "somebody won; this was no draw");

        // The wire name the ledger, the HUD and the headless log all print.
        // docs/ARENA.md's `END_REASONS` has listed it since round 10.
        assert_eq!(GameOverReason::Score.name(), "score");
    }

    /// The match-over gate is one question with one answer. A draw that ended
    /// the match for the snapshot but not for `check_game_over` would let a
    /// late raze overwrite a verdict two commanders had already been handed.
    #[test]
    fn a_decided_draw_stops_the_game_over_check() {
        let mut app = App::new();
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<GameOver>()
            .add_event::<Surrender>()
            .add_systems(Update, check_game_over);
        app.world_mut()
            .resource_mut::<GameOver>()
            .decide_draw(GameOverReason::Score);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(20));
        // Human owns no production building, which on any undecided frame is a
        // raze verdict for Claude.
        app.world_mut()
            .spawn((Building { kind: BuildingKind::Barracks }, Team::Claude));
        app.update();

        let over = app.world().resource::<GameOver>();
        assert_eq!(over.winner, None, "the draw stands");
        assert_eq!(over.reason, Some(GameOverReason::Score));
    }

    /// **The team that took the cache is told it took the cache** —
    /// `wc3clone-azo`. The feed's only word used to be the unattributed
    /// `bounty gone`, so the team standing on the cache saw exactly what a
    /// distant watcher saw and had to diff its own gold against harvest income
    /// arriving in the same second to find out whether it had won the race.
    ///
    /// The asymmetry that remains is the fog rule, not an oversight: who took
    /// a cache appears in no snapshot, so the enemy gets nothing from here.
    /// Documented in docs/FOG.md.
    #[test]
    fn the_claiming_team_is_told_it_claimed_and_the_enemy_is_not() {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<GameEvents>()
            .add_event::<BountyClaim>()
            .add_systems(Update, announce_bounty_claims);
        app.world_mut().send_event(BountyClaim {
            team: Team::Claude,
            gold: 270,
            pos: Vec3::new(4.0, 0.0, -8.0),
            id: 7,
        });
        app.update();

        let feed = app.world().resource::<GameEvents>();
        let mine: Vec<&str> = feed
            .feed(Team::Claude)
            .iter()
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(mine, vec!["we claimed the cache (+270g)"]);
        assert!(
            feed.feed(Team::Human).is_empty(),
            "a claim is not visible in any snapshot — telling the enemy would \
             hand out intel the map does not contain"
        );
        // The cache is where it can be pointed at, so the HUD can focus it.
        assert_eq!(feed.feed(Team::Claude)[0].pos, Some(Vec3::new(4.0, 0.0, -8.0)));
        // And the id is filed for the diff, so the claimer is not ALSO shown
        // the anonymous `bounty gone` line for its own cache a second later.
        assert_eq!(feed.claims, vec![(7, Team::Claude)]);
    }

    /// A newly sighted army announces itself, in the aggregate shape a human
    /// would use out loud — and then stops, because a feed that repeated it
    /// every second would be a stream.
    #[test]
    fn a_newly_sighted_enemy_army_is_announced_once_per_place() {
        let mut memo = EventMemo::default();
        // Skip the seeding tick: this test is about the army block, not about
        // the diff's first-frame behaviour.
        memo.seeded = true;
        let mut grids = FogGrids::test_dark();
        for i in 0..5u64 {
            grids.test_sight(
                Team::Human,
                Sighting {
                    id: i,
                    team: Team::Claude,
                    kind: if i < 3 {
                        UnitKind::Footman
                    } else {
                        UnitKind::Archer
                    },
                    pos: Vec3::new(i as f32, 0.0, 0.0),
                    hp_frac: 1.0,
                    heading: None,
                    t_seen: 100.0,
                },
            );
        }
        let orders = SquadOrders::default();
        let mut run = |now: f32, memo: &mut EventMemo| {
            diff_team(
                Team::Human,
                now,
                memo,
                &[],
                &[],
                &[],
                &orders,
                grids.get(Team::Human),
                &[],
            )
        };

        let out = run(100.0, &mut memo);
        assert_eq!(out.len(), 1, "one line for one army");
        assert!(
            out[0].0.starts_with("enemy army spotted: ~5 (3 Footman, 2 Archer)"),
            "unexpected wording: {}",
            out[0].0
        );
        assert_eq!(out[0].1, EventSeverity::Warning);
        assert!(out[0].2.is_some(), "and somewhere for the camera to look");

        // Same army, same ground, one second later: silence.
        assert!(
            run(101.0, &mut memo).is_empty(),
            "re-sightings are rate-limited — this is news, not telemetry"
        );

        // Long enough later that the suppression lapses, but the sighting is
        // now far too stale to be called a spotting. Still silence, and for
        // the OTHER reason: remembered is not spotted.
        assert!(
            run(100.0 + ARMY_REANNOUNCE_S + 1.0, &mut memo).is_empty(),
            "a ninety-second-old rumour must never be re-announced as news"
        );
    }

    /// The feed obeys the same ledger the snapshot does: no sightings, no
    /// army line, however many enemies are actually standing there.
    #[test]
    fn an_unseen_army_raises_no_event() {
        let mut memo = EventMemo::default();
        memo.seeded = true;
        let grids = FogGrids::test_dark();
        let orders = SquadOrders::default();
        let out = diff_team(
            Team::Human,
            100.0,
            &mut memo,
            &[],
            &[],
            &[],
            &orders,
            grids.get(Team::Human),
            &[],
        );
        assert!(out.is_empty());
    }

    /// The Sanctum's whole content wiring, asked of the derived tables rather
    /// than of the enum: it is placeable, it is a production building (so it
    /// counts for the win condition), it wants a Keep, and a Castle satisfies
    /// that for free.
    #[test]
    fn the_sanctum_is_a_tier_two_building_that_trains_the_sorcerer() {
        assert_eq!(trainable(BuildingKind::Sanctum), &[UnitKind::Sorcerer]);
        assert!(building_placeable(BuildingKind::Sanctum));
        assert_eq!(building_tier(BuildingKind::Sanctum), 1, "not on a ladder");

        let reqs = building_requires(BuildingKind::Sanctum);
        assert_eq!(reqs, &[BuildingKind::Keep]);
        assert!(!requirements_met(reqs, [BuildingKind::TownHall].into_iter()));
        assert!(requirements_met(reqs, [BuildingKind::Keep].into_iter()));
        assert!(
            requirements_met(reqs, [BuildingKind::Castle].into_iter()),
            "teching past the gate must never close it",
        );

        // The Sorcerer needs no requirement of its own: the Sanctum IS the
        // gate, and stating it twice would let the two drift.
        assert!(unit_requires(UnitKind::Sorcerer).is_empty());

        // The catalog picks both up with no bespoke entry.
        let cat = game_catalog();
        let unit = cat
            .units
            .iter()
            .find(|u| u.id == "Sorcerer")
            .expect("Sorcerer in catalog");
        assert_eq!(unit.trained_at, "Sanctum");
        assert!(unit.vision > 0.0, "every kind needs a vision radius");
        let building = cat
            .buildings
            .iter()
            .find(|b| b.id == "Sanctum")
            .expect("Sanctum in catalog");
        assert_eq!(building.trains, vec!["Sorcerer"]);
        assert_eq!(building.requires, vec!["Keep"]);
        assert!(building.vision > 0.0);
        assert!(
            cat.abilities
                .iter()
                .any(|a| a.caster == "Sorcerer" && a.id == "Slow" && a.status == Some("Slow")),
            "the catalog must describe Slow so both seats can read it",
        );
    }

    // -----------------------------------------------------------------------
    // Hero slots
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // v3: races
    // -----------------------------------------------------------------------

    /// **The default is the game that shipped.** With neither env var set both
    /// teams are Kingdom, which is what makes "zero change unless you opt in"
    /// a property rather than a hope.
    #[test]
    fn the_default_race_pair_is_the_pre_race_game() {
        let races = Races::default();
        assert_eq!(races.get(Team::Human), Race::Kingdom);
        assert_eq!(races.get(Team::Claude), Race::Kingdom);
        assert_eq!(race_hall(Race::Kingdom), BuildingKind::TownHall);
        assert_eq!(race_worker(Race::Kingdom), UnitKind::Worker);
    }

    /// The env words a match is selected with, including the aliases and the
    /// rejection of anything else (which the reader turns into a warning and a
    /// Kingdom fallback rather than a panic — a typo in a shell variable must
    /// not stop a sim).
    #[test]
    fn race_parses_the_words_a_match_is_started_with() {
        assert_eq!(Race::parse("kingdom"), Some(Race::Kingdom));
        assert_eq!(Race::parse("HORDE"), Some(Race::Horde));
        assert_eq!(Race::parse("  Horde  "), Some(Race::Horde));
        assert_eq!(Race::parse("orc"), Some(Race::Horde));
        assert_eq!(Race::parse("undead"), None);
        assert_eq!(Race::parse(""), None);
        for race in ALL_RACES {
            assert_eq!(Race::parse(race.name()), Some(race), "{race:?} round-trips");
        }
    }

    /// **Each race's opening position resolves, and to different kinds.** This
    /// is what `initial_spawns` reads, and a race whose hall or worker did not
    /// resolve would panic at the first frame of a match rather than at load.
    #[test]
    fn every_race_has_an_opening_position() {
        let halls: Vec<BuildingKind> = ALL_RACES.into_iter().map(race_hall).collect();
        let workers: Vec<UnitKind> = ALL_RACES.into_iter().map(race_worker).collect();
        assert_eq!(halls, vec![BuildingKind::TownHall, BuildingKind::Stronghold]);
        assert_eq!(workers, vec![UnitKind::Worker, UnitKind::Peon]);
        for &hall in &halls {
            assert!(is_hall(hall), "{hall:?} must be a hall");
            assert!(building_placeable(hall), "{hall:?} must be placeable");
            assert_eq!(building_tier(hall), 1);
            // Three rungs, because `hero_slots` and every tier gate are read
            // off the rung.
            assert_eq!(
                tech_tier_for(std::iter::once(top_rung(hall))).level(),
                3,
                "{hall:?}'s ladder must reach tier 3"
            );
        }
        for &w in &workers {
            assert!(is_worker_kind(w));
        }
    }

    /// The top of a hall's ladder, walked through `upgrades_to`.
    fn top_rung(mut kind: BuildingKind) -> BuildingKind {
        while let Some(next) = building_upgrades_to(kind) {
            kind = next;
        }
        kind
    }

    /// **`is_hall` covers both ladders**, which is the one derived fact that
    /// used to name `TownHall` directly. Drop-off, Town Portal, rally
    /// fallbacks, the AI's base bookkeeping and `tech_tier_for` all ask this
    /// question, so a second ladder it did not answer for would be six silent
    /// bugs rather than one.
    #[test]
    fn is_hall_answers_for_every_ladder() {
        let halls: Vec<BuildingKind> =
            ALL_BUILDING_KINDS.into_iter().filter(|&k| is_hall(k)).collect();
        assert_eq!(
            halls,
            vec![
                BuildingKind::TownHall,
                BuildingKind::Keep,
                BuildingKind::Castle,
                BuildingKind::Stronghold,
                BuildingKind::Fortress,
                BuildingKind::Hold,
            ],
        );
        // Every rung of a race's ladder belongs to that race — a team must
        // never tier up into a building it may not own.
        for race in ALL_RACES {
            let mut rung = race_hall(race);
            loop {
                assert!(race_has_building(race, rung), "{race:?} cannot own {rung:?}");
                match building_upgrades_to(rung) {
                    Some(next) => rung = next,
                    None => break,
                }
            }
        }
    }

    /// **The role lookups every consumer asks through.** `race_unit` /
    /// `race_building` are what replaced twenty `UnitKind::Footman` literals in
    /// ai.rs, so if either stopped resolving the scripted commander would go
    /// quietly passive rather than fail.
    #[test]
    fn every_race_resolves_the_roles_the_engine_asks_for() {
        for race in ALL_RACES {
            for role in [UnitRole::Worker, UnitRole::Line, UnitRole::Ranged, UnitRole::AntiCavalry]
            {
                let got = race_unit(race, role);
                assert!(got.is_some(), "{race:?} has no {role:?}");
                assert_eq!(unit_role(got.unwrap()), role);
                assert!(race_has_unit(race, got.unwrap()));
            }
            for role in [BuildingRole::Production, BuildingRole::Supply, BuildingRole::Defense] {
                assert!(race_building(race, role).is_some(), "{race:?} has no {role:?}");
            }
            // Two hero classes each, melee first — the order ai.rs fills slots
            // in and the order the command card lays them out in.
            let heroes = race_hero_classes(race);
            assert_eq!(heroes.len(), 2, "{race:?}: {heroes:?}");
            assert_eq!(unit_role(heroes[0]), UnitRole::HeroMelee);
            assert_eq!(unit_role(heroes[1]), UnitRole::HeroSupport);
            // Exactly one forge, because research is shared and the building
            // that sells it is not.
            assert!(research_building_for(race).is_some(), "{race:?} has no forge");
        }
        // The asymmetries, asserted rather than assumed — these are design,
        // and a later edit that quietly gave the Horde a wall or a shock unit
        // should have to change this line.
        assert!(race_unit(Race::Horde, UnitRole::Shock).is_none());
        assert!(race_building(Race::Horde, BuildingRole::Barrier).is_none());
        assert!(race_building(Race::Horde, BuildingRole::Siegeworks).is_none());
        assert!(race_unit(Race::Horde, UnitRole::Siege).is_some());
        assert!(race_unit(Race::Horde, UnitRole::Flyer).is_some());
    }

    /// The build menu is per-race, and neither race can place the other's
    /// buildings. The Shop is the one row both may build.
    #[test]
    fn the_build_menu_is_the_race_s_own() {
        let kingdom = race_build_menu(Race::Kingdom);
        let horde = race_build_menu(Race::Horde);
        assert!(kingdom.contains(&BuildingKind::Barracks));
        assert!(!kingdom.contains(&BuildingKind::WarCamp));
        assert!(horde.contains(&BuildingKind::WarCamp));
        assert!(!horde.contains(&BuildingKind::Barracks));
        assert!(!horde.contains(&BuildingKind::Wall), "the Horde does not turtle");
        let shared: Vec<BuildingKind> = kingdom
            .iter()
            .copied()
            .filter(|k| horde.contains(k))
            .collect();
        assert_eq!(shared, vec![BuildingKind::Shop]);
        // Nothing on either menu is an upgrade-only rung.
        for k in kingdom.iter().chain(horde.iter()) {
            assert!(building_placeable(*k), "{k:?} is not placeable");
        }
    }

    /// **Every kind classifies.** `TargetClass::of` was a list of kinds that a
    /// new unit had to be added to by hand, and that returned `None` — silently
    /// un-focusable — if you forgot. It reads the row's role now, so this is
    /// the test that the whole roster is focus-fireable, both races at once.
    #[test]
    fn every_unit_kind_has_a_target_class() {
        for kind in ALL_UNIT_KINDS {
            assert!(
                TargetClass::of(Some(kind), false).is_some(),
                "{kind:?} has no target class — a doctrine cannot name it"
            );
        }
        // The classes that carry a multiplier are the ones worth spelling out.
        assert_eq!(
            TargetClass::of(Some(UnitKind::Wolfrider), false),
            Some(TargetClass::Cavalry)
        );
        assert_eq!(
            TargetClass::of(Some(UnitKind::Grunt), false),
            Some(TargetClass::Footman)
        );
        assert_eq!(
            TargetClass::of(Some(UnitKind::Impaler), false),
            Some(TargetClass::Footman)
        );
        assert_eq!(
            TargetClass::of(Some(UnitKind::Shaman), false),
            Some(TargetClass::Archer)
        );
        assert_eq!(
            TargetClass::of(Some(UnitKind::Demolisher), false),
            Some(TargetClass::Siege)
        );
        assert_eq!(
            TargetClass::of(Some(UnitKind::Wyvern), false),
            Some(TargetClass::Air)
        );
        for hero in [UnitKind::Warchief, UnitKind::FarSeer] {
            assert_eq!(TargetClass::of(Some(hero), false), Some(TargetClass::Hero));
            assert!(is_hero_kind(hero));
        }
    }

    /// The catalog carries the race and the role of every row, which is how a
    /// bridge commander finds its own roster in a document both seats share.
    #[test]
    fn the_catalog_tells_a_commander_which_rows_are_its_own() {
        let catalog = game_catalog();
        let peon = catalog.units.iter().find(|u| u.id == "Peon").expect("Peon row");
        assert_eq!(peon.race, vec!["horde"]);
        assert_eq!(peon.role, "Worker");
        let footman = catalog.units.iter().find(|u| u.id == "Footman").unwrap();
        assert_eq!(footman.race, vec!["kingdom"]);
        assert_eq!(footman.role, "Line");
        // The neutral row says so by listing both.
        let shop = catalog.buildings.iter().find(|b| b.id == "Shop").unwrap();
        assert_eq!(shop.race, vec!["kingdom", "horde"]);
        assert_eq!(shop.role, "Vendor");
        // Every row carries at least one race, or a commander cannot tell
        // whether it may build the thing.
        assert!(catalog.units.iter().all(|u| !u.race.is_empty()));
        assert!(catalog.buildings.iter().all(|b| !b.race.is_empty()));
        // Research is shared content sold by two different forges.
        let attack = catalog.research.iter().find(|r| r.id == "attack").unwrap();
        assert_eq!(attack.forges, vec!["Blacksmith", "WarMill"]);
    }

    /// One rung, one hero. Derived from the ladder, so a fourth rung needs no
    /// edit here — and tier 3's third slot is deliberately unreachable today,
    /// because only two hero classes exist.
    #[test]
    fn hero_slots_climb_the_hall_ladder_one_per_rung() {
        assert_eq!(hero_slots(TechTier::T1), 1);
        assert_eq!(hero_slots(TechTier::T2), 2);
        assert_eq!(hero_slots(TechTier::T3), 3);

        for kind in ALL_BUILDING_KINDS.into_iter().filter(|k| is_hall(*k)) {
            let tier = tech_tier_for([kind].into_iter());
            assert_eq!(
                hero_slots(tier),
                building_tier(kind),
                "{} must open exactly its rung's worth of hero slots",
                building_name(kind),
            );
        }

        // Two classes PER RACE, and a team only ever sees its own — so
        // tier 3's third slot is still unreachable, which is the claim this
        // line has always been making. Counting every hero kind in the game
        // stopped being the way to ask it the moment a second roster shipped.
        for race in ALL_RACES {
            assert_eq!(
                race_hero_classes(race).len(),
                2,
                "if {race:?} gets a third hero class, tier 3's third slot becomes \
                 reachable — update docs and ai.rs's `Roster::heroes`",
            );
        }
    }

    /// The slot rule itself, including the case that made it worth extracting:
    /// a hero already sitting in a QUEUE spends a slot exactly like one
    /// standing on the map, so two halls cannot each pay for one.
    #[test]
    fn hero_slots_count_living_and_queued_heroes_and_forbid_duplicate_classes() {
        use HeroSlotVerdict::*;
        let champion = UnitKind::Hero;
        let priestess = UnitKind::Priestess;

        // Tier 1: one slot, and it is the first hero of either class.
        assert_eq!(hero_slot_check(&[], champion, TechTier::T1), Ok);
        assert_eq!(hero_slot_check(&[], priestess, TechTier::T1), Ok);
        assert_eq!(
            hero_slot_check(&[champion], priestess, TechTier::T1),
            NoSlot { used: 1, slots: 1 },
            "a TownHall team may not have a second hero of ANY class",
        );

        // Tier 2: the second class fits, a duplicate never does.
        assert_eq!(hero_slot_check(&[champion], priestess, TechTier::T2), Ok);
        assert_eq!(
            hero_slot_check(&[champion], champion, TechTier::T2),
            DuplicateClass,
            "distinct classes only — a Keep buys a different hero, not a copy",
        );
        assert_eq!(
            hero_slot_check(&[champion, priestess], champion, TechTier::T2),
            DuplicateClass,
            "the class check outranks the slot check, so the error is the true one",
        );

        // THE QUEUE EDGE CASE: `held` is living heroes PLUS everything in
        // flight. A Champion alive and a Priestess merely queued fills a
        // tier-2 team's slate — a further request is refused even though only
        // one hero is standing on the map.
        let held_with_queued = [champion, priestess];
        assert_eq!(
            hero_slot_check(&held_with_queued, priestess, TechTier::T2),
            DuplicateClass,
        );
        assert_eq!(
            hero_slot_check(&held_with_queued, champion, TechTier::T3),
            DuplicateClass,
            "even at tier 3 with a free slot, the class is the wall",
        );

        // Two of the SAME hero queued in two different halls: the second one
        // to reach a pay-point sees the first in `held` and is refused.
        assert_eq!(
            hero_slot_check(&[champion], champion, TechTier::T3),
            DuplicateClass,
        );

        // Losing the Keep closes the slot for FUTURE heroes without
        // confiscating the ones standing: the check refuses, it never kills.
        assert_eq!(
            hero_slot_check(&[champion, priestess], UnitKind::Hero, TechTier::T1),
            DuplicateClass,
        );
        assert_eq!(
            hero_slot_check(&[champion], priestess, TechTier::T1),
            NoSlot { used: 1, slots: 1 },
        );
    }

    /// Where hero SLOTS meet hero ULTIMATES: with two heroes on the field the
    /// unlock predicate has to read the level of the hero it is asked about,
    /// and revival has to hand back that hero's own progression.
    ///
    /// The failure this pins is specific and was reachable: while records were
    /// one-per-team, a Champion's level answered for the Priestess too, so a
    /// level-6 Champion would have unlocked Sanctuary on a level-1 Priestess
    /// the moment she was trained. Records are per class, so each hero's
    /// ultimate opens on its own XP — and survives its own death.
    #[test]
    fn each_hero_class_unlocks_its_own_ultimate_from_its_own_record() {
        let team = Team::Human;
        let mut records = HeroRecords::default();

        // A veteran Champion and a brand-new Priestess.
        records.set(team, HeroRecord { level: 6, xp: 40.0, kind: UnitKind::Hero });
        records.set(team, HeroRecord { level: 1, xp: 0.0, kind: UnitKind::Priestess });

        let ultimate = |kind: UnitKind| {
            let list = abilities_of_unit(kind);
            *list
                .iter()
                .find(|d| matches!(d.unlock, AbilityUnlock::HeroLevel(_)))
                .expect("both hero classes ship a level-gated ultimate")
        };
        let champion_ult = ultimate(UnitKind::Hero);
        let priestess_ult = ultimate(UnitKind::Priestess);
        assert_eq!(champion_ult.name, "Warcry");
        assert_eq!(priestess_ult.name, "Sanctuary");

        // Each hero is rebuilt from ITS OWN record, exactly as units.rs does.
        let champion = Hero::from_record(records.get(team, UnitKind::Hero));
        let priestess = Hero::from_record(records.get(team, UnitKind::Priestess));
        assert_eq!(champion.level, 6);
        assert_eq!(priestess.level, 1);

        // T3 in hand, so nothing here is a tier gate in disguise.
        let ctx = |h: &Hero| UnlockCtx::new(h.level, TechTier::T3);
        assert!(
            ability_unlocked(&champion_ult, ctx(&champion)),
            "a level-6 Champion has earned Warcry",
        );
        assert!(
            !ability_unlocked(&priestess_ult, ctx(&priestess)),
            "a level-1 Priestess must NOT inherit the Champion's level",
        );

        // She levels to 5 and it opens — then she dies, and revival restores
        // her own record, so the ultimate is still hers.
        records.set(team, HeroRecord { level: 5, xp: 10.0, kind: UnitKind::Priestess });
        let revived = Hero::from_record(records.get(team, UnitKind::Priestess));
        assert_eq!(revived.level, 5);
        assert!(
            ability_unlocked(&priestess_ult, ctx(&revived)),
            "Sanctuary must survive a revival, because the level does",
        );
        // ...and the Champion's own record is untouched by any of it.
        assert_eq!(records.get(team, UnitKind::Hero).map(|r| r.level), Some(6));

        // Both ultimates coexist: two classes, two slots, one team, one match.
        assert_eq!(hero_slots(TechTier::T2), 2);
        assert_eq!(
            hero_slot_check(&[UnitKind::Hero], UnitKind::Priestess, TechTier::T2),
            HeroSlotVerdict::Ok,
            "a Keep team may field both ultimates at once",
        );
    }

    /// The bug this test exists for: six of eleven unit kinds fell through the
    /// old hand-written `match` and were worth **zero** XP, so a hero could
    /// grind an entire tier-3 army and never level. Every kind must pay, and
    /// no kind may out-pay a kind that costs more than it.
    #[test]
    fn every_unit_kind_grants_xp_scaled_by_what_it_cost() {
        for kind in ALL_UNIT_KINDS {
            let xp = xp_for_kill(Some(kind), None);
            assert!(xp > 0.0, "{kind:?} grants no XP at all");
            // 5-XP steps, so the numbers stay readable in a combat log.
            assert_eq!(xp % 5.0, 0.0, "{kind:?} grants a ragged {xp} XP");
        }
        // Monotone in WORTH: a body worth more is always worth at least as
        // much experience. `unit_value`, not the raw row, because a hero's
        // cost fields are 0 — it trains free — and sorting heroes to the
        // bottom of the table would assert the opposite of the design.
        let mut rows: Vec<(UnitKind, u32, f32)> = ALL_UNIT_KINDS
            .iter()
            .map(|&k| {
                let (gold, lumber) = unit_value(k);
                (k, gold + lumber, xp_for_kill(Some(k), None))
            })
            .collect();
        rows.sort_by_key(|r| r.1);
        for pair in rows.windows(2) {
            assert!(
                pair[0].2 <= pair[1].2,
                "{:?} costs less than {:?} but grants more XP",
                pair[0].0,
                pair[1].0
            );
        }

        // The three rows the formula had to reproduce, spelled out: these are
        // the values the hero curve was tuned against before the rule existed.
        assert_eq!(xp_for_kill(Some(UnitKind::Worker), None), 15.0);
        assert_eq!(xp_for_kill(Some(UnitKind::Footman), None), 30.0);
        assert_eq!(xp_for_kill(Some(UnitKind::Archer), None), 30.0);
        // And the kinds that used to pay nothing now pay by their price tag.
        assert_eq!(xp_for_kill(Some(UnitKind::Spearman), None), 20.0);
        assert_eq!(xp_for_kill(Some(UnitKind::Raider), None), 50.0);
        assert_eq!(xp_for_kill(Some(UnitKind::Knight), None), 80.0);
        assert_eq!(xp_for_kill(Some(UnitKind::GryphonRider), None), 100.0);

        // **Heroes train for free and are still the richest kill on the map.**
        // The regression this guards is precise: `xp_for_kill` used to read
        // `cost_gold + cost_lumber` off the row, and the day hero training
        // became free that expression became 0 — killing the enemy Champion,
        // the single most valuable thing either side owns, would have granted
        // no experience at all. Worth is the REVIVAL price, so the number is
        // unchanged from the era when 400g/100l was what you paid up front.
        for hero in ALL_UNIT_KINDS.iter().copied().filter(|&k| is_hero_kind(k)) {
            assert_eq!(unit_stats(hero).cost_gold, 0, "{hero:?} trains free");
            assert_eq!(
                xp_for_kill(Some(hero), None),
                125.0,
                "{hero:?} is the richest kill on the map (500 worth / 20 * 5)"
            );
            let dearest = ALL_UNIT_KINDS
                .iter()
                .copied()
                .filter(|&k| !is_hero_kind(k))
                .map(|k| xp_for_kill(Some(k), None))
                .fold(0.0f32, f32::max);
            assert!(
                xp_for_kill(Some(hero), None) > dearest,
                "{hero:?} must out-pay every line unit"
            );
        }
    }

    /// Structures pay too, and the hall ladder never pays *less* for a taller
    /// rung — the reason buildings price off `building_value` (cumulative)
    /// rather than their own row (an upgrade delta).
    #[test]
    fn every_building_kind_grants_xp_and_the_hall_ladder_never_shrinks() {
        for kind in ALL_BUILDING_KINDS {
            let xp = xp_for_kill(None, Some(kind));
            assert!(xp > 0.0, "{kind:?} grants no XP at all");
            assert_eq!(xp % 5.0, 0.0, "{kind:?} grants a ragged {xp} XP");
        }
        let hall = xp_for_kill(None, Some(BuildingKind::TownHall));
        let keep = xp_for_kill(None, Some(BuildingKind::Keep));
        let castle = xp_for_kill(None, Some(BuildingKind::Castle));
        assert!(hall < keep && keep < castle, "{hall} {keep} {castle}");
        // A 35-resource Wall and a 590-resource TownHall used to be worth the
        // same flat 60. Wall spam is no longer an XP faucet.
        assert!(xp_for_kill(None, Some(BuildingKind::Wall)) < hall / 10.0);
    }


    /// **The whole inversion, per class, on both rosters.** A team's FIRST
    /// hero is free to queue; the same class with a record costs the revival
    /// price. Written as a loop over every hero kind because the bug it guards
    /// against is the boring one: a Kingdom-only edit that left the Warchief
    /// and the Far Seer priced the old way, which is exactly the shape the
    /// previous hero-pricing bug took (the Priestess was priced off her raw
    /// stats while the Champion went through `hero_train_cost`).
    #[test]
    fn every_hero_class_on_both_rosters_is_free_first_and_costly_to_revive() {
        for hero in ALL_UNIT_KINDS.iter().copied().filter(|&k| is_hero_kind(k)) {
            let races = unit_races(hero);
            let s = unit_stats(hero);

            for team in [Team::Human, Team::Claude] {
                let mut records = HeroRecords::default();

                let (gold, lumber, time) = hero_train_cost(&records, team, hero, &[]);
                assert_eq!(
                    (gold, lumber),
                    (0, 0),
                    "{hero:?} ({races:?}) must be FREE the first time"
                );
                assert_eq!(
                    time, s.train_time,
                    "{hero:?} is free, not instant — the train time is the row's"
                );
                assert!(s.supply > 0, "{hero:?} still costs supply");

                records.set(team, HeroRecord { level: 1, xp: 0.0, kind: hero });
                let (gold, lumber, time) = hero_train_cost(&records, team, hero, &[]);
                assert_eq!(
                    (gold, lumber),
                    (s.revive_gold, s.revive_lumber),
                    "{hero:?} revival is the row's revive cost"
                );
                assert_eq!((gold, lumber), (400, 100), "{hero:?}: 400g/100l flat");
                assert_eq!(time, HERO_REVIVE_TIME);

                // The OTHER team's ledger is untouched by this one's death.
                assert_eq!(
                    hero_train_cost(&records, team.enemy(), hero, &[]),
                    (0, 0, s.train_time),
                    "{hero:?}: one team's loss is not the other's bill"
                );
            }
        }
    }

    /// **The waiver is one per team, not one per class.** With a hero already
    /// standing (or already dead) the NEXT class is full fare — the same
    /// 400g/100l its own revival would cost — and it is fielded on a fresh
    /// hero's clock rather than a revival's, because nothing is being bought
    /// back. This is the rule the tier-slot ladder needed: reaching a Keep
    /// buys the *right* to a second hero, not the hero.
    #[test]
    fn only_the_first_hero_is_free_and_the_second_class_pays_full_fare() {
        let team = Team::Human;
        let heroes: Vec<UnitKind> = ALL_UNIT_KINDS
            .iter()
            .copied()
            .filter(|&k| is_hero_kind(k))
            .collect();

        for &first in &heroes {
            for &second in heroes.iter().filter(|&&k| k != first) {
                // Both classes must be fieldable by one team for the pairing to
                // mean anything — the rosters are per race.
                if !unit_races(first).iter().any(|r| unit_races(second).contains(r)) {
                    continue;
                }
                let s = unit_stats(second);

                // (a) hero #1 alive: it holds a slot AND has a record.
                let mut records = HeroRecords::default();
                records.set(team, HeroRecord { level: 3, xp: 0.0, kind: first });
                assert_eq!(
                    hero_train_cost(&records, team, second, &[first]),
                    (s.revive_gold, s.revive_lumber, s.train_time),
                    "{second:?} after a living {first:?}: full fare, fresh clock"
                );

                // (b) hero #1 dead: no body held, but the team has had a hero,
                // and that is what the waiver is spent on.
                assert_eq!(
                    hero_train_cost(&records, team, second, &[]),
                    (s.revive_gold, s.revive_lumber, s.train_time),
                    "{second:?} after a DEAD {first:?}: the waiver is already spent"
                );

                // ...and the freebie is still there for a team that has done
                // neither. Nothing about `first` leaks across the line.
                assert_eq!(
                    hero_train_cost(&HeroRecords::default(), team, second, &[]),
                    (0, 0, s.train_time),
                    "{second:?} for a team with no hero at all: free"
                );
            }
        }
    }

    /// **The queue edge, decided the way `hero_slot_check` decides it.** A
    /// first hero sitting in a training queue has no record yet — nothing has
    /// spawned — so only the `held` list knows it exists. It counts. Two heroes
    /// queued in the same breath must not both come out free, and the slot rule
    /// already counts in-flight heroes for the same reason.
    #[test]
    fn a_first_hero_still_in_the_queue_already_prices_the_second() {
        let team = Team::Human;
        let (first, second) = (UnitKind::Hero, UnitKind::Priestess);
        let records = HeroRecords::default();
        let s = unit_stats(second);

        // Nothing anywhere: the next hero is the free one.
        assert_eq!(
            hero_train_cost(&records, team, first, &[]),
            (0, 0, unit_stats(first).train_time)
        );
        // The Champion is queued and not yet born. No record exists — the
        // in-flight list is the only witness, and it is enough.
        assert_eq!(
            hero_train_cost(&records, team, second, &[first]),
            (s.revive_gold, s.revive_lumber, s.train_time),
            "a queued first hero must already have spent the waiver"
        );
        // Same list, same answer as the slot rule reading it: both functions
        // are asked with one slice on purpose.
        assert_eq!(
            hero_slot_check(&[first], second, TechTier::T2),
            HeroSlotVerdict::Ok,
            "the slot rule lets it through; it is the PRICE that changed"
        );
        assert_eq!(
            hero_slot_check(&[first], first, TechTier::T2),
            HeroSlotVerdict::DuplicateClass
        );
    }

    /// **The revival fee is flat — it does not scale with the level it buys
    /// back.** This is the design decision the bead turned on, so it is
    /// asserted rather than left to the data: scaling the price with level
    /// would hand the largest invoice to the commander who kept a hero alive
    /// longest, which is an anti-incentive aimed squarely at the behaviour the
    /// free-hero change exists to reward. The gradient lives on the other side
    /// — revival preserves the level, so a flat fee buys back *more* the
    /// higher the hero was.
    #[test]
    fn the_revival_price_is_flat_across_every_level() {
        let team = Team::Human;
        for hero in ALL_UNIT_KINDS.iter().copied().filter(|&k| is_hero_kind(k)) {
            let prices: Vec<_> = [1, 2, 5, 10, 99]
                .into_iter()
                .map(|level| {
                    let mut records = HeroRecords::default();
                    records.set(
                        team,
                        HeroRecord { level, xp: level as f32 * 10.0, kind: hero },
                    );
                    hero_train_cost(&records, team, hero, &[])
                })
                .collect();
            assert!(
                prices.windows(2).all(|w| w[0] == w[1]),
                "{hero:?}: the revival fee moved with the level: {prices:?}"
            );
        }
    }

    /// A hero's WORTH and a hero's PRICE are different questions, and the
    /// answer to the first must never be zero. `unit_value` is what
    /// `xp_for_kill` and `asset_score` read; if it followed `cost_gold` down
    /// to zero, killing the enemy Champion would grant nothing and a hero
    /// army would settle a timeout as if it owned nothing.
    #[test]
    fn a_free_hero_is_still_worth_its_replacement_cost() {
        for kind in ALL_UNIT_KINDS {
            let s = unit_stats(kind);
            let (gold, lumber) = unit_value(kind);
            if is_hero_kind(kind) {
                assert_eq!((s.cost_gold, s.cost_lumber), (0, 0), "{kind:?} trains free");
                assert_eq!(
                    (gold, lumber),
                    (s.revive_gold, s.revive_lumber),
                    "{kind:?} is worth what replacing it costs"
                );
                assert!(gold + lumber > 0, "{kind:?} must not be worth nothing");
            } else {
                assert_eq!(
                    (gold, lumber),
                    (s.cost_gold, s.cost_lumber),
                    "{kind:?}: for everything else, worth IS cost"
                );
            }
        }

        // The timeout tiebreaker sees the hero, and sees it as the most
        // valuable body on the field.
        let bank = Economy::default();
        let with_hero = asset_score(&bank, [UnitKind::Hero].into_iter(), [].into_iter());
        let with_knight = asset_score(&bank, [UnitKind::Knight].into_iter(), [].into_iter());
        assert!(
            with_hero > with_knight && with_hero >= 500,
            "a hero must outweigh the priciest line unit: {with_hero} vs {with_knight}"
        );
    }

    /// Records are per CLASS, and so is REVIVAL: a team that has lost both
    /// heroes owes for each of them separately, and buying the Champion back
    /// does not make the Priestess cheaper. What is *not* per class is the
    /// one-time waiver — a team with a level-6 Champion has already had its
    /// free hero, so the first Priestess is full fare.
    #[test]
    fn hero_records_and_prices_are_per_class() {
        let mut records = HeroRecords::default();
        let team = Team::Human;

        let (g, l, t) = hero_train_cost(&records, team, UnitKind::Hero, &[]);
        let base = unit_stats(UnitKind::Hero);
        assert_eq!(
            (g, l, t),
            (base.cost_gold, base.cost_lumber, base.train_time)
        );

        records.set(
            team,
            HeroRecord {
                level: 6,
                xp: 12.0,
                kind: UnitKind::Hero,
            },
        );
        assert_eq!(
            hero_train_cost(&records, team, UnitKind::Hero, &[UnitKind::Hero]),
            (base.revive_gold, base.revive_lumber, HERO_REVIVE_TIME),
        );
        let priestess = unit_stats(UnitKind::Priestess);
        assert_eq!(
            hero_train_cost(&records, team, UnitKind::Priestess, &[UnitKind::Hero]),
            (
                priestess.revive_gold,
                priestess.revive_lumber,
                priestess.train_time
            ),
            "a Champion on the field spends the team's one free hero",
        );

        // Both classes coexist, upsert keeps one record each, and the other
        // team is untouched.
        records.set(
            team,
            HeroRecord {
                level: 2,
                xp: 5.0,
                kind: UnitKind::Priestess,
            },
        );
        records.set(
            team,
            HeroRecord {
                level: 7,
                xp: 0.0,
                kind: UnitKind::Hero,
            },
        );
        assert_eq!(records.list(team).len(), 2);
        assert_eq!(records.get(team, UnitKind::Hero).map(|r| r.level), Some(7));
        assert_eq!(
            records.get(team, UnitKind::Priestess).map(|r| r.level),
            Some(2)
        );
        assert!(records.list(Team::Claude).is_empty());
    }

    // -----------------------------------------------------------------------
    // Research
    // -----------------------------------------------------------------------

    /// Costs and durations climb strictly with the rung, on every ladder. A
    /// flat or non-monotonic price list would make level 3 the obvious opening
    /// purchase and the whole escalation decorative.
    #[test]
    fn research_costs_escalate_strictly_on_every_ladder() {
        for kind in ALL_RESEARCH_KINDS {
            let steps: Vec<ResearchStep> = (1..=RESEARCH_MAX_LEVEL)
                .map(|l| research_step(kind, l).expect("every rung up to the cap exists"))
                .collect();
            assert_eq!(steps.len() as u32, RESEARCH_MAX_LEVEL);
            for pair in steps.windows(2) {
                let (lo, hi) = (&pair[0], &pair[1]);
                assert_eq!(hi.level, lo.level + 1, "{} rungs are contiguous", kind.id());
                assert!(hi.cost_gold > lo.cost_gold, "{} gold escalates", kind.id());
                assert!(hi.cost_lumber > lo.cost_lumber, "{} lumber escalates", kind.id());
                assert!(
                    hi.research_time > lo.research_time,
                    "{} takes longer each rung",
                    kind.id()
                );
            }
            // Every rung costs both resources: research is deliberately not
            // purchasable out of a pure gold economy.
            assert!(steps.iter().all(|s| s.cost_gold > 0 && s.cost_lumber > 0));
        }
    }

    /// The cap holds from both directions: `research_step` refuses to quote a
    /// price above it, and `advance` refuses to climb past it however many
    /// times it is called.
    #[test]
    fn research_levels_stop_at_the_cap() {
        for kind in ALL_RESEARCH_KINDS {
            assert!(research_step(kind, 0).is_none(), "level 0 is not a rung");
            assert!(
                research_step(kind, RESEARCH_MAX_LEVEL + 1).is_none(),
                "{} has nothing above the cap",
                kind.id()
            );

            let mut state = ResearchState::default();
            assert_eq!(state.level(kind), 0);
            for expected in 1..=RESEARCH_MAX_LEVEL {
                assert!(state.next_step(kind).is_some(), "a rung remains below the cap");
                assert_eq!(state.advance(kind), Some(expected));
                assert_eq!(state.level(kind), expected);
            }
            // Saturated: further advances are refused and change nothing.
            assert!(state.next_step(kind).is_none(), "nothing left to buy");
            assert_eq!(state.advance(kind), None);
            assert_eq!(state.level(kind), RESEARCH_MAX_LEVEL);
            assert_eq!(
                research_bonus(kind, state.level(kind)),
                RESEARCH_MAX_LEVEL as f32
            );
        }
    }

    /// The two ladders are independent: buying attack does not move armor.
    #[test]
    fn research_ladders_advance_independently() {
        let mut state = ResearchState::default();
        state.advance(ResearchKind::Attack);
        state.advance(ResearchKind::Attack);
        assert_eq!(state.attack_bonus(), 2.0);
        assert_eq!(state.armor_bonus(), 0.0);
        assert_eq!(state.bonus().bonus_damage, 2.0);
        assert_eq!(state.bonus().flat_armor, 0.0);
    }

    /// The whole point of the mechanic, measured end to end through the stat
    /// law: a Footman with attack research swings for exactly +N, and a Footman
    /// with armor research takes exactly -N. Flat, not scaled.
    #[test]
    fn research_shifts_effective_damage_by_exactly_the_flat_bonus() {
        let kind = UnitKind::Footman;
        let base = unit_stats(kind).damage;

        let mut attacker = ResearchState::default();
        let mut victim = ResearchState::default();
        for level in 1..=RESEARCH_MAX_LEVEL {
            attacker.advance(ResearchKind::Attack);
            victim.advance(ResearchKind::Armor);
            let bonus = level as f32;

            // Outgoing: the swing is base + N.
            let out = effective_unit_stats_with(kind, None, attacker.bonus());
            assert_eq!(out.bonus_damage, bonus);
            assert_eq!(base * out.damage_mult + out.bonus_damage, base + bonus);

            // Incoming: the hit lands for base - N.
            let inc = effective_unit_stats_with(kind, None, victim.bonus());
            assert_eq!(inc.flat_armor, bonus);
            assert_eq!(
                damage_after_armor(base, inc.damage_taken_mult, inc.flat_armor),
                base - bonus
            );
        }

        // Both at once: +3 attack against +3 armor is a wash, exactly.
        let out = effective_unit_stats_with(kind, None, attacker.bonus());
        let inc = effective_unit_stats_with(kind, None, victim.bonus());
        let swing = base * out.damage_mult + out.bonus_damage;
        assert_eq!(
            damage_after_armor(swing, inc.damage_taken_mult, inc.flat_armor),
            base
        );
    }

    /// A flat bonus must never be caught by a multiplier. A Catapult's 6x vs
    /// buildings would turn +3 into +18 if the term were added before the
    /// multiply — this pins the order of operations combat.rs uses.
    #[test]
    fn attack_research_is_never_multiplied_by_a_type_bonus() {
        let mut state = ResearchState::default();
        for _ in 0..RESEARCH_MAX_LEVEL {
            state.advance(ResearchKind::Attack);
        }
        let stats = unit_stats(UnitKind::Catapult);
        let eff = effective_unit_stats_with(UnitKind::Catapult, None, state.bonus());
        // The arithmetic combat.rs performs, verbatim in shape.
        let vs_building =
            stats.damage * stats.vs_building_mult * eff.damage_mult + eff.bonus_damage;
        assert_eq!(vs_building, stats.damage * 6.0 + 3.0);
        assert_ne!(vs_building, (stats.damage + 3.0) * 6.0);
    }

    /// Armor is a discount, never an immunity: the floor holds even when the
    /// research would otherwise erase a weak attack, and it never rounds a hit
    /// UP.
    #[test]
    fn armor_floors_damage_without_ever_raising_it() {
        let mut state = ResearchState::default();
        for _ in 0..RESEARCH_MAX_LEVEL {
            state.advance(ResearchKind::Armor);
        }
        let armor = state.armor_bonus();

        // A Worker's 5-damage swing is heavily reduced but still lands.
        let worker = unit_stats(UnitKind::Worker).damage;
        assert_eq!(damage_after_armor(worker, 1.0, armor), worker - armor);
        // An attack weaker than the armour is floored, not zeroed or negated.
        assert_eq!(damage_after_armor(2.0, 1.0, armor), MIN_DAMAGE_PER_HIT);
        assert_eq!(damage_after_armor(0.5, 1.0, armor), 0.5, "never rounded up");
        // Unresearched teams are completely unaffected by the new pipeline.
        assert_eq!(damage_after_armor(12.0, 1.0, 0.0), 12.0);
    }

    /// The forge sits at tier 2, is placeable, trains nothing, and therefore
    /// cannot keep a losing team alive under the win condition.
    #[test]
    fn the_blacksmith_is_a_tier_two_support_building() {
        let kind = BuildingKind::Blacksmith;
        assert!(building_placeable(kind), "workers place it");
        assert_eq!(building_requires(kind), &[BuildingKind::Keep]);
        assert!(trainable(kind).is_empty(), "not a production building");
        assert_eq!(building_researches(kind), &ALL_RESEARCH_KINDS);
        assert!(building_stats(kind).vision > 0.0, "every building sees");

        // A Keep satisfies it, a TownHall does not, and a Castle does — the
        // tier comparison, not kind equality.
        assert!(!requirements_met(
            building_requires(kind),
            [BuildingKind::TownHall].into_iter()
        ));
        assert!(requirements_met(
            building_requires(kind),
            [BuildingKind::Keep].into_iter()
        ));
        assert!(requirements_met(
            building_requires(kind),
            [BuildingKind::Castle].into_iter()
        ));

        // Nothing else researches, so a `research` command naming any other
        // building is refused by the same table the card draws from. "Nothing
        // else" means "nothing outside the one forge PER RACE": research is
        // shared content sold by two different buildings, and each race can
        // only ever reach its own.
        for other in ALL_BUILDING_KINDS
            .into_iter()
            .filter(|k| !research_buildings().contains(k))
        {
            assert!(
                building_researches(other).is_empty(),
                "{other:?} is not a forge"
            );
        }
        for race in ALL_RACES {
            let forge = research_building_for(race).expect("every race has a forge");
            assert!(race_has_building(race, forge));
            for r in ALL_RESEARCH_KINDS {
                assert!(building_researches(forge).contains(&r));
            }
        }
    }

    /// The catalog carries enough to plan a research investment without
    /// reading the source, and deliberately does NOT carry a current level.
    #[test]
    fn the_catalog_exports_every_research_rung() {
        let catalog = game_catalog();
        assert_eq!(catalog.research.len(), ALL_RESEARCH_KINDS.len());
        for entry in &catalog.research {
            let kind = ALL_RESEARCH_KINDS
                .into_iter()
                .find(|k| k.id() == entry.id)
                .expect("catalog ids are ladder ids");
            assert_eq!(entry.researched_at, building_name(BuildingKind::Blacksmith));
            assert_eq!(entry.max_level, RESEARCH_MAX_LEVEL);
            assert_eq!(entry.levels.len() as u32, RESEARCH_MAX_LEVEL);
            for (i, level) in entry.levels.iter().enumerate() {
                let step = research_step(kind, i as u32 + 1).expect("a rung per level");
                assert_eq!(level.level, step.level);
                assert_eq!(level.cost_gold, step.cost_gold);
                assert_eq!(level.cost_lumber, step.cost_lumber);
                assert_eq!(level.research_time, step.research_time);
                // Cumulative, not incremental: level 2 reads 2.
                assert_eq!(level.bonus, level.level as f32);
            }
        }
        // The forge itself appears in the building catalog, gated on the Keep.
        let forge = catalog
            .buildings
            .iter()
            .find(|b| b.id == "Blacksmith")
            .expect("the forge is catalog content");
        assert_eq!(forge.requires, vec!["Keep"]);
        assert!(forge.trains.is_empty());
        assert!(forge.placeable);
    }
}
