//! bridge.rs — the "live bridge": a file-based control channel that lets one or
//! two external agents (Claude in a terminal) play a faction, either against the
//! human at the keyboard or against each other.
//!
//! Activation is opt-in through `WC3_BRIDGE` (case-insensitive). Each accepted
//! value opens one *seat* per faction it names:
//!   * `1` / `red` / `claude` — the Claude (red) faction, in `bridge/red/`,
//!   * `blue` / `human` — the Human (blue) faction, in `bridge/blue/`,
//!   * `both` / `2` — both seats at once, so two commanders can fight.
//!
//! NOTE (breaking change): a seat's files live in `bridge/<seat>/`, not directly
//! in `bridge/` as they did when there was only one seat. `WC3_BRIDGE=1` now
//! writes `bridge/red/state.json`, not `bridge/state.json`.
//!
//! For every active seat the bridge
//!   * creates `bridge/<seat>/` next to the working directory,
//!   * writes `bridge/<seat>/catalog.json` once at startup: the whole content
//!     catalog (units, buildings, abilities — costs, stats, tech requirements,
//!     what trains what) straight from `shared::game_catalog()`, so a commander
//!     can learn the game's affordances without hard-coding them,
//!   * switches that faction's `AiControlled` flag off (the external commander
//!     replaces the scripted macro AI; `{"type":"autopilot","on":true}` hands it
//!     back — always to the *seat's* team),
//!   * writes a world snapshot to `bridge/<seat>/state.json` once a second
//!     (atomically, via `state.tmp` + rename, so a reader never sees a half
//!     file),
//!   * polls `bridge/<seat>/commands.json` four times a second and applies the
//!     newest batch whose `seq` is greater than the last applied one.
//!
//! Seats are fully independent: separate seq counters, error buffers, event
//! memos, timers and file-stat caches. A malformed batch on one seat cannot
//! disturb the other, and both may command in the same tick.
//!
//! When the env var is absent every system early-returns before touching the
//! filesystem, so a normal `cargo run` never creates `bridge/`. (The intent
//! log is separate and has its own lazy-open rule — see intent.rs.)
//!
//! The bridge does not act on the world at all any more. It deserializes each
//! command into a `shared::Intent` and submits it; intent.rs validates and
//! applies it — the very same compiler the human's mouse gestures go through.
//! That is stronger than the old promise that the bridge "acts only through the
//! primitives the UI uses", because it is no longer a promise: there is one
//! implementation, so the two seats cannot drift apart. See docs/INTENT.md.
//!
//! The wire format did not change, because the wire format *is* the schema:
//! `Intent`'s serde shape is the historical command shape, field for field.
//!
//! Enemy units and buildings are reported in the snapshot (full vision parity
//! with the scripted AI) but enemy gold/lumber never is — and a seat only ever
//! sees its *own* squads and policies, never the opponent's command structure.
//!
//! Everything in a snapshot is relative to the seat that receives it: `me` is
//! the seat's economy, `my_team` names it, `trees_near` are the trees nearest
//! that seat's base, and the event stream reports that seat's losses, hero and
//! base threats.
//!
//! Beyond one-shot orders the commander can also install *doctrine*: standing
//! policies (`priority`, `retreat`, `leash`, `autocast`) and squads
//! (`squad`, `posture`). These are intents like any other; the compiler writes
//! the components and the `SquadOrders` resource, and doctrine.rs and combat.rs
//! act on them. Setting a policy is therefore as cheap as any other command,
//! and a commander that polls slowly still gets sensible behaviour between
//! polls.
//!
//! Even a commander that says nothing gets an army: doctrine.rs enrols every
//! unassigned army unit (workers excepted) into squad 0 and seeds it a `Defend`
//! at your base, so squad 0 is always present in `squads` — overwrite its
//! posture whenever you like, including with the `forage` posture, which sends
//! the squad attack-moving from bounty cache to bounty cache and holds it at
//! the given muster point while the map has none.
//!
//! `template` goes one step further and puts a standing doctrine on a
//! *production building*: every unit it trains from then on is born with that
//! squad, retreat, priority and autocast already applied (units.rs does the
//! stamping), so reinforcements never arrive doctrine-less between polls. The
//! snapshot reports only `buildings[].template: true` for own buildings that
//! carry one — the commander wrote the details, it knows them.
//!
//! The catalog is static, so tech *availability* rides along with every
//! snapshot instead: a top-level `unlocked` map answers "may I build/train this
//! right now?" for every catalog entry, computed from the seat's own completed
//! buildings. The same check gates the `build` and `train` commands, so a
//! commander that respects `unlocked` never has an order bounced by economy.rs.
//!
//! Abilities and items are described by the catalog (`abilities`, `items`) and
//! driven by three commands: `cast` takes any caster — a hero of either class
//! or one of our own finished ability buildings (the TownHall's Call to Arms)
//! — while `buy` (at an own Shop) and `use_item` (slot 0 or 1) need no unit id
//! at all, because a team fields exactly one hero and only heroes carry an
//! inventory. The snapshot answers back with `units[].items`,
//! `units[].militia` and `buildings[].ability_cd`.
//!
//! Bounty caches (bounty.rs) ride along as a top-level `bounties` array —
//! `pos`, `gold` and `expires_in` — identical for both seats, because treasure
//! glowing on the ground is public information. The event feed reports them
//! *unattributed*: `"bounty spawned: 300g @(x,z)"` when one appears, and
//! `"bounty gone @(x,z)"` when one disappears before its deadline, which means
//! somebody claimed it. The bridge deliberately does not say who: a claim
//! leaves no trace in any snapshot, and own gold moves every second from the
//! harvest, so guessing from an income jump would be worse than silence. A
//! commander with a unit on the spot knows the gold was its own; one without
//! knows it lost the race. Caches that simply time out are not reported —
//! `expires_in` already counted them down.
//!
//! The snapshot reports doctrine back (`units[].squad` / `units[].policies`,
//! plus a top-level `squads` array) and carries an `events` ring buffer: a
//! game-time-stamped log of losses, damage, hero milestones, base threats and
//! squad wipes. The bridge does not build that log — `shared::GameEvents` does,
//! once per team per tick, and ui.rs renders the identical buffer as HUD
//! notifications for the player at the keyboard. That is deliberate: an event
//! feed only one side can read is an advantage, not a channel. The bridge is
//! one renderer of a shared artifact, the HUD is the other, and the wire format
//! here (`[[game_time, message], ...]`) is unchanged by the move.
//!
//! Every failure mode — missing file, malformed JSON, dead entity, unaffordable
//! order — turns into an error string carried in the next snapshot's `errors`
//! array instead of a panic.

