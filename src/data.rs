//! Content data: the stat tables, loaded from RON instead of compiled in as
//! match arms.
//!
//! # Why this module exists
//!
//! Every parallel campaign wave hit the same hazard. Two agents each add a
//! field or a row to `unit_stats`, git merges both hunks cleanly because they
//! touch different lines of the same `match`, and the result is a table where
//! one unit silently kept the other's numbers — caught, if at all, by a
//! missing-field compile error three commits later. Row literals interleave.
//!
//! So the DATA moved out of the code. `assets/data/*.ron` holds one file per
//! table and one record per row; a merge either conflicts loudly (two edits to
//! the same record) or not at all (edits to different records). What stayed in
//! Rust is *identity and rules*: the `UnitKind`/`BuildingKind` enums, the
//! formulas, the systems. See DESIGN.md § "Content data files" for the
//! contract.
//!
//! # Load mechanism
//!
//! Each table has a COMPILED-IN default (`include_str!`), so `cargo run` works
//! from any working directory, a `cargo test` needs no fixture path, and a
//! shipped binary carries its own content. Setting `WC3_DATA_DIR=<dir>` makes
//! the loader prefer `<dir>/<file>.ron` for any file that exists there — the
//! modder / balance-tuner path, and the way to change a number without
//! recompiling anything.
//!
//! Tables are `LazyLock`s, so "loaded before anything reads stats" is a
//! structural guarantee rather than a system-ordering promise: the first read
//! *is* the load, in a headless run, a windowed run, or a unit test. The
//! `CorePlugin` additionally forces the load (and therefore the validator)
//! during `App` construction, so a bad data file kills the process at startup
//! with a naming error instead of mid-match.

use bevy::prelude::*;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::shared::{
    is_hero_kind, kind_name, normalize_name, status_probe_enabled, upgrade_root, AbilityDef,
    AbilityTarget, AbilityTargets, Effect, EffectAtom, EffectSchedule,
    BuildingKind,
    BuildingStats, ItemDef, ItemId, ResearchKind, ResearchStep, TechTier, UnitKind,
    UnitStats,
    AbilityUnlock, ALL_BUILDING_KINDS, ALL_ITEMS, ALL_RESEARCH_KINDS, ALL_UNIT_KINDS,
    RESEARCH_MAX_LEVEL,
};

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// The environment variable that points the loader at a directory of override
/// files. Any `<name>.ron` present there replaces the compiled-in default;
/// anything absent falls back, so a modder ships only the files they changed.
pub const DATA_DIR_ENV: &str = "WC3_DATA_DIR";

const UNITS_RON: &str = include_str!("../assets/data/units.ron");
const BUILDINGS_RON: &str = include_str!("../assets/data/buildings.ron");
const ABILITIES_RON: &str = include_str!("../assets/data/abilities.ron");
const ITEMS_RON: &str = include_str!("../assets/data/items.ron");
const RESEARCH_RON: &str = include_str!("../assets/data/research.ron");

/// Read one table's text: the override file if `WC3_DATA_DIR` names a
/// directory containing it, otherwise the compiled-in copy.
///
/// A file that is present but unreadable is a hard error rather than a silent
/// fallback: a modder who edited a file and got the built-in numbers anyway
/// would have no way to tell.
fn source(file: &str, builtin: &'static str) -> String {
    let Some(dir) = std::env::var_os(DATA_DIR_ENV) else {
        return builtin.to_string();
    };
    let path = std::path::Path::new(&dir).join(file);
    if !path.exists() {
        return builtin.to_string();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            info!("{DATA_DIR_ENV}: loaded {} from disk", path.display());
            text
        }
        Err(err) => panic!("{DATA_DIR_ENV}: cannot read {}: {err}", path.display()),
    }
}

fn parse<T: serde::de::DeserializeOwned>(file: &str, builtin: &'static str) -> T {
    let text = source(file, builtin);
    match ron::from_str::<T>(&text) {
        Ok(value) => value,
        Err(err) => panic!("content data: {file} is not valid RON: {err}"),
    }
}

/// Turn a loaded `String` into a `&'static str`.
///
/// Every string in these tables is an id or a caption that lives for the whole
/// process, and the two tables whose rows are handed out BY VALUE (`AbilityDef`
/// and `ItemDef` are `Copy`, at call sites that expect `&'static str`) cannot
/// borrow from the table. Leaking a few dozen strings once, at load, buys
/// every one of those call sites staying exactly as it was.
///
/// The row tables (`UnitRow`, `BuildingRow`, …) do NOT leak: they live in
/// `TABLES` forever, so `row.name.as_str()` is already `&'static str`.
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

// ---------------------------------------------------------------------------
// File schemas
// ---------------------------------------------------------------------------

/// One row of `units.ron`.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct UnitRow {
    pub kind: UnitKind,
    pub name: String,
    pub description: String,
    /// Tech gate beyond owning the trainer building.
    pub requires: Vec<BuildingKind>,
    pub stats: UnitStats,
}

/// One row of `buildings.ron`.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BuildingRow {
    pub kind: BuildingKind,
    pub name: String,
    pub description: String,
    pub requires: Vec<BuildingKind>,
    pub trains: Vec<UnitKind>,
    pub researches: Vec<ResearchKind>,
    pub upgrades_to: Option<BuildingKind>,
    pub stats: BuildingStats,
}

/// The wire shape of an `AbilityDef`.
///
/// A mirror rather than a `Deserialize` derive on `AbilityDef` itself, because
/// `AbilityDef` is `Copy` with `&'static str` fields, and serde will not
/// deserialize a `&'static str` from an owned document at any price (its
/// derive infers a `'de: 'static` bound the moment it sees a reference field).
/// This is the one schema in the crate written twice; `From` below is where
/// the two halves are joined, and a field added to one without the other is a
/// compile error rather than a silent default.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct AbilityDefRow {
    name: String,
    /// **The sentence.** One or more `(atom: …, schedule: …)` clauses, applied
    /// in order from one centre. `schedule` defaults to `Instant`, so the
    /// common row reads `effects: [(atom: Damage(amount: 45.0))]`.
    effects: Vec<Effect>,
    /// Where the effect is centred. DEFAULTED, unlike every other field here:
    /// `AbilityTarget::Caster` is what every row meant before geometry was a
    /// thing, so the caster-centred majority stays silent and only a thrown
    /// spell says `target: Point(range: 9.0)`. A row that omits it reads
    /// exactly as it did before this field existed.
    #[serde(default)]
    target: AbilityTarget,
    mana_cost: f32,
    cooldown: f32,
    radius: f32,
    hits_air: bool,
    unlock: AbilityUnlock,
    description: String,
}

impl From<AbilityDefRow> for AbilityDef {
    fn from(row: AbilityDefRow) -> AbilityDef {
        let AbilityDefRow {
            name,
            effects,
            target,
            mana_cost,
            cooldown,
            radius,
            hits_air,
            unlock,
            description,
        } = row;
        AbilityDef {
            name: leak(name),
            // Same trick as `leak`, one level up: the parsed atoms live as long
            // as `TABLES` does, which is forever, so the slice may be
            // `&'static` and `AbilityDef` may stay `Copy`.
            effects: Box::leak(effects.into_boxed_slice()),
            target,
            mana_cost,
            cooldown,
            radius,
            hits_air,
            unlock,
            description: leak(description),
        }
    }
}

