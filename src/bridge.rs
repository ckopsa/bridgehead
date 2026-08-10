//! bridge.rs — the "live bridge": a file-based control channel that lets one or
//! two external agents (Claude in a terminal) play a faction, either against the
//! human at the keyboard or against each other.
//!
//! Activation is opt-in through `WC3_BRIDGE` (case-insensitive). Each accepted
//! value opens one *seat* per faction it names:
//!   * `1` / `red` / `claude` — the Claude (red) faction, in `bridge/red/`,
//!   * `blue` / `human` — the Human (blue) faction, in `bridge/blue/`,
//!   * `both` / `2` — both seats at once, so two commanders can fight,
//!   * `copilot` / `co` — CO-COMMAND: one seat on `Team::Human`, in
//!     `bridge/copilot/`, *beside* the player at the keyboard rather than
//!     opposite them. Transport is identical; what differs is that its
//!     non-doctrine commands are proposals until the human approves them, and
//!     that it does not displace anybody. See copilot.rs and
//!     docs/INTENT.md § co-command.
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
//! applies it — the very same compiler the human's mouse gestures go through,
//! with the same ownership checks, the same fog rule and the same error
//! strings. That is stronger than the old promise that the bridge "acts only
//! through the primitives the UI uses", because it is no longer a promise:
//! there is one implementation, so the two seats cannot drift apart.
//! See docs/INTENT.md.
//!
//! The wire format did not change, because the wire format *is* the schema:
//! `Intent`'s serde shape is the historical command shape, field for field.
//!
//! Enemy gold/lumber is never reported, and a seat only ever sees its *own*
//! squads and policies, never the opponent's command structure.
//!
//! FOG OF WAR. Every seat's snapshot is filtered through that seat's team's
//! `shared::FogGrid` — the same grid ui.rs paints for the player at the
//! keyboard. The bridge does not decide what is knowable; it renders a
//! decision made once in shared.rs. Concretely:
//!
//!   * enemy `units` appear only while currently visible. They are not
//!     remembered: an army that walks out of sight is simply gone from the
//!     snapshot, because a remembered army is a lie a commander would act on.
//!   * enemy `buildings` appear as themselves while visible, and afterwards as
//!     REMEMBERED GHOSTS carrying a `last_seen` game-time stamp and the hp/
//!     queue state observed at that moment. A ghost can be stale — a razed
//!     barracks keeps its ghost until somebody looks at the spot again. Own
//!     buildings never carry `last_seen`, so the field's presence is exactly
//!     the "this is memory, not observation" flag.
//!   * `bounties` only while visible (see below).
//!   * `mines`, `trees_near`, `map` are unfiltered map GEOGRAPHY (see below).
//!
//! `WC3_FOG=0` restores the old omniscient snapshot with no other change.
//! The top-level `fog` object reports which mode is in force plus this seat's
//! explored/visible fraction of the map.
//!
//! The other half of the fog rule — refusing an `attack` on a target the seat
//! cannot see or remember, with the error `cmd N: target X is not visible` —
//! now lives in intent.rs, because it binds *whoever is speaking* rather than
//! this file's seat. The behaviour and the string are unchanged.
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
//! Tech tiers are the `upgrade` command: `{"type":"upgrade","building":<id>}`
//! converts one of our own finished buildings into the next rung of its ladder
//! in place (`catalog.buildings[].upgrades_to` says which, at what price, for
//! how long). It is validated like `build` and `train` — ours, finished, has a
//! next tier, not already converting, affordable — and paid in full the moment
//! it is accepted, because no worker has to walk anywhere first. The building
//! keeps its id, position, footprint, rally, template and training queue; what
//! it loses is production TIME, since a converting building trains nothing.
//! The snapshot answers back with `buildings[].tier`, `buildings[].upgrading`
//! (`{to, remaining}`) and the seat's headline `me.tier`. Requirements are
//! compared by tier, not by kind, so a Castle satisfies anything that asks for
//! a Keep.
//!
//! The catalog is static, so tech *availability* rides along with every
//! snapshot instead: a top-level `unlocked` map answers "may I build/train this
//! right now?" for every catalog entry, computed from the seat's own completed
//! buildings. The same check gates the `build` and `train` commands, so a
//! commander that respects `unlocked` never has an order bounced by economy.rs.
//! For a unit that means BOTH halves of "right now" — the tech gates met and a
//! finished building of ours that trains it standing somewhere. The map used to
//! report only the first half, so a team with no Barracks read `Footman: true`;
//! the honest answer, and the one that stops a `train` bouncing, is no. Planning
//! ahead is still the catalog's job: `units[].requires` lists the whole chain,
//! trainer included, and does not care what you own yet.
//!
//! Abilities and items are described by the catalog (`abilities`, `items`) and
//! driven by three commands: `cast` takes any caster — a hero of either class
//! or one of our own finished ability buildings (the TownHall's Call to Arms)
//! — while `buy` (at an own Shop) and `use_item` (slot 0 or 1) need no unit id
//! at all, because a team fields exactly one hero and only heroes carry an
//! inventory. The snapshot answers back with `units[].items`,
//! `units[].militia` and `buildings[].ability_cd`.
//!
//! MAP GEOGRAPHY IS PUBLIC, and always was: the `map` block (layout, summary,
//! chokepoints), `mines` (position and remaining gold) and `trees_near` ship
//! unfiltered to both seats, exactly as ui.rs paints mines and the terrain
//! barrier on the minimap from the first frame. Fog hides what the opponent is
//! DOING, not where the map's furniture sits. Mine `remaining` is the one
//! deliberate concession: it is the shared clock the whole economy is timed
//! against (expansion windows, "mines run dry"), both `plan_expansion` and a
//! commander budget against it, and scouting reveals it anyway — hiding it
//! would buy a little intel and cost the design principle it serves.
//!
//! Bounty caches (bounty.rs) ride along as a top-level `bounties` array —
//! `pos`, `gold` and `expires_in` — but only while the seat's team can SEE
//! them. A cache is treasure on open ground, not geography, and open ground
//! nobody is looking at tells you nothing. This is a real gameplay change:
//! a Forage squad now hunts only what its team has eyes on. The event feed
//! reports them
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

use crate::command::{CommandLink, PendingOrder};
use crate::copilot::{Copilot, CopilotWire, Proposal};
use crate::intent::{set_autopilot, IntentApply};
use crate::shared::*;
use bevy::ecs::system::SystemParam;
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

