//! bridge.rs — the "live bridge": a file-based control channel that lets one or
//! two external agents (Claude in a terminal) play a faction, either against the
//! human at the keyboard or against each other.
//!
//! Activation is opt-in through `BH_BRIDGE` (case-insensitive). Each accepted
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
//! in `bridge/` as they did when there was only one seat. `BH_BRIDGE=1` now
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
//!   * enemy `units` appear only while currently visible, and are never
//!     remembered *in that array*: an army that walks out of sight is gone
//!     from `units`, because a remembered army reported in the same shape as a
//!     seen one is a lie a commander would act on.
//!   * what it does not do is throw the observation away. The separate
//!     top-level `intel` block carries the SIGHTINGS LEDGER — every enemy unit
//!     this seat has seen, each stamped with `t_seen` and `age`, expiring at
//!     `intel.ttl_s`; the armies those sightings cluster into; and the standing
//!     of each enemy hero class. Memory is legal, and shaped so that it cannot
//!     be mistaken for sight: `units[]` has no timestamps and every intel
//!     record has nothing else.
//!   * enemy `buildings` appear as themselves while visible, and afterwards as
//!     REMEMBERED GHOSTS carrying a `last_seen` game-time stamp and the hp/
//!     queue state observed at that moment. A ghost can be stale — a razed
//!     barracks keeps its ghost until somebody looks at the spot again. Own
//!     buildings never carry `last_seen`, so the field's presence is exactly
//!     the "this is memory, not observation" flag.
//!   * `bounties` only while visible (see below).
//!   * `mines`, `trees_near`, `map` are unfiltered map GEOGRAPHY (see below).
//!
//! `BH_FOG=0` restores the old omniscient snapshot with no other change.
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
//! `use_item` also takes an optional `destination` — a building id naming
//! WHICH of your own standing halls a teleport item arrives at, read straight
//! off the `buildings[]` you already have. Omitted, both scrolls fall back to
//! the hall nearest the hero, which is what they always did. The catalog
//! advertises the option as `items[].destination: "choosable"`, so a commander
//! can discover it without being told. See docs/INTENT.md, "Which hall a
//! teleport item means".
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
use crate::copilot::{Copilot, CopilotWire, Proposal, Resolution};
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
const BRIDGE_ENV: &str = "BH_BRIDGE";

/// Root of every seat's directory; each seat gets its own subdirectory.
const BRIDGE_DIR: &str = "bridge";
const STATE_NAME: &str = "state.json";
/// Snapshots are written here first and renamed over `STATE_NAME`.
const STATE_TMP_NAME: &str = "state.tmp";
const COMMANDS_NAME: &str = "commands.json";
/// Written once per session at startup; identical for every seat.
const CATALOG_NAME: &str = "catalog.json";
const CATALOG_TMP_NAME: &str = "catalog.tmp";

/// Wall-clock seconds between snapshots (independent of `BH_SPEED`).
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
            // Declared once, so anything later tagged only `.in_set(BridgePoll)`
            // inherits the frame order rather than floating outside it.
            .configure_sets(Update, BridgePoll.in_set(SimSet::Input))
            // Before `ReadyGateHold`: that system stops the clock, and it can
            // only know which seats to wait for after this one has opened
            // them. Both are `Startup`, so without the edge the order is
            // whatever the executor felt like — and getting it wrong means a
            // match that is never held rather than one that is held wrongly,
            // which is the failure mode that would go unnoticed longest.
            .add_systems(Startup, bridge_startup.before(ReadyGateHold))
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
                    // `SimSet::Feed`, the frame's reporting phase: the
                    // snapshot describes the finished frame, so it now lands
                    // after movement, combat and the economy rather than
                    // racing them and photographing a half-stepped world.
                    write_snapshot.in_set(SimSet::Feed).after(IntentApply),
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