use crate::intent::{set_autopilot, IntentApply};
use crate::shared::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Tuning knobs
// ---------------------------------------------------------------------------

/// Set to `1`/`red`/`claude`, `blue`/`human`, or `both`/`2` to open seats.
const BRIDGE_ENV: &str = "WC3_BRIDGE";

/// Root of every seat's directory; each seat gets its own subdirectory.
const BRIDGE_DIR: &str = "bridge";
const STATE_NAME: &str = "state.json";
/// Snapshots are written here first and renamed over `STATE_NAME`.
const STATE_TMP_NAME: &str = "state.tmp";
const COMMANDS_NAME: &str = "commands.json";
/// Written once per session at startup; identical for every seat.
const CATALOG_NAME: &str = "catalog.json";
const CATALOG_TMP_NAME: &str = "catalog.tmp";

/// Wall-clock seconds between snapshots (independent of `WC3_SPEED`).
const SNAPSHOT_INTERVAL: f32 = 1.0;
/// Wall-clock seconds between `commands.json` polls.
const POLL_INTERVAL: f32 = 0.25;

/// How many trees to include in the snapshot (there are hundreds).
const TREES_NEAR: usize = 12;


// ---------------------------------------------------------------------------
// Plugin & state
// ---------------------------------------------------------------------------

pub struct BridgePlugin;

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Bridge>()
            .add_systems(Startup, bridge_startup)
            .add_systems(
                Update,
                // Poll, compile, snapshot — in that order, so a batch read this
                // frame is applied this frame and its validation errors ride
                // out in the snapshot written the same frame. The middle step
                // belongs to intent.rs now; the bridge only brackets it.
                (
                    poll_commands.before(IntentApply),
                    write_snapshot.after(IntentApply),
                )
                    .run_if(bridge_enabled),
            );
    }
}

/// One external commander's channel: its faction, its directory, and all of the
/// protocol state that must never be shared with another seat.
struct Seat {
    /// The faction this seat commands. Every "own"/"me"/"enemy" decision in
    /// this file is taken against it.
    team: Team,
    /// `bridge/red` or `bridge/blue`.
    dir: PathBuf,
    /// `<dir>/state.json`, `<dir>/state.tmp`, `<dir>/commands.json`.
    state_file: PathBuf,
    state_tmp: PathBuf,
    commands_file: PathBuf,
    snapshot_timer: Timer,
    poll_timer: Timer,
    /// Highest `seq` applied so far; batches at or below it are ignored.
    last_seq: u64,
    /// File- and batch-level errors from the most recent poll (unreadable
    /// file, malformed JSON, commands after game over). Per-command errors
    /// live in `shared::IntentErrors`, written by the compiler; the snapshot
    /// concatenates the two.
    errors: Vec<String>,
    /// Write a snapshot on the next tick regardless of the timer.
    force_snapshot: bool,
    /// (mtime, len) of this seat's `commands.json` when it was last read, so an
    /// unchanged file is not re-parsed four times a second.
    last_stat: Option<(std::time::SystemTime, u64)>,
}

impl Seat {
    fn new(team: Team) -> Self {
        let dir = PathBuf::from(BRIDGE_DIR).join(seat_name(team));
        Seat {
            team,
            state_file: dir.join(STATE_NAME),
            state_tmp: dir.join(STATE_TMP_NAME),
            commands_file: dir.join(COMMANDS_NAME),
            dir,
            snapshot_timer: Timer::from_seconds(SNAPSHOT_INTERVAL, TimerMode::Repeating),
            poll_timer: Timer::from_seconds(POLL_INTERVAL, TimerMode::Repeating),
            last_seq: 0,
            errors: Vec::new(),
            force_snapshot: true,
            last_stat: None,
        }
    }
}

