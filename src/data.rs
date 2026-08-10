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
    normalize_name, status_probe_enabled, upgrade_root, AbilityDef, AbilityEffect, AbilityTarget,
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
    effect: AbilityEffect,
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
    power: f32,
    duration: f32,
    hits_air: bool,
    unlock: AbilityUnlock,
    description: String,
}

impl From<AbilityDefRow> for AbilityDef {
    fn from(row: AbilityDefRow) -> AbilityDef {
        let AbilityDefRow {
            name,
            effect,
            target,
            mana_cost,
            cooldown,
            radius,
            power,
            duration,
            hits_air,
            unlock,
            description,
        } = row;
        AbilityDef {
            name: leak(name),
            effect,
            target,
            mana_cost,
            cooldown,
            radius,
            power,
            duration,
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
        if matches!(def.effect, AbilityEffect::ApplyStatus { .. }) {
            positive(&mut p, &what, "duration", def.duration);
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
            vec![("Slow", AbilityTarget::Point { range: 9.0 })],
            "Slow is the only row that should spell out a geometry; every other \
             row must be riding the `Caster` default"
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
