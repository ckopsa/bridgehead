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
//! filesystem, so a normal `cargo run` never creates `bridge/`.
//!
//! Like ai.rs, the bridge acts ONLY through the primitives the UI uses: it
//! writes `Order` components with `try_insert`, pushes `UnitKind`s onto its own
//! buildings' `TrainingQueue`, sets `RallyPoint`, and sends `CastAbility`.
//! It never spawns anything, never writes `Health`, never touches enemy
//! entities, and only *reads* its own `Economies` entry (economy.rs pays).
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
//! (`squad`, `posture`). The bridge only writes those components and the
//! `SquadOrders` resource — doctrine.rs and combat.rs act on them. Setting a
//! policy is therefore as cheap as any other command, and a commander that
//! polls slowly still gets sensible behaviour between polls.
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
//! squad wipes, built by diffing consecutive snapshots inside this file.
//!
//! Every failure mode — missing file, malformed JSON, dead entity, unaffordable
//! order — turns into an error string carried in the next snapshot's `errors`
//! array instead of a panic.

use crate::shared::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
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

/// Same formation grid the UI uses for multi-unit ground orders.
const FORMATION_SPACING: f32 = 2.6;
/// Same training queue cap the UI enforces.
const MAX_QUEUE: usize = 7;
/// Hero inventory size, read off the shared component so it cannot drift.
const INVENTORY_SLOTS: usize = Inventory([None; 2]).0.len();
/// How many trees to include in the snapshot (there are hundreds).
const TREES_NEAR: usize = 12;

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
/// Ring-buffer capacity for the event stream. Every snapshot carries the whole
/// buffer, so a commander polling every ~15s still sees everything that
/// happened in between; the reader filters by timestamp.
const MAX_EVENTS: usize = 40;
/// Slack on a vanished bounty's deadline before we call its disappearance
/// early (i.e. claimed rather than timed out). Snapshot clocks are rounded to
/// one decimal, so an exact comparison would misread a natural expiry.
const BOUNTY_EXPIRY_EPS: f32 = 0.5;

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
                // Poll first, snapshot second: a batch applied this frame is
                // visible in the snapshot written the same frame.
                (poll_commands, write_snapshot)
                    .chain()
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
    /// Errors produced by the most recent batch (or its parse attempt).
    errors: Vec<String>,
    /// Write a snapshot on the next tick regardless of the timer.
    force_snapshot: bool,
    /// (mtime, len) of this seat's `commands.json` when it was last read, so an
    /// unchanged file is not re-parsed four times a second.
    last_stat: Option<(std::time::SystemTime, u64)>,
    /// Game-time-stamped notable happenings, oldest first, capped at
    /// `MAX_EVENTS`. Produced purely by diffing consecutive snapshots (see
    /// `diff_events`) — no other module knows this exists.
    events: VecDeque<(f32, String)>,
    /// The previous tick's own-team picture, the other half of that diff.
    prev: EventMemo,
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
            events: VecDeque::new(),
            prev: EventMemo::default(),
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