/// Directory (and log) name of a seat: red plays Claude, blue plays Human —
/// the same colours the game window uses.
fn seat_name(team: Team) -> &'static str {
    match team {
        Team::Claude => "red",
        Team::Human => "blue",
    }
}

/// The open seats. Empty means the bridge is inactive and every system
/// early-returns before touching the filesystem.
#[derive(Resource, Default)]
struct Bridge {
    seats: Vec<Seat>,
}

fn bridge_enabled(bridge: Res<Bridge>) -> bool {
    !bridge.seats.is_empty()
}

/// Which factions `WC3_BRIDGE` asks for. `None` means "leave the bridge off".
fn seats_from_env(raw: &str) -> Option<Vec<Team>> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" => None,
        "1" | "red" | "claude" => Some(vec![Team::Claude]),
        "blue" | "human" => Some(vec![Team::Human]),
        "both" | "2" => Some(vec![Team::Claude, Team::Human]),
        // Anything else truthy keeps the historical "any value enables it"
        // behaviour rather than silently starting no bridge at all.
        other => {
            warn!("{BRIDGE_ENV}: unrecognized value '{other}' — assuming 'red'");
            Some(vec![Team::Claude])
        }
    }
}

/// Opt in from the environment, prepare each seat's directory, and take the
/// seated factions off the scripted macro AI.
fn bridge_startup(
    mut bridge: ResMut<Bridge>,
    mut ai_controlled: ResMut<AiControlled>,
    mut external: ResMut<ExternallyCommanded>,
) {
    let Ok(raw) = std::env::var(BRIDGE_ENV) else {
        return;
    };
    let Some(teams) = seats_from_env(&raw) else {
        return;
    };

    // One catalog for the whole session: content is static, so both seats get
    // byte-identical files and nothing has to be re-serialized per snapshot.
    let catalog_json = match serde_json::to_string_pretty(&game_catalog()) {
        Ok(json) => Some(json),
        Err(err) => {
            warn!("{BRIDGE_ENV}: catalog serialization failed ({err}) — no {CATALOG_NAME}");
            None
        }
    };

    for team in teams {
        let seat = Seat::new(team);
        if let Err(err) = std::fs::create_dir_all(&seat.dir) {
            error!(
                "{BRIDGE_ENV}: cannot create {}/ ({err}) — {:?} seat disabled",
                seat.dir.display(),
                team
            );
            continue;
        }
        if let Some(json) = &catalog_json {
            write_catalog(&seat.dir, json);
        }
        // Each session starts from a clean protocol state (seq 0), so a
        // leftover batch from a previous run can never replay onto this
        // world's entities.
        if seat.commands_file.exists() {
            if let Err(err) = std::fs::remove_file(&seat.commands_file) {
                warn!(
                    "{BRIDGE_ENV}: could not clear stale {} ({err})",
                    seat.commands_file.display()
                );
            }
        }
        // The external commander replaces the scripted macro AI on *its* side
        // only; the other faction keeps whatever ai.rs decided.
        set_autopilot(&mut ai_controlled, team, false);
        // ...but the team is still machine-driven, which is what keeps
        // doctrine's default-squad autonomy active for it.
        match team {
            Team::Human => external.human = true,
            Team::Claude => external.claude = true,
        }
        info!(
            "{BRIDGE_ENV}: live bridge seat '{}' active — {:?} is under external \
             control (snapshot {}, commands {})",
            seat_name(team),
            team,
            seat.state_file.display(),
            seat.commands_file.display()
        );
        bridge.seats.push(seat);
    }
}

/// Publish the content catalog into a seat's directory, atomically (tmp +
/// rename) so a commander that reads it while we write never sees half a file.
fn write_catalog(dir: &Path, json: &str) {
    let file = dir.join(CATALOG_NAME);
    let tmp = dir.join(CATALOG_TMP_NAME);
    match std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &file)) {
        Ok(()) => info!("{BRIDGE_ENV}: wrote content catalog {}", file.display()),
        Err(err) => warn!("bridge: could not write {}: {err}", file.display()),
    }
}