/// The wire shape of an `ItemDef` — a mirror for the same reason as above.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ItemDefRow {
    name: String,
    cost_gold: u32,
    tier: TechTier,
    description: String,
}

impl From<ItemDefRow> for ItemDef {
    fn from(row: ItemDefRow) -> ItemDef {
        let ItemDefRow { name, cost_gold, tier, description } = row;
        ItemDef {
            name: leak(name),
            cost_gold,
            tier,
            description: leak(description),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct AbilityFile {
    defs: Vec<AbilityDefRow>,
    unit_casters: Vec<UnitCasterRow>,
    building_casters: Vec<BuildingCasterRow>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct UnitCasterRow {
    kind: UnitKind,
    abilities: Vec<String>,
    /// Appended to `abilities` only under `WC3_STATUS_PROBE=1`.
    #[serde(default)]
    probe_abilities: Vec<String>,
    #[serde(default)]
    autocast: Option<AutoCastRow>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct AutoCastRow {
    /// Named, not indexed, so inserting a row ahead of it moves the rule too.
    ability: String,
    min_targets: u32,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct BuildingCasterRow {
    /// The LADDER ROOT. Every rung above it inherits the list.
    kind: BuildingKind,
    abilities: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ItemRow {
    id: ItemId,
    def: ItemDefRow,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ResearchFile {
    building: BuildingKind,
    ladders: Vec<ResearchLadderRow>,
    steps: Vec<ResearchStep>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ResearchLadderRow {
    kind: ResearchKind,
    id: String,
    label: String,
    description: String,
}

// ---------------------------------------------------------------------------
// The loaded tables
// ---------------------------------------------------------------------------

/// Everything the accessors in shared.rs read. Slot-indexed by
/// `kind as usize`, which is the enum's declaration order.
pub struct Tables {
    units: Vec<UnitRow>,
    buildings: Vec<BuildingRow>,
    unit_abilities: Vec<Vec<AbilityDef>>,
    unit_autocast: Vec<Option<(usize, u32)>>,
    /// Indexed by LADDER ROOT; `abilities_of_building` resolves the root.
    building_abilities: Vec<Vec<AbilityDef>>,
    items: Vec<ItemDef>,
    research_building: BuildingKind,
    research_ladders: Vec<ResearchLadderRow>,
    research_steps: Vec<ResearchStep>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(load);

fn tables() -> &'static Tables {
    &TABLES
}

/// Force the load (and therefore the validator) now. `CorePlugin` calls this
/// during `App` construction so bad data is a startup panic with a message
/// naming what is wrong, not a surprise in the middle of a match.
pub fn ensure_loaded() {
    LazyLock::force(&TABLES);
}

/// Sort `rows` into a slot per enum variant, or report the variants that have
/// no row and the ones that have several. This is the "every enum variant must
/// have a row, naming which" check.
fn slot_by_kind<T, K: Copy>(
    table: &str,
    rows: Vec<T>,
    all: &[K],
    kind_of: impl Fn(&T) -> K,
    index_of: impl Fn(K) -> usize,
    name_of: impl Fn(K) -> String,
    problems: &mut Vec<String>,
) -> Vec<T> {
    let mut slots: Vec<Option<T>> = (0..all.len()).map(|_| None).collect();
    for row in rows {
        let kind = kind_of(&row);
        let index = index_of(kind);
        match slots.get_mut(index) {
            Some(slot) if slot.is_none() => *slot = Some(row),
            Some(_) => problems.push(format!("{table}: duplicate row for {}", name_of(kind))),
            None => problems.push(format!(
                "{table}: {} is outside the table (slot {index} of {})",
                name_of(kind),
                all.len()
            )),
        }
    }
    let mut out = Vec::with_capacity(all.len());
    for (index, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(row) => out.push(row),
            None => {
                let missing = all
                    .iter()
                    .copied()
                    .find(|k| index_of(*k) == index)
                    .map(&name_of)
                    .unwrap_or_else(|| format!("slot {index}"));
                problems.push(format!("{table}: no row for {missing}"));
            }
        }
    }
    out
}

fn load() -> Tables {
    let mut problems: Vec<String> = Vec::new();

    let unit_rows: Vec<UnitRow> = parse("units.ron", UNITS_RON);
    let building_rows: Vec<BuildingRow> = parse("buildings.ron", BUILDINGS_RON);
    let AbilityFile { defs, unit_casters, building_casters } =
        parse::<AbilityFile>("abilities.ron", ABILITIES_RON);
    let defs: Vec<AbilityDef> = defs.into_iter().map(AbilityDef::from).collect();
    let item_rows: Vec<ItemRow> = parse("items.ron", ITEMS_RON);
    let research_file: ResearchFile = parse("research.ron", RESEARCH_RON);

    // Slotting first: every later check assumes a full table, and a missing
    // row is the failure this whole exercise exists to make loud.
    let units = slot_by_kind(
        "units.ron",
        unit_rows,
        &ALL_UNIT_KINDS,
        |row| row.kind,
        |kind| kind as usize,
        |kind| format!("{kind:?}"),
        &mut problems,
    );
    let buildings = slot_by_kind(
        "buildings.ron",
        building_rows,
        &ALL_BUILDING_KINDS,
        |row| row.kind,
        |kind| kind as usize,
        |kind| format!("{kind:?}"),
        &mut problems,
    );
    let items: Vec<ItemDef> = slot_by_kind(
        "items.ron",
        item_rows,
        &ALL_ITEMS,
        |row| row.id,
        |id| id as usize,
        |id| format!("{id:?}"),
        &mut problems,
    )
    .into_iter()
    .map(|row| ItemDef::from(row.def))
    .collect();
    let research_ladders = slot_by_kind(
        "research.ron",
        research_file.ladders,
        &ALL_RESEARCH_KINDS,
        |row| row.kind,
        |kind| kind as usize,
        |kind| format!("{kind:?}"),
        &mut problems,
    );

    if !problems.is_empty() {
        panic!("{}", report(&problems));
    }

    // Ability lists: names resolved to defs, in slot order.
    let find = |name: &str| defs.iter().find(|d| d.name == name).copied();
    let resolve = |where_: &str, names: &[String], problems: &mut Vec<String>| -> Vec<AbilityDef> {
        names
            .iter()
            .filter_map(|name| match find(name) {
                Some(def) => Some(def),
                None => {
                    problems.push(format!("abilities.ron: {where_} names unknown ability {name:?}"));
                    None
                }
            })
            .collect()
    };

    let mut unit_abilities: Vec<Vec<AbilityDef>> =
        (0..ALL_UNIT_KINDS.len()).map(|_| Vec::new()).collect();
    let mut unit_autocast: Vec<Option<(usize, u32)>> =
        (0..ALL_UNIT_KINDS.len()).map(|_| None).collect();
    for row in &unit_casters {
        let slot = row.kind as usize;
        if slot >= unit_abilities.len() {
            problems.push(format!("abilities.ron: {:?} is not a unit kind", row.kind));
            continue;
        }
        if !unit_abilities[slot].is_empty() {
            problems.push(format!("abilities.ron: duplicate caster row for {:?}", row.kind));
        }
        let mut list = resolve(&format!("{:?}", row.kind), &row.abilities, &mut problems);
        // The `WC3_STATUS_PROBE` dev mutation, applied at LOAD time: the probe
        // abilities are appended to the tail of the list, so every shipping
        // slot index is unchanged whether the probe is on or off.
        if status_probe_enabled() {
            list.extend(resolve(
                &format!("{:?} probe", row.kind),
                &row.probe_abilities,
                &mut problems,
            ));
        }
        if let Some(auto) = &row.autocast {
            match list.iter().position(|d| d.name == auto.ability) {
                Some(index) => unit_autocast[slot] = Some((index, auto.min_targets)),
                None => problems.push(format!(
                    "abilities.ron: {:?} auto-casts {:?}, which is not in its own list",
                    row.kind, auto.ability
                )),
            }
        }
        unit_abilities[slot] = list;
    }

    let mut building_abilities: Vec<Vec<AbilityDef>> =
        (0..ALL_BUILDING_KINDS.len()).map(|_| Vec::new()).collect();
    for row in &building_casters {
        let slot = row.kind as usize;
        if slot >= building_abilities.len() {
            problems.push(format!("abilities.ron: {:?} is not a building kind", row.kind));
            continue;
        }
        if !building_abilities[slot].is_empty() {
            problems.push(format!("abilities.ron: duplicate caster row for {:?}", row.kind));
        }
        building_abilities[slot] = resolve(&format!("{:?}", row.kind), &row.abilities, &mut problems);
    }

    let tables = Tables {
        units,
        buildings,
        unit_abilities,
        unit_autocast,
        building_abilities,
        items,
        research_building: research_file.building,
        research_ladders,
        research_steps: research_file.steps,
    };

    problems.extend(check_values(&tables, &defs));
    if !problems.is_empty() {
        panic!("{}", report(&problems));
    }
    tables
}

fn report(problems: &[String]) -> String {
    format!(
        "content data is invalid ({} problem{}):\n  - {}",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" },
        problems.join("\n  - ")
    )
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Every value check the tables have to pass. Pure over already-slotted
/// tables so the tests can prove it bites by handing it a deliberately broken
/// copy.
///
/// Costs are `u32`, so "no negative costs" is enforced by the type; what is
/// checkable here is everything a zero or a negative would quietly break —
/// a divide-by-zero attack cooldown, a unit that can see nothing, a building
/// with no footprint, a weapon that cannot reach.
fn check_values(t: &Tables, defs: &[AbilityDef]) -> Vec<String> {
    let mut p = Vec::new();
    let positive = |p: &mut Vec<String>, what: &str, field: &str, value: f32| {
        if !(value > 0.0) {
            p.push(format!("{what}: {field} must be > 0, got {value}"));
        }
    };

    // --- units ------------------------------------------------------------
    let mut unit_names: Vec<String> = Vec::new();
    for row in &t.units {
        let what = format!("units.ron/{:?}", row.kind);
        let s = &row.stats;
        positive(&mut p, &what, "hp", s.hp);
        positive(&mut p, &what, "damage", s.damage);
        positive(&mut p, &what, "range", s.range);
        positive(&mut p, &what, "attack_cooldown", s.attack_cooldown);
        positive(&mut p, &what, "speed", s.speed);
        positive(&mut p, &what, "train_time", s.train_time);
        positive(&mut p, &what, "vision", s.vision);
        positive(&mut p, &what, "vs_building_mult", s.vs_building_mult);
        positive(&mut p, &what, "vs_siege_mult", s.vs_siege_mult);
        positive(&mut p, &what, "vs_cavalry_mult", s.vs_cavalry_mult);
        if s.supply == 0 {
            p.push(format!("{what}: supply must be >= 1"));
        }
        if !s.can_hit_air && !s.can_hit_ground {
            p.push(format!("{what}: can hit neither air nor ground"));
        }
        if row.name.trim().is_empty() {
            p.push(format!("{what}: name is empty"));
        }
        if row.description.trim().is_empty() {
            p.push(format!("{what}: description is empty"));
        }
        unit_names.push(normalize_name(&row.name));
    }
    duplicates("units.ron", "name", &unit_names, &mut p);

    // --- buildings --------------------------------------------------------
    let mut building_names: Vec<String> = Vec::new();
    for row in &t.buildings {
        let what = format!("buildings.ron/{:?}", row.kind);
        let s = &row.stats;
        positive(&mut p, &what, "hp", s.hp);
        positive(&mut p, &what, "build_time", s.build_time);
        positive(&mut p, &what, "size", s.size);
        positive(&mut p, &what, "vision", s.vision);
        if let Some(a) = s.attack {
            positive(&mut p, &what, "attack.damage", a.damage);
            positive(&mut p, &what, "attack.range", a.range);
            positive(&mut p, &what, "attack.cooldown", a.cooldown);
        }
        if row.name.trim().is_empty() {
            p.push(format!("{what}: name is empty"));
        }
        if row.description.trim().is_empty() {
            p.push(format!("{what}: description is empty"));
        }
        if row.upgrades_to == Some(row.kind) {
            p.push(format!("{what}: upgrades_to itself"));
        }
        building_names.push(normalize_name(&row.name));
    }
    duplicates("buildings.ron", "name", &building_names, &mut p);

    // The ladder must be a forest: at most one rung below any kind, and no
    // cycles. `upgrade_root` and `building_tier` walk it with a loop bound,
    // so a cycle here would silently truncate a tier instead of hanging.
    for &kind in &ALL_BUILDING_KINDS {
        let parents: Vec<_> = t
            .buildings
            .iter()
            .filter(|r| r.upgrades_to == Some(kind))
            .map(|r| r.kind)
            .collect();
        if parents.len() > 1 {
            p.push(format!(
                "buildings.ron: {kind:?} is the upgrade target of {} kinds ({parents:?}); \
                 the ladder must be a tree",
                parents.len()
            ));
        }
        let mut seen = vec![kind];
        let mut current = kind;
        for _ in 0..ALL_BUILDING_KINDS.len() {
            let Some(next) = t.buildings[current as usize].upgrades_to else {
                break;
            };
            if seen.contains(&next) {
                p.push(format!("buildings.ron: upgrade ladder from {kind:?} cycles at {next:?}"));
                break;
            }
            seen.push(next);
            current = next;
        }
    }

    // --- abilities --------------------------------------------------------
    let mut ability_names: Vec<String> = Vec::new();
    for def in defs {
        let what = format!("abilities.ron/{}", def.name);
        if def.name.trim().is_empty() {
            p.push("abilities.ron: an ability has an empty name".to_string());
        }
        if def.description.trim().is_empty() {
            p.push(format!("{what}: description is empty"));
        }
        if def.mana_cost < 0.0 {
            p.push(format!("{what}: mana_cost must be >= 0"));
        }
        if def.cooldown < 0.0 {
            p.push(format!("{what}: cooldown must be >= 0"));
        }
        positive(&mut p, &what, "radius", def.radius);

        // --- the composition ----------------------------------------------
        //
        // v3 made an ability a LIST of atoms, which means a row can now be
        // wrong in ways v2's closed enum made unrepresentable: an empty
        // sentence, a summoned hero, a recall thrown across the map, a
        // teleport that repeats every second. Each of those is refused here,
        // by name, at startup — the same contract every other table has, and
        // the reason a content author can compose freely without reading
        // combat.rs.
        if def.effects.is_empty() {
            p.push(format!(
                "{what}: `effects` is empty — an ability that does nothing is a \
                 button that lies about being castable"
            ));
        }
        let mut status_kinds: Vec<&str> = Vec::new();
        for (i, effect) in def.effects.iter().enumerate() {
            let clause = format!("{what}/effects[{i}]");

            // Schedules that exist as schema but have no machinery behind
            // them. Refused rather than ignored: a row that silently did
            // nothing would be a worse lie than a startup panic.
            if !effect.schedule.supported() {
                p.push(format!(
                    "{clause}: schedule `{}` is not yet supported — the damage \
                     pipeline has no hook for it (see EffectSchedule)",
                    effect.schedule.name()
                ));
            }
            if let EffectSchedule::OverTime { interval, ticks } = effect.schedule {
                positive(&mut p, &clause, "schedule interval", interval);
                if ticks == 0 {
                    p.push(format!("{clause}: schedule ticks must be > 0"));
                }
                // A recall repeated every second is not a mechanic, it is a
                // unit that can never leave home; and a militia term that
                // re-arms itself is a permanent militia written the hard way.
                if matches!(
                    effect.atom,
                    EffectAtom::Teleport { .. } | EffectAtom::Militia { .. }
                ) {
                    p.push(format!(
                        "{clause}: `{}` cannot be scheduled OverTime — it sets a \
                         state rather than paying out a quantity",
                        effect.atom.name()
                    ));
                }
            }

            match effect.atom {
                EffectAtom::Damage { amount, .. } => {
                    positive(&mut p, &clause, "damage amount", amount);
                }
                EffectAtom::Heal { amount, targets } => {
                    positive(&mut p, &clause, "heal amount", amount);
                    // The only reading of "heal the enemy" is a typo.
                    if targets == AbilityTargets::Enemies {
                        p.push(format!("{clause}: Heal at Enemies mends the people you are fighting"));
                    }
                }
                EffectAtom::ApplyStatus { status, magnitude, duration, .. } => {
                    positive(&mut p, &clause, "status magnitude", magnitude);
                    positive(&mut p, &clause, "status duration", duration);
                    // Two instances of one kind from one cast: the second only
                    // refreshes or stacks onto the first, so the row means
                    // something other than it says.
                    if status_kinds.contains(&status.name()) {
                        p.push(format!(
                            "{clause}: applies {} twice in one cast",
                            status.name()
                        ));
                    }
                    status_kinds.push(status.name());
                }
                EffectAtom::Militia { duration, targets } => {
                    positive(&mut p, &clause, "militia duration", duration);
                    // Arming the ENEMY's workers for them, or handing a sword
                    // to a Knight who has one: militia is a worker's answer.
                    if targets != AbilityTargets::OwnWorkers {
                        p.push(format!(
                            "{clause}: Militia at {} — militia is what OWN WORKERS do",
                            targets.name()
                        ));
                    }
                }
                EffectAtom::Summon { unit_kind, count, lifetime } => {
                    if count == 0 {
                        p.push(format!("{clause}: summon count must be > 0"));
                    }
                    // A hero is a progression, a revival cost and a record —
                    // not a body. Summoning one would mint a second Champion
                    // with the first one's level and no way to bury it.
                    if is_hero_kind(unit_kind) {
                        p.push(format!(
                            "{clause}: cannot Summon {} — hero kinds carry progression \
                             and a revival contract, so they are trained, never called",
                            kind_name(unit_kind)
                        ));
                    }
                    if let Some(lifetime) = lifetime {
                        positive(&mut p, &clause, "summon lifetime", lifetime);
                    }
                }
                EffectAtom::Teleport { .. } => {
                    // The recall gathers everything around the CASTER and
                    // moves it; there is no way to express "gather around a
                    // point 9 away", so a thrown recall would quietly ignore
                    // its own aim.
                    if def.target.is_targeted() {
                        p.push(format!(
                            "{clause}: Teleport needs `target: Caster` — a recall \
                             gathers around the caster, so a thrown one would ignore \
                             where it was thrown"
                        ));
                    }
                }
            }
        }
        // Geometry. A targeted ability with no reach is a spell that can only
        // ever be cast on the caster's own feet — which is a `Caster` row
        // written the long way round, and far more likely a typo. Catching it
        // here means the aimer and the range check never have to consider a
        // zero or negative range at all.
        if let Some(range) = def.target.range() {
            positive(&mut p, &what, "target range", range);
        }
        ability_names.push(normalize_name(def.name));
    }
    duplicates("abilities.ron", "name", &ability_names, &mut p);

    // --- items ------------------------------------------------------------
    let mut item_names: Vec<String> = Vec::new();
    for (index, def) in t.items.iter().enumerate() {
        let what = format!("items.ron/{}", def.name);
        if def.name.trim().is_empty() {
            p.push(format!("items.ron: item at slot {index} has an empty name"));
        }
        if def.description.trim().is_empty() {
            p.push(format!("{what}: description is empty"));
        }
        if def.cost_gold == 0 {
            p.push(format!("{what}: cost_gold must be > 0"));
        }
        item_names.push(normalize_name(def.name));
    }
    duplicates("items.ron", "name", &item_names, &mut p);

    // --- research ---------------------------------------------------------
    let mut ladder_ids: Vec<String> = Vec::new();
    for row in &t.research_ladders {
        let what = format!("research.ron/{:?}", row.kind);
        if row.id.trim().is_empty() {
            p.push(format!("{what}: id is empty"));
        }
        if row.label.trim().is_empty() {
            p.push(format!("{what}: label is empty"));
        }
        if row.description.trim().is_empty() {
            p.push(format!("{what}: description is empty"));
        }
        ladder_ids.push(normalize_name(&row.id));
    }
    duplicates("research.ron", "id", &ladder_ids, &mut p);

    for level in 1..=RESEARCH_MAX_LEVEL {
        let matching = t.research_steps.iter().filter(|s| s.level == level).count();
        if matching != 1 {
            p.push(format!(
                "research.ron: level {level} has {matching} steps, expected exactly 1 \
                 (steps must cover 1..={RESEARCH_MAX_LEVEL})"
            ));
        }
    }
    for step in &t.research_steps {
        if step.level == 0 || step.level > RESEARCH_MAX_LEVEL {
            p.push(format!(
                "research.ron: step level {} is outside 1..={RESEARCH_MAX_LEVEL}",
                step.level
            ));
        }
        positive(
            &mut p,
            &format!("research.ron/level {}", step.level),
            "research_time",
            step.research_time,
        );
    }
    // The forge and the building table have to agree about who researches
    // what, or the command card draws buttons the intent compiler refuses.
    let forge = &t.buildings[t.research_building as usize];
    for &kind in &ALL_RESEARCH_KINDS {
        if !forge.researches.contains(&kind) {
            p.push(format!(
                "research.ron names {:?} as the research building, but buildings.ron does not \
                 list {kind:?} in its `researches`",
                t.research_building
            ));
        }
    }

    // --- cross-table ------------------------------------------------------
    // Every unit must be trainable somewhere, or it is content nobody can
    // reach; every `researches` entry must be on the forge.
    for &kind in &ALL_UNIT_KINDS {
        if !t.buildings.iter().any(|b| b.trains.contains(&kind)) {
            p.push(format!("buildings.ron: nothing trains {kind:?}"));
        }
    }
    for row in &t.buildings {
        for r in &row.researches {
            if row.kind != t.research_building {
                p.push(format!(
                    "buildings.ron/{:?}: researches {r:?}, but research.ron names {:?} as the \
                     research building",
                    row.kind, t.research_building
                ));
            }
        }
    }
    p
}

fn duplicates(table: &str, field: &str, values: &[String], p: &mut Vec<String>) {
    for (i, value) in values.iter().enumerate() {
        if values[..i].contains(value) {
            p.push(format!("{table}: duplicate {field} {value:?}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Accessors — the only way shared.rs touches the tables
// ---------------------------------------------------------------------------

pub fn unit_row(kind: UnitKind) -> &'static UnitRow {
    tables().units.get(kind as usize).unwrap_or_else(|| {
        panic!("{kind:?} has no row in units.ron — add it to ALL_UNIT_KINDS and the data file")
    })
}

pub fn building_row(kind: BuildingKind) -> &'static BuildingRow {
    tables().buildings.get(kind as usize).unwrap_or_else(|| {
        panic!(
            "{kind:?} has no row in buildings.ron — add it to ALL_BUILDING_KINDS and the data file"
        )
    })
}

pub fn item_row(id: ItemId) -> &'static ItemDef {
    tables()
        .items
        .get(id as usize)
        .unwrap_or_else(|| panic!("{id:?} has no row in items.ron"))
}

pub fn unit_abilities(kind: UnitKind) -> &'static [AbilityDef] {
    tables()
        .unit_abilities
        .get(kind as usize)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub fn unit_autocast(kind: UnitKind) -> Option<(usize, u32)> {
    tables().unit_autocast.get(kind as usize).copied().flatten()
}

/// Abilities of a building kind, resolved through its LADDER ROOT so an
/// upgraded hall keeps everything the original could cast.
pub fn building_abilities(kind: BuildingKind) -> &'static [AbilityDef] {
    tables()
        .building_abilities
        .get(upgrade_root(kind) as usize)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub fn research_building() -> BuildingKind {
    tables().research_building
}

pub fn research_ladder(kind: ResearchKind) -> (&'static str, &'static str, &'static str) {
    let row = tables()
        .research_ladders
        .get(kind as usize)
        .unwrap_or_else(|| panic!("{kind:?} has no ladder in research.ron"));
    (row.id.as_str(), row.label.as_str(), row.description.as_str())
}

pub fn research_step(level: u32) -> Option<ResearchStep> {
    tables()
        .research_steps
        .iter()
        .find(|s| s.level == level)
        .copied()
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::{StatusKind, TeleportDestination};

    /// The built-in tables load and pass every check. This is the test the
    /// startup validator is a copy of: if it fails, the game will not boot.
    #[test]
    fn the_shipped_data_files_load_and_validate() {
        ensure_loaded();
        assert_eq!(tables().units.len(), ALL_UNIT_KINDS.len());
        assert_eq!(tables().buildings.len(), ALL_BUILDING_KINDS.len());
        assert_eq!(tables().items.len(), ALL_ITEMS.len());
        assert!(check_values(tables(), &[]).is_empty());
    }

    /// Every kind, item and ladder resolves — the "no silent hole" property
    /// that the old exhaustive `match` used to give us for free.
    #[test]
    fn every_enum_variant_has_a_row() {
        for kind in ALL_UNIT_KINDS {
            assert_eq!(unit_row(kind).kind, kind, "units.ron slot order");
        }
        for kind in ALL_BUILDING_KINDS {
            assert_eq!(building_row(kind).kind, kind, "buildings.ron slot order");
        }
        for id in ALL_ITEMS {
            assert!(!item_row(id).name.is_empty());
        }
        for kind in ALL_RESEARCH_KINDS {
            assert!(!research_ladder(kind).0.is_empty());
        }
        for level in 1..=RESEARCH_MAX_LEVEL {
            assert!(research_step(level).is_some(), "research step {level}");
        }
    }

    /// The loader reports a MISSING ROW by name rather than panicking on an
    /// index. This is the failure mode the whole bead exists to make loud.
    #[test]
    fn a_missing_row_is_reported_by_name() {
        let mut problems = Vec::new();
        let rows: Vec<UnitRow> = ron::from_str(
            r#"[(kind: Worker, name: "Worker", description: "d", requires: [], stats: (
                cost_gold: 1, cost_lumber: 0, supply: 1, hp: 1.0, damage: 1.0, range: 1.0,
                attack_cooldown: 1.0, speed: 1.0, train_time: 1.0, projectile: false,
                vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
                flying: false, can_hit_air: false, can_hit_ground: true, vision: 1.0))]"#,
        )
        .expect("fixture parses");
        slot_by_kind(
            "units.ron",
            rows,
            &ALL_UNIT_KINDS,
            |row| row.kind,
            |kind| kind as usize,
            |kind| format!("{kind:?}"),
            &mut problems,
        );
        assert_eq!(problems.len(), ALL_UNIT_KINDS.len() - 1);
        assert!(
            problems.iter().any(|m| m.contains("no row for Footman")),
            "expected the missing kind to be named: {problems:?}"
        );
    }

    /// A duplicate row is a merge that interleaved two edits to the same kind
    /// — the exact accident this bead is about. It must not be silently
    /// last-one-wins.
    #[test]
    fn a_duplicate_row_is_reported() {
        let mut problems = Vec::new();
        let one = r#"(kind: Worker, name: "Worker", description: "d", requires: [], stats: (
            cost_gold: 1, cost_lumber: 0, supply: 1, hp: 1.0, damage: 1.0, range: 1.0,
            attack_cooldown: 1.0, speed: 1.0, train_time: 1.0, projectile: false,
            vs_building_mult: 1.0, vs_siege_mult: 1.0, vs_cavalry_mult: 1.0,
            flying: false, can_hit_air: false, can_hit_ground: true, vision: 1.0))"#;
        let rows: Vec<UnitRow> = ron::from_str(&format!("[{one},{one}]")).expect("fixture parses");
        slot_by_kind(
            "units.ron",
            rows,
            &ALL_UNIT_KINDS,
            |row| row.kind,
            |kind| kind as usize,
            |kind| format!("{kind:?}"),
            &mut problems,
        );
        assert!(
            problems.iter().any(|m| m.contains("duplicate row for Worker")),
            "{problems:?}"
        );
    }

    /// Nonsense numbers are refused with the field named. Vision is the one
    /// worth spelling out: a 0 there is invisible in play (the unit simply
    /// never reveals anything) and would survive every other test in the repo.
    #[test]
    fn zero_vision_and_zero_cooldown_are_refused() {
        let mut broken = load_for_test();
        broken.units[UnitKind::Footman as usize].stats.vision = 0.0;
        broken.units[UnitKind::Archer as usize].stats.attack_cooldown = 0.0;
        broken.buildings[BuildingKind::Farm as usize].stats.hp = -1.0;
        let problems = check_values(&broken, &[]);
        assert!(problems.iter().any(|m| m.contains("Footman") && m.contains("vision")), "{problems:?}");
        assert!(
            problems.iter().any(|m| m.contains("Archer") && m.contains("attack_cooldown")),
            "{problems:?}"
        );
        assert!(problems.iter().any(|m| m.contains("Farm") && m.contains("hp")), "{problems:?}");
    }

    /// A unit nothing trains is unreachable content; the loader says so.
    #[test]
    fn an_untrainable_unit_is_refused() {
        let mut broken = load_for_test();
        for row in &mut broken.buildings {
            row.trains.retain(|k| *k != UnitKind::Catapult);
        }
        let problems = check_values(&broken, &[]);
        assert!(
            problems.iter().any(|m| m.contains("nothing trains Catapult")),
            "{problems:?}"
        );
    }

    /// An ability list that names a row which is not in the file must not
    /// silently produce a shorter list — slot indices are handles.
    #[test]
    fn an_unknown_ability_name_is_refused() {
        let file: AbilityFile = ron::from_str(&source("abilities.ron", ABILITIES_RON))
            .expect("shipped abilities.ron parses");
        assert!(file
            .unit_casters
            .iter()
            .flat_map(|r| r.abilities.iter().chain(r.probe_abilities.iter()))
            .chain(file.building_casters.iter().flat_map(|r| r.abilities.iter()))
            .all(|name| file.defs.iter().any(|d| &d.name == name)));
    }

    /// **Geometry survives the round trip, and the default is silence.**
    /// The whole reason `target` is a defaulted field is that a caster-centred
    /// row should read exactly as it did before geometry existed — so the
    /// shipped file must have precisely one row that mentions it.
    #[test]
    fn ability_geometry_loads_from_ron_and_defaults_to_caster() {
        let file: AbilityFile = ron::from_str(&source("abilities.ron", ABILITIES_RON))
            .expect("shipped abilities.ron parses");
        let targeted: Vec<(&str, AbilityTarget)> = file
            .defs
            .iter()
            .filter(|d| d.target.is_targeted())
            .map(|d| (d.name.as_str(), d.target))
            .collect();
        assert_eq!(
            targeted,
            vec![
                ("Slow", AbilityTarget::Point { range: 9.0 }),
                // The v3 demo row, dev-gated and thrown for the same reason
                // Slow is: a nuke you cannot aim is a nuke you stand inside.
                (concat!("Frost", "Nova"), AbilityTarget::Point { range: 9.0 }),
            ],
            "only a genuinely THROWN row should spell out a geometry; every \
             other row must be riding the `Caster` default"
        );
        // And the default really is Caster rather than merely absent.
        let slam = file.defs.iter().find(|d| d.name == "Slam").unwrap();
        assert_eq!(slam.target, AbilityTarget::Caster);
    }

    /// A targeted ability with no reach can only ever be cast on the caster's
    /// own feet — a `Caster` row written the long way round, and far more
    /// likely a typo. The loader refuses it, so the aimer never has to think
    /// about a zero range.
    #[test]
    fn a_targeted_ability_with_no_reach_is_refused() {
        let tables = load_for_test();
        let mut broken = unit_abilities(UnitKind::Sorcerer)[0];
        broken.target = AbilityTarget::Point { range: 0.0 };
        let problems = check_values(&tables, &[broken]);
        assert!(
            problems.iter().any(|m| m.contains("target range")),
            "{problems:?}"
        );
    }

    // -----------------------------------------------------------------------
    // v3: the effect-atom vocabulary
    // -----------------------------------------------------------------------

    /// Every shipped ability's def, by name, straight out of the file (so a
    /// probe-gated row is visible here even when the probe flag is not set).
    fn shipped(name: &str) -> AbilityDef {
        let file: AbilityFile = ron::from_str(&source("abilities.ron", ABILITIES_RON))
            .expect("shipped abilities.ron parses");
        let row = file
            .defs
            .into_iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("abilities.ron has no row named {name}"));
        AbilityDef::from(row)
    }

    /// **The equivalence proof, row by row.** Every ability that shipped under
    /// v2's closed `AbilityEffect` enum, re-expressed as atoms, must mean
    /// EXACTLY what it meant before — same numbers, same targets, same
    /// durations, and the same `power`/`duration` pair on the wire.
    ///
    /// This is the test that makes the refactor safe to believe: if the
    /// vocabulary had quietly changed a magnitude, a duration or a side, one
    /// of these lines would say so by name.
    #[test]
    fn every_shipped_ability_means_exactly_what_it_meant_under_v2() {
        // (name, atoms, v2 `power`, v2 `duration`)
        let expected: Vec<(&str, Vec<EffectAtom>, f32, f32)> = vec![
            (
                "Slam",
                vec![EffectAtom::Damage { amount: 45.0, targets: AbilityTargets::Enemies }],
                45.0,
                0.0,
            ),
            (
                "Heal",
                vec![EffectAtom::Heal { amount: 60.0, targets: AbilityTargets::Allies }],
                60.0,
                0.0,
            ),
            (
                // Militia's seconds lived in `power` under v2 and still do —
                // the wire format is reproduced, oddity included.
                "CallToArms",
                vec![EffectAtom::Militia {
                    duration: 40.0,
                    targets: AbilityTargets::OwnWorkers,
                }],
                40.0,
                0.0,
            ),
            (
                "Warcry",
                vec![EffectAtom::ApplyStatus {
                    status: StatusKind::DamageBuff,
                    magnitude: 0.30,
                    duration: 8.0,
                    targets: AbilityTargets::Allies,
                }],
                0.30,
                8.0,
            ),
            (
                // **`also` retired into a second atom.** Same two statuses,
                // same shared duration, no bespoke field.
                "Sanctuary",
                vec![
                    EffectAtom::ApplyStatus {
                        status: StatusKind::HealOverTime,
                        magnitude: 15.0,
                        duration: 6.0,
                        targets: AbilityTargets::Allies,
                    },
                    EffectAtom::ApplyStatus {
                        status: StatusKind::ArmorBuff,
                        magnitude: 0.25,
                        duration: 6.0,
                        targets: AbilityTargets::Allies,
                    },
                ],
                15.0,
                6.0,
            ),
            (
                "Slow",
                vec![EffectAtom::ApplyStatus {
                    status: StatusKind::Slow,
                    magnitude: 0.4,
                    duration: 5.0,
                    targets: AbilityTargets::Enemies,
                }],
                0.4,
                5.0,
            ),
            (
                "ProbeChill",
                vec![EffectAtom::ApplyStatus {
                    status: StatusKind::Slow,
                    magnitude: 0.4,
                    duration: 6.0,
                    targets: AbilityTargets::Enemies,
                }],
                0.4,
                6.0,
            ),
        ];
        for (name, atoms, power, duration) in expected {
            let def = shipped(name);
            let got: Vec<EffectAtom> = def.effects.iter().map(|e| e.atom).collect();
            assert_eq!(got, atoms, "{name}: composition changed");
            // Every shipped row is instant; nothing acquired a schedule by
            // accident on the way through the rewrite.
            assert!(
                def.effects.iter().all(|e| e.schedule == EffectSchedule::Instant),
                "{name}: shipped rows are all instant"
            );
            assert_eq!(def.power(), power, "{name}: v2 `power` on the wire");
            assert_eq!(def.duration(), duration, "{name}: v2 `duration` on the wire");
        }
        // And the two back-compat catalog fields Sanctuary is the only user of.
        let sanctuary = shipped("Sanctuary");
        assert_eq!(sanctuary.status(), Some(StatusKind::HealOverTime));
        assert_eq!(sanctuary.extra_status(), Some((StatusKind::ArmorBuff, 0.25)));
        assert_eq!(shipped("Slow").extra_status(), None);
    }

    /// **Every atom and every schedule survives a trip through RON**, spelled
    /// the way a content author would spell it — including the defaults that
    /// let a quiet row stay quiet (`targets` on Damage/Heal/Militia,
    /// `schedule` everywhere, `lifetime` on a permanent summon).
    #[test]
    fn effect_atoms_round_trip_through_ron() {
        let cases: Vec<(&str, Effect)> = vec![
            (
                "(atom: Damage(amount: 45.0))",
                Effect {
                    atom: EffectAtom::Damage { amount: 45.0, targets: AbilityTargets::Enemies },
                    schedule: EffectSchedule::Instant,
                },
            ),
            (
                "(atom: Heal(amount: 12.0, targets: Allies), schedule: OverTime(interval: 1.0, ticks: 5))",
                Effect {
                    atom: EffectAtom::Heal { amount: 12.0, targets: AbilityTargets::Allies },
                    schedule: EffectSchedule::OverTime { interval: 1.0, ticks: 5 },
                },
            ),
            (
                "(atom: ApplyStatus(status: Slow, magnitude: 0.4, duration: 5.0, targets: Enemies))",
                Effect {
                    atom: EffectAtom::ApplyStatus {
                        status: StatusKind::Slow,
                        magnitude: 0.4,
                        duration: 5.0,
                        targets: AbilityTargets::Enemies,
                    },
                    schedule: EffectSchedule::Instant,
                },
            ),
            (
                "(atom: Militia(duration: 40.0))",
                Effect {
                    atom: EffectAtom::Militia {
                        duration: 40.0,
                        targets: AbilityTargets::OwnWorkers,
                    },
                    schedule: EffectSchedule::Instant,
                },
            ),
            (
                "(atom: Summon(unit_kind: Footman, count: 2, lifetime: Some(30.0)))",
                Effect {
                    atom: EffectAtom::Summon {
                        unit_kind: UnitKind::Footman,
                        count: 2,
                        lifetime: Some(30.0),
                    },
                    schedule: EffectSchedule::Instant,
                },
            ),
            (
                "(atom: Summon(unit_kind: Spearman, count: 1))",
                Effect {
                    atom: EffectAtom::Summon {
                        unit_kind: UnitKind::Spearman,
                        count: 1,
                        lifetime: None,
                    },
                    schedule: EffectSchedule::Instant,
                },
            ),
            (
                "(atom: Teleport(destination: NearestHall, army_only: true))",
                Effect {
                    atom: EffectAtom::Teleport {
                        destination: TeleportDestination::NearestHall,
                        army_only: true,
                    },
                    schedule: EffectSchedule::Instant,
                },
            ),
        ];
        for (text, want) in cases {
            let got: Effect = ron::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(got, want, "{text}");
        }
        // A misspelled field is a startup error, not a silent default: the
        // atom types deny unknown fields exactly as the row types do.
        assert!(ron::from_str::<Effect>("(atom: Damage(ammount: 45.0))").is_err());
        assert!(ron::from_str::<Effect>("(atom: Damage(amount: 45.0), schedual: Instant)").is_err());
    }

    /// A row whose ability list is empty draws a button that does nothing.
    #[test]
    fn an_ability_with_no_effects_is_refused() {
        let tables = load_for_test();
        let mut broken = shipped("Slam");
        broken.effects = &[];
        let problems = check_values(&tables, &[broken]);
        assert!(problems.iter().any(|m| m.contains("`effects` is empty")), "{problems:?}");
    }

    /// **The nonsense table.** Each of these is a combination the grammar can
    /// express and the game cannot mean; the loader names the offender and
    /// refuses to start rather than shipping a spell that silently does
    /// nothing (or something surprising) in a live match.
    #[test]
    fn nonsense_atom_combinations_are_refused() {
        let tables = load_for_test();
        let instant = |atom| Effect { atom, schedule: EffectSchedule::Instant };
        // (what it is, the effects, the phrase the report must contain)
        let cases: Vec<(&str, Vec<Effect>, &str, AbilityTarget)> = vec![
            (
                "a summoned hero",
                vec![instant(EffectAtom::Summon {
                    unit_kind: UnitKind::Hero,
                    count: 1,
                    lifetime: None,
                })],
                "cannot Summon",
                AbilityTarget::Caster,
            ),
            (
                "a summon of nobody",
                vec![instant(EffectAtom::Summon {
                    unit_kind: UnitKind::Footman,
                    count: 0,
                    lifetime: None,
                })],
                "summon count must be > 0",
                AbilityTarget::Caster,
            ),
            (
                "a recall that repeats",
                vec![Effect {
                    atom: EffectAtom::Teleport {
                        destination: TeleportDestination::NearestHall,
                        army_only: false,
                    },
                    schedule: EffectSchedule::OverTime { interval: 1.0, ticks: 5 },
                }],
                "cannot be scheduled OverTime",
                AbilityTarget::Caster,
            ),
            (
                "a thrown recall",
                vec![instant(EffectAtom::Teleport {
                    destination: TeleportDestination::NearestHall,
                    army_only: false,
                })],
                "Teleport needs `target: Caster`",
                AbilityTarget::Point { range: 9.0 },
            ),
            (
                "militia raised from the enemy's workers",
                vec![instant(EffectAtom::Militia {
                    duration: 40.0,
                    targets: AbilityTargets::Enemies,
                })],
                "militia is what OWN WORKERS do",
                AbilityTarget::Caster,
            ),
            (
                "a heal aimed at the people you are fighting",
                vec![instant(EffectAtom::Heal {
                    amount: 60.0,
                    targets: AbilityTargets::Enemies,
                })],
                "mends the people you are fighting",
                AbilityTarget::Caster,
            ),
            (
                "the same status applied twice by one cast",
                vec![
                    instant(EffectAtom::ApplyStatus {
                        status: StatusKind::Slow,
                        magnitude: 0.4,
                        duration: 5.0,
                        targets: AbilityTargets::Enemies,
                    }),
                    instant(EffectAtom::ApplyStatus {
                        status: StatusKind::Slow,
                        magnitude: 0.2,
                        duration: 5.0,
                        targets: AbilityTargets::Enemies,
                    }),
                ],
                "applies Slow twice",
                AbilityTarget::Caster,
            ),
            (
                "a status with no magnitude",
                vec![instant(EffectAtom::ApplyStatus {
                    status: StatusKind::Slow,
                    magnitude: 0.0,
                    duration: 5.0,
                    targets: AbilityTargets::Enemies,
                })],
                "status magnitude",
                AbilityTarget::Caster,
            ),
            (
                "a status that ends the instant it lands",
                vec![instant(EffectAtom::ApplyStatus {
                    status: StatusKind::Slow,
                    magnitude: 0.4,
                    duration: 0.0,
                    targets: AbilityTargets::Enemies,
                })],
                "status duration",
                AbilityTarget::Caster,
            ),
            (
                "damage that deals nothing",
                vec![instant(EffectAtom::Damage {
                    amount: 0.0,
                    targets: AbilityTargets::Enemies,
                })],
                "damage amount",
                AbilityTarget::Caster,
            ),
            (
                "an over-time effect with no ticks",
                vec![Effect {
                    atom: EffectAtom::Damage { amount: 5.0, targets: AbilityTargets::Enemies },
                    schedule: EffectSchedule::OverTime { interval: 1.0, ticks: 0 },
                }],
                "schedule ticks must be > 0",
                AbilityTarget::Caster,
            ),
            (
                "an over-time effect with no interval",
                vec![Effect {
                    atom: EffectAtom::Damage { amount: 5.0, targets: AbilityTargets::Enemies },
                    schedule: EffectSchedule::OverTime { interval: 0.0, ticks: 3 },
                }],
                "schedule interval",
                AbilityTarget::Caster,
            ),
        ];
        for (what, effects, phrase, target) in cases {
            let mut broken = shipped("Slam");
            broken.effects = Box::leak(effects.into_boxed_slice());
            broken.target = target;
            let problems = check_values(&tables, &[broken]);
            assert!(
                problems.iter().any(|m| m.contains(phrase)),
                "{what}: expected a complaint containing {phrase:?}, got {problems:?}"
            );
        }
    }

    /// **The two schedules that are schema and nothing else.** They parse (so
    /// the bead that wires them up starts from an agreed spelling) and the
    /// loader refuses them by name, because a row that silently did nothing
    /// would be a worse lie than a startup panic.
    #[test]
    fn unimplemented_schedules_are_refused_by_name() {
        let tables = load_for_test();
        for schedule in [EffectSchedule::OnHit { attacks: 3 }, EffectSchedule::OnDeath] {
            let mut broken = shipped("Slam");
            broken.effects = Box::leak(
                vec![Effect {
                    atom: EffectAtom::Damage { amount: 20.0, targets: AbilityTargets::Enemies },
                    schedule,
                }]
                .into_boxed_slice(),
            );
            let problems = check_values(&tables, &[broken]);
            assert!(
                problems
                    .iter()
                    .any(|m| m.contains("not yet supported") && m.contains(schedule.name())),
                "{schedule:?}: {problems:?}"
            );
        }
    }

    /// **The zero-Rust demonstration.** The demo row is a mechanic this
    /// engine never had — a thrown nuke that damages AND chills — and it
    /// exists only as a row. This test asserts both halves of that claim: the
    /// row parses into the two atoms it promises, and its NAME appears in no
    /// Rust source file in the crate.
    ///
    /// The needle is assembled from two halves so that this test's own source
    /// cannot satisfy the search it is making.
    #[test]
    fn the_demo_ability_is_defined_purely_in_ron() {
        let name = concat!("Frost", "Nova");
        let def = shipped(name);
        assert_eq!(
            def.effects.iter().map(|e| e.atom).collect::<Vec<_>>(),
            vec![
                EffectAtom::Damage { amount: 60.0, targets: AbilityTargets::Enemies },
                EffectAtom::ApplyStatus {
                    status: StatusKind::Slow,
                    magnitude: 0.35,
                    duration: 4.0,
                    targets: AbilityTargets::Enemies,
                },
            ],
        );
        assert_eq!(def.target, AbilityTarget::Point { range: 9.0 });
        // Two clauses, one cast: the aim (and so the ring, and so doctrine's
        // count) belongs to the damage.
        assert!(matches!(def.aim(), EffectAtom::Damage { .. }));

        for (file, text) in [
            ("shared.rs", include_str!("shared.rs")),
            ("combat.rs", include_str!("combat.rs")),
            ("doctrine.rs", include_str!("doctrine.rs")),
            ("data.rs", include_str!("data.rs")),
            ("ui.rs", include_str!("ui.rs")),
            ("ai.rs", include_str!("ai.rs")),
            ("bridge.rs", include_str!("bridge.rs")),
            ("units.rs", include_str!("units.rs")),
        ] {
            assert!(
                !text.contains(name),
                "src/{file} mentions {name} — the demonstration is that NO Rust \
                 knows this ability's name"
            );
        }
    }

    /// A fresh parse of the shipped files, for tests that need to break one.
    fn load_for_test() -> Tables {
        let mut problems = Vec::new();
        let units = slot_by_kind(
            "units.ron",
            parse::<Vec<UnitRow>>("units.ron", UNITS_RON),
            &ALL_UNIT_KINDS,
            |row| row.kind,
            |kind| kind as usize,
            |kind| format!("{kind:?}"),
            &mut problems,
        );
        let buildings = slot_by_kind(
            "buildings.ron",
            parse::<Vec<BuildingRow>>("buildings.ron", BUILDINGS_RON),
            &ALL_BUILDING_KINDS,
            |row| row.kind,
            |kind| kind as usize,
            |kind| format!("{kind:?}"),
            &mut problems,
        );
        let research_file: ResearchFile = parse("research.ron", RESEARCH_RON);
        let research_ladders = slot_by_kind(
            "research.ron",
            research_file.ladders,
            &ALL_RESEARCH_KINDS,
            |row| row.kind,
            |kind| kind as usize,
            |kind| format!("{kind:?}"),
            &mut problems,
        );
        let items = slot_by_kind(
            "items.ron",
            parse::<Vec<ItemRow>>("items.ron", ITEMS_RON),
            &ALL_ITEMS,
            |row| row.id,
            |id| id as usize,
            |id| format!("{id:?}"),
            &mut problems,
        )
        .into_iter()
        .map(|row| ItemDef::from(row.def))
        .collect();
        assert!(problems.is_empty(), "{problems:?}");
        Tables {
            units,
            buildings,
            unit_abilities: Vec::new(),
            unit_autocast: Vec::new(),
            building_abilities: Vec::new(),
            items,
            research_building: research_file.building,
            research_ladders,
            research_steps: research_file.steps,
        }
    }
}