/// Which factions `BH_BRIDGE` asks for, and in what role. `None` means "leave
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
    mut gate: ResMut<ReadyGate>,
) {
    let Ok(raw) = std::env::var(BRIDGE_ENV) else {
        return;
    };
    let Some(teams) = seats_from_env(&raw) else {
        return;
    };
    // Read once, here, so every seat in this run agrees about the regime.
    let handshake = ready_handshake_enabled();

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
                // The scripted approver is announced at the seat, not buried
                // in the snapshot: a sim log that does not say "nobody human
                // answered these" is a result somebody will misread later.
                let approver = match copilot.auto_approve {
                    Some(delay) => {
                        format!(", SCRIPTED APPROVER: auto-approving after {delay}s")
                    }
                    None => String::new(),
                };
                info!(
                    "{BRIDGE_ENV}: CO-COMMAND seat active — {:?} is played by the human \
                     at the keyboard WITH a co-commander (trust: {}{approver}, snapshot {}, \
                     commands {})",
                    team,
                    copilot.policy.name(),
                    seat.state_file.display(),
                    seat.commands_file.display()
                );
            }
        }
        // Every seat that actually opened gates the start — commander AND
        // copilot, on the argument in `ReadyGate`'s docs. A seat whose
        // directory could not be created `continue`d above and is therefore
        // absent here as well as from `bridge.seats`: a seat that does not
        // exist must not be able to hold the match forever.
        if handshake {
            gate.seats.push(ReadySeat {
                name: seat_dir(team, role),
                team,
                ready: false,
            });
        }
        bridge.seats.push(seat);
    }

    if !gate.seats.is_empty() {
        gate.timeout = ready_timeout_from_env();
    } else if !handshake {
        info!(
            "{READY_ENV}=0: ready handshake OFF — the clock runs from process start, \
             as it did before the handshake existed"
        );
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
    /// **Which ROSTER you are playing**: `"kingdom"` or `"horde"`.
    ///
    /// The catalog is written once per session and describes both races, with
    /// every unit and building row carrying its own `race` and `role`. This
    /// key is what turns that shared document into your build tree: filter the
    /// catalog by it, or — better — ignore kind names entirely and plan against
    /// `role`, which means the same thing on both sides of the matchup.
    ///
    /// Always present. It reads `"kingdom"` for both seats in a match nobody
    /// opted into a second race for, which is every match by default.
    my_race: &'static str,
    seq_applied: u64,
    errors: Vec<String>,
    /// docs/TEMPO.md §3/§4 — **what your last batch cost to deliver.** One
    /// entry per command that had to travel, naming it with the same `cmd N`
    /// identity the `errors` above use, and the seconds the slowest unit it
    /// spoke to took to receive it.
    ///
    /// The positive half of the acknowledgement: `errors` says what was
    /// refused, this says what the rest cost. Commands that landed in the frame
    /// they were spoken are simply absent — silence means instant — so this key
    /// does not exist at all when `BH_COMMAND_LATENCY` is off.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied: Vec<AppliedOut>,
    /// **The seats that have not yet said `{"type":"ready"}`**, by name —
    /// `["red","blue"]`. Present ONLY while the match is held at t=0; the
    /// moment the clock starts this key and `match_started` both disappear and
    /// the snapshot's historical key set is exactly what it always was. See
    /// `shared::ReadyGate`.
    ///
    /// Your own name in this list means the engine is waiting on YOU. Another
    /// seat's name means you have been heard and the hold is not yours. Either
    /// way `t` stays 0 and nothing in the world moves — reading the map and
    /// writing your opening now is the intended use of the time, and it is
    /// time both sides get.
    ///
    /// `skip_serializing_if` on the same reasoning the `game_over_reason` note
    /// below spells out: a key that exists only in a regime the historical
    /// tooling never saw must be absent outside that regime, or every
    /// exact-key-set assertion in tools/ breaks the moment it ships.
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting_for: Option<Vec<&'static str>>,
    /// **Has the match clock started?** `false` while held, and then absent —
    /// not `true` — forever after. An always-present boolean would be the
    /// friendlier shape in isolation, but it would also be a permanent
    /// addition to every snapshot of every run, which is precisely the key-set
    /// change this pair is written to avoid. The transition is legible without
    /// it: `waiting_for` vanishes, a `match start` line appears in `events`,
    /// and `t` begins to move.
    #[serde(skip_serializing_if = "Option::is_none")]
    match_started: Option<bool>,
    game_over: Option<&'static str>,
    /// **Which win it was**: `"razed"` (the loser has no production buildings
    /// left) or `"surrender"`. Round-9's winner could not tell the two apart —
    /// a conceded match and a fought-out one call for completely different
    /// AARs, and `game_over` alone never distinguished them.
    ///
    /// Deliberately a SIBLING key rather than turning `game_over` into
    /// `{winner, reason}`. `game_over` is read as a team name or null by
    /// tools/bridge_view.py (`f"{s['game_over']} wins"`), tools/bridge_wait.py
    /// and tools/COMMANDER_BRIEF.md's poll loop; an object there breaks all
    /// three the moment a match ends, which is exactly the moment nobody is
    /// watching the tooling. So `game_over` keeps its shape forever.
    ///
    /// `skip_serializing_if` keeps it ABSENT for the entire live match, so the
    /// snapshot's historical key set is untouched right up to the last tick —
    /// `verify_intent_bridge.py`'s exact-key-set assertion runs mid-match and
    /// still passes unmodified.
    #[serde(skip_serializing_if = "Option::is_none")]
    game_over_reason: Option<&'static str>,
    me: MeOut,
    /// The ground both seats are fighting over: which layout is loaded and
    /// where its impassable terrain can be crossed. The human sees the canyon
    /// on screen and on the minimap; this is the same fact in JSON.
    map: MapOut,
    /// What this seat can currently know. Read it before concluding anything
    /// from an empty `units` list.
    fog: FogOut,
    /// What this seat REMEMBERS of the enemy — sightings, the armies they
    /// cluster into, and the standing of each enemy hero. Every entry carries
    /// the game time it was observed.
    intel: IntelOut,
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
    /// reading it. Absent entirely when `BH_COMMAND_LATENCY` is off.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command_nodes: Vec<CommandNodeOut>,
    /// **Your armed triggers** (`trigger_set`), in the order you set them —
    /// which is the order they fire in when two come true on the same tick.
    ///
    /// Own team only, and for a stronger reason than the usual one: a trigger
    /// is a *plan*, and reading your opponent's contingency plans is the single
    /// most valuable thing a snapshot could leak. Nothing here is derived from
    /// the other faction's state.
    ///
    /// Absent when you have none, on the same rule as `command_nodes` — a
    /// snapshot from a seat that has never spoken the word is byte-shape
    /// identical to a v1 one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    triggers: Vec<TriggerOut>,
    /// **This seat's own named regions**, in the order they were set.
    ///
    /// Own-team only, and that is doctrine rather than bookkeeping: a region is
    /// a decision about which ground matters, and handing the enemy a list of
    /// the places you have decided to watch would be the single most valuable
    /// intelligence leak in the protocol. The map's built-in names are the
    /// public half and live in `map.places`, where both seats see the same
    /// list.
    ///
    /// Skipped when empty, so a snapshot from a seat that has never named
    /// ground is byte-shape identical to a v2 one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    regions: Vec<RegionOut>,
    /// **Your plans** (`plan_set`), in the order you set them.
    ///
    /// Own team only, and for the same stronger reason `triggers` is: a plan is
    /// your build order and your follow-up, and reading the opponent's is the
    /// single most valuable thing a snapshot could leak.
    ///
    /// This is the array to read FIRST on a poll where you have a plan running.
    /// `step` and `status` together answer "is the sequence I wrote still
    /// happening", which is the only question a plan raises — and a `blocked:`
    /// or `halted:` status carries the compiler's own words, so you never have
    /// to go correlate it against `errors`.
    ///
    /// Absent when you have none, on the same rule as `command_nodes`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    plans: Vec<PlanOut>,

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
    /// in **answer order** — urgent first, then oldest. A proposal leaves this
    /// list by being approved, vetoed or lapsing.
    #[serde(skip_serializing_if = "Option::is_none")]
    proposals: Option<Vec<ProposalOut>>,
    /// The last few proposals that LEFT `proposals`, oldest first, each with
    /// how it ended — and, on a veto, which of the three answers it was.
    ///
    /// A tail rather than a terminal `status` left on `proposals` for one
    /// cycle: a seat polling slower than the snapshot ticks would miss a
    /// one-write status, and "did my partner ever answer #3, and what did they
    /// say?" is exactly the question you ask when you have *not* kept up. It
    /// also keeps `proposals` meaning one thing — the queue you can still act
    /// on — instead of a mixed list every reader has to filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    recent_resolutions: Option<Vec<ResolutionOut>>,
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
    /// `"split"` (default), `"full"` or `"strict"` — `BH_COPILOT_TRUST`.
    trust: &'static str,
    /// Verbs this seat may send WITHOUT a `propose` wrapper. `["*"]` under
    /// full trust, empty under strict. Anything not here is refused with an
    /// error that shows the wrapper.
    direct: Vec<&'static str>,
    /// Game seconds an unanswered proposal survives.
    propose_ttl: f32,
    /// How many proposals may be outstanding before new ones are refused.
    /// Unchanged by urgency: the cap is about how many questions a human can
    /// hold, and marking one urgent does not add attention.
    max_pending: usize,
    /// The values `severity` accepts on a `propose` wrapper, least urgent
    /// first. The first is the default.
    severities: [&'static str; 2],
    /// Every veto reason your partner can answer with, mapped to what it asks
    /// of you next. Advertised for the same reason `direct` is: a
    /// co-commander should learn its etiquette by reading the snapshot, not by
    /// being told out of band.
    veto_reasons: BTreeMap<&'static str, &'static str>,
    /// Present only when a SCRIPTED approver is standing in for the human
    /// (`BH_COPILOT_AUTOAPPROVE`, sims only): the seconds it waits before
    /// approving. Absent in every real match — so a seat can tell whether the
    /// thing answering it is a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_approve_after: Option<f32>,
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
    /// `"routine"` or `"urgent"` — as sent, echoed back so a seat can see that
    /// its urgency was accepted rather than assume it.
    severity: &'static str,
    /// Game seconds until it lapses unanswered.
    expires_in: f32,
}

/// One proposal that has been answered, and how.
#[derive(Serialize)]
struct ResolutionOut {
    id: u32,
    /// Game seconds at which it was answered.
    t: f32,
    /// The seat's own note, echoed back — a resolution names the IDEA, not
    /// just a number the model has to have remembered.
    note: String,
    severity: &'static str,
    /// `"approved"`, `"vetoed"` or `"expired"`. There is no `"pending"`:
    /// being in `proposals` is what pending means, and a status that restates
    /// a list membership is a second source of truth.
    outcome: &'static str,
    /// Vetoes only: `"not_now"`, `"never"` or `"wrong_target"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    /// Vetoes only: what that reason asks you to do next, in one clause, so
    /// acting on a veto correctly needs no second document.
    #[serde(skip_serializing_if = "Option::is_none")]
    advice: Option<&'static str>,
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

/// One delivered command and its realised link cost, in seconds.
#[derive(Serialize)]
struct AppliedOut {
    /// `"cmd 3"` — the same handle the matching error string is prefixed with.
    cmd: String,
    /// Worst link any unit this command named actually paid. A group order
    /// spread across the map reports its slowest member, which is when the
    /// whole order is in effect.
    delay: f32,
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
    /// The `BH_MAP` value that produced this world: `"open"`, `"crossings"`.
    name: &'static str,
    /// What the layout means for a plan, in one sentence.
    summary: &'static str,
    /// Every layout this build offers, so a commander can see what else exists
    /// without being told (the human reads the same list from `BH_MAP`).
    available: Vec<&'static str>,
    /// Gaps in the impassable terrain — empty on a map that has none. Armies,
    /// workers and expansions can only cross here.
    chokes: Vec<ChokeOut>,
    /// **The map's own vocabulary**: every name both seats may speak without
    /// arming anything — the two bases, `mid`, the four mines, and one entry
    /// per ford. Any of these is legal wherever a verb takes `x`/`z`, as
    /// `"region":"<name>"`.
    ///
    /// Public and neutral like everything else in this struct, with one honest
    /// asymmetry: `our base` and `their base` are seat-relative, so the two
    /// snapshots disagree about which coordinates those two names carry. That
    /// is the point of the aliases — the WORDS are shared, and each seat reads
    /// them from where it is standing.
    places: Vec<RegionOut>,
}

/// A named circle of ground, in the seat's own words.
///
/// One shape for both kinds of name — the map's built-ins in `map.places` and
/// the seat's own in `regions` — because a commander should not need to know
/// which kind a name is before using it. The only difference is which array it
/// arrived in, and that is exactly the difference: one is a map fact, the other
/// is your doctrine.
#[derive(Serialize)]
struct RegionOut {
    name: String,
    /// Centre on the ground plane.
    pos: [f32; 2],
    radius: f32,
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
    /// hero is currently alive. `alive` false means it is dead and can be
    /// bought back at that class's `revive_gold`/`revive_lumber`, keeping the
    /// `level` printed here.
    hero_records: Vec<HeroRecordOut>,
    /// What each hero class costs you to put in a queue RIGHT NOW, and what it
    /// will cost when it dies. **Your FIRST hero is free** — 0g 0l, paid only
    /// in 25 seconds of hall time and 5 supply — and that waiver is one per
    /// team, not one per class.
    ///
    /// Read the numbers, not the rule: with no hero yet, EVERY class prices at
    /// 0 because any one of them could be the free one, and the instant you
    /// queue one the rest jump to 400g/100l in the next snapshot. They are
    /// alternatives, not a shopping list. Every class is listed, including ones
    /// your slots have no room for; `hero_slots_used` vs `hero_slots` is the
    /// gate, not this.
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
    /// What queuing this class costs you RIGHT NOW. **Zero only while your
    /// team has no hero at all** — alive, queued, or dead and awaiting
    /// revival. After that this is 400g/100l whether the class is new to you
    /// or coming back.
    gold: u32,
    lumber: u32,
    /// Seconds in the queue — the part that is never free. A fresh hero is
    /// 25s of hall time you are not spending on workers; a revival is faster.
    time: f32,
    /// True when `gold`/`lumber` above are a REVIVAL price, i.e. this class
    /// has died at least once and comes back at its recorded level.
    revive: bool,
    /// What this class will cost the NEXT time it dies — the same numbers
    /// whether it is currently free, alive, or already dead, because the
    /// revival fee is flat rather than scaled by level. Carried alongside
    /// `gold`/`lumber` so a commander can budget for a hero's death BEFORE it
    /// happens instead of discovering the price at the funeral.
    revive_gold: u32,
    revive_lumber: u32,
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
    /// entirely when `BH_COMMAND_LATENCY` is off, which is also when it is
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
    /// **The stance word this squad is holding**, when one put it there —
    /// `"turtle"`, `"stage"`, `"push"`, `"secure"`, `"harass"`. Absent for a
    /// squad whose posture was set by hand, which is the honest answer rather
    /// than a missing one: those four verbs are still available and a squad
    /// under them is genuinely in no stance.
    ///
    /// This is the key the whole feature is steered by. A commander that says
    /// nothing reads its own stance back in the next snapshot and knows what its
    /// army is still doing — silence continues the stance rather than dissolving
    /// it, so the answer to "what am I doing?" survives a poll with no command.
    ///
    /// Additive and skipped when absent, so a match that never sends a `stance`
    /// produces a byte-identical snapshot to the one that shipped before it.
    #[serde(skip_serializing_if = "Option::is_none")]
    stance: Option<&'static str>,
}

/// One armed trigger, as its owner reads it back.
///
/// `when` and `then` are the **same JSON you sent**, round-tripped through the
/// `Intent` type rather than re-described — so a commander can read a trigger
/// out of the snapshot, edit one number, and send it back as a `trigger_set`
/// under the same name. A prose summary would have been a second spelling of
/// the language, which is the thing docs/INTENT.md exists to prevent.
#[derive(Serialize)]
struct TriggerOut {
    name: String,
    when: TriggerWhen,
    then: Intent,
    /// Cooldown in game seconds for a repeating trigger; absent for a
    /// once-trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat: Option<f32>,
    /// `"armed"` (will fire when the predicate holds), `"cooling"` (repeating,
    /// inside its cooldown) or `"spent"` (a once-trigger that has fired).
    status: &'static str,
    /// Game time of the last fire; absent if it never has. Spent triggers are
    /// KEPT in this list precisely so this field can answer "did my rule ever
    /// go off?" — an absence cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_fired: Option<f32>,
    /// The English of the whole statement, identical to the line the replay log
    /// wrote when it was armed and the line the event feed writes when it
    /// fires.
    sentence: String,
}

/// One plan, as its owner reads it back.
///
/// `steps` is the **same JSON you sent**, round-tripped through the `Intent`
/// and `PlanAdvance` types rather than re-described — the rule `TriggerOut`
/// follows and for the same reason: a commander should be able to read a plan
/// out of the snapshot, change one step, and send it back under the same name.
#[derive(Serialize)]
struct PlanOut {
    name: String,
    /// Which step is being worked, one-based, and how many there are. Two
    /// fields rather than the string `"2/5"` because a reader that wants to
    /// branch on progress should not have to parse a fraction.
    step: usize,
    of: usize,
    /// `"running"`, `"done"`, or `"blocked: <why>"` / `"halted: <why>"` with
    /// the compiler's verbatim refusal. A bare word would be a status you have
    /// to go research; plans.rs's whole failure design is that you do not.
    status: String,
    /// The English of the step it is on right now — the same sentence the
    /// event feed wrote when the step went out.
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,
    /// The whole sequence, editable and re-sendable.
    steps: Vec<PlanStep>,
    /// The English of the entire plan, identical to the line the replay log
    /// wrote when it was set. What a co-commander's proposal is reviewed on.
    sentence: String,
}

/// This seat's plans, wire-shaped. A free function rather than an inline map so
/// the shape a commander parses can be tested without standing up a whole
/// snapshot — the same reason `resolutions_out` is one.
fn plans_out(my_plans: &[PlanRun]) -> Vec<PlanOut> {
    my_plans
        .iter()
        .map(|p| PlanOut {
            name: p.name.as_str().to_string(),
            step: p.step_no(),
            of: p.steps.len(),
            status: p.status(),
            current: p.current().map(|s| s.intent.sentence()),
            steps: p.steps.clone(),
            sentence: Intent::PlanSet {
                name: p.name.as_str().to_string(),
                steps: p.steps.clone(),
            }
            .sentence(),
        })
        .collect()
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

/// One enemy unit as this seat last observed it.
///
/// **Every entry is a MEMORY.** `units[]` is what is on the board now; this is
/// what was on the board then, and the two are separate arrays specifically so
/// nothing can read one as the other. `t_seen` and `age` are mandatory, not
/// optional-when-stale: there is no shape of this record that omits its own
/// staleness.
///
/// Nothing here is knowable that a human could not read off their screen while
/// the unit stood in their vision — no level, no mana, no orders. See
/// `shared::Sighting` for the field-by-field argument.
#[derive(Serialize)]
struct SightingOut {
    /// The unit's entity id — the same id `units[].id` carries while it is
    /// visible, so a commander can tell "the raider I saw" from "a raider".
    id: u64,
    kind: &'static str,
    pos: [f32; 2],
    /// Health fraction at the moment of observation, 0..1. The bar a watcher
    /// could see.
    hp_frac: f32,
    /// Coarse 8-point heading it was walking when last seen (`"NE"`), absent
    /// if it was standing still or if this was a first glimpse with nothing to
    /// measure against.
    #[serde(skip_serializing_if = "Option::is_none")]
    heading: Option<&'static str>,
    /// Game time of the observation.
    t_seen: f32,
    /// Game-seconds since. Derived, and shipped anyway: the arithmetic is
    /// trivial and getting it wrong is not, and a commander that has to
    /// subtract before it can discount will sometimes forget to.
    age: f32,
}

/// Concurrent sightings read as one body of troops — the aggregate view.
#[derive(Serialize)]
struct ArmyGroupOut {
    /// How many units the group holds. Approximate by nature: it is what was
    /// seen, never what is there.
    size: usize,
    /// `"5 Footman, 3 Archer"`, most numerous first.
    composition: String,
    pos: [f32; 2],
    /// Freshest observation in the group; members may be up to ten seconds
    /// older, which is what makes them one picture.
    t_seen: f32,
    age: f32,
    /// The public name of the ground it is on — `"near the center ford"`. The
    /// same phrase the event feed uses, from the same function.
    place: String,
}

/// This seat's belief about one enemy hero class.
#[derive(Serialize)]
struct HeroIntelOut {
    /// `"unknown"` — never seen; `"alive"` — seen alive and nothing since says
    /// otherwise; `"seen-dying"` — you watched it die.
    ///
    /// Read `"alive"` as *alive as far as you know*. It is a belief with a
    /// timestamp, not a fact: a hero that died out of your sight goes on
    /// reporting `"alive"` here for as long as nobody looks, which is exactly
    /// the mistake fog of war exists to let you make.
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pos: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    t_seen: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    age: Option<f32>,
    /// Health fraction when last observed. **No level, no xp, no mana** — a
    /// human cannot select an enemy hero (ui.rs's pickers are own-team only),
    /// so no human has ever read those off a screen, and handing them to a
    /// commander would be an information right with no gesture behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    hp_frac: Option<f32>,
}

/// What this seat REMEMBERS of the enemy, as opposed to what it can see.
///
/// The counterpart to `fog`: that block says how much of the map is knowable,
/// this one says what was learned from having known it. Always present, and
/// deliberately so — an absent `intel` and an empty one are different claims
/// ("this build has no ledger" vs "you have seen nothing"), on exactly the
/// reasoning `fog` itself is always present for.
#[derive(Serialize)]
struct IntelOut {
    /// Every enemy unit seen and not yet expired, in id order. Entries are
    /// dropped after `ttl_s` without a refresh, and immediately when this seat
    /// watches the unit die.
    sightings: Vec<SightingOut>,
    /// The same sightings clustered into forces. Workers are excluded — a
    /// mining crew is not an army.
    groups: Vec<ArmyGroupOut>,
    /// Belief about each enemy hero class, keyed by class name. Always holds
    /// every class, `"unknown"` included: a missing row and a row saying "no
    /// idea" are different claims and only one of them is true.
    heroes: BTreeMap<&'static str, HeroIntelOut>,
    /// The staleness horizon in game-seconds. Past this a sighting is a rumour
    /// and the ledger drops it, so an empty `sightings` means "nothing seen
    /// recently", never "nothing exists".
    ttl_s: f32,
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
            // reached it yet. Always `None` with BH_COMMAND_LATENCY off.
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
    /// Which roster each team is playing. Here rather than as its own
    /// parameter for the reason the doc above gives, and it belongs with the
    /// other two anyway: all three answer "what content is this team's".
    races: Res<'w, Races>,
}

/// What the compiler had to say about the last batch a seat sent: what it
/// **refused**, and what the rest **cost to deliver**. One acknowledgement in
/// two halves, so they travel as one parameter — which is also what keeps
/// `write_snapshot` off Bevy's 16-parameter ceiling now that Chain of Command
/// has taken two of the slots.
#[derive(SystemParam)]
struct SeatVerdicts<'w> {
    errors: Res<'w, IntentErrors>,
    applied: Res<'w, IntentApplied>,
}

/// Where the match is in its own life: has it started, and has it ended. The
/// two questions bracket everything else in the snapshot, and they travel
/// together for the reason `TeamTech` does — `write_snapshot` sits exactly on
/// Bevy's 16-parameter ceiling, and the ready handshake needed a slot that did
/// not exist. `GameOver` used to be its own parameter; pairing it with
/// `ReadyGate` costs nothing and reads better than either alone.
#[derive(SystemParam)]
struct MatchState<'w> {
    over: Res<'w, GameOver>,
    ready: Res<'w, ReadyGate>,
}

/// The standing policy a team has set and the engine executes for it: squad
/// postures (continuous) and armed triggers (contingent). Bundled for the same
/// reason `TeamTech` is — `write_snapshot` sits exactly on Bevy's 16-parameter
/// ceiling — and the pairing is not arbitrary: both are written only by the
/// intent compiler, read only here and in the HUD, and answer the one question
/// "what has this commander told the engine to do without them".
#[derive(SystemParam)]
struct StandingOrders<'w> {
    squads: Res<'w, SquadOrders>,
    /// The stance word behind each squad's posture, travelling with the posture
    /// it produced so `SquadOut` cannot report one without the other.
    stances: Res<'w, SquadStances>,
    triggers: Res<'w, Triggers>,
    /// The third store on the same rule: written only by the intent compiler's
    /// two `region_*` verbs, read here and in the HUD, and it answers the same
    /// question about a different noun — what ground has this commander decided
    /// to name.
    regions: Res<'w, Regions>,
    /// The third kind, sequenced. Same one-writer rule, same two readers.
    plans: Res<'w, Plans>,
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
    match_state: MatchState,
    standing: StandingOrders,
    tech: TeamTech,
    feed: Res<GameEvents>,
    fog: Res<FogGrids>,
    verdicts: SeatVerdicts,
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
        let co_out = (seat.role == SeatRole::Copilot).then(|| CoOut {
            copilot: CopilotOut {
                trust: co.copilot.policy.name(),
                direct: crate::copilot::direct_verbs(co.copilot.policy),
                propose_ttl: crate::copilot::PROPOSAL_TTL,
                max_pending: crate::copilot::MAX_PENDING,
                severities: crate::copilot::ProposalSeverity::NAMES,
                veto_reasons: crate::copilot::VetoReason::all()
                    .into_iter()
                    .map(|r| (r.wire(), r.advice()))
                    .collect(),
                auto_approve_after: co.copilot.auto_approve,
            },
            proposals: proposals_out(&co.copilot.pending, now),
            recent_resolutions: resolutions_out(&co.copilot.resolved),
            partner_log: journal_out(co.journal.get(seat.team)),
        });
        write_seat_snapshot(
            seat,
            now,
            &economies,
            &records,
            &match_state.over,
            &match_state.ready,
            &standing.squads,
            &standing.stances,
            standing.triggers.get(seat.team),
            standing.regions.get(seat.team),
            standing.plans.get(seat.team),
            *tech.tiers,
            *tech.research,
            tech.races.get(seat.team),
            &feed,
            (fog.enabled(), seat_fog),
            verdicts.errors.get(seat.team),
            verdicts.applied.get(seat.team),
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
    // Whether the match has started, and who it is still waiting for. Read for
    // the two optional keys at the top of `StateOut`; see `shared::ReadyGate`.
    ready: &ReadyGate,
    squad_orders: &SquadOrders,
    // The stance word behind those postures, when one put it there. Whole
    // resource rather than pre-sliced because it is keyed by `(team, squad)`
    // exactly like `squad_orders` beside it, and the two are read together.
    squad_stances: &SquadStances,
    // This seat's own armed triggers. Passed pre-sliced by team rather than as
    // the whole resource, so this function cannot read the opponent's plans
    // even by accident.
    my_triggers: &[TriggerRule],
    my_regions: &[Region],
    // This seat's own plans, pre-sliced by team on the same reasoning.
    my_plans: &[PlanRun],
    tiers: TechTiers,
    team_research: TeamResearch,
    // **Your roster.** The catalog is one document for the whole session and
    // carries every row of both races, tagged; this is the key that tells a
    // commander which of those rows are its own. Match it against
    // `catalog.units[].race` / `catalog.buildings[].race`, or ignore names
    // entirely and go by `role`.
    my_race: Race,
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
    // What the rest of that batch cost to deliver — see `StateOut::applied`.
    // Empty whenever nothing was charged, which is always with the feature off.
    intent_applied: &[AppliedCommand],
    // `Some` for a copilot seat: its etiquette, its pending queue, what became
    // of the ones it already asked, and its team's recent sentences. `None`
    // for every other seat, which is what keeps their wire format unchanged.
    co: Option<CoOut>,
    units: &SnapshotUnits,
    buildings: &SnapshotBuildings,
    nodes: &SnapshotNodes,
    bounties: &SnapshotBounties,
    link: &CommandLink,
) {
    let me = seat.team;
    let (fog_enabled, fog) = fog;

    // Our own army, plus whatever of theirs we can see RIGHT NOW. Enemy units
    // are never remembered *here*: a stale unit position reported in this
    // array would not be information, it would be a decoy, because nothing in
    // a `UnitOut` says when it was true. The memory lives in `intel` instead,
    // where every record carries its own age and the reader cannot fail to see
    // it. Doctrine only for our own units, as before.
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

    // --- intel -----------------------------------------------------------
    // The unit half of memory. Note it does NOT join `units_out` the way the
    // building ghosts join `buildings_out`, and that asymmetry is the whole
    // design: a remembered structure is still standing where it was, so it
    // belongs in the same list as the ones you can see; a remembered army is
    // somewhere else by now, so putting it there would be the lie
    // `CellVis::Explored` refuses to tell. It gets its own section, and every
    // record in it wears the clock.
    let intel_out = IntelOut {
        sightings: fog
            .sightings()
            .map(|s| SightingOut {
                id: s.id,
                kind: kind_name(s.kind),
                pos: [r1(s.pos.x), r1(s.pos.z)],
                hp_frac: r1(s.hp_frac * 100.0) / 100.0,
                heading: s.heading.map(|h| h.as_str()),
                t_seen: r1(s.t_seen),
                age: r1(s.age(now)),
            })
            .collect(),
        groups: fog
            .army_groups()
            .into_iter()
            .map(|g| ArmyGroupOut {
                size: g.size,
                composition: g.summary(),
                pos: [r1(g.centroid.x), r1(g.centroid.z)],
                t_seen: r1(g.t_seen),
                age: r1((now - g.t_seen).max(0.0)),
                place: place_name(g.centroid, me),
            })
            .collect(),
        heroes: fog
            .hero_intel()
            .iter()
            .map(|h| {
                (
                    kind_name(h.kind),
                    HeroIntelOut {
                        status: h.status.as_str(),
                        pos: h.pos.map(|p| [r1(p.x), r1(p.z)]),
                        t_seen: h.t_seen.map(r1),
                        age: h.t_seen.map(|t| r1((now - t).max(0.0))),
                        hp_frac: h.hp_frac.map(|f| r1(f * 100.0) / 100.0),
                    },
                )
            })
            .collect(),
        ttl_s: SIGHTING_TTL_S,
    };

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
            stance: squad_stances.0.get(&(me, id)).map(|s| s.word()),
        })
        .collect();

    // Armed triggers, in the order they were set — which is the order they fire
    // in, so it is the order they must be read in. NOT sorted, unlike every
    // other list here: sorting would hide the one thing about the list that is
    // load-bearing.
    // This seat's own named ground, in the order it was named. Not sorted, for
    // the same reason the trigger list is not: the order is the commander's.
    let regions: Vec<RegionOut> = my_regions
        .iter()
        .map(|r| RegionOut {
            name: r.name.clone(),
            pos: [r1(r.center.x), r1(r.center.z)],
            radius: r1(r.radius),
        })
        .collect();

    let triggers: Vec<TriggerOut> = my_triggers
        .iter()
        .map(|t| TriggerOut {
            name: t.name.as_str().to_string(),
            when: t.when.clone(),
            then: t.then.clone(),
            repeat: t.repeat,
            status: t.status(now),
            last_fired: t.last_fired.map(r1),
            sentence: Intent::TriggerSet {
                name: t.name.as_str().to_string(),
                when: t.when.clone(),
                then: Box::new(t.then.clone()),
                repeat: t.repeat,
            }
            .sentence(),
        })
        .collect();

    // Plans, in the order they were set — which is the order they step in, so
    // it is the order they must be read in. NOT sorted, for the reason the
    // trigger list is not.
    let plans = plans_out(my_plans);

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
    // The same two lists, concatenated, are what prices a hero: the one free
    // hero is spent by anything alive OR in flight, so `hero_costs` below
    // starts charging the moment the first one is queued rather than the
    // moment it walks out of the hall.
    let my_held_heroes: Vec<UnitKind> = my_hero_classes
        .iter()
        .copied()
        .chain(queued_hero_classes.iter().copied())
        .collect();

    let map = crate::terrain::active_map();
    // All four co-command keys appear together or not at all — including as
    // empty lists — so the shape a co-commander parses never changes under it.
    let (copilot_out, proposals_out, resolutions_out, partner_log) = match co {
        Some(co) => (
            Some(co.copilot),
            Some(co.proposals),
            Some(co.recent_resolutions),
            Some(co.partner_log),
        ),
        None => (None, None, None, None),
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
        my_race: my_race.name(),
        seq_applied: seat.last_seq,
        // Batch-level first, then the compiler's per-command verdicts — one
        // flat array of strings, exactly the shape the protocol always had.
        errors: seat
            .errors
            .iter()
            .chain(intent_errors.iter())
            .cloned()
            .collect(),
        // Rounded to the tenth the same way `link` and the intent log's own
        // `link` field are, so a commander comparing the estimate it read
        // against the cost it paid is comparing like with like.
        applied: intent_applied
            .iter()
            .map(|a| AppliedOut {
                cmd: a.cmd.clone(),
                delay: r1(a.delay),
            })
            .collect(),
        // Both keys live and die together: while held they are `Some`, and the
        // instant the clock starts they are `None` and the snapshot is shaped
        // exactly as it has always been.
        waiting_for: ready.holding().then(|| ready.waiting_for()),
        match_started: ready.holding().then_some(false),
        game_over: game_over.winner.map(team_name),
        game_over_reason: game_over.reason.map(GameOverReason::name),
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
                    let (gold, lumber, time) =
                        hero_train_cost(records, me, k, &my_held_heroes);
                    let (revive_gold, revive_lumber) = unit_value(k);
                    HeroCostOut {
                        kind: kind_name(k),
                        gold,
                        lumber,
                        time,
                        revive: records.get(me, k).is_some(),
                        revive_gold,
                        revive_lumber,
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
            places: crate::shared::builtin_places(seat.team)
                .into_iter()
                .map(|r| RegionOut {
                    name: r.name,
                    pos: [r1(r.center.x), r1(r.center.z)],
                    radius: r1(r.radius),
                })
                .collect(),
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
        intel: intel_out,
        unlocked,
        units: units_out,
        buildings: buildings_out,
        squads,
        mines,
        trees_near,
        bounties: bounties_out,
        events,
        command_nodes,
        triggers,
        regions,
        plans,
        copilot: copilot_out,
        proposals: proposals_out,
        recent_resolutions: resolutions_out,
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

/// Everything only a copilot seat's snapshot carries, built once per write.
///
/// A struct rather than the tuple this used to be: four positional fields of
/// which two are `Vec`s of different things is exactly the shape that gets
/// swapped at a call site and compiles.
struct CoOut {
    copilot: CopilotOut,
    proposals: Vec<ProposalOut>,
    recent_resolutions: Vec<ResolutionOut>,
    partner_log: Vec<JournalOut>,
}

/// The pending queue in ANSWER order — urgent first, then oldest — which is
/// the order the human's panel shows and their approve/veto keys walk, so the
/// first entry here is the one `[Enter]` takes. `copilot::insert_index` keeps
/// `pending` in that order, so this is a straight copy and never a sort.
fn proposals_out(pending: &[Proposal], now: f32) -> Vec<ProposalOut> {
    pending
        .iter()
        .map(|p| ProposalOut {
            id: p.id,
            note: p.note.clone(),
            sentences: p.sentences.clone(),
            conflicts: p.conflicts.clone(),
            severity: p.severity.name(),
            expires_in: r1(p.expires_in(now)),
        })
        .collect()
}

/// The answered-proposal tail, oldest first — same direction as `events`,
/// `proposals` and `partner_log`, so a reader that walks one walks them all
/// the same way.
fn resolutions_out(resolved: &std::collections::VecDeque<Resolution>) -> Vec<ResolutionOut> {
    resolved
        .iter()
        .map(|r| ResolutionOut {
            id: r.id,
            t: r1(r.at),
            note: r.note.clone(),
            severity: r.severity.name(),
            outcome: r.outcome.name(),
            reason: r.outcome.reason().map(|reason| reason.wire()),
            advice: r.outcome.reason().map(|reason| reason.advice()),
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
    mut intent_applied: ResMut<IntentApplied>,
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
        // ...and so does the other half of the verdict: `applied` describes the
        // batch being acknowledged, never the one before it.
        intent_applied.get_mut(seat.team).clear();

        if game_over.winner.is_some() {
            seat.errors
                .push("batch: game over — commands ignored".to_string());
        } else {
            for (i, raw) in batch.commands.iter().enumerate() {
                // The historical error prefix, so a commander that greps for
                // `cmd 3` still finds its third command — both roles.
                let tag = format!("cmd {i}");
                // `ready` is the one verb that goes straight through on EVERY
                // seat, copilot included. It is a statement about the match,
                // not an order to the human's army, so routing it into the
                // proposal queue would ask a player to approve their partner's
                // willingness to start — and would hold the match until they
                // did. Handled ahead of the copilot branch rather than inside
                // copilot.rs so the gate has exactly one door.
                if raw.get("type").and_then(|v| v.as_str()) == Some("ready") {
                    submissions.write(SubmitIntent {
                        team: seat.team,
                        source: IntentSource::Bridge,
                        tag,
                        intent: Intent::Ready,
                        trigger: None,
                        plan: None,
                    });
                    continue;
                }
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
                            // A seat speaks for itself. Only trigger.rs sets
                            // this, and only for a rule it is firing.
                            trigger: None,
                            plan: None,
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

    // -----------------------------------------------------------------------
    // Co-command on the wire
    // -----------------------------------------------------------------------

    use crate::copilot::{Outcome, ProposalSeverity, Resolution, VetoReason};

    fn a_resolution(id: u32, outcome: Outcome) -> Resolution {
        Resolution {
            id,
            at: 12.25,
            note: "hit their siege".to_string(),
            severity: ProposalSeverity::Urgent,
            outcome,
        }
    }

    /// **A plan, end to end on the wire.** Parsed back out of the JSON rather
    /// than inspected as Rust, for the reason the veto test below gives: only
    /// the round trip catches a field that got renamed, skipped, or nested a
    /// level too deep.
    ///
    /// The contract is that `step`/`of`/`status` answer "is the sequence I
    /// wrote still happening" without any correlation work, and that `steps` is
    /// the JSON the commander sent — so reading a plan out, changing one step,
    /// and sending it back under the same name is a legal `plan_set`.
    #[test]
    fn a_plan_round_trips_through_the_snapshot_json() {
        let step = |json: &str| -> PlanStep { serde_json::from_str(json).expect("step parses") };
        let mut running = PlanRun {
            name: PlanName::new("boomer").unwrap(),
            steps: vec![
                step(
                    r#"{"intent":{"type":"build","worker":7,"kind":"Barracks","x":-60.0,"z":-60.0},
                        "advance":{"type":"when","when":{"type":"tier_reached","tier":2}}}"#,
                ),
                step(r#"{"intent":{"type":"train","building":9,"unit":"Sorcerer"}}"#),
            ],
            source: IntentSource::Bridge,
            state: PlanState::Running,
            at: 1,
            submitted: true,
            applied: true,
            applied_at: 12.0,
            last_try: 12.0,
            blocked_since: None,
            told_blocked: false,
        };
        let json = serde_json::to_string(&plans_out(std::slice::from_ref(&running))).unwrap();
        let back: Vec<serde_json::Value> = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0]["name"], "boomer");
        assert_eq!(back[0]["step"], 2);
        assert_eq!(back[0]["of"], 2);
        assert_eq!(back[0]["status"], "running");
        assert_eq!(back[0]["current"], "building 9 trains Sorcerer");
        assert!(back[0]["sentence"].as_str().unwrap().starts_with("plan boomer (2 steps): worker 7 builds"));

        // `steps` is what was sent, re-sendable: the terse step keeps its
        // omitted `advance` implicit and the explicit one keeps its predicate.
        let steps = back[0]["steps"].as_array().expect("an array of steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["intent"]["type"], "build");
        assert_eq!(steps[0]["advance"]["when"]["tier"], 2);
        assert_eq!(steps[1]["advance"]["type"], "on_applied");
        let resent: Intent = serde_json::from_value(serde_json::json!({
            "type": "plan_set",
            "name": back[0]["name"],
            "steps": back[0]["steps"],
        }))
        .expect("a plan read out of a snapshot is a legal plan_set");
        assert_eq!(resent.verb(), "plan_set");

        // A stopped plan carries the compiler's own words in its status, so
        // nothing has to be correlated against `errors`.
        running.state = PlanState::Blocked("not enough gold (need 160, have 120)".to_string());
        let json = serde_json::to_string(&plans_out(std::slice::from_ref(&running))).unwrap();
        let back: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(back[0]["status"], "blocked: not enough gold (need 160, have 120)");
    }

    /// **The snapshot echoes the stance, and only when there is one.**
    ///
    /// Two claims in one test, and the second is the compatibility one. A squad
    /// in a stance carries the word, so a commander that says nothing next poll
    /// reads back what its army is still doing — that echo is what makes
    /// persistence usable rather than merely true. A squad that is not in one
    /// carries **no key at all**, so a match that never sends a `stance`
    /// produces a `squads[]` byte-identical to the one that shipped before the
    /// feature existed. `skip_serializing_if` is doing that, and a `null` here
    /// would quietly break every reader that pins the shape.
    #[test]
    fn a_squads_stance_rides_the_snapshot_without_disturbing_the_old_shape() {
        let stanced = SquadOut {
            id: 1,
            posture: Some("push@(60,60)".to_string()),
            members: 6,
            stance: Some(StanceKind::Push.word()),
        };
        let hand_tasked = SquadOut {
            id: 2,
            posture: Some("defend@(-70,-70)r=22".to_string()),
            members: 3,
            stance: None,
        };
        let json =
            serde_json::to_string(&vec![stanced, hand_tasked]).expect("squads serialize");
        let back: Vec<serde_json::Value> = serde_json::from_str(&json).expect("parses");

        assert_eq!(back[0]["stance"], "push");
        // The historical keys are untouched beside it.
        assert_eq!(back[0]["id"], 1);
        assert_eq!(back[0]["members"], 6);
        assert_eq!(back[0]["posture"], "push@(60,60)");

        let plain: std::collections::BTreeSet<&str> =
            back[1].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            plain,
            ["id", "members", "posture"].into_iter().collect(),
            "a squad in no stance must serialize exactly as it always did"
        );
    }

    /// **The veto reason, end to end on the wire.** The human's answer is
    /// worth nothing to a co-commander unless it survives serialization as
    /// something a model can match on — so this parses the JSON back rather
    /// than inspecting the Rust structs, which is the only way to catch a
    /// field that got renamed, skipped or nested one level too deep.
    #[test]
    fn a_veto_reason_round_trips_through_the_snapshot_json() {
        let resolved: std::collections::VecDeque<Resolution> = [
            a_resolution(1, Outcome::Vetoed(VetoReason::NotNow)),
            a_resolution(2, Outcome::Vetoed(VetoReason::Never)),
            a_resolution(3, Outcome::Vetoed(VetoReason::WrongTarget)),
            a_resolution(4, Outcome::Approved),
            a_resolution(5, Outcome::Expired),
        ]
        .into();
        let json = serde_json::to_string(&resolutions_out(&resolved)).expect("serializes");
        let back: Vec<serde_json::Value> = serde_json::from_str(&json).expect("parses");

        assert_eq!(back.len(), 5, "oldest first, one entry each");
        let reasons: Vec<Option<&str>> = back
            .iter()
            .map(|r| r["reason"].as_str())
            .collect();
        assert_eq!(
            reasons,
            vec![
                Some("not_now"),
                Some("never"),
                Some("wrong_target"),
                None,
                None
            ],
            "the three vetoes name themselves; an approval and a lapse have \
             no reason to give, and the key is absent rather than null"
        );
        assert_eq!(
            back.iter().map(|r| r["outcome"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["vetoed", "vetoed", "vetoed", "approved", "expired"]
        );
        // Every veto also carries what it ASKS FOR, so acting on one correctly
        // needs no second document.
        assert_eq!(
            back[2]["advice"].as_str(),
            Some("re-propose with a different target")
        );
        assert!(back[3].get("advice").is_none(), "no advice without a veto");
        // And a resolution identifies the idea, not just a number the model
        // has to have remembered.
        assert_eq!(back[0]["note"].as_str(), Some("hit their siege"));
        assert_eq!(back[0]["severity"].as_str(), Some("urgent"));
        assert_eq!(back[0]["id"].as_u64(), Some(1));
        assert_eq!(back[0]["t"].as_f64(), Some(12.3), "rounded like every other t");
        // There is no `pending` outcome, because membership in `proposals` is
        // what pending means.
        assert!(
            !json.contains("pending"),
            "a status that restates a list membership is a second source of truth"
        );
    }

    /// The queue goes out in ANSWER order with its severity attached, so a
    /// co-commander can see that its urgency was accepted rather than assume
    /// it — and can tell which proposal `[Enter]` will take.
    #[test]
    fn the_pending_queue_carries_its_severity_in_answer_order() {
        let proposal = |id: u32, severity| Proposal {
            id,
            note: "n".to_string(),
            intents: Vec::new(),
            sentences: vec!["s".to_string()],
            conflicts: Vec::new(),
            severity,
            proposed_at: 0.0,
            expires_at: 20.0,
            pos: None,
        };
        // Exactly the order `copilot::insert_index` maintains.
        let pending = vec![
            proposal(3, ProposalSeverity::Urgent),
            proposal(1, ProposalSeverity::Routine),
        ];
        let out = serde_json::to_value(proposals_out(&pending, 5.0)).expect("serializes");
        assert_eq!(out[0]["id"].as_u64(), Some(3));
        assert_eq!(out[0]["severity"].as_str(), Some("urgent"));
        assert_eq!(out[0]["expires_in"].as_f64(), Some(15.0));
        assert_eq!(out[1]["id"].as_u64(), Some(1));
        assert_eq!(out[1]["severity"].as_str(), Some("routine"));
    }

    /// The seat learns its own etiquette by reading, never out of band — so
    /// the vocabulary the human can answer with, and the two words `severity`
    /// accepts, are in the snapshot next to `direct`.
    #[test]
    fn the_copilot_block_advertises_the_whole_negotiation_vocabulary() {
        let out = CopilotOut {
            trust: "split",
            direct: vec!["posture"],
            propose_ttl: crate::copilot::PROPOSAL_TTL,
            max_pending: crate::copilot::MAX_PENDING,
            severities: ProposalSeverity::NAMES,
            veto_reasons: VetoReason::all()
                .into_iter()
                .map(|r| (r.wire(), r.advice()))
                .collect(),
            auto_approve_after: None,
        };
        let json = serde_json::to_value(&out).expect("serializes");
        assert_eq!(json["severities"][0].as_str(), Some("routine"));
        assert_eq!(json["severities"][1].as_str(), Some("urgent"));
        assert_eq!(
            json["veto_reasons"]["never"].as_str(),
            Some("do not re-propose this match")
        );
        assert_eq!(
            json["veto_reasons"]["not_now"].as_str(),
            Some("re-propose when conditions change")
        );
        // Absent in a real match: a seat must be able to tell whether the
        // thing answering it is a person.
        assert!(
            json.get("auto_approve_after").is_none(),
            "no scripted approver, no key"
        );

        let scripted = CopilotOut {
            auto_approve_after: Some(3.0),
            ..out
        };
        assert_eq!(
            serde_json::to_value(&scripted).unwrap()["auto_approve_after"].as_f64(),
            Some(3.0)
        );
    }
}