// ---------------------------------------------------------------------------
// Snapshot: world -> bridge/<seat>/state.json
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StateOut {
    t: f32,
    /// Which faction this snapshot is written for: `"Claude"` or `"Human"`.
    /// Everything below named "my"/"me"/"own" is relative to it.
    my_team: &'static str,
    seq_applied: u64,
    errors: Vec<String>,
    game_over: Option<&'static str>,
    me: MeOut,
    /// The ground both seats are fighting over: which layout is loaded and
    /// where its impassable terrain can be crossed. The human sees the canyon
    /// on screen and on the minimap; this is the same fact in JSON.
    map: MapOut,
    /// `catalog.json` entry id -> may this seat build/train it right now?
    /// Every unit and building in the catalog appears, whether or not it has
    /// requirements, so a commander can gate its build order on one lookup.
    unlocked: BTreeMap<&'static str, bool>,
    units: Vec<UnitOut>,
    buildings: Vec<BuildingOut>,
    squads: Vec<SquadOut>,
    mines: Vec<MineOut>,
    trees_near: Vec<TreeOut>,
    /// Live bounty caches, identical for both seats — treasure on the ground is
    /// public information.
    bounties: Vec<BountyOut>,
    /// `[[game_time, message], ...]`, oldest first — see `diff_events`.
    events: Vec<(f32, String)>,
}

/// The map, as neutral public information — identical in both seats'
/// snapshots, because both players are looking at the same ground.
#[derive(Serialize)]
struct MapOut {
    /// The `WC3_MAP` value that produced this world: `"open"`, `"crossings"`.
    name: &'static str,
    /// What the layout means for a plan, in one sentence.
    summary: &'static str,
    /// Every layout this build offers, so a commander can see what else exists
    /// without being told (the human reads the same list from `WC3_MAP`).
    available: Vec<&'static str>,
    /// Gaps in the impassable terrain — empty on a map that has none. Armies,
    /// workers and expansions can only cross here.
    chokes: Vec<ChokeOut>,
}

#[derive(Serialize)]
struct ChokeOut {
    name: &'static str,
    pos: [f32; 2],
    /// Opening width in world units. Anything standing in the gap (a gold mine,
    /// a wall, a tower) narrows it further.
    width: f32,
}

#[derive(Serialize)]
struct MeOut {
    gold: u32,
    lumber: u32,
    supply_used: u32,
    supply_cap: u32,
    /// Fraction of each gold delivery you actually receive (upkeep tax).
    upkeep_rate: f32,
    hero_record: Option<HeroRecordOut>,
    hero_cost: CostOut,
}

#[derive(Serialize)]
struct HeroRecordOut {
    level: u32,
    xp: f32,
}

#[derive(Serialize)]
struct CostOut {
    gold: u32,
    lumber: u32,
    time: f32,
}

#[derive(Serialize)]
struct UnitOut {
    id: u64,
    team: &'static str,
    kind: &'static str,
    pos: [f32; 2],
    hp: f32,
    max_hp: f32,
    order: &'static str,
    moving: bool,
    carrying: bool,
    hero: Option<HeroOut>,
    /// Heroes only (they are the only units with an `Inventory`): the two
    /// consumable slots, `null` for an empty one. Absent for everything else.
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Vec<Option<&'static str>>>,
    /// A worker under Call to Arms. Absent (rather than `false`) the rest of
    /// the time, which is almost always.
    #[serde(skip_serializing_if = "is_false")]
    militia: bool,
    /// Airborne. `pos` is the ground cell it is over (altitude is not a
    /// tactical variable — every range check in the game is ground-plane), but
    /// WHETHER it is airborne decides who can shoot it, so it has to be
    /// visible in the snapshot and not merely inferable from `kind` via the
    /// catalog. Omitted for the overwhelmingly common ground unit.
    #[serde(skip_serializing_if = "is_false")]
    flying: bool,
    /// Own units only (`null` when unassigned); absent entirely for enemies —
    /// we can see their army, not their command structure.
    #[serde(skip_serializing_if = "Option::is_none")]
    squad: Option<Option<u8>>,
    /// Own units only, and only when at least one policy is set — the common
    /// case is "no doctrine", and an empty object per unit is pure noise.
    #[serde(skip_serializing_if = "Option::is_none")]
    policies: Option<PoliciesOut>,
}

/// Mirror of the doctrine components, in the same shape the `priority` /
/// `retreat` / `leash` / `autocast` commands take them.
#[derive(Serialize)]
struct PoliciesOut {
    prio: Option<Vec<&'static str>>,
    /// `[below_frac, rally_x, rally_z]`
    retreat: Option<[f32; 3]>,
    /// `[anchor_x, anchor_z, radius]`
    leash: Option<[f32; 3]>,
    autocast: Option<u32>,
}

#[derive(Serialize)]
struct SquadOut {
    id: u8,
    /// `"defend@(x,z)r=20"`, `"push@(x,z)"`, `"escort:<unitid>"`,
    /// `"forage@(x,z)"`, or `null` for a squad that has members but no standing
    /// posture yet. Squad 0 always has one: doctrine.rs seeds it a home
    /// `defend` whenever it is missing.
    posture: Option<String>,
    members: usize,
}

#[derive(Serialize)]
struct HeroOut {
    level: u32,
    xp: f32,
    xp_next: f32,
    mana: f32,
    max_mana: f32,
    cd: f32,
}