/// Everything `diff_events` needs to remember from one snapshot to the next.
/// Module-private by design: the event stream is a bridge concern, not a
/// shared-contract one. One of these per seat — never shared.
#[derive(Default)]
struct EventMemo {
    /// False until the first snapshot has been recorded. The first tick only
    /// seeds this memo — with nothing to diff against, every unit would
    /// otherwise look newly noteworthy.
    seeded: bool,
    /// own unit id -> (kind, last known position)
    units: HashMap<u64, (&'static str, [f32; 2])>,
    /// own building id -> (kind, position, hp, max_hp)
    buildings: HashMap<u64, (&'static str, [f32; 2], f32, f32)>,
    hero_alive: bool,
    hero_level: u32,
    /// Latched so "hero low" fires once per crossing rather than every tick.
    hero_low: bool,
    threat: usize,
    squad_members: HashMap<u8, usize>,
    /// Largest membership seen since each squad was last empty. A squad that
    /// bleeds out one member per tick is still a squad that got wiped, so the
    /// report keys off this rather than the previous tick's count.
    squad_peak: HashMap<u8, usize>,
    /// bounty entity id -> (position, gold, expiry deadline). Bounties are the
    /// one thing in this memo that isn't own-team: treasure is neutral.
    bounties: HashMap<u64, ([f32; 2], u32, f32)>,
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

/// Hand a faction to (or take it from) the scripted macro AI.
fn set_autopilot(ai_controlled: &mut AiControlled, team: Team, on: bool) {
    match team {
        Team::Claude => ai_controlled.claude = on,
        Team::Human => ai_controlled.human = on,
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

    // Diff this picture against this seat's previous one before publishing it.
    diff_events(
        now,
        me,
        &mut seat.prev,
        &mut seat.events,
        &units_out,
        &buildings_out,
        &members,
        squad_orders,
        &bounty_snaps,
    );
    let events: Vec<(f32, String)> = seat.events.iter().cloned().collect();

    let eco = *economies.get(me);
    let (hero_gold, hero_lumber, hero_time) = hero_train_cost(records, me);

    let map = crate::terrain::active_map();
    let state = StateOut {
        t: now,
        my_team: team_name(me),
        seq_applied: seat.last_seq,
        errors: seat.errors.clone(),
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

/// `None` when `reqs` are satisfied, otherwise the error line to report, e.g.
/// `"cmd 3: Tower requires Barracks"`.
fn requirement_error(
    index: usize,
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
    Some(format!("cmd {index}: {what} requires {}", missing.join(" + ")))
}

// ---------------------------------------------------------------------------
// Event stream: snapshot-to-snapshot diffing
// ---------------------------------------------------------------------------
//
// A commander polling once every ten or fifteen seconds used to see only the
// aftermath: fewer units, less base, no idea what happened. The event stream
// closes that gap without any other module's cooperation — nothing here hooks
// combat or economy, it simply remembers the last own-team picture and reports
// what changed. Everything is game-time stamped and kept in a ring buffer that
// outlives individual snapshots, so a slow reader misses nothing; it just
// filters by timestamp against what it already saw.
//
// "Own" means the seat's team, so the two seats produce mirror-image streams
// from the same world.

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

/// Compare this tick's own-team picture with the previous one and append any
/// notable differences to the seat's ring buffer.
#[allow(clippy::too_many_arguments)]
fn diff_events(
    now: f32,
    me: Team,
    memo: &mut EventMemo,
    events: &mut VecDeque<(f32, String)>,
    units_out: &[UnitOut],
    buildings_out: &[BuildingOut],
    members: &HashMap<u8, usize>,
    squad_orders: &SquadOrders,
    bounties: &[BountySnap],
) {
    let mine = team_name(me);
    let home = me.base_pos();

    // --- gather the current picture -------------------------------------
    let mut cur_units: HashMap<u64, (&'static str, [f32; 2])> = HashMap::new();
    let mut hero_alive = false;
    let mut hero_level = memo.hero_level;
    let mut hero_frac = 1.0f32;
    let mut hostiles: Vec<[f32; 2]> = Vec::new();
    for u in units_out {
        if u.team == mine {
            cur_units.insert(u.id, (u.kind, u.pos));
            if let Some(h) = &u.hero {
                hero_alive = true;
                hero_level = h.level;
                hero_frac = if u.max_hp > 0.0 { u.hp / u.max_hp } else { 1.0 };
            }
        } else if u.kind != "Worker" {
            // Workers wander; only combat units count as a threat.
            let d = (u.pos[0] - home.x).hypot(u.pos[1] - home.z);
            if d <= THREAT_RADIUS {
                hostiles.push(u.pos);
            }
        }
    }

    let mut cur_buildings: HashMap<u64, (&'static str, [f32; 2], f32, f32)> = HashMap::new();
    for b in buildings_out {
        if b.team == mine {
            cur_buildings.insert(b.id, (b.kind, b.pos, b.hp, b.max_hp));
        }
    }

    let threat = hostiles.len();

    let cur_bounties: HashMap<u64, ([f32; 2], u32, f32)> = bounties
        .iter()
        .map(|b| (b.id, (b.pos, b.gold, b.expires_at)))
        .collect();

    // The very first tick has nothing to compare against; seed and stay quiet.
    if !memo.seeded {
        memo.seeded = true;
        memo.units = cur_units;
        memo.buildings = cur_buildings;
        memo.hero_alive = hero_alive;
        memo.hero_level = hero_level;
        memo.hero_low = hero_alive && hero_frac < HERO_LOW_FRAC;
        memo.threat = threat;
        memo.squad_members = members.clone();
        memo.squad_peak = members.clone();
        memo.bounties = cur_bounties;
        return;
    }

    let mut out: Vec<String> = Vec::new();

    // --- unit losses ----------------------------------------------------
    // Grouped by kind so a wiped squad reads as one line, not eight.
    let mut lost: HashMap<&'static str, Vec<[f32; 2]>> = HashMap::new();
    for (id, (kind, pos)) in &memo.units {
        if cur_units.contains_key(id) || is_hero_name(kind) {
            continue; // the hero gets its own, better, event below
        }
        lost.entry(kind).or_default().push(*pos);
    }
    let mut lost: Vec<(&'static str, Vec<[f32; 2]>)> = lost.into_iter().collect();
    lost.sort_unstable_by_key(|(kind, _)| *kind);
    for (kind, positions) in lost {
        if positions.len() >= LOSS_AGGREGATE {
            let (cx, cz) = centroid(&positions);
            out.push(format!(
                "lost {} {kind} near ({cx:.1},{cz:.1})",
                positions.len()
            ));
        } else {
            for p in positions {
                out.push(format!("lost {kind} @({:.1},{:.1})", p[0], p[1]));
            }
        }
    }

    // --- buildings destroyed or newly hurt ------------------------------
    let mut building_ids: Vec<&u64> = memo.buildings.keys().collect();
    building_ids.sort_unstable();
    for id in building_ids {
        let (kind, pos, hp, max_hp) = memo.buildings[id];
        match cur_buildings.get(id) {
            None => out.push(format!("{kind} @({:.1},{:.1}) destroyed", pos[0], pos[1])),
            Some((_, now_pos, now_hp, now_max)) => {
                let was_hurt = max_hp > 0.0 && hp / max_hp < BUILDING_HURT_FRAC;
                let is_hurt = *now_max > 0.0 && now_hp / now_max < BUILDING_HURT_FRAC;
                if is_hurt && !was_hurt {
                    out.push(format!(
                        "{kind} @({:.1},{:.1}) under attack ({:.0}/{:.0})",
                        now_pos[0], now_pos[1], now_hp, now_max
                    ));
                }
            }
        }
    }

    // --- the Champion ---------------------------------------------------
    if memo.hero_alive && !hero_alive {
        out.push("hero died".to_string());
    }
    if memo.hero_alive && hero_alive && hero_level > memo.hero_level {
        out.push(format!("hero level up: {hero_level}"));
    }
    let hero_low = hero_alive && hero_frac < HERO_LOW_FRAC;
    if hero_low && !memo.hero_low {
        out.push(format!("hero low: {}%", (hero_frac * 100.0).round() as i32));
    }

    // --- pressure on the base -------------------------------------------
    // Report the arrival of a threat, and any sharp escalation of one, but not
    // every tick a siege continues — the commander can read `units` for that.
    if threat > 0 && (memo.threat == 0 || threat >= memo.threat + THREAT_SPIKE) {
        let (cx, cz) = centroid(&hostiles);
        out.push(format!("{threat} hostiles near base @({cx:.1},{cz:.1})"));
    }

    // --- squad wipes ----------------------------------------------------
    let mut posture_ids: Vec<u8> = squad_orders
        .0
        .keys()
        .filter(|(team, _)| *team == me)
        .map(|(_, id)| *id)
        .collect();
    posture_ids.sort_unstable();
    for (&id, &n) in members {
        let peak = memo.squad_peak.entry(id).or_insert(0);
        *peak = (*peak).max(n);
    }
    for id in posture_ids {
        let before = memo.squad_members.get(&id).copied().unwrap_or(0);
        let after = members.get(&id).copied().unwrap_or(0);
        let peak = memo.squad_peak.get(&id).copied().unwrap_or(0);
        if after == 0 && before > 0 && peak >= 2 {
            out.push(format!("squad {id} wiped"));
        }
    }
    // An emptied squad forgets its peak, so a rebuilt one can be wiped again.
    memo.squad_peak
        .retain(|id, _| members.get(id).copied().unwrap_or(0) > 0);

    // --- bounty caches ---------------------------------------------------
    // Deliberately *unattributed*. bounty.rs despawns a cache the moment
    // somebody claims it, and it does not record who — a claim is not visible
    // in any snapshot, and diffing our own gold cannot separate a bounty from
    // the harvest income arriving in the same second. So the feed reports the
    // observable fact only: a cache appeared, and a cache went away before its
    // deadline (i.e. somebody took it — possibly us). A commander that had a
    // unit on the spot knows it was theirs; one that did not knows it lost the
    // race. Caches that simply time out say nothing: `expires_in` already
    // counted them down in every snapshot.
    for b in bounties {
        if !memo.bounties.contains_key(&b.id) {
            out.push(format!(
                "bounty spawned: {}g @({:.1},{:.1})",
                b.gold, b.pos[0], b.pos[1]
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
        // Tolerance absorbs the snapshot's rounded clock; anything still short
        // of its deadline was taken, not timed out.
        if now + BOUNTY_EXPIRY_EPS < *expires_at {
            out.push(format!("bounty gone @({:.1},{:.1})", pos[0], pos[1]));
        }
    }

    // --- publish & remember ---------------------------------------------
    for message in out {
        events.push_back((now, message));
    }
    while events.len() > MAX_EVENTS {
        events.pop_front();
    }

    memo.units = cur_units;
    memo.buildings = cur_buildings;
    memo.hero_alive = hero_alive;
    memo.hero_level = hero_level;
    memo.hero_low = hero_low;
    memo.threat = threat;
    memo.squad_members = members.clone();
    memo.bounties = cur_bounties;
}

/// Is this snapshot `kind` string one of the hero classes? Derived from the
/// shared tables, so a third class needs no edit here.
fn is_hero_name(kind: &str) -> bool {
    ALL_UNIT_KINDS
        .iter()
        .any(|k| is_hero_kind(*k) && kind_name(*k) == kind)
}

fn centroid(points: &[[f32; 2]]) -> (f32, f32) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let n = points.len() as f32;
    (
        points.iter().map(|p| p[0]).sum::<f32>() / n,
        points.iter().map(|p| p[1]).sum::<f32>() / n,
    )
}

// ---------------------------------------------------------------------------
// Commands: bridge/<seat>/commands.json -> orders
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Batch {
    seq: u64,
    /// Kept as raw values so one malformed command can't sink the batch.
    #[serde(default)]
    commands: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Cmd {
    Move {
        units: Vec<u64>,
        x: f32,
        z: f32,
    },
    AttackMove {
        units: Vec<u64>,
        x: f32,
        z: f32,
    },
    Attack {
        units: Vec<u64>,
        target: u64,
    },
    Harvest {
        units: Vec<u64>,
        target: u64,
    },
    Return {
        units: Vec<u64>,
    },
    Follow {
        units: Vec<u64>,
        target: u64,
    },
    Stop {
        units: Vec<u64>,
    },
    Build {
        worker: u64,
        kind: String,
        x: f32,
        z: f32,
    },
    Train {
        building: u64,
        unit: String,
    },
    Cancel {
        building: u64,
        index: usize,
    },
    Rally {
        building: u64,
        x: Option<f32>,
        z: Option<f32>,
        target: Option<u64>,
    },
    /// Cast the caster's one ability. The caster is a hero (`ability_of_unit`)
    /// or one of our own finished buildings (`ability_of_building`, today only
    /// the TownHall's Call to Arms). `hero` is the historical field name;
    /// `caster` says what it really means now.
    Cast {
        #[serde(alias = "caster")]
        hero: u64,
    },
    /// Buy a consumable at one of our own finished Shops. The buyer is implied:
    /// a team has at most one living hero, and only heroes carry an inventory.
    Buy {
        shop: u64,
        item: String,
    },
    /// Consume the hero's inventory slot 0 or 1.
    #[serde(rename = "use_item")]
    UseSlot {
        slot: usize,
    },
    Autopilot {
        on: bool,
    },
    /// Concede the match: the opponent immediately wins.
    Surrender,
    // --- doctrine: standing policies the executor carries out every tick ---
    /// Focus-fire order. Empty/omitted `classes` clears the policy.
    Priority {
        units: Vec<u64>,
        #[serde(default)]
        classes: Vec<String>,
    },
    /// Break off below `below` (a fraction in the open range 0..1) and fall
    /// back to x/z. `below` omitted, null, or 0 clears the policy.
    Retreat {
        units: Vec<u64>,
        #[serde(default)]
        below: Option<f32>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        z: Option<f32>,
    },
    /// Anchor to x/z within `radius`. `radius <= 0` clears the policy.
    Leash {
        units: Vec<u64>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        z: Option<f32>,
        #[serde(default)]
        radius: Option<f32>,
    },
    /// Champion-only. `min_enemies` omitted, null, or 0 clears the policy.
    Autocast {
        units: Vec<u64>,
        #[serde(default)]
        min_enemies: Option<u32>,
    },
    /// Squad membership. `id` omitted or null removes the units from any squad.
    Squad {
        units: Vec<u64>,
        #[serde(default)]
        id: Option<u8>,
    },
    /// What a squad is for. `posture` omitted or null clears the entry, which
    /// leaves the members where they are without disbanding the squad.
    Posture {
        id: u8,
        #[serde(default)]
        posture: Option<PostureIn>,
    },
    /// Standing doctrine for everything a production building trains from now
    /// on. Each piece is independent and absolute: whatever is given replaces
    /// the whole template, and every piece omitted or null is left unset. A
    /// command with no pieces at all removes the template entirely.
    Template {
        building: u64,
        #[serde(default)]
        squad: Option<u8>,
        #[serde(default)]
        retreat: Option<RetreatIn>,
        #[serde(default)]
        priority: Option<Vec<String>>,
        #[serde(default)]
        autocast: Option<u32>,
    },
}

/// The `retreat` piece of a `template` command: break off below `below` (a
/// fraction in the open range 0..1) and fall back to x/z.
#[derive(Deserialize)]
struct RetreatIn {
    below: f32,
    x: f32,
    z: f32,
}

/// The inner object of a `posture` command.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum PostureIn {
    Defend { x: f32, z: f32, radius: f32 },
    Push { x: f32, z: f32 },
    Escort { unit: u64 },
    /// Hunt bounty caches; x/z is the muster point held while none exist.
    Forage { x: f32, z: f32 },
}

/// Entity first so the seat's own hero can be *found*, not just checked — the
/// `buy` and `use_item` commands name no unit and infer it from the team.
type CmdUnits<'w, 's> =
    Query<'w, 's, (Entity, &'static Unit, &'static Team, &'static Transform)>;

type CmdBuildings<'w, 's> = Query<
    'w,
    's,
    (
        &'static Building,
        &'static Team,
        Option<&'static UnderConstruction>,
        Option<&'static mut TrainingQueue>,
    ),
>;

/// Anything that can be attacked: a live unit or building with a team.
type CmdTargets<'w, 's> = Query<
    'w,
    's,
    (
        &'static Team,
        Option<&'static Unit>,
        Option<&'static Building>,
    ),
>;

type CmdNodes<'w, 's> = Query<'w, 's, &'static ResourceNode>;

#[allow(clippy::too_many_arguments)]
fn poll_commands(
    real: Res<Time<Real>>,
    mut bridge: ResMut<Bridge>,
    mut ai_controlled: ResMut<AiControlled>,
    game_over: Res<GameOver>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    nav: Res<NavGrid>,
    mut squad_orders: ResMut<SquadOrders>,
    mut commands: Commands,
    mut casts: EventWriter<CastAbility>,
    mut buys: EventWriter<BuyItem>,
    mut item_uses: EventWriter<UseItem>,
    units: CmdUnits,
    mut buildings: CmdBuildings,
    targets: CmdTargets,
    nodes: CmdNodes,
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

        let mut errors: Vec<String> = Vec::new();
        if game_over.0.is_some() {
            errors.push("batch: game over — commands ignored".to_string());
        } else {
            apply_batch(
                &batch,
                seat.team,
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
                &units,
                &mut buildings,
                &targets,
                &nodes,
            );
        }

        seat.last_seq = batch.seq;
        seat.errors = errors;
        // Publish the result of this batch immediately instead of up to a
        // second later.
        seat.force_snapshot = true;
    }
}

/// Apply one seat's batch. `me` is that seat's team: every ownership check,
/// economy read and squad key below is taken against it, so the same code runs
/// for red and blue without either being able to touch the other's units.
#[allow(clippy::too_many_arguments)]
fn apply_batch(
    batch: &Batch,
    me: Team,
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
    units: &CmdUnits,
    buildings: &mut CmdBuildings,
    targets: &CmdTargets,
    nodes: &CmdNodes,
) {
    for (i, raw) in batch.commands.iter().enumerate() {
        let cmd: Cmd = match serde_json::from_value(raw.clone()) {
            Ok(cmd) => cmd,
            Err(err) => {
                errors.push(format!("cmd {i}: unrecognized command ({err})"));
                continue;
            }
        };
        match cmd {
            Cmd::Move { units: ids, x, z } => {
                ground_order(
                    commands,
                    errors,
                    i,
                    &ids,
                    units,
                    me,
                    Vec3::new(x, 0.0, z),
                    false,
                );
            }
            Cmd::AttackMove { units: ids, x, z } => {
                ground_order(
                    commands,
                    errors,
                    i,
                    &ids,
                    units,
                    me,
                    Vec3::new(x, 0.0, z),
                    true,
                );
            }
            Cmd::Attack { units: ids, target } => {
                let Some(target_entity) = entity_of(target) else {
                    errors.push(format!("cmd {i}: target {target} not found"));
                    continue;
                };
                match targets.get(target_entity) {
                    Ok((team, unit, building)) => {
                        // Only the seat's enemy is a legal attack target.
                        if *team != me.enemy() {
                            errors.push(format!("cmd {i}: target {target} is your own"));
                            continue;
                        }
                        if unit.is_none() && building.is_none() {
                            errors.push(format!("cmd {i}: target {target} is not attackable"));
                            continue;
                        }
                    }
                    Err(_) => {
                        errors.push(format!("cmd {i}: target {target} not found"));
                        continue;
                    }
                }
                for (entity, _) in own_units(&ids, units, me, i, errors) {
                    commands
                        .entity(entity)
                        .try_insert(Order::Attack(target_entity));
                }
            }
            Cmd::Harvest { units: ids, target } => {
                // Resource nodes are neutral: either seat may harvest any of
                // them.
                let node = match entity_of(target).filter(|e| nodes.get(*e).is_ok()) {
                    Some(node) => node,
                    None => {
                        errors.push(format!("cmd {i}: resource node {target} not found"));
                        continue;
                    }
                };
                for (entity, _) in own_units(&ids, units, me, i, errors) {
                    // Only workers can gather; anyone else would just stand there.
                    if !is_worker(units, entity) {
                        errors.push(format!(
                            "cmd {i}: unit {} is not a Worker",
                            entity.to_bits()
                        ));
                        continue;
                    }
                    commands.entity(entity).try_insert(Order::Harvest(node));
                }
            }
            Cmd::Return { units: ids } => {
                for (entity, _) in own_units(&ids, units, me, i, errors) {
                    commands.entity(entity).try_insert(Order::ReturnResources);
                }
            }
            Cmd::Follow { units: ids, target } => {
                let leader = match entity_of(target) {
                    Some(e) => match units.get(e) {
                        Ok((_, _, team, _)) if *team == me => e,
                        _ => {
                            errors.push(format!("cmd {i}: unit {target} not found/not yours"));
                            continue;
                        }
                    },
                    None => {
                        errors.push(format!("cmd {i}: unit {target} not found/not yours"));
                        continue;
                    }
                };
                for (entity, _) in own_units(&ids, units, me, i, errors) {
                    if entity == leader {
                        continue; // a unit following itself would deadlock its own order
                    }
                    commands.entity(entity).try_insert(Order::Follow(leader));
                }
            }
            Cmd::Stop { units: ids } => {
                // The established Stop: re-issue a Move to the unit's own spot,
                // which halts it and clears any attack target.
                for (entity, pos) in own_units(&ids, units, me, i, errors) {
                    commands.entity(entity).try_insert(Order::Move(pos));
                }
            }
            Cmd::Build {
                worker,
                kind,
                x,
                z,
            } => {
                let Some(building_kind) = parse_building_kind(&kind) else {
                    errors.push(format!("cmd {i}: unknown building kind '{kind}'"));
                    continue;
                };
                let Some((entity, _)) = own_unit(worker, units, me) else {
                    errors.push(format!("cmd {i}: unit {worker} not found/not yours"));
                    continue;
                };
                if !is_worker(units, entity) {
                    errors.push(format!("cmd {i}: unit {worker} is not a Worker"));
                    continue;
                }
                // Same tech gate economy.rs applies at placement — reported
                // here so the commander learns why instead of watching a
                // worker walk out and come back empty-handed.
                if let Some(err) = requirement_error(
                    i,
                    building_name(building_kind),
                    building_requires(building_kind),
                    &completed_kinds(buildings, me),
                ) {
                    errors.push(err);
                    continue;
                }
                let stats = building_stats(building_kind);
                // Snap to nav-cell boundaries exactly like the placement ghost.
                let pos = snap_footprint(clamp_to_map(Vec3::new(x, 0.0, z)), stats.size);
                if !nav.rect_is_free(pos, stats.size) {
                    errors.push(format!(
                        "cmd {i}: site ({:.1}, {:.1}) is blocked for {kind}",
                        pos.x, pos.z
                    ));
                    continue;
                }
                if !economies
                    .get(me)
                    .can_afford(stats.cost_gold, stats.cost_lumber)
                {
                    errors.push(format!(
                        "cmd {i}: cannot afford {kind} ({}g {}l)",
                        stats.cost_gold, stats.cost_lumber
                    ));
                    continue;
                }
                // economy.rs pays when the worker reaches the site, same as the UI.
                commands.entity(entity).try_insert(Order::Build {
                    kind: building_kind,
                    pos,
                });
            }
            Cmd::Train { building, unit } => {
                let Some(kind) = parse_unit_kind(&unit) else {
                    errors.push(format!("cmd {i}: unknown unit kind '{unit}'"));
                    continue;
                };
                // Read the tech state before taking the mutable borrow of the
                // producing building below.
                let completed = completed_kinds(buildings, me);
                if let Some(err) =
                    requirement_error(i, kind_name(kind), unit_requires(kind), &completed)
                {
                    errors.push(err);
                    continue;
                }
                let Some(entity) = entity_of(building) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                let Ok((b, team, under, queue)) = buildings.get_mut(entity) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                if *team != me {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                }
                if under.is_some() {
                    errors.push(format!("cmd {i}: building {building} is under construction"));
                    continue;
                }
                if !trainable(b.kind).contains(&kind) {
                    errors.push(format!(
                        "cmd {i}: {} cannot train {unit}",
                        building_name(b.kind)
                    ));
                    continue;
                }
                let Some(mut queue) = queue else {
                    errors.push(format!("cmd {i}: building {building} has no training queue"));
                    continue;
                };
                if queue.queue.len() >= MAX_QUEUE {
                    errors.push(format!("cmd {i}: training queue full ({MAX_QUEUE})"));
                    continue;
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
                        "cmd {i}: cannot afford {unit} ({cost_gold}g {cost_lumber}l)"
                    ));
                    continue;
                }
                // Gate only — economy.rs deducts when training starts.
                queue.queue.push_back(kind);
            }
            Cmd::Cancel { building, index } => {
                let Some(entity) = entity_of(building) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                let Ok((_, team, _, queue)) = buildings.get_mut(entity) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                if *team != me {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                }
                let Some(mut queue) = queue else {
                    errors.push(format!("cmd {i}: building {building} has no training queue"));
                    continue;
                };
                if index >= queue.queue.len() {
                    errors.push(format!("cmd {i}: queue index {index} out of range"));
                    continue;
                }
                queue.queue.remove(index);
                if index == 0 {
                    queue.progress = 0.0;
                }
            }
            Cmd::Rally {
                building,
                x,
                z,
                target,
            } => {
                let Some(entity) = entity_of(building) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                let Ok((b, team, _, _)) = buildings.get(entity) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                if *team != me {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                }
                if trainable(b.kind).is_empty() {
                    errors.push(format!(
                        "cmd {i}: {} produces no units",
                        building_name(b.kind)
                    ));
                    continue;
                }
                let rally = match (x, z, target) {
                    (Some(x), Some(z), _) => {
                        Some(RallyTarget::Ground(clamp_to_map(Vec3::new(x, 0.0, z))))
                    }
                    (_, _, Some(id)) => match entity_of(id) {
                        // A resource node (neutral, so either seat may name
                        // one) makes new workers start gathering; one of our
                        // own units makes new units follow it.
                        Some(e) if nodes.get(e).is_ok() => Some(RallyTarget::Node(e)),
                        Some(e) => match units.get(e) {
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
                        "cmd {i}: rally needs x/z or a valid node/own-unit target"
                    )),
                }
            }
            Cmd::Cast { hero } => {
                let Some(entity) = entity_of(hero) else {
                    errors.push(format!("cmd {i}: caster {hero} not found/not yours"));
                    continue;
                };
                // A caster is either one of our heroes (any class — the Hero
                // component and `ability_of_unit` agree on which kinds have an
                // ability) or one of our finished buildings with an ability.
                // combat.rs owns the mana/cooldown verdict either way, exactly
                // as it does for the R and C hotkeys.
                let unit_caster = matches!(
                    units.get(entity),
                    Ok((_, u, team, _)) if *team == me && ability_of_unit(u.kind).is_some()
                );
                if !unit_caster {
                    match buildings.get(entity) {
                        Ok((b, team, under, _)) if *team == me => {
                            if under.is_some() {
                                errors.push(format!(
                                    "cmd {i}: building {hero} is under construction"
                                ));
                                continue;
                            }
                            if ability_of_building(b.kind).is_none() {
                                errors.push(format!(
                                    "cmd {i}: {} has no ability",
                                    building_name(b.kind)
                                ));
                                continue;
                            }
                        }
                        _ => {
                            errors.push(format!(
                                "cmd {i}: caster {hero} is not a hero or an own ability building"
                            ));
                            continue;
                        }
                    }
                }
                casts.write(CastAbility { caster: entity });
            }
            Cmd::Buy { shop, item } => {
                let Some(item) = parse_item(&item) else {
                    errors.push(format!("cmd {i}: unknown item '{item}'"));
                    continue;
                };
                let Some(entity) = entity_of(shop) else {
                    errors.push(format!("cmd {i}: building {shop} not found/not yours"));
                    continue;
                };
                let Ok((b, team, under, _)) = buildings.get(entity) else {
                    errors.push(format!("cmd {i}: building {shop} not found/not yours"));
                    continue;
                };
                if *team != me {
                    errors.push(format!("cmd {i}: building {shop} not found/not yours"));
                    continue;
                }
                if b.kind != BuildingKind::Shop {
                    errors.push(format!(
                        "cmd {i}: {} does not sell items",
                        building_name(b.kind)
                    ));
                    continue;
                }
                if under.is_some() {
                    errors.push(format!("cmd {i}: building {shop} is under construction"));
                    continue;
                }
                // The buyer is implied: a team fields exactly one hero.
                let Some(hero) = own_hero(units, me) else {
                    errors.push(format!("cmd {i}: no living hero to buy for"));
                    continue;
                };
                // economy.rs re-validates and pays (gold, free slot, distance-
                // free just like the UI's Shop card).
                buys.write(BuyItem {
                    shop: entity,
                    hero,
                    item,
                });
            }
            Cmd::UseSlot { slot } => {
                if slot >= INVENTORY_SLOTS {
                    errors.push(format!(
                        "cmd {i}: item slot {slot} out of range (0..{})",
                        INVENTORY_SLOTS - 1
                    ));
                    continue;
                }
                let Some(hero) = own_hero(units, me) else {
                    errors.push(format!("cmd {i}: no living hero to use an item"));
                    continue;
                };
                // combat.rs checks the slot is actually filled.
                item_uses.write(UseItem { hero, slot });
            }
            Cmd::Autopilot { on } => {
                // Only ever this seat's own faction.
                set_autopilot(ai_controlled, me, on);
                info!(
                    "bridge: autopilot {} for {:?} — scripted AI {} the macro game",
                    if on { "ON" } else { "OFF" },
                    me,
                    if on { "takes over" } else { "releases" }
                );
            }
            Cmd::Surrender => {
                info!("bridge: {:?} seat surrenders", me);
                commands.send_event(Surrender { team: me });
            }
            Cmd::Priority {
                units: ids,
                classes,
            } => {
                // One bad class name invalidates the whole list rather than
                // silently installing a priority order the commander didn't ask
                // for.
                let parsed = match parse_target_classes(&classes) {
                    Ok(parsed) => parsed,
                    Err(name) => {
                        errors.push(format!("cmd {i}: unknown target class '{name}'"));
                        continue;
                    }
                };
                for (entity, _) in own_units(&ids, units, me, i, errors) {
                    let mut ec = commands.entity(entity);
                    if parsed.is_empty() {
                        ec.try_remove::<TargetPriority>();
                    } else {
                        ec.try_insert(TargetPriority(parsed.clone()));
                    }
                }
            }
            Cmd::Retreat {
                units: ids,
                below,
                x,
                z,
            } => {
                let below_frac = below.unwrap_or(0.0);
                let clear = below_frac == 0.0;
                if !clear && !(below_frac > 0.0 && below_frac < 1.0) {
                    errors.push(format!(
                        "cmd {i}: retreat 'below' must be a fraction in (0,1), got {below_frac}"
                    ));
                    continue;
                }
                let rally = match (x, z) {
                    (Some(x), Some(z)) => Some(clamp_to_map(Vec3::new(x, 0.0, z))),
                    _ => None,
                };
                if !clear && rally.is_none() {
                    errors.push(format!("cmd {i}: retreat needs a rally x/z"));
                    continue;
                }
                for (entity, _) in own_units(&ids, units, me, i, errors) {
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
            Cmd::Leash {
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
                    errors.push(format!("cmd {i}: leash needs an anchor x/z"));
                    continue;
                }
                for (entity, _) in own_units(&ids, units, me, i, errors) {
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
            Cmd::Autocast {
                units: ids,
                min_enemies,
            } => {
                let min_enemies = min_enemies.unwrap_or(0);
                for (entity, _) in own_units(&ids, units, me, i, errors) {
                    // Any hero class can auto-cast; nothing else has an ability.
                    if !matches!(units.get(entity), Ok((_, u, _, _)) if is_hero_kind(u.kind)) {
                        errors.push(format!(
                            "cmd {i}: unit {} is not a hero",
                            entity.to_bits()
                        ));
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
            Cmd::Squad { units: ids, id } => {
                for (entity, _) in own_units(&ids, units, me, i, errors) {
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
            Cmd::Posture { id, posture } => {
                // Squad ids are per-team, so red's squad 1 and blue's squad 1
                // are different squads.
                let posture = match posture {
                    None => {
                        // Clearing a posture leaves membership intact: the squad
                        // simply stops being re-tasked.
                        squad_orders.0.remove(&(me, id));
                        continue;
                    }
                    Some(PostureIn::Defend { x, z, radius }) => {
                        if !(radius > 0.0) {
                            errors.push(format!(
                                "cmd {i}: defend radius must be > 0, got {radius}"
                            ));
                            continue;
                        }
                        SquadPosture::Defend {
                            pos: clamp_to_map(Vec3::new(x, 0.0, z)),
                            radius,
                        }
                    }
                    Some(PostureIn::Push { x, z }) => SquadPosture::Push {
                        pos: clamp_to_map(Vec3::new(x, 0.0, z)),
                    },
                    Some(PostureIn::Forage { x, z }) => SquadPosture::Forage {
                        muster: clamp_to_map(Vec3::new(x, 0.0, z)),
                    },
                    Some(PostureIn::Escort { unit }) => {
                        let Some((target, _)) = own_unit(unit, units, me) else {
                            errors
                                .push(format!("cmd {i}: unit {unit} not found/not yours"));
                            continue;
                        };
                        SquadPosture::Escort { unit: target }
                    }
                };
                squad_orders.0.insert((me, id), posture);
            }
            Cmd::Template {
                building,
                squad,
                retreat,
                priority,
                autocast,
            } => {
                // Only our own, finished, unit-producing buildings can carry a
                // template — anywhere else it would never be read.
                let Some(entity) = entity_of(building) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                let Ok((b, team, under, queue)) = buildings.get(entity) else {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                };
                if *team != me {
                    errors.push(format!("cmd {i}: building {building} not found/not yours"));
                    continue;
                }
                if under.is_some() {
                    errors.push(format!("cmd {i}: building {building} is under construction"));
                    continue;
                }
                if queue.is_none() {
                    errors.push(format!(
                        "cmd {i}: {} has no training queue",
                        building_name(b.kind)
                    ));
                    continue;
                }
                // Same class parsing (and same all-or-nothing rule) as the
                // `priority` command; an empty list means "no priority piece".
                let priority = match priority {
                    Some(names) => match parse_target_classes(&names) {
                        Ok(parsed) => (!parsed.is_empty()).then_some(parsed),
                        Err(name) => {
                            errors.push(format!("cmd {i}: unknown target class '{name}'"));
                            continue;
                        }
                    },
                    None => None,
                };
                let retreat = match retreat {
                    Some(r) => {
                        if !(r.below > 0.0 && r.below < 1.0) {
                            errors.push(format!(
                                "cmd {i}: template retreat 'below' must be a fraction in (0,1), \
                                 got {}",
                                r.below
                            ));
                            continue;
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Ids on the wire are `Entity::to_bits`; invalid bit patterns resolve to None
/// instead of panicking.
fn entity_of(id: u64) -> Option<Entity> {
    Entity::try_from_bits(id).ok()
}

/// Resolve one id to a living unit of the seat's own team.
fn own_unit(id: u64, units: &CmdUnits, me: Team) -> Option<(Entity, Vec3)> {
    let entity = entity_of(id)?;
    match units.get(entity) {
        Ok((_, _, team, tf)) if *team == me => Some((entity, tf.translation)),
        _ => None,
    }
}

/// The seat's living hero, whichever class it plays. `buy` and `use_item` name
/// no unit: a team has at most one hero, so there is nothing to disambiguate.
fn own_hero(units: &CmdUnits, me: Team) -> Option<Entity> {
    units
        .iter()
        .find(|(_, u, team, _)| **team == me && is_hero_kind(u.kind))
        .map(|(entity, ..)| entity)
}

/// Resolve a list of ids to living units of the seat's own team, recording one
/// error per id that doesn't qualify (an enemy's unit included).
fn own_units(
    ids: &[u64],
    units: &CmdUnits,
    me: Team,
    index: usize,
    errors: &mut Vec<String>,
) -> Vec<(Entity, Vec3)> {
    if ids.is_empty() {
        errors.push(format!("cmd {index}: no units given"));
        return Vec::new();
    }
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        match own_unit(id, units, me) {
            Some(found) => out.push(found),
            None => errors.push(format!("cmd {index}: unit {id} not found/not yours")),
        }
    }
    out
}

/// The seat's completed (not under construction) buildings — the input to
/// every requirement check on the command path.
fn completed_kinds(buildings: &CmdBuildings, me: Team) -> Vec<BuildingKind> {
    buildings
        .iter()
        .filter(|(_, team, under, _)| **team == me && under.is_none())
        .map(|(building, ..)| building.kind)
        .collect()
}

fn is_worker(units: &CmdUnits, entity: Entity) -> bool {
    matches!(units.get(entity), Ok((_, u, _, _)) if u.kind == UnitKind::Worker)
}

/// Move / AttackMove for a group, spread over the UI's formation grid.
#[allow(clippy::too_many_arguments)]
fn ground_order(
    commands: &mut Commands,
    errors: &mut Vec<String>,
    index: usize,
    ids: &[u64],
    units: &CmdUnits,
    me: Team,
    ground: Vec3,
    attack_move: bool,
) {
    let group = own_units(ids, units, me, index, errors);
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
fn formation_offset(index: usize, count: usize) -> Vec3 {
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

fn clamp_to_map(p: Vec3) -> Vec3 {
    Vec3::new(
        p.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
        0.0,
        p.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
    )
}

/// Snap a footprint centre so its edges land on nav-cell boundaries.
fn snap_footprint(p: Vec3, size: f32) -> Vec3 {
    let half = size * 0.5;
    Vec3::new(
        ((p.x - half) / CELL).round() * CELL + half,
        0.0,
        ((p.z - half) / CELL).round() * CELL + half,
    )
}

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

/// Parse a whole class list, all-or-nothing: `Err(name)` names the first
/// unknown class so the caller can reject the command outright rather than
/// install a focus-fire order nobody asked for.
fn parse_target_classes(names: &[String]) -> Result<Vec<TargetClass>, String> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        match parse_target_class(name) {
            Some(class) => out.push(class),
            None => return Err(name.clone()),
        }
    }
    Ok(out)
}

fn parse_target_class(name: &str) -> Option<TargetClass> {
    ALL_TARGET_CLASSES
        .iter()
        .copied()
        .find(|c| c.name().eq_ignore_ascii_case(name))
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

/// Loose form of a name on the wire: case, spaces, dashes and underscores are
/// all noise, so `"town_hall"`, `"Town Hall"` and `"townhall"` are one name.
fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Both parsers match against the catalog's own ids (`shared::kind_name` /
/// `building_name`), so a kind added to the shared enums is orderable through
/// the bridge the moment it exists — no table here to fall out of date.
fn parse_unit_kind(name: &str) -> Option<UnitKind> {
    let wanted = normalize_name(name);
    ALL_UNIT_KINDS
        .into_iter()
        .find(|k| normalize_name(kind_name(*k)) == wanted)
}

fn parse_building_kind(name: &str) -> Option<BuildingKind> {
    let wanted = normalize_name(name);
    ALL_BUILDING_KINDS
        .into_iter()
        .find(|k| normalize_name(building_name(*k)) == wanted)
}

/// Items parse off the catalog's own ids too (`item_def(..).name`), so
/// `"town_portal"`, `"Town Portal"` and `"TownPortal"` are one item.
fn parse_item(name: &str) -> Option<ItemId> {
    let wanted = normalize_name(name);
    ALL_ITEMS
        .into_iter()
        .find(|id| normalize_name(item_def(*id).name) == wanted)
}