/// `poll_commands`, as a set copilot.rs can order after: a co-commander's
/// negotiation layer sits between reading the wire and compiling it, and needs
/// to be in the same frame as both.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BridgePoll;

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Bridge>()
            .add_systems(Startup, bridge_startup)
            .add_systems(
                Update,
                // Poll, compile, snapshot — in that order, so a batch read
                // this frame is applied this frame and its validation errors
                // ride out in the snapshot written the same frame. The middle
                // step belongs to intent.rs now; the bridge only brackets it.
                // After `FogSet`: both the snapshot a seat reads and the orders
                // it may issue are filtered through this frame's fog, never
                // the previous frame's. (`IntentApply` is itself `.after(FogSet)`,
                // so the compiler judges visibility against the same grid.)
                (
                    poll_commands.in_set(BridgePoll).before(IntentApply),
                    write_snapshot.after(IntentApply),
                )
                    .after(FogSet)
                    .run_if(bridge_enabled),
            );
    }
}

/// What a seat *is* to its faction. Transport is identical either way; this
/// decides the directory name, whether the scripted AI is displaced, and
/// whether commands go through the negotiation layer in copilot.rs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeatRole {
    /// The seat IS the faction: it replaces the scripted macro AI and is the
    /// only author on its side. The historical behaviour of every seat.
    Commander,
    /// The seat is a SECOND author on a faction the human is playing. It does
    /// not displace anybody, and its non-doctrine commands are proposals until
    /// the human approves them. See copilot.rs.
    Copilot,
}