#[derive(Serialize)]
struct BuildingOut {
    id: u64,
    team: &'static str,
    kind: &'static str,
    pos: [f32; 2],
    hp: f32,
    max_hp: f32,
    done: bool,
    queue: Vec<&'static str>,
    progress: f32,
    /// Own buildings with an active ability only: seconds until it may be cast
    /// again (0 = ready). Absent for buildings that have no ability.
    #[serde(skip_serializing_if = "Option::is_none")]
    ability_cd: Option<f32>,
    /// Own production buildings only, and only when a `template` command has
    /// installed a `DoctrineTemplate`: a flag, not the contents — the
    /// commander that set the template already knows what is in it.
    #[serde(skip_serializing_if = "is_false")]
    template: bool,
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// A live bounty cache. Neutral information: both seats get the identical
/// list, exactly as both players see the same glow on the ground.
#[derive(Serialize)]
struct BountyOut {
    pos: [f32; 2],
    gold: u32,
    /// Game seconds until it vanishes unclaimed.
    expires_in: f32,
}

/// The same caches with their entity ids attached, for `diff_events` only —
/// the id is a bridge-internal handle for matching one tick's caches against
/// the next, not something a commander needs to see.
struct BountySnap {
    id: u64,
    pos: [f32; 2],
    gold: u32,
    expires_at: f32,
}

#[derive(Serialize)]
struct MineOut {
    id: u64,
    pos: [f32; 2],
    remaining: u32,
}

/// Trees carry their entity id so `{"type":"harvest","target":<id>}` can name
/// one — without it lumber was unorderable through the bridge.
#[derive(Serialize)]
struct TreeOut {
    id: u64,
    pos: [f32; 2],
}

type SnapshotUnits<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Unit,
        &'static Team,
        &'static Transform,
        &'static Health,
        Option<&'static Order>,
        Option<&'static MoveTo>,
        Option<&'static Carrying>,
        Option<&'static Hero>,
        // Doctrine, nested so the outer tuple stays comfortably short.
        (
            Option<&'static SquadId>,
            Option<&'static TargetPriority>,
            Option<&'static RetreatPolicy>,
            Option<&'static LeashPolicy>,
            Option<&'static AutoCastPolicy>,
        ),
        // Hero kit & Call-to-Arms state, nested for the same reason.
        (Option<&'static Inventory>, Has<Militia>),
    ),
>;

type SnapshotBuildings<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Building,
        &'static Team,
        &'static Transform,
        &'static Health,
        Option<&'static UnderConstruction>,
        Option<&'static TrainingQueue>,
        Option<&'static DoctrineTemplate>,
        Option<&'static AbilityCooldown>,
    ),
>;

type SnapshotNodes<'w, 's> =
    Query<'w, 's, (Entity, &'static ResourceNode, &'static Transform)>;

type SnapshotBounties<'w, 's> =
    Query<'w, 's, (Entity, &'static Bounty, &'static Transform)>;

#[allow(clippy::too_many_arguments)]
fn write_snapshot(
    time: Res<Time>,
    real: Res<Time<Real>>,
    mut bridge: ResMut<Bridge>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    game_over: Res<GameOver>,
    squad_orders: Res<SquadOrders>,
    feed: Res<GameEvents>,
    intent_errors: Res<IntentErrors>,
    units: SnapshotUnits,
    buildings: SnapshotBuildings,
    nodes: SnapshotNodes,
    bounties: SnapshotBounties,
) {
    let now = r1(time.elapsed_secs());
    let delta = real.delta();
    for seat in &mut bridge.seats {
        let due = seat.snapshot_timer.tick(delta).just_finished();
        if !due && !seat.force_snapshot {
            continue;
        }
        seat.force_snapshot = false;
        write_seat_snapshot(
            seat,
            now,
            &economies,
            &records,
            &game_over,
            &squad_orders,
            &feed,
            intent_errors.get(seat.team),
            &units,
            &buildings,
            &nodes,
            &bounties,
        );
    }
}

/// Build and publish one seat's snapshot. Everything here is relative to
/// `seat.team`: the two seats never look at the same picture.
#[allow(clippy::too_many_arguments)]
fn write_seat_snapshot(
    seat: &mut Seat,
    now: f32,
    economies: &Economies,
    records: &HeroRecords,
    game_over: &GameOver,
    squad_orders: &SquadOrders,
    feed: &GameEvents,
    // Per-command validation errors this team's intents produced, from the
    // shared compiler. Reported alongside the seat's own batch-level errors.
    intent_errors: &[String],
    units: &SnapshotUnits,
    buildings: &SnapshotBuildings,
    nodes: &SnapshotNodes,
    bounties: &SnapshotBounties,
) {
    let me = seat.team;

    // Full vision of both armies; doctrine only for our own units.
    let mut units_out: Vec<UnitOut> = units
        .iter()
        .map(|(e, unit, team, tf, health, order, move_to, carrying, hero, doctrine, kit)| {
            let (squad, prio, retreat, leash, autocast) = doctrine;
            let (inventory, militia) = kit;
            let mine = *team == me;
            let has_policy =
                prio.is_some() || retreat.is_some() || leash.is_some() || autocast.is_some();
            UnitOut {
                id: e.to_bits(),
                team: team_name(*team),
                kind: kind_name(unit.kind),
                pos: [r1(tf.translation.x), r1(tf.translation.z)],
                hp: r1(health.current),
                max_hp: r1(health.max),
                order: order_name(order.unwrap_or(&Order::Idle)),
                moving: move_to.is_some(),
                carrying: carrying.is_some(),
                hero: hero.map(|h| HeroOut {
                    level: h.level,
                    xp: r1(h.xp),
                    xp_next: r1(Hero::xp_to_next(h.level)),
                    mana: r1(h.mana),
                    max_mana: r1(Hero::max_mana(h.level)),
                    cd: r1(h.ability_cooldown),
                }),
                items: inventory.map(|inv| {
                    inv.0
                        .iter()
                        .map(|slot| slot.map(|id| item_def(id).name))
                        .collect()
                }),
                militia,
                flying: is_flying_kind(unit.kind),
                squad: mine.then(|| squad.map(|s| s.0)),
                policies: (mine && has_policy).then(|| PoliciesOut {
                    prio: prio.map(|p| p.0.iter().map(|c| target_class_name(*c)).collect()),
                    retreat: retreat
                        .map(|r| [r1(r.below_frac), r1(r.rally.x), r1(r.rally.z)]),
                    leash: leash.map(|l| [r1(l.anchor.x), r1(l.anchor.z), r1(l.radius)]),
                    autocast: autocast.map(|a| a.min_enemies),
                }),
            }
        })
        .collect();
    units_out.sort_by_key(|u| u.id);

    let mut buildings_out: Vec<BuildingOut> = buildings
        .iter()
        .map(|(e, building, team, tf, health, under, queue, template, cooldown)| BuildingOut {
            id: e.to_bits(),
            team: team_name(*team),
            kind: building_name(building.kind),
            pos: [r1(tf.translation.x), r1(tf.translation.z)],
            hp: r1(health.current),
            max_hp: r1(health.max),
            done: under.is_none(),
            queue: queue
                .map(|q| q.queue.iter().map(|k| kind_name(*k)).collect())
                .unwrap_or_default(),
            progress: r1(queue.map(|q| q.progress).unwrap_or(0.0)),
            // Our own casters only: an ability we can actually order.
            ability_cd: (*team == me && ability_of_building(building.kind).is_some())
                .then(|| r1(cooldown.map(|c| c.0).unwrap_or(0.0))),
            // Never for the opponent: a template is command structure.
            template: *team == me && template.is_some(),
        })
        .collect();
    buildings_out.sort_by_key(|b| b.id);

    // Tech state, for this seat only: what its completed buildings unlock.
    let completed: Vec<BuildingKind> = buildings
        .iter()
        .filter(|(_, _, team, _, _, under, _, _, _)| **team == me && under.is_none())
        .map(|(_, building, ..)| building.kind)
        .collect();
    let unlocked = unlocked_map(&completed);

    let mut mines: Vec<MineOut> = nodes
        .iter()
        .filter(|(_, node, _)| node.kind == ResourceKind::Gold)
        .map(|(e, node, tf)| MineOut {
            id: e.to_bits(),
            pos: [r1(tf.translation.x), r1(tf.translation.z)],
            remaining: node.remaining,
        })
        .collect();
    mines.sort_by_key(|m| m.id);

    // Trees are far too numerous to ship whole — send the handful nearest this
    // seat's own base.
    let home = me.base_pos();
    let mut trees: Vec<(f32, TreeOut)> = nodes
        .iter()
        .filter(|(_, node, _)| node.kind == ResourceKind::Lumber && node.remaining > 0)
        .map(|(e, _, tf)| {
            (
                tf.translation.distance(home),
                TreeOut {
                    id: e.to_bits(),
                    pos: [r1(tf.translation.x), r1(tf.translation.z)],
                },
            )
        })
        .collect();
    trees.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let trees_near: Vec<TreeOut> = trees.into_iter().take(TREES_NEAR).map(|(_, t)| t).collect();

    // Squads: every id of ours with a standing posture, plus any id that merely
    // has members — a squad the commander built but hasn't tasked is still
    // real. The opponent's squads are none of this seat's business.
    let members = squad_members(&units_out);
    let mut squad_ids: Vec<u8> = squad_orders
        .0
        .keys()
        .filter(|(team, _)| *team == me)
        .map(|(_, id)| *id)
        .chain(members.keys().copied())
        .collect();
    squad_ids.sort_unstable();
    squad_ids.dedup();
    let squads: Vec<SquadOut> = squad_ids
        .iter()
        .map(|&id| SquadOut {
            id,
            posture: squad_orders.0.get(&(me, id)).map(posture_name),
            members: members.get(&id).copied().unwrap_or(0),
        })
        .collect();

    // Bounty caches, sorted by id so both seats serialize the same order.
    let mut bounty_snaps: Vec<BountySnap> = bounties
        .iter()
        .map(|(e, bounty, tf)| BountySnap {
            id: e.to_bits(),
            pos: [r1(tf.translation.x), r1(tf.translation.z)],
            gold: bounty.gold,
            expires_at: bounty.expires_at,
        })
        .collect();
    bounty_snaps.sort_by_key(|b| b.id);
    let bounties_out: Vec<BountyOut> = bounty_snaps
        .iter()
        .map(|b| BountyOut {
            pos: b.pos,
            gold: b.gold,
            expires_in: r1((b.expires_at - now).max(0.0)),
        })
        .collect();

    // The event stream is not ours to compute: shared.rs runs one diff per team
    // per tick and the HUD reads the identical buffer. We only serialize this
    // seat's half of it — `severity` and `pos` are renderer sugar a file reader
    // does not need, and the wire format predates them.
    let events: Vec<(f32, String)> = feed
        .feed(me)
        .iter()
        .map(|e| (e.t, e.message.clone()))
        .collect();

    let eco = *economies.get(me);
    let (hero_gold, hero_lumber, hero_time) = hero_train_cost(records, me);

    let map = crate::terrain::active_map();
    let state = StateOut {
        t: now,
        my_team: team_name(me),
        seq_applied: seat.last_seq,
        // Batch-level first, then the compiler's per-command verdicts — one
        // flat array of strings, exactly the shape the protocol always had.
        errors: seat
            .errors
            .iter()
            .chain(intent_errors.iter())
            .cloned()
            .collect(),
        game_over: game_over.0.map(team_name),
        me: MeOut {
            gold: eco.gold,
            lumber: eco.lumber,
            supply_used: eco.supply_used,
            supply_cap: eco.supply_cap,
            upkeep_rate: upkeep_rate(eco.supply_used),
            hero_record: records.get(me).map(|r| HeroRecordOut {
                level: r.level,
                xp: r1(r.xp),
            }),
            hero_cost: CostOut {
                gold: hero_gold,
                lumber: hero_lumber,
                time: hero_time,
            },
        },
        map: MapOut {
            name: map.id(),
            summary: map.summary(),
            available: crate::terrain::MapKind::ALL.iter().map(|m| m.id()).collect(),
            chokes: map
                .chokepoints()
                .into_iter()
                .map(|c| ChokeOut {
                    name: c.name,
                    pos: [r1(c.pos.x), r1(c.pos.z)],
                    width: r1(c.width),
                })
                .collect(),
        },
        unlocked,
        units: units_out,
        buildings: buildings_out,
        squads,
        mines,
        trees_near,
        bounties: bounties_out,
        events,
    };

    let json = match serde_json::to_string(&state) {
        Ok(json) => json,
        Err(err) => {
            warn!("bridge: snapshot serialization failed: {err}");
            return;
        }
    };
    // Atomic publish, per seat: readers see either the old file or the new one.
    if let Err(err) = std::fs::write(&seat.state_tmp, json)
        .and_then(|_| std::fs::rename(&seat.state_tmp, &seat.state_file))
    {
        warn!("bridge: could not write {}: {err}", seat.state_file.display());
    }
}