/// One external commander's channel: its faction, its directory, and all of the
/// protocol state that must never be shared with another seat.
struct Seat {
    /// The faction this seat commands. Every "own"/"me"/"enemy" decision in
    /// this file is taken against it.
    team: Team,
    role: SeatRole,
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
    fn new(team: Team, role: SeatRole) -> Self {
        let dir = PathBuf::from(BRIDGE_DIR).join(seat_dir(team, role));
        Seat {
            team,
            role,
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
/// the same colours the game window uses. A co-commander gets its own name
/// rather than its team's colour, because it is not the faction: `bridge/blue`
/// is "whoever is playing blue", and in copilot mode that is the human.
fn seat_dir(team: Team, role: SeatRole) -> &'static str {
    match (role, team) {
        (SeatRole::Copilot, _) => "copilot",
        (SeatRole::Commander, Team::Claude) => "red",
        (SeatRole::Commander, Team::Human) => "blue",
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

/// Which factions `WC3_BRIDGE` asks for, and in what role. `None` means "leave
/// the bridge off".
fn seats_from_env(raw: &str) -> Option<Vec<(Team, SeatRole)>> {
    use SeatRole::{Commander, Copilot};
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" => None,
        "1" | "red" | "claude" => Some(vec![(Team::Claude, Commander)]),
        "blue" | "human" => Some(vec![(Team::Human, Commander)]),
        "both" | "2" => Some(vec![(Team::Claude, Commander), (Team::Human, Commander)]),
        // CO-COMMAND. One seat, on the HUMAN's team, alongside the player at
        // the keyboard rather than opposite them. Deliberately its own value
        // instead of a modifier on `blue`: "who else is playing this faction"
        // and "which faction is this" are different questions, and a mode that
        // changes what approval a command needs should be impossible to enter
        // by accident.
        "copilot" | "co" => Some(vec![(Team::Human, Copilot)]),
        // Anything else truthy keeps the historical "any value enables it"
        // behaviour rather than silently starting no bridge at all.
        other => {
            warn!("{BRIDGE_ENV}: unrecognized value '{other}' — assuming 'red'");
            Some(vec![(Team::Claude, Commander)])
        }
    }
}

/// Opt in from the environment, prepare each seat's directory, and take the
/// seated factions off the scripted macro AI.
fn bridge_startup(
    mut bridge: ResMut<Bridge>,
    mut ai_controlled: ResMut<AiControlled>,
    mut external: ResMut<ExternallyCommanded>,
    mut copilot: ResMut<Copilot>,
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

    for (team, role) in teams {
        let seat = Seat::new(team, role);
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
        match role {
            SeatRole::Commander => {
                // The external commander replaces the scripted macro AI on
                // *its* side only; the other faction keeps whatever ai.rs
                // decided.
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
                    seat_dir(team, role),
                    team,
                    seat.state_file.display(),
                    seat.commands_file.display()
                );
            }
            SeatRole::Copilot => {
                // Deliberately NEITHER of the two lines above.
                //
                // `ExternallyCommanded` is what tells doctrine.rs "a machine
                // is driving this team, so pool its idle units into squad 0
                // and seed them a posture" — an autonomy floor that exists to
                // compensate for a slow commander. There is no slow commander
                // here: there is a human with a mouse, who keeps full
                // authority over where their idle units stand. Setting the
                // flag would have the engine start quietly enrolling the
                // player's army the moment a co-commander connected, which is
                // the opposite of asking permission.
                //
                // Nor is autopilot touched: `Team::Human` is not AI-controlled
                // by default, and if the player *has* handed their faction to
                // the scripted AI, a co-commander connecting is no reason to
                // take it back for them.
                copilot.seat(team);
                info!(
                    "{BRIDGE_ENV}: CO-COMMAND seat active — {:?} is played by the human \
                     at the keyboard WITH a co-commander (trust: {}, snapshot {}, \
                     commands {})",
                    team,
                    copilot.policy.name(),
                    seat.state_file.display(),
                    seat.commands_file.display()
                );
            }
        }
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
    /// What this seat can currently know. Read it before concluding anything
    /// from an empty `units` list.
    fog: FogOut,
    /// `catalog.json` entry id -> may this seat build/train it right now?
    /// Every unit and building in the catalog appears, whether or not it has
    /// requirements, so a commander can gate its build order on one lookup.
    unlocked: BTreeMap<&'static str, bool>,
    units: Vec<UnitOut>,
    buildings: Vec<BuildingOut>,
    squads: Vec<SquadOut>,
    mines: Vec<MineOut>,
    trees_near: Vec<TreeOut>,
    /// Bounty caches this seat can currently SEE. Treasure on the ground is
    /// public information to whoever is looking at that ground.
    bounties: Vec<BountyOut>,
    /// `[[game_time, message], ...]`, oldest first — see `diff_events`.
    events: Vec<(f32, String)>,
    /// docs/TEMPO.md §3 — your own command nodes: the finished halls and the
    /// living hero your orders radiate from. Orders to units inside one of
    /// these circles arrive instantly; everything else pays for the distance.
    /// Own team only, symmetric with what the HUD shows the human — and the
    /// enemy's chain of command is something you learn by razing it, not by
    /// reading it. Absent entirely when `WC3_COMMAND_LATENCY` is off.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command_nodes: Vec<CommandNodeOut>,

    // --- co-command (copilot seats only) ---------------------------------
    //
    // All three are `Option` and skipped when absent, so an ordinary seat's
    // snapshot keeps exactly the 16 keys it has always had, byte-shape
    // identical. A copilot seat gets them always — including as empty lists,
    // so the shape a co-commander parses does not change the moment its first
    // proposal lands.
    /// This seat's own etiquette, read out of the game rather than out of the
    /// environment: which verbs it may send directly, how long a proposal
    /// lives, how many may be outstanding.
    #[serde(skip_serializing_if = "Option::is_none")]
    copilot: Option<CopilotOut>,
    /// Directives this seat has proposed that the human has not answered yet,
    /// oldest first. A proposal leaves this list by being approved, vetoed or
    /// lapsing — and all three outcomes are announced in `events`.
    #[serde(skip_serializing_if = "Option::is_none")]
    proposals: Option<Vec<ProposalOut>>,
    /// **The other half of legibility.** The recent intents of this seat's
    /// team, oldest first, each tagged with which author spelled it — so the
    /// ones tagged `"ui"` are the human's own, in the same English the replay
    /// log will show. Without this a co-commander would be the only one of the
    /// two partners who cannot see what the other just did.
    #[serde(skip_serializing_if = "Option::is_none")]
    partner_log: Option<Vec<JournalOut>>,
}

/// The co-command contract, as this seat sees it.
#[derive(Serialize)]
struct CopilotOut {
    /// `"split"` (default), `"full"` or `"strict"` — `WC3_COPILOT_TRUST`.
    trust: &'static str,
    /// Verbs this seat may send WITHOUT a `propose` wrapper. `["*"]` under
    /// full trust, empty under strict. Anything not here is refused with an
    /// error that shows the wrapper.
    direct: Vec<&'static str>,
    /// Game seconds an unanswered proposal survives.
    propose_ttl: f32,
    /// How many proposals may be outstanding before new ones are refused.
    max_pending: usize,
}

/// One directive waiting on the human.
#[derive(Serialize)]
struct ProposalOut {
    id: u32,
    /// What this seat said it was for.
    note: String,
    /// The compiled English of each command — the same `Intent::sentence()`
    /// the human is reading off the HUD and the log will write afterwards.
    sentences: Vec<String>,
    /// What the human was told this would disturb: their squads, their recent
    /// orders. Empty when it steps on nothing.
    conflicts: Vec<String>,
    /// Game seconds until it lapses unanswered.
    expires_in: f32,
}

/// One remembered intent of this team, whoever wrote it.
#[derive(Serialize)]
struct JournalOut {
    t: f32,
    /// `"ui"` (the human at the keyboard), `"copilot"` (this seat), or
    /// `"bridge"`. The same tag `units[].why` carries.
    source: &'static str,
    verb: &'static str,
    sentence: String,
    /// False when the compiler refused some or all of it.
    ok: bool,
}

/// One command node, as the commander sees it.
#[derive(Serialize)]
struct CommandNodeOut {
    pos: [f32; 2],
    /// Orders to a unit within this many world units of `pos` are free.
    radius: f32,
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
    /// Highest hall tier you have STANDING and finished: 1 TownHall, 2 Keep,
    /// 3 Castle. The one number tier-gated content is written against; the
    /// per-building `tier` field says where each hall individually sits.
    tier: u32,
    /// How many heroes you may field at once, and how many of those slots are
    /// spoken for right now (living heroes + heroes sitting in any of your
    /// training queues). Slots come from `tier`: **1 at TownHall, 2 at Keep,
    /// 3 at Castle** — teching up is how you get a second hero. Heroes must be
    /// of DISTINCT classes, so `hero_records` below tells you which classes
    /// are already taken; with only two classes shipping today, a Castle's
    /// third slot has nothing to put in it yet.
    hero_slots: u32,
    hero_slots_used: u32,
    /// One entry per hero CLASS you have ever fielded, whether or not that
    /// hero is currently alive. `alive` false means it is dead and revivable
    /// at `cost` (cheaper, and it keeps `level`).
    hero_records: Vec<HeroRecordOut>,
    /// What each hero class would cost you to put in a queue RIGHT NOW: full
    /// price for a class you have never fielded, revival price for one you
    /// have. Every class is listed, including ones your slots have no room
    /// for — `hero_slots_used` vs `hero_slots` is the gate, not this.
    hero_costs: Vec<HeroCostOut>,
    /// Team-wide research: one entry per ladder in `catalog.research`, always
    /// present and always both ladders, so a commander can read a level off a
    /// fixed shape rather than testing whether a key exists. Levels are yours
    /// alone — the opponent's research is never reported, and the only way to
    /// learn it is to notice your units dying faster.
    research: Vec<ResearchOut>,
}

/// One research ladder's state for the seat that owns it.
#[derive(Serialize)]
struct ResearchOut {
    /// Ladder id — the `upgrade` field of a `research` command.
    id: &'static str,
    name: &'static str,
    /// Levels completed, 0..=`max_level`.
    level: u32,
    max_level: u32,
    /// The flat bonus currently in force. Attack adds it to every unit attack;
    /// armor subtracts it from every hit a unit takes.
    bonus: f32,
    /// Cost and duration of the next rung, or null at the cap. Absent is the
    /// honest way to say "there is nothing left to buy here".
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<ResearchStepOut>,
    /// Present while one of your forges is working on this ladder.
    #[serde(skip_serializing_if = "Option::is_none")]
    in_progress: Option<ResearchProgressOut>,
}

#[derive(Serialize)]
struct ResearchStepOut {
    level: u32,
    cost_gold: u32,
    cost_lumber: u32,
    research_time: f32,
}

/// A research job running, as it appears in a snapshot.
#[derive(Serialize)]
struct ResearchProgressOut {
    /// The level this will produce when it finishes.
    level: u32,
    /// Seconds left.
    remaining: f32,
    /// Which of your Blacksmiths is doing the work — the same id the
    /// `research` command names, so a commander can tell two forges apart.
    building: u64,
}

#[derive(Serialize)]
struct HeroRecordOut {
    kind: &'static str,
    level: u32,
    xp: f32,
    /// Is this hero standing on the map right now?
    alive: bool,
}

#[derive(Serialize)]
struct HeroCostOut {
    kind: &'static str,
    gold: u32,
    lumber: u32,
    time: f32,
    /// True when this is the (cheaper) revival price of a hero you already
    /// opened a record for.
    revive: bool,
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
    /// Own units only: this unit's answer to "why are you doing that?" —
    /// `"order:move by bridge t=123"`, `"posture:push sq1"`,
    /// `"template:Barracks#4294968163"`, `"policy:retreat t=210"`, `"idle"`.
    /// Always present for your own units; never for the enemy's, because
    /// reading an opponent's chain of command is reading their plan.
    ///
    /// The human's selection panel prints this same string for the same unit.
    /// That is the point: introspection is part of the decision surface, so it
    /// has to be equitable too, or one seat gets to ask a question the other
    /// cannot. Join it against `intent_log.jsonl`'s `why` to find the sentence
    /// that caused it.
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
    /// Own units only, and only when at least one policy is set — the common
    /// case is "no doctrine", and an empty object per unit is pure noise.
    #[serde(skip_serializing_if = "Option::is_none")]
    policies: Option<PoliciesOut>,
    /// Own casters only: every ability this unit HAS, with its slot index, its
    /// own cooldown and whether it is unlocked yet. This is what a `cast`
    /// command's optional `ability` field selects from. Absent for units with
    /// no abilities and for the opponent's army.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    abilities: Vec<AbilityOut>,
    /// docs/TEMPO.md §3 — Chain of Command. Own units only: seconds a direct
    /// order to THIS unit would take to arrive, given where it is standing
    /// relative to your nearest hall or your hero. `0.0` means it is inside a
    /// command node's radius and your hands reach it instantly. Absent
    /// entirely when `WC3_COMMAND_LATENCY` is off, which is also when it is
    /// meaningless.
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<f32>,
    /// An order you already gave this unit is still in transit. Absent
    /// (rather than `false`) the rest of the time. Re-ordering a unit that is
    /// `pending` replaces what was travelling — it does not queue behind it.
    #[serde(skip_serializing_if = "is_false")]
    pending: bool,
}

/// One ability slot of one caster, as the commander sees it. The catalog says
/// what an ability DOES; this says whether this caster can use it right now.
#[derive(Serialize)]
struct AbilityOut {
    /// `AbilityDef::name` — accepted as the `ability` field of a `cast`.
    id: &'static str,
    /// Slot index — also accepted as the `ability` field of a `cast`.
    index: usize,
    /// Seconds until this slot may be cast again (0 = ready).
    cd: f32,
    /// Has this caster met the unlock condition?
    unlocked: bool,
    /// Unlocked, off cooldown, and affordable (heroes pay mana).
    ready: bool,
    mana_cost: f32,
    /// The unlock condition, verbatim, when it is not yet met.
    #[serde(skip_serializing_if = "Option::is_none")]
    requires: Option<String>,
}

/// Build the per-caster ability view. One function for heroes and buildings —
/// the readiness rule is shared.rs's, so the snapshot can never promise a cast
/// combat.rs would refuse.
fn abilities_out(
    list: &'static [AbilityDef],
    ctx: UnlockCtx,
    hero: Option<&Hero>,
    cooldowns: Option<&AbilityCooldowns>,
) -> Vec<AbilityOut> {
    list.iter()
        .enumerate()
        .map(|(index, def)| {
            let unlocked = ability_unlocked(def, ctx);
            AbilityOut {
                id: def.name,
                index,
                cd: r1(cooldowns.map_or(0.0, |c| c.remaining(index))),
                unlocked,
                ready: unlocked && ability_ready(def, hero, cooldowns, index),
                mana_cost: r1(def.mana_cost),
                requires: (!unlocked).then(|| unlock_label(def.unlock)),
            }
        })
        .collect()
}

/// One rung of a Shop's shelf, as the commander sees it. `locked` is the same
/// verdict economy.rs will reach, so the snapshot can never advertise a
/// purchase the buy handler would refuse.
#[derive(Serialize)]
struct ShopItemOut {
    /// Accepted verbatim as the `item` field of a `buy`.
    id: &'static str,
    cost_gold: u32,
    /// Team tech tier this rung needs: 1, 2 or 3.
    tier: u32,
    /// Our tier is below `tier` — climb the hall ladder first.
    locked: bool,
}

/// The shelf a Shop shows a team at `tier`. Walks `ALL_ITEMS`, so a content
/// bead that adds an item adds it to the bridge by adding the table row.
fn shop_shelf(tier: TechTier) -> Vec<ShopItemOut> {
    ALL_ITEMS
        .iter()
        .map(|&id| {
            let def = item_def(id);
            ShopItemOut {
                id: def.name,
                cost_gold: def.cost_gold,
                tier: def.tier.level(),
                locked: !item_unlocked(id, tier),
            }
        })
        .collect()
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
    /// Cooldown of the hero's FIRST ability. Kept as a scalar for readers
    /// written against the one-ability world; `UnitOut::abilities` carries the
    /// full per-slot picture.
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
    /// Own buildings with an active ability only: seconds until the FIRST one
    /// may be cast again (0 = ready). Absent for buildings that have no
    /// ability. `abilities` below is the per-slot version.
    #[serde(skip_serializing_if = "Option::is_none")]
    ability_cd: Option<f32>,
    /// Own ability buildings only: every slot, with unlock and cooldown state.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    abilities: Vec<AbilityOut>,
    /// Own completed SHOPS only: the shelf, with this team's tier already
    /// applied. The catalog says what each item costs and does; this says
    /// which rungs of it a `buy` would actually be allowed to take right now.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    sells: Vec<ShopItemOut>,
    /// Own production buildings only, and only when a `template` command has
    /// installed a `DoctrineTemplate`: a flag, not the contents — the
    /// commander that set the template already knows what is in it.
    #[serde(skip_serializing_if = "is_false")]
    template: bool,
    /// Rung on this building's upgrade ladder — 1 for everything not on one.
    /// Not secret: a Keep looks like a Keep to anyone LOOKING at it. Under fog
    /// that qualifier does the work — an unseen building is not in this array
    /// at all, and a remembered one reports the tier it wore when last
    /// observed, which is exactly what a scout brings home.
    tier: u32,
    /// Present only while this building is converting to its next tier. The
    /// `upgrade` command starts one; while it runs the building trains nothing
    /// (the queue is frozen, not cancelled). Never set on a remembered ghost:
    /// a conversion is a live thing, and a stale progress bar would be
    /// invented intelligence rather than preserved intelligence.
    #[serde(skip_serializing_if = "Option::is_none")]
    upgrading: Option<UpgradeOut>,
    /// Present only on YOUR OWN forge while it is working. Never on an enemy
    /// building you can see, for the same reason `queue` is not: what a rival
    /// is spending its lumber on is exactly the intelligence that scouting is
    /// supposed to be unable to give you. You learn their attack upgrade
    /// landed by losing a fight, not by looking at their base.
    #[serde(skip_serializing_if = "Option::is_none")]
    researching: Option<ResearchJobOut>,
    /// Present ONLY on remembered enemy structures: the game time at which
    /// this seat last actually saw it. Everything else in the record is the
    /// state observed at that moment and may since have changed — including
    /// the building having upgraded to a higher tier, or been destroyed.
    /// Absent means "observed right now".
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen: Option<f32>,
}

/// A research job on one of your own forges.
#[derive(Serialize)]
struct ResearchJobOut {
    /// Ladder id — what a `research` command would name.
    upgrade: &'static str,
    /// The level this will produce.
    level: u32,
    /// Seconds left.
    remaining: f32,
}

/// An in-place upgrade in progress, as it appears in a snapshot.
#[derive(Serialize)]
struct UpgradeOut {
    /// Catalog id of what it is becoming.
    to: &'static str,
    /// Seconds left on the conversion.
    remaining: f32,
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// A live bounty cache. Neutral but not free: the treasure belongs to nobody,
/// and seeing the glow on the ground still costs you eyes on that ground. A
/// cache is listed only while this seat's team can see it, exactly as the
/// player at the keyboard sees a glow only outside the fog.
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

/// How much of the map this seat has ever seen / can see right now, plus the
/// mode in force. Not a gameplay input — a legibility one, so a commander (and
/// an after-action report) can tell "I have no information" apart from "there
/// is nothing there", which are the same empty `units` array otherwise.
#[derive(Serialize)]
struct FogOut {
    enabled: bool,
    explored: f32,
    visible: f32,
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
            Option<&'static Provenance>,
        ),
        // Hero kit, Call-to-Arms state and per-ability cooldowns, nested for
        // the same reason.
        (
            Option<&'static Inventory>,
            Has<Militia>,
            Option<&'static AbilityCooldowns>,
            // docs/TEMPO.md §3: an order this unit has been given that has not
            // reached it yet. Always `None` with WC3_COMMAND_LATENCY off.
            Option<&'static PendingOrder>,
        ),
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
        Option<&'static AbilityCooldowns>,
        Option<&'static Upgrading>,
        Option<&'static Researching>,
    ),
>;

type SnapshotNodes<'w, 's> =
    Query<'w, 's, (Entity, &'static ResourceNode, &'static Transform)>;

type SnapshotBounties<'w, 's> =
    Query<'w, 's, (Entity, &'static Bounty, &'static Transform)>;

/// The neutral furniture of the map: resource nodes and bounty caches. Bundled
/// for the same reason `TeamTech` is — `write_snapshot` sits exactly on Bevy's
/// 16-parameter ceiling, and Chain of Command needed one of those slots.
#[derive(SystemParam)]
struct SnapshotNeutrals<'w, 's> {
    nodes: SnapshotNodes<'w, 's>,
    bounties: SnapshotBounties<'w, 's>,
}

#[allow(clippy::too_many_arguments)]
/// Per-team tech state the snapshot reports: how far up the hall ladder a team
/// has climbed and how far up its two research ladders. Bundled because
/// `write_snapshot` sits exactly on Bevy's 16-parameter ceiling and these two
/// resources answer the same question from different tables. Both are `Copy`,
/// so `write_seat_snapshot` takes them by value and needs no bundle of its own.
#[derive(SystemParam)]
struct TeamTech<'w> {
    tiers: Res<'w, TechTiers>,
    research: Res<'w, TeamResearch>,
}

/// The co-command side-channel: the pending proposal queue and the team's
/// recent intent history. Bundled for the same reason `TeamTech` is — this
/// system sits on Bevy's 16-parameter ceiling, and these two answer the one
/// question ("what has been said on this team, and what is still being asked")
/// that only a copilot seat's snapshot carries.
#[derive(SystemParam)]
struct CoCommand<'w> {
    copilot: Res<'w, Copilot>,
    journal: Res<'w, IntentJournal>,
}

fn write_snapshot(
    time: Res<Time>,
    real: Res<Time<Real>>,
    mut bridge: ResMut<Bridge>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    game_over: Res<GameOver>,
    squad_orders: Res<SquadOrders>,
    tech: TeamTech,
    feed: Res<GameEvents>,
    fog: Res<FogGrids>,
    intent_errors: Res<IntentErrors>,
    co: CoCommand,
    units: SnapshotUnits,
    buildings: SnapshotBuildings,
    neutrals: SnapshotNeutrals,
    // docs/TEMPO.md §3/§4: the seat's own command nodes, and the curve that
    // says what an order to a given unit would cost. An information right —
    // reported to a commander for exactly the same reason the HUD draws it for
    // the human, and never for the opponent's team.
    link: CommandLink,
) {
    let now = r1(time.elapsed_secs());
    let delta = real.delta();
    for seat in &mut bridge.seats {
        let due = seat.snapshot_timer.tick(delta).just_finished();
        if !due && !seat.force_snapshot {
            continue;
        }
        seat.force_snapshot = false;
        // The seat's own team's fog — the whole point of a per-seat snapshot.
        let seat_fog = fog.get(seat.team);
        // The co-command block, for the one role that has one. An ordinary
        // seat is handed `None` and serializes exactly what it always did.
        let co_out = (seat.role == SeatRole::Copilot).then(|| {
            (
                CopilotOut {
                    trust: co.copilot.policy.name(),
                    direct: crate::copilot::direct_verbs(co.copilot.policy),
                    propose_ttl: crate::copilot::PROPOSAL_TTL,
                    max_pending: crate::copilot::MAX_PENDING,
                },
                proposals_out(&co.copilot.pending, now),
                journal_out(co.journal.get(seat.team)),
            )
        });
        write_seat_snapshot(
            seat,
            now,
            &economies,
            &records,
            &game_over,
            &squad_orders,
            *tech.tiers,
            *tech.research,
            &feed,
            (fog.enabled(), seat_fog),
            intent_errors.get(seat.team),
            co_out,
            &units,
            &buildings,
            &neutrals.nodes,
            &neutrals.bounties,
            &link,
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
    tiers: TechTiers,
    team_research: TeamResearch,
    feed: &GameEvents,
    fog: (bool, &FogGrid),
    // Per-command validation errors this team's intents produced, from the
    // shared compiler. Reported alongside the seat's own batch-level errors.
    //
    // NOTE for co-command: `IntentErrors` is keyed by TEAM, not by seat, so a
    // copilot's array also carries the human's refused gestures (`ui: …`).
    // That is deliberate rather than tolerated — a partner who can see that
    // your click just bounced off a stale ghost is a partner who can stop
    // proposing around it.
    intent_errors: &[String],
    // `Some` for a copilot seat: its etiquette, its pending queue, its team's
    // recent sentences. `None` for every other seat, which is what keeps their
    // wire format unchanged.
    co: Option<(CopilotOut, Vec<ProposalOut>, Vec<JournalOut>)>,
    units: &SnapshotUnits,
    buildings: &SnapshotBuildings,
    nodes: &SnapshotNodes,
    bounties: &SnapshotBounties,
    link: &CommandLink,
) {
    let me = seat.team;
    let (fog_enabled, fog) = fog;

    // Our own army, plus whatever of theirs we can see RIGHT NOW. Enemy units
    // are never remembered: a stale unit position is not information, it is a
    // decoy, and a commander acting on one has been lied to by its own
    // interface. Doctrine only for our own units, as before.
    let mut units_out: Vec<UnitOut> = units
        .iter()
        .filter(|(_, _, team, tf, ..)| **team == me || fog.sees(tf.translation))
        .map(|(e, unit, team, tf, health, order, move_to, carrying, hero, doctrine, kit)| {
            let (squad, prio, retreat, leash, autocast, why) = doctrine;
            let (inventory, militia, cooldowns, in_transit) = kit;
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
                    cd: r1(cooldowns.map_or(0.0, |c| c.remaining(0))),
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
                why: mine.then(|| {
                    why.map_or_else(|| NO_PROVENANCE.to_string(), Provenance::why)
                }),
                policies: (mine && has_policy).then(|| PoliciesOut {
                    prio: prio.map(|p| p.0.iter().map(|c| target_class_name(*c)).collect()),
                    retreat: retreat
                        .map(|r| [r1(r.below_frac), r1(r.rally.x), r1(r.rally.z)]),
                    leash: leash.map(|l| [r1(l.anchor.x), r1(l.anchor.z), r1(l.radius)]),
                    autocast: autocast.and_then(|a| a.primary()),
                }),
                // What the chain of command costs to reach this unit, and
                // whether something is already on its way to it.
                link: (mine && link.latency.on)
                    .then(|| r1(link.delay(*team, tf.translation))),
                pending: mine && in_transit.is_some(),
                // Our own casters only: abilities we can actually order.
                abilities: if mine {
                    abilities_out(
                        abilities_of_unit(unit.kind),
                        UnlockCtx::new(hero.map_or(0, |h| h.level), tiers.get(me)),
                        hero,
                        cooldowns,
                    )
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    units_out.sort_by_key(|u| u.id);

    // Ours, plus enemy structures we can see now. Ones we have seen before and
    // cannot see now are appended below as remembered ghosts, so the array is
    // "everything this seat has grounds to believe is standing".
    let mut buildings_out: Vec<BuildingOut> = buildings
        .iter()
        .filter(|(_, _, team, tf, ..)| **team == me || fog.sees(tf.translation))
        .map(
            |(e, building, team, tf, health, under, queue, template, cooldown, upgrading, researching)| BuildingOut {
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
            ability_cd: (*team == me && !abilities_of_building(building.kind).is_empty())
                .then(|| r1(cooldown.map_or(0.0, |c| c.remaining(0)))),
            abilities: if *team == me {
                abilities_out(
                    abilities_of_building(building.kind),
                    UnlockCtx::building(tiers.get(me)),
                    None,
                    cooldown,
                )
            } else {
                Vec::new()
            },
            // The shelf, tier applied. Only our own finished Shops: what an
            // ENEMY Shop would sell us is not a fact about the battlefield.
            sells: if *team == me && building.kind == BuildingKind::Shop && under.is_none() {
                shop_shelf(tiers.get(me))
            } else {
                Vec::new()
            },
            // Never for the opponent: a template is command structure.
            template: *team == me && template.is_some(),
            tier: building_tier(building.kind),
            upgrading: upgrading.map(|u| UpgradeOut {
                to: building_name(u.to),
                remaining: r1(u.remaining),
            }),
            // Ours only — see the field's doc.
            researching: researching.filter(|_| *team == me).map(|r| ResearchJobOut {
                upgrade: r.kind.id(),
                level: r.to_level,
                remaining: r1(r.remaining),
            }),
            // Observed this instant.
            last_seen: None,
            },
        )
        .collect();

    // Memory. Everything this seat scouted and can no longer see, reported at
    // the state it was in when last observed and stamped with when that was.
    // `queue`/`progress`/`ability_cd` are deliberately empty rather than
    // stale: a production queue is a live thing, and remembering one would be
    // inventing intelligence rather than preserving it. The building's
    // existence, kind, place and health are what a scout actually brings home.
    for ghost in fog.ghosts() {
        buildings_out.push(BuildingOut {
            id: ghost.id,
            team: team_name(ghost.team),
            kind: building_name(ghost.kind),
            pos: [r1(ghost.pos.x), r1(ghost.pos.z)],
            hp: ghost.hp,
            max_hp: ghost.max_hp,
            done: ghost.done,
            queue: Vec::new(),
            progress: 0.0,
            ability_cd: None,
            abilities: Vec::new(),
            // A remembered enemy Shop sells us nothing.
            sells: Vec::new(),
            template: false,
            // The tier it wore when last observed. A hall that has since been
            // upgraded behind our back still reports the old rung — the memory
            // is stale in exactly the way the scouting report was.
            tier: building_tier(ghost.kind),
            // Never on a ghost: see each field's doc. A remembered forge is a
            // building, not a work order.
            upgrading: None,
            researching: None,
            last_seen: Some(ghost.last_seen),
        });
    }
    buildings_out.sort_by_key(|b| b.id);

    // Tech state, for this seat only: what its completed buildings unlock.
    let completed: Vec<BuildingKind> = buildings
        .iter()
        .filter(|(_, _, team, _, _, under, _, _, _, _, _)| **team == me && under.is_none())
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

    // Bounty caches this seat can see, sorted by id so a seat serializes the
    // same order every tick. The two seats' lists now legitimately differ.
    let mut bounty_snaps: Vec<BountySnap> = bounties
        .iter()
        .filter(|(_, _, tf)| fog.sees(tf.translation))
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
    // Hero slots: the ladder decides the ceiling, and everything alive or
    // queued spends one. Counted here rather than inferred from `hero_records`
    // because a hero already in a training queue has no record yet and still
    // occupies a slot — the edge case economy.rs enforces at its pay-point.
    let my_hero_classes: Vec<UnitKind> = units
        .iter()
        .filter(|(_, unit, team, ..)| **team == me && is_hero_kind(unit.kind))
        .map(|(_, unit, ..)| unit.kind)
        .collect();
    let queued_hero_classes: Vec<UnitKind> = buildings
        .iter()
        .filter(|(_, _, team, ..)| **team == me)
        .filter_map(|(_, _, _, _, _, _, queue, ..)| queue)
        .flat_map(|q| q.queue.iter().copied())
        .filter(|k| is_hero_kind(*k))
        .collect();
    let hero_slots_used = (my_hero_classes.len() + queued_hero_classes.len()) as u32;

    let map = crate::terrain::active_map();
    let (copilot_out, proposals_out, partner_log) = match co {
        Some((copilot, proposals, journal)) => (Some(copilot), Some(proposals), Some(journal)),
        None => (None, None, None),
    };
    let command_nodes: Vec<CommandNodeOut> = if link.latency.on {
        link.nodes
            .own(me)
            .map(|(pos, radius)| CommandNodeOut {
                pos: [r1(pos.x), r1(pos.z)],
                radius: r1(radius),
            })
            .collect()
    } else {
        Vec::new()
    };

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
            tier: completed
                .iter()
                .filter(|k| is_hall(**k))
                .map(|k| building_tier(*k))
                .max()
                .unwrap_or(0),
            hero_slots: hero_slots(tiers.get(me)),
            hero_slots_used,
            hero_records: records
                .list(me)
                .iter()
                .map(|r| HeroRecordOut {
                    kind: kind_name(r.kind),
                    level: r.level,
                    xp: r1(r.xp),
                    alive: my_hero_classes.contains(&r.kind),
                })
                .collect(),
            hero_costs: ALL_UNIT_KINDS
                .iter()
                .copied()
                .filter(|k| is_hero_kind(*k))
                .map(|k| {
                    let (gold, lumber, time) = hero_train_cost(records, me, k);
                    HeroCostOut {
                        kind: kind_name(k),
                        gold,
                        lumber,
                        time,
                        revive: records.get(me, k).is_some(),
                    }
                })
                .collect(),
            research: {
                let levels = team_research.get(me);
                ALL_RESEARCH_KINDS
                    .iter()
                    .map(|&k| ResearchOut {
                        id: k.id(),
                        name: k.label(),
                        level: levels.level(k),
                        max_level: RESEARCH_MAX_LEVEL,
                        bonus: research_bonus(k, levels.level(k)),
                        next: levels.next_step(k).map(|s| ResearchStepOut {
                            level: s.level,
                            cost_gold: s.cost_gold,
                            cost_lumber: s.cost_lumber,
                            research_time: s.research_time,
                        }),
                        // Our own forges only. `buildings` also holds enemy
                        // structures we can see, so the team filter is what
                        // keeps this from reporting theirs.
                        in_progress: buildings
                            .iter()
                            .filter(|(_, _, team, ..)| **team == me)
                            .find_map(|(e, _, _, _, _, _, _, _, _, _, job)| {
                                job.filter(|j| j.kind == k).map(|j| ResearchProgressOut {
                                    level: j.to_level,
                                    remaining: r1(j.remaining),
                                    building: e.to_bits(),
                                })
                            }),
                    })
                    .collect()
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
        fog: FogOut {
            enabled: fog_enabled,
            explored: r1(fog.explored_frac()),
            visible: r1(fog.visible_frac()),
        },
        unlocked,
        units: units_out,
        buildings: buildings_out,
        squads,
        mines,
        trees_near,
        bounties: bounties_out,
        events,
        command_nodes,
        copilot: copilot_out,
        proposals: proposals_out,
        partner_log,
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
///
/// "Right now" is the whole contract, and for a UNIT it takes two facts, not
/// one. `unit_requires` is deliberately partial — it lists the gates BEYOND
/// owning the trainer, because the trainer is normally checked by the order
/// being given AT it. A map built from that half alone answered `Footman: true`
/// for a team with no Barracks: every tech gate satisfied (there are none),
/// and nowhere on the map to train one. That is not a caveat, it is a wrong
/// answer to the only question this map is asked, and it cost a commander a
/// bounced `train` to discover.
///
/// So a unit is unlocked when its tech gates are met AND this team has a
/// finished building standing that trains it. Buildings are unchanged: nothing
/// produces a building except a worker, which every team always has.
fn unlocked_map(completed: &[BuildingKind]) -> BTreeMap<&'static str, bool> {
    let mut out = BTreeMap::new();
    for kind in ALL_BUILDING_KINDS {
        // An upgrade-only kind is never "buildable", however much tech you
        // have: the honest answer to "may I place this right now" is no, and
        // the route to one is the catalog's `upgrades_to` plus the `upgrade`
        // command. Reporting `true` here would send a commander to a `build`
        // that always bounces.
        out.insert(
            building_name(kind),
            building_placeable(kind)
                && requirements_met(building_requires(kind), completed.iter().copied()),
        );
    }
    for kind in ALL_UNIT_KINDS {
        let has_trainer = completed.iter().any(|b| trainable(*b).contains(&kind));
        out.insert(
            kind_name(kind),
            has_trainer && requirements_met(unit_requires(kind), completed.iter().copied()),
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

// ---------------------------------------------------------------------------
// Co-command serializers
// ---------------------------------------------------------------------------

/// The pending queue, oldest first — the same order the human's panel shows
/// and the same order their approve/veto keys walk, so "#3" means one thing.
fn proposals_out(pending: &[Proposal], now: f32) -> Vec<ProposalOut> {
    pending
        .iter()
        .map(|p| ProposalOut {
            id: p.id,
            note: p.note.clone(),
            sentences: p.sentences.clone(),
            conflicts: p.conflicts.clone(),
            expires_in: r1(p.expires_in(now)),
        })
        .collect()
}

/// The team's recent sentences, oldest last-in-the-array first — same order as
/// `events`, so a reader that walks one walks the other the same way.
fn journal_out(entries: &std::collections::VecDeque<JournalEntry>) -> Vec<JournalOut> {
    entries
        .iter()
        .map(|e| JournalOut {
            t: e.t,
            source: e.source.name(),
            verb: e.verb,
            sentence: e.sentence.clone(),
            ok: e.ok,
        })
        .collect()
}

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
// submit it. intent.rs validates and applies it — the same compiler the
// human's mouse gestures go through, with the same fog rule, the same
// ownership checks and the same error strings.
//
// The wire format did not change, because the wire format *is* the schema:
// `Intent`'s serde shape is the historical command shape, tag for tag and
// field for field, `caster` alias, `use_item` rename and untagged ability
// selector included. `tools/bridge_send.py`, `bridge_view.py` and every
// COMMANDER_BRIEF.md flow keep working untouched, and rejected commands come
// back as the same strings in the same `errors` array, still prefixed
// `cmd <i>`.

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
/// before this frame's snapshot is written — the protocol's long-standing
/// promise that a batch applied this frame is visible in that frame's
/// snapshot, including its errors.
fn poll_commands(
    real: Res<Time<Real>>,
    mut bridge: ResMut<Bridge>,
    game_over: Res<GameOver>,
    mut intent_errors: ResMut<IntentErrors>,
    mut submissions: EventWriter<SubmitIntent>,
    mut copilot_wire: EventWriter<CopilotWire>,
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
                // The historical error prefix, so a commander that greps for
                // `cmd 3` still finds its third command — both roles.
                let tag = format!("cmd {i}");
                if seat.role == SeatRole::Copilot {
                    // Transport stops here. A co-commander's wire carries one
                    // shape an ordinary seat's does not (`propose`), and what
                    // it is allowed to say without asking is a question about
                    // partnership rather than about files — so copilot.rs
                    // parses and classifies, in this same frame.
                    copilot_wire.write(CopilotWire {
                        team: seat.team,
                        tag,
                        raw: raw.clone(),
                    });
                    continue;
                }
                match serde_json::from_value::<Intent>(raw.clone()) {
                    Ok(intent) => {
                        submissions.write(SubmitIntent {
                            team: seat.team,
                            source: IntentSource::Bridge,
                            tag,
                            intent,
                        });
                    }
                    Err(err) => intent_errors
                        .get_mut(seat.team)
                        .push(format!("{tag}: unrecognized command ({err})")),
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

#[cfg(test)]
mod tests {
    use super::*;

    // `every_unit_kind_is_orderable_by_name` moved to intent.rs with the name
    // parsers it exercises — the property it guards is now a property of the
    // shared vocabulary rather than of this file.

    /// The Barracks really does offer the Spearman, so the order has somewhere
    /// to land.
    #[test]
    fn barracks_trains_the_spearman() {
        assert!(trainable(BuildingKind::Barracks).contains(&UnitKind::Spearman));
        assert!(
            unit_requires(UnitKind::Spearman).is_empty(),
            "the tier-1 answer to cavalry must not itself be tech-gated"
        );
    }

    /// The bug this replaced: a team holding nothing but its town hall was told
    /// `Footman: true`, because the Footman has no tech gate and the map never
    /// asked where one would be trained.
    #[test]
    fn unlocked_needs_the_trainer_standing_not_just_the_tech() {
        let opening = unlocked_map(&[BuildingKind::TownHall]);
        assert_eq!(
            opening["Footman"], false,
            "no Barracks means no Footman, whatever the tech table says"
        );
        assert_eq!(opening["Archer"], false);
        assert_eq!(opening["Spearman"], false);
        // The hall trains these three itself, so they are honestly available.
        assert_eq!(opening["Worker"], true);
        assert_eq!(opening["Hero"], true);
        assert_eq!(opening["Priestess"], true);
        // Buildings are unaffected: a worker is the trainer, and every team
        // has one.
        assert_eq!(opening["Barracks"], true);
        assert_eq!(opening["Tower"], false, "Tower is still gated on Barracks");

        let with_barracks = unlocked_map(&[BuildingKind::TownHall, BuildingKind::Barracks]);
        assert_eq!(with_barracks["Footman"], true);
        assert_eq!(with_barracks["Tower"], true);
    }

    /// Both halves are required, in both directions: owning the trainer is not
    /// enough when the unit carries its own gate, and satisfying the gate is
    /// not enough without the trainer.
    #[test]
    fn a_unit_gate_and_its_trainer_are_both_load_bearing() {
        // Castle satisfies the Knight's gate; without a Barracks he has no
        // stable.
        let castle_only = unlocked_map(&[BuildingKind::Castle]);
        assert_eq!(castle_only["Knight"], false);
        assert_eq!(
            castle_only["GryphonRider"], false,
            "the Gryphon needs the Workshop as well as the Castle"
        );

        // Barracks without the Castle: the gate bites instead.
        let barracks_only = unlocked_map(&[BuildingKind::TownHall, BuildingKind::Barracks]);
        assert_eq!(barracks_only["Knight"], false);

        let both = unlocked_map(&[BuildingKind::Castle, BuildingKind::Barracks]);
        assert_eq!(both["Knight"], true);

        // And the Gryphon's full chain, which is what the AI's air path needs
        // standing before it can ever pick one: Castle + Workshop.
        let air = unlocked_map(&[
            BuildingKind::Castle,
            BuildingKind::Barracks,
            BuildingKind::Workshop,
        ]);
        assert_eq!(air["GryphonRider"], true);
        assert_eq!(air["Catapult"], true);
    }

    /// A Keep is a TownHall that grew: intersecting with trainers must go
    /// through `trainable`, which knows the whole hall ladder, and not through
    /// `kind == TownHall`.
    #[test]
    fn an_upgraded_hall_still_trains_its_roster() {
        for hall in [BuildingKind::Keep, BuildingKind::Castle] {
            let map = unlocked_map(&[hall]);
            assert_eq!(map["Worker"], true, "{hall:?} must still train workers");
            assert_eq!(map["Priestess"], true);
        }
    }
}