// ---------------------------------------------------------------------------
// Tech requirements
// ---------------------------------------------------------------------------
//
// shared.rs owns the requirement tables; economy.rs enforces them at placement
// and ui.rs greys the buttons. The bridge does both jobs for its commander:
// it *reports* availability every snapshot (`unlocked`) and *validates* the
// build/train commands against exactly the same predicate, so a rejection
// arrives as a sentence in `errors` rather than as an order that quietly
// evaporates when the worker gets there.

/// Every catalog entry -> can this team build/train it with what it has
/// standing right now. Derived from the shared kind tables, so new content is
/// reported without touching this file.
fn unlocked_map(completed: &[BuildingKind]) -> BTreeMap<&'static str, bool> {
    let mut out = BTreeMap::new();
    for kind in ALL_BUILDING_KINDS {
        out.insert(
            building_name(kind),
            requirements_met(building_requires(kind), completed.iter().copied()),
        );
    }
    for kind in ALL_UNIT_KINDS {
        out.insert(
            kind_name(kind),
            requirements_met(unit_requires(kind), completed.iter().copied()),
        );
    }
    out
}


// ---------------------------------------------------------------------------
// Squad membership
// ---------------------------------------------------------------------------
//
// The event stream used to be computed here, from a private diff of consecutive
// snapshots. It now lives in `shared::GameEvents`, produced once per team per
// tick — because the HUD needs the same stream, and a feed with two producers
// is two feeds. See the "Event feed" section of shared.rs.

/// Count squad members per id, own team only (the snapshot only carries
/// `squad` for our own units, so enemies can't leak in here).
fn squad_members(units_out: &[UnitOut]) -> HashMap<u8, usize> {
    let mut counts: HashMap<u8, usize> = HashMap::new();
    for u in units_out {
        if let Some(Some(id)) = u.squad {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    counts
}


// ---------------------------------------------------------------------------
// Commands: bridge/<seat>/commands.json -> Intent
// ---------------------------------------------------------------------------
//
// This file used to be where a command became game state. It is not any more.
// The bridge's whole job on this side is now transport and protocol: read the
// file, honour `seq`, deserialize each command into a `shared::Intent`, and
// submit it. intent.rs validates it and applies it — the same compiler the
// human's mouse gestures go through, so neither seat has a private path into
// the world.
//
// The wire format did not change, because the wire format *is* the schema:
// `Intent`'s serde shape is the historical command shape, tag for tag and
// field for field, `caster` alias and `use_item` rename included.
// `tools/bridge_send.py`, `bridge_view.py` and every COMMANDER_BRIEF.md flow
// keep working untouched, and rejected commands come back as the same strings
// in the same `errors` array, still prefixed `cmd <i>`.

#[derive(Deserialize)]
struct Batch {
    seq: u64,
    /// Kept as raw values so one malformed command can't sink the batch.
    #[serde(default)]
    commands: Vec<serde_json::Value>,
}

/// Read each seat's `commands.json` and submit its contents as intents.
///
/// Ordered `.before(IntentApply)`, so everything submitted here is compiled
/// before this frame's snapshot is written.
fn poll_commands(
    real: Res<Time<Real>>,
    mut bridge: ResMut<Bridge>,
    game_over: Res<GameOver>,
    mut intent_errors: ResMut<IntentErrors>,
    mut submissions: EventWriter<SubmitIntent>,
) {
    let delta = real.delta();
    // Every seat is polled independently; one seat's unreadable or malformed
    // file leaves the other's protocol state untouched.
    for seat in &mut bridge.seats {
        if !seat.poll_timer.tick(delta).just_finished() {
            continue;
        }

        // Stat first: an untouched file is not worth re-reading or re-parsing.
        let stat = match std::fs::metadata(&seat.commands_file) {
            Ok(meta) => (meta.modified().ok(), meta.len()),
            Err(_) => continue, // no file yet — perfectly normal
        };
        let stamp = stat.0.map(|m| (m, stat.1));
        if stamp.is_some() && stamp == seat.last_stat {
            continue;
        }
        seat.last_stat = stamp;

        let raw = match std::fs::read_to_string(&seat.commands_file) {
            Ok(raw) => raw,
            Err(err) => {
                seat.errors = vec![format!("batch: unreadable ({err})")];
                seat.force_snapshot = true;
                continue;
            }
        };
        let batch: Batch = match serde_json::from_str(&raw) {
            Ok(batch) => batch,
            Err(err) => {
                seat.errors = vec![format!("batch: malformed JSON ({err})")];
                seat.force_snapshot = true;
                continue;
            }
        };
        // Only the newest batch matters; there is no queue of pending batches.
        if batch.seq <= seat.last_seq {
            continue;
        }

        // A new batch replaces the last one's verdict, exactly as before: the
        // seat's own file-level errors and the compiler's per-command errors
        // for this team both start empty.
        seat.errors.clear();
        intent_errors.get_mut(seat.team).clear();

        if game_over.0.is_some() {
            seat.errors
                .push("batch: game over — commands ignored".to_string());
        } else {
            for (i, raw) in batch.commands.iter().enumerate() {
                match serde_json::from_value::<Intent>(raw.clone()) {
                    Ok(intent) => {
                        submissions.write(SubmitIntent {
                            team: seat.team,
                            source: IntentSource::Bridge,
                            // The historical error prefix, so a commander that
                            // greps for `cmd 3` still finds its third command.
                            tag: format!("cmd {i}"),
                            intent,
                        });
                    }
                    Err(err) => intent_errors
                        .get_mut(seat.team)
                        .push(format!("cmd {i}: unrecognized command ({err})")),
                }
            }
        }

        seat.last_seq = batch.seq;
        // Publish the result of this batch immediately instead of up to a
        // second later.
        seat.force_snapshot = true;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One decimal place — snapshots stay small and diff cleanly.
fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn team_name(team: Team) -> &'static str {
    match team {
        Team::Human => "Human",
        Team::Claude => "Claude",
    }
}

fn order_name(order: &Order) -> &'static str {
    match order {
        Order::Idle => "Idle",
        Order::Move(_) => "Move",
        Order::AttackMove(_) => "AttackMove",
        Order::Attack(_) => "Attack",
        Order::Harvest(_) => "Harvest",
        Order::ReturnResources => "ReturnResources",
        Order::Build { .. } => "Build",
        Order::Follow(_) => "Follow",
    }
}

// Class names derive from shared so new classes can't drift out of the
// protocol (the enum's `name()` is the wire format).
fn target_class_name(class: TargetClass) -> &'static str {
    class.name()
}

/// One-line rendering of a posture for the snapshot.
fn posture_name(posture: &SquadPosture) -> String {
    match posture {
        SquadPosture::Defend { pos, radius } => {
            format!("defend@({:.1},{:.1})r={:.0}", pos.x, pos.z, radius)
        }
        SquadPosture::Push { pos } => format!("push@({:.1},{:.1})", pos.x, pos.z),
        SquadPosture::Escort { unit } => format!("escort:{}", unit.to_bits()),
        SquadPosture::Forage { muster } => {
            format!("forage@({:.1},{:.1})", muster.x, muster.z)
        }
    }
}
