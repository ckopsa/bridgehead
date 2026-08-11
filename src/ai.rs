//! ai.rs — the scripted RTS brain. Always drives the Claude faction (red, NE
//! base) and optionally the Human one too (blue, SW base) for AI-vs-AI
//! spectating: launch with `WC3_AI_BOTH=1`, or press F9 at runtime.
//!
//! A macro-focused RTS AI that plays strictly through the same primitives the
//! human UI uses — and, since wc3clone-jem, through the *identical* ones.
//!
//! ## The third seat
//!
//! This file mutates nothing. Its entire output is `SubmitIntent` events with
//! `IntentSource::Script`: the same `shared::Intent` values ui.rs compiles out
//! of a right-click and bridge.rs deserializes out of `commands.json`, read by
//! the same `intent.rs` compiler, in the same frame. It used to write `Order`
//! components, push `TrainingQueue`s and send `UpgradeBuilding`/`StartResearch`
//! /`BuyItem`/`UseItem`/`CastAbility` directly, which made the fairness
//! invariant a claim with a footnote attached. There is no footnote now.
//!
//! What that buys, concretely:
//!
//!   * **Validation.** The script is refused by the same rules a commander is
//!     — fog, ownership, tech gates, affordability, hero slots, queue caps. It
//!     cannot place a building on ground a player would be told is blocked.
//!   * **The replay.** `WC3_INTENT_LOG` now records all three authors. A sim
//!     against the scripted baseline is readable as a match transcript rather
//!     than as one commander talking into a silence.
//!   * **Provenance.** Its units answer `order:attackmove by script t=…`,
//!     which joins against the log by exactly the rule everyone else joins by.
//!   * **Latency.** The compiler prices the link (docs/TEMPO.md §3). ai.rs no
//!     longer reaches for `OrderIssuer` itself to stay honest — it is honest
//!     because it has no other way to act.
//!
//! **Planning is not acting.** The ring-fences, reserves, cadences and site
//! choices below stay exactly where they are: deciding what to want is this
//! file's job, and doing it is the compiler's. The one place the two touch is
//! build placement, which snaps to the compiler's nav lattice *before* the
//! script vets the ground — see `think`'s build section.
//!
//! **Rejection is normal.** A think tick states what it wants against the
//! world it saw; the compiler judges it against the world as it is. When the
//! two disagree the intent is refused, the refusal goes to the debug log (not
//! to any seat's error channel — see `intent::apply_intents`), and the script
//! re-thinks a second later. Nothing latches on a rejection: every optimistic
//! bookkeeping flag below (`pending_build` above all) re-arms itself from what
//! the world actually looks like on the next tick.
//!
//! Everything runs from one `ai_think` system on a ~1s timer, which runs the
//! same `think` body once per AI-controlled team against that team's own
//! `AiBrain`. Nothing is positional: every base/target lookup is derived from
//! the team being thought for. All difficulty knobs live in the const block
//! below.

use crate::intent::snap_footprint;
use crate::shared::*;
// The map's published geography. Read-only, and the same three facts a bridge
// commander is handed in every snapshot (`map.chokepoints`) — the scripted AI
// is not being told anything a seat isn't.
use crate::terrain::active_map;
use bevy::prelude::*;

// ---------------------------------------------------------------------------
// Tuning knobs
// ---------------------------------------------------------------------------

/// Seconds between AI "thoughts". Cheap enough to raise, dumb enough to lower.
const THINK_INTERVAL: f32 = 1.0;

/// Economy.
const TARGET_WORKERS: usize = 12;
/// Every Nth idle worker assignment goes to lumber instead of gold (~70/30).
const LUMBER_EVERY_NTH: u32 = 3;
/// Build a farm when free supply drops below this.
const SUPPLY_BUFFER: u32 = 4;

/// Production.
const MAX_BARRACKS: usize = 2;
const SECOND_BARRACKS_GOLD: u32 = 400;
const BARRACKS_QUEUE_MAX: usize = 2;
/// Every Nth army unit is an Archer, the rest Footmen (~2:1).
const ARCHER_EVERY_NTH: u32 = 3;
/// Every Nth army unit is a Raider instead — cavalry to chase siege and
/// workers. Checked before the Archer slot, so the 15th unit is a Raider.
const RAIDER_EVERY_NTH: u32 = 5;
/// Every Nth army unit is a Spearman — checked last, so it only takes slots
/// the Raider and Archer rules left alone (~1 in 6 of actual Barracks output).
/// A flat fraction rather than a reaction to scouted cavalry: the script has
/// no memory of what it has seen, and a standing hedge in front of the archers
/// is worth its 90 gold as cheap hit points even in a match where the enemy
/// never mounts up. Reacting properly is a commander's job, and a commander
/// gets the same unit through the same catalog.
const SPEARMAN_EVERY_NTH: u32 = 4;
/// Every Nth army unit is a Knight, but ONLY once a Castle stands. Checked
/// first, so a tier-3 team's Barracks really does put a 270g line-breaker on
/// the field instead of a fourth Footman — and checked at 7, so it is roughly
/// one unit in seven and never the backbone. The script has no scouting memory
/// and cannot tell a Spearman screen from an Archer line, so it must not lean
/// on a unit that a 90-gold counter deletes; a commander reading the catalog
/// can, and that difference is the point.
const KNIGHT_EVERY_NTH: u32 = 7;

/// ---- Reactive composition ------------------------------------------------
///
/// The script has no scouting *memory* and is not allowed one: everything below
/// keys off units this team can SEE THIS TICK through its own fog grid, and
/// decays on a timer afterwards. Two decaying counters, no planner.
///
/// Why fog and not `GameEvents`: the feed's whole vocabulary is own-side facts —
/// "lost 3 Footman near (x,z)", "hero low: 40%", "N hostiles near base",
/// "squad 2 wiped". It never names an ENEMY unit kind, so it cannot tell the
/// script that the thing killing it was a Raider rather than a Catapult. The
/// fog grid can, and reading it is the same honesty the rest of `think` already
/// obeys (`fog.sees` gates every enemy fact in the snapshot below).
///
/// Think ticks an alert stays live after the last sighting. At
/// `THINK_INTERVAL` = 1s this is ~50 seconds — long enough to actually change
/// the mix coming out of a Barracks (a Footman is ~20s of queue), short enough
/// that one dead scout flyer doesn't rebuild the army.
const ALERT_TICKS: u32 = 50;
/// Archer cadence while enemy AIR has been seen. Replaces `ARCHER_EVERY_NTH`,
/// so roughly half the Barracks output becomes anti-air instead of a third.
const ARCHER_EVERY_NTH_AIR: u32 = 2;
/// Spearman cadence while enemy CAVALRY (Raider or Knight) has been seen.
const SPEARMAN_EVERY_NTH_CAVALRY: u32 = 2;
/// ...and the cadence it degrades to when BOTH alerts are live at once. With
/// air and cavalry on the field the Archer rule already owns every second slot,
/// so leaving the Spearman rule at 2 would starve it to nothing; 3 keeps a
/// visible screen in front of the archers without out-bidding them.
const SPEARMAN_EVERY_NTH_BOTH: u32 = 3;

/// ---- Static defense ------------------------------------------------------
///
/// Towers cost no supply. That is the whole danger: every other purchase in
/// this file competes with the army for a supply cap that farms have to be
/// built to raise, and a Tower competes with nothing. An AI allowed to answer
/// "am I safe?" with "one more Tower" builds twenty-five of them, never pushes,
/// and turns the scripted matchup — the decisive 5-20 minute baseline every
/// balance run measures against — into a mutual siege that ends on the time
/// cap. So the count is capped by construction, not by budget:
/// `tower_quota` can never return more than `MAX_TOWERS`, and the build branch
/// re-checks the same cap against what is actually standing.
//
// ONE, not two, and this number was measured rather than chosen. Against the
// pre-bead baseline the `open` map converges in 7.1 / 7.1 / 7.2 / 8.4 minutes —
// four runs, all decisive, remarkably tight. With two base towers a side it
// went 6.7 / 10.3 / 23.3 / 25.0-and-timed-out: a 550 HP emplacement that shoots
// 16 at range 16 does not merely cost 110 gold, it makes a 6-14 unit wave
// bounce, and when BOTH scripted sides own that, neither can ever close and the
// match runs to the cap with two intact economies staring at each other. That
// is precisely the turtle failure this file is not allowed to have. Notably the
// `crossings` runs stayed decisive (5.5-12.4) with the same code, because there
// the towers go to a ford instead of the front door — which is the clearest
// possible evidence that the problem was base defense, not the spending.
const BASELINE_TOWERS: usize = 1;
/// Hard ceiling on towers the script will ever own, baseline plus reactive.
/// Four is one more than the rules below can currently ask for (2 baseline + 1
/// on air contact): the slack is deliberate, so a future reactive rule has room
/// to exist without anybody having to remember that the ceiling was implicit.
const MAX_TOWERS: usize = 4;
/// Gold in hand before the AI spends 110g/80l on a baseline Tower. A reactive
/// (air-contact) Tower ignores this: by the time a Gryphon is overhead, "we
/// cannot afford to answer it" is not a position, it is a loss.
// 240 -> 350, alongside the second-hall gate below. Both exist to push the
// baseline tower late and make it come out of genuine surplus, for the pacing
// reason recorded on BASELINE_TOWERS.
const TOWER_GOLD: u32 = 350;

/// ---- Fortifying a crossing ----------------------------------------------
///
/// On a map that publishes chokepoints, a Tower in the base ring is a Tower
/// pointed at nothing: everything that will ever attack us has to walk through
/// a ford first. So towers go to the ford instead — but only a ford we already
/// live next to. Distance from our nearest hall to the ford, above which the
/// script gives up and fortifies home. On `crossings` the two flank fords ARE
/// the neutral gold mines, so this fires exactly once the expansion lands: the
/// same building then guards the second mine and the crossing, which is the
/// map's central claim ("taking a second mine and holding a crossing are the
/// same decision") turned into behaviour. The centre ford sits ~99 from either
/// start position; walking a lone worker out there to plant one tower in the
/// middle of the map is how a script donates 110 gold.
const FORD_HOLD_RADIUS: f32 = 45.0;
/// How far back from the gap's centre the emplacement sits, measured toward
/// our own side. A Tower shoots 16 and sees 20, so standing off keeps the whole
/// opening covered while putting the structure behind the fight rather than in
/// it.
const FORD_STANDOFF: f32 = 9.0;
/// Wall segments the script will plant beside a ford tower. Two, ever.
const FORD_WALLS: usize = 2;
/// ...and only in a gap at least this wide. A wall is 2 wide and a tower is 3;
/// dropping them into the 16-wide centre ford would start narrowing the one
/// route our OWN army uses to attack with, and an AI that walls itself in is
/// the turtle failure mode wearing a different hat. The flank fords are 30
/// wide, so this admits them and nothing else.
const FORD_WALL_MIN_WIDTH: f32 = 20.0;
/// Offsets along the barrier from the tower, where wall segments go.
const FORD_WALL_OFFSETS: [f32; 2] = [-5.0, 5.0];
/// Radius around the hold point counted as "at this crossing" when the script
/// asks whether it has already garrisoned the ford.
const FORD_AREA: f32 = 14.0;
/// Rings searched outward from an emplacement's ideal spot. Much tighter than
/// `BUILD_RING_RADII`: a Tower wants to be AT a place, and a Tower 12 units off
/// its ford is a Tower covering the wrong ground.
const EMPLACE_RING_RADII: [f32; 4] = [3.0, 5.0, 8.0, 12.0];

/// ---- Shop and hero items -------------------------------------------------
///
/// Gold in hand before the AI spends 75g/60l on a Shop, and a Keep standing.
/// The Shop is the cheapest building in the game after a Farm, but it produces
/// nothing: it is a *conversion* — surplus gold into hero uptime — so it is
/// gated like every other surplus purchase here.
const SHOP_GOLD: u32 = 300;
/// Hero HP fraction at or below which the script drinks a held potion.
const POTION_HP_FRAC: f32 = 0.5;
/// Gold in hand before the script restocks the 100g potion — its own price plus
/// a Footman's worth of change, so buying one is never the reason a Barracks
/// went idle.
const POTION_RICH_GOLD: u32 = 200;
/// "Idle-rich": gold in hand before the script buys the 50g Boots. Boots are a
/// convenience, the last thing on the shelf worth buying, so the bar is high.
const BOOTS_RICH_GOLD: u32 = 350;
/// Gold in hand before the script buys the 125g tier-2 Banner.
const BANNER_RICH_GOLD: u32 = 300;
/// Enemies within `BANNER_RADIUS` of the hero that make a fight worth a Banner.
const BANNER_MIN_TARGETS: usize = 3;

/// Siege. A Workshop is a luxury: only once a Barracks stands and the treasury
/// is comfortably ahead of army production does the AI branch into siege.
// 350, not 500: sim runs showed peak treasury in short games (~320) never
// clears a 500 gate, so siege only appeared in long games — the opposite of
// its purpose.
/// Gold on hand before the script commits to an Arcane Sanctum. The Keep gate
/// is doing most of the work already — this only stops the purchase coming out
/// of an empty treasury the frame the upgrade lands.
const SANCTUM_GOLD: u32 = 300;
/// How many Sorcerers the script ever wants alive at once. A few, not a
/// screen: `StatusKind::Slow` REFRESHES rather than stacks, so the fourth
/// caster adds frontage and nothing else, and each one is 2 supply of almost
/// no combat value.
const MAX_SORCERERS: usize = 3;
const SANCTUM_QUEUE_MAX: usize = 1;
/// Fighting units (heroes excluded) the script wants on the field before it
/// opens a SECOND hero slot. A Keep lands with the treasury already stretched,
/// and a 400g/100l hero bought out of that leaves a base defended by two
/// characters and twelve workers — which is exactly what the first tier-2 sim
/// run produced. Reviving a hero the team already owns is not gated by this:
/// a level-6 Champion at 250g is the best gold on the map.
const SECOND_HERO_MIN_ARMY: usize = 6;

const WORKSHOP_GOLD: u32 = 350;
const WORKSHOP_QUEUE_MAX: usize = 2;
/// Target mix: one Catapult per this many Barracks-produced line units.
const CATAPULT_PER_ARMY: u32 = 4;
/// Every Nth Workshop item is a Gryphon Rider instead of a Catapult, once a
/// Castle stands AND the bank is genuinely fat. Kept deliberately RARE.
///
/// The scripted AI does not build Towers (known gap, wc3clone-7gv) and does not
/// mass Archers on purpose, so in an AI-vs-AI match neither side reliably holds
/// the counter to air. A script that spent freely on flyers would therefore win
/// on a unit the baseline cannot answer, and the scripted matchup would stop
/// being the decisive, readable baseline the era runs measure against. So air
/// appears — tier-3 content that never shows up is not content — but only out
/// of surplus, and only about one Workshop item in three.
const GRYPHON_EVERY_NTH: u32 = 3;
/// ...and only with this much gold banked after the reserve. A Gryphon is the
/// last thing the script buys, never the thing it saves for.
const GRYPHON_BANK_GOLD: u32 = 700;

/// ---- Making the air branch observable ------------------------------------
///
/// The two constants above are so conservative that across every sim run of
/// the era, NO scripted match ever produced a Gryphon: it needs a Castle, a
/// Workshop, 700 gold spare after the reserve, and the siege counter on its
/// every-third beat, all true on the same think tick. That is the correct
/// default — the reasoning above is not a bug — but it meant the air branch
/// and everything downstream of it (the enemy's archer shift, the reactive
/// Tower) had never once run in a real match. Content nobody has seen is
/// indistinguishable from content that does not work.
///
/// So the two gates are env-tunable, and ONLY the two gates: unset, they read
/// their constants and the script's behaviour is byte-identical to before.
/// `WC3_AI_GRYPHON_BANK=0 WC3_AI_GRYPHON_NTH=1 WC3_HEADLESS=1 cargo run` puts
/// flyers in the air as soon as a Castle and Workshop stand, which is how the
/// path gets exercised in a real sim rather than only in a unit test.
///
/// These are probe knobs, not balance knobs. A run with them set is not a
/// baseline run and its timings mean nothing.
const GRYPHON_BANK_ENV: &str = "WC3_AI_GRYPHON_BANK";
const GRYPHON_NTH_ENV: &str = "WC3_AI_GRYPHON_NTH";

/// Read a `u32` override once per process. Anything unparseable is ignored
/// rather than fatal — a typo in a probe knob must not change a match.
fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

/// `GRYPHON_BANK_GOLD`, or the `WC3_AI_GRYPHON_BANK` override.
fn gryphon_bank_gold() -> u32 {
    static VALUE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| env_u32(GRYPHON_BANK_ENV, GRYPHON_BANK_GOLD))
}

/// `GRYPHON_EVERY_NTH`, or the `WC3_AI_GRYPHON_NTH` override, floored at 1 —
/// the value is a modulus, and a zero here would panic the think tick rather
/// than mis-plan it.
fn gryphon_every_nth() -> u32 {
    static VALUE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| env_u32(GRYPHON_NTH_ENV, GRYPHON_EVERY_NTH).max(1))
}

/// Building placement: rings of candidate offsets around the base.
const BUILD_PADDING: f32 = 2.0;
const BUILD_RING_RADII: [f32; 4] = [12.0, 16.0, 20.0, 25.0];
const BUILD_RING_SPOKES: usize = 16;

/// Expansion. A gold mine is finite (thousands, not endless), so a base that
/// never expands simply stops earning halfway through a long game. A mine
/// counts as *ours* once one of our TownHalls stands within this range — the
/// starting hall sits ~19.2 away from its home mine, so this must clear that.
const MINE_CLAIM_RADIUS: f32 = 26.0;
/// Expand once the gold left across all claimed mines drops below this. Sized
/// for lead time, not panic: the trek to a neutral mine plus a 40s build has to
/// finish *before* the home mine runs dry.
const EXPAND_GOLD_LEFT: u32 = 2000;
/// ...or once we are running more gold workers than this per claimed mine.
/// At the scripted worker target (~8 on gold) this never fires on its own; it
/// exists so an over-saturated line — a commander's, or one inherited after a
/// mine died — still gets a second mine to spread across.
const WORKERS_PER_MINE: usize = 8;
/// Never plant an expansion within this range of an enemy combat unit. Mirrors
/// economy.rs's danger-aware auto-rebalance, with slack: a building can't run.
const EXPAND_DANGER_RADIUS: f32 = 24.0;
/// ...nor this close to the enemy's main base. Their home mine is inside this
/// ring, so the script never tries to settle in someone else's front yard.
const ENEMY_BASE_KEEPOUT: f32 = 45.0;
/// Rings of candidate hall sites around the mine we are expanding to. The
/// inner ring keeps the haul short without overlapping the mine's footprint.
const EXPAND_RING_RADII: [f32; 4] = [10.0, 13.0, 16.0, 20.0];
/// A worker within this of a mine is treated as working it (used only as a
/// fallback when its `Order::Harvest` no longer names a live node).
const MINE_WORKER_RADIUS: f32 = 16.0;
/// Workers moved between mines per think tick — a trickle, so the line never
/// abandons a mine wholesale.
const SHIFT_PER_TICK: usize = 2;

/// Military.
const RALLY_DIST: f32 = 18.0;
const RALLY_ARRIVE_DIST: f32 = 6.0;
const FIRST_WAVE_SIZE: usize = 6;
const WAVE_SIZE_STEP: usize = 3;
const WAVE_SIZE_CAP: usize = 14;
/// Below this many army units a wave is considered wiped out.
const WAVE_ABORT_ARMY: usize = 3;
/// After this long a stalled wave re-targets the nearest enemy building.
const WAVE_TIMEOUT: f32 = 90.0;
/// Enemy units this close to our base trigger a full defensive recall.
const DEFEND_RADIUS: f32 = 30.0;
/// Workers this close to an enemy combat unit run home.
const WORKER_FLEE_RADIUS: f32 = 10.0;
/// Army units on the field before the AI spends 320g/160l on a Keep. Low on
/// purpose: the tier-up is meant to land in the 4-6 minute window, right after
/// the first Barracks has produced a real squad, not once the game is decided.
const KEEP_MIN_ARMY: usize = 6;
/// The Castle is a late-game surplus purchase, so it wants a standing army...
const CASTLE_MIN_ARMY: usize = 6;
/// ...and this much gold still in the bank AFTER paying for it.
//
// 300 -> 60. The old gate was `army >= 6 && gold - 480 >= 300`, i.e. 780 gold
// in hand at the instant of a think tick, on top of 240 lumber. Across every
// sim on record that number was reached exactly never: the scripted economy
// runs two Barracks flat out and converges the match in 7-13 minutes, and its
// peak treasury in that window is a few hundred. So tier 3 was unreachable in
// practice — Knights and Gryphon Riders were content the baseline could not
// produce, and the whole tier-3 branch of this file had never once executed.
//
// The fix is not "make the Castle cheap", it is "ask the right question". A
// Castle is worth buying when the economy behind it is *durable*, and the
// script already has two hard, observable proofs of durability that it was
// throwing away: an attack-research rung finished (the forge only exists at
// Keep, and only bought a rung out of surplus), or a second mining base
// standing. Either one means the treasury has already survived a real
// withdrawal. Given that proof, the leftover-gold test can drop to a token
// buffer — it is there so the Castle is never the last gold in the bank, not
// as a second durability test.
const CASTLE_SPARE_GOLD: u32 = 60;
/// Gold in hand before the Castle's price may be ring-fenced against army
/// production. The reserve is the dangerous half of this feature: holding 480g
/// and 240l back from the Barracks in a game that will never reach it is how a
/// script stops building units and loses to its own ambition. So the latch only
/// arms once the bank is already within striking distance — half the price —
/// exactly the guard `RESEARCH_SPARE_GOLD` puts on the research reserve.
const CASTLE_LATCH_GOLD: u32 = 240;
/// Gold in hand before the AI spends 140g/80l on a Blacksmith.
// 180, not 260. The forge sits below the Workshop in the `want` chain, and the
// Workshop's own gate is `gold > 350` — so the window in which the chain even
// REACHES the forge is the band between the two thresholds, plus whatever is
// left once a Workshop already stands. Sim runs at 260 built the forge once, at
// t=520 in a match that ended at t=530, and never researched anything: the
// scripted economy banks lumber and runs gold-poor (1135 lumber unspent against
// 20 gold in one trace), so a gold gate set by eye is a gate that never opens.
const BLACKSMITH_GOLD: u32 = 180;
/// Gold left AFTER paying for a research rung. The whole justification for the
/// mechanic is converting a SURPLUS into fighting strength, so the test is
/// against what remains, exactly like `CASTLE_SPARE_GOLD` — research must never
/// come out of the army budget, or a scripted opponent that researches is
/// simply a scripted opponent with fewer footmen.
// 120 for the same reason as above: at 200 a level-1 rung needed 300 gold in
// hand at the moment a forge happened to be idle, which in an 8-minute match
// coincided approximately never.
const RESEARCH_SPARE_GOLD: u32 = 120;
/// Army on the field before research is worth buying. A +1 attack upgrade
/// applied to two Footmen is 2 damage a swing; applied to a dozen it is the
/// reason the fight is won. Research pays off in proportion to how much army
/// it is multiplying, so the AI waits until it has one. Set to `KEEP_MIN_ARMY`:
/// the same "the opening is genuinely over" test the tier-up uses, and research
/// cannot start before the Keep that gates the forge anyway.
const RESEARCH_MIN_ARMY: usize = KEEP_MIN_ARMY;
/// Slam is worth casting once this many enemies stand in (or just outside) it.
const SLAM_MIN_TARGETS: usize = 3;
/// Slack added to `hero_ability_radius()` when counting slam targets.
const SLAM_RADIUS_SLACK: f32 = 2.0;

// ---------------------------------------------------------------------------
// Plugin & state
// ---------------------------------------------------------------------------

/// Env var that puts the Human side under AI control from startup.
const AI_BOTH_ENV: &str = "WC3_AI_BOTH";
/// Runtime toggle for AI control of the Human side.
const AI_TOGGLE_KEY: KeyCode = KeyCode::F9;

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AiState>()
            .add_systems(Startup, ai_apply_env)
            // `ai_think` after `FogSet`: the scripted commander plans from
            // this frame's visibility, exactly like the bridge seats.
            .add_systems(Update, ai_toggle_hotkey.in_set(SimSet::Input))
            .add_systems(
                Update,
                // Chained and boxed into `SimSet::AiThink`, which sits between
                // the fog recompute and the compiler: the scripted commander
                // plans from this frame's visibility, and the `SubmitIntent`s
                // it writes are drained by `apply_intents` in `SimSet::Intent`
                // — four sets later, the SAME frame. Nothing is deferred by a
                // tick.
                //
                // **The one thing that did move** (wc3clone-jem): the script's
                // actions used to land in `AiThink`, i.e. BEFORE doctrine.rs
                // ran in `SimSet::Think`, so a squad posture or a retreat
                // trigger could overwrite an order the script had given in the
                // same frame. They land in `SimSet::Intent` now, after
                // doctrine — which is precisely where a human's right-click
                // and a bridge command have always landed. The script lost a
                // privilege rather than gaining one, and it only bites on a
                // faction that is under autopilot *and* still carrying
                // doctrine a player set before handing it over.
                (seed_machine_autocast, ai_think)
                    .chain()
                    .in_set(SimSet::AiThink)
                    .after(FogSet),
            );
    }
}

/// Own heroes and whatever auto-cast doctrine they already carry.
type HeroPolicyQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Unit,
        &'static Team,
        &'static Health,
        Option<&'static AutoCastPolicy>,
    ),
    With<Hero>,
>;

/// Standing ultimate doctrine for the heroes of teams THIS module is driving.
///
/// The scripted commander casts Slam by hand (it knows what a clump of enemies
/// on the Champion is worth). Ultimates are a different kind of decision —
/// long cooldown, situational, worthless when spent early — so instead of
/// scripting them, the AI writes an `AutoCastPolicy` and lets doctrine.rs's
/// auto-caster decide, under exactly the gate a player's button obeys:
///
///   * Warcry — 4+ own units AND 4+ enemies inside radius 8 (doctrine.rs asks
///     the offensive-buff question twice, so it never fires at a worker line);
///   * Sanctuary — 3+ own units below 60% HP inside radius 7.
///
/// Rules are named, not numbered (`machine_autocast_rules`), and installed
/// only while the team is machine-driven: flip F9 or take a seat on the bridge
/// and the rules stop being re-applied, so a human or an LLM commander keeps
/// its ultimates in its own hands. Idempotent — once the rules match, nothing
/// is written.
fn seed_machine_autocast(
    ai_controlled: Res<AiControlled>,
    mut intents: EventWriter<SubmitIntent>,
    heroes: HeroPolicyQuery,
) {
    for (entity, unit, team, health, policy) in &heroes {
        let driving = match team {
            Team::Human => ai_controlled.human,
            Team::Claude => ai_controlled.claude,
        };
        if !driving || health.current <= 0.0 {
            continue;
        }
        let wanted = machine_autocast_rules(unit.kind);
        if wanted
            .iter()
            .all(|(index, min)| policy.and_then(|p| p.min_enemies_for(*index)) == Some(*min))
        {
            continue;
        }
        // One line per hero (so, per revive): the standing doctrine a
        // machine-driven team just acquired, by ability name rather than slot.
        let list = abilities_of_unit(unit.kind);
        info!(
            "{team:?}: ultimate auto-cast doctrine installed — {}",
            wanted
                .iter()
                .map(|(index, min)| format!(
                    "{} at {min}+ targets",
                    list.get(*index).map_or("?", |d| d.name)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // One `autocast` intent per rule — the verb is per-slot for exactly
        // this reason ("a hero told to auto-heal does not thereby stop
        // auto-slamming"), so the script edits the rules it owns and leaves
        // anything else on the policy alone, which is what the direct
        // `AutoCastPolicy::set` loop this replaced did by hand.
        //
        // Idempotent through the compiler as well as before it: the check
        // above skips a hero whose policy already matches, and the compiler's
        // writes flush at the end of `SimSet::Intent`, so the next frame reads
        // the applied policy and says nothing.
        for (index, min) in &wanted {
            intents.write(SubmitIntent::script(
                *team,
                Intent::Autocast {
                    units: vec![intent_id(entity)],
                    min_enemies: Some(*min),
                    ability: Some(AbilitySelector::Index(*index)),
                },
            ));
        }
    }
}

/// Everything one team's brain remembers between thoughts. Two of these live
/// side by side; a brain never reads or writes the other team's copy.
struct AiBrain {
    /// Worker we last handed an `Order::Build` to (one build in flight).
    pending_build: Option<Entity>,
    /// That in-flight build is an expansion TownHall. An expansion site is a
    /// long walk from home, and economy.rs only takes the money when the
    /// builder *arrives* — so the price has to stay ring-fenced for the whole
    /// trip, or the Barracks spends it en route and the build is refused on
    /// arrival (observed: three expansions ordered, none placed, income zero).
    expansion_pending: bool,
    /// Own hall count at the last thought — logged on change, which is how an
    /// expansion completing (or being razed) shows up in a sim trace.
    last_halls: usize,
    /// Highest hall tier we held at the last thought, logged on change so a
    /// tier-up is visible in a sim trace without reading the event feed.
    last_tier: u32,
    /// A tier-up is wanted but not yet paid for. Ring-fences the price against
    /// army production for the same reason the expansion does: continuous
    /// Footmen keep a treasury permanently a few gold short of a 320g upgrade,
    /// and the AI would "want" to tech forever. Unlike the expansion this
    /// clears on the very next thought once the order lands, because an
    /// upgrade is paid the instant it is accepted — nobody has to walk there.
    tierup_pending: bool,
    /// A research rung is wanted but not yet paid for. Same ring-fence as
    /// `tierup_pending`, and for the identical reason: continuous Footman
    /// production keeps a treasury permanently a few gold short of a 175g
    /// upgrade, so without holding the price back the AI would "want" to
    /// research forever and never do it. Cleared on the next thought once the
    /// order lands, because research is paid the instant it is accepted.
    research_pending: bool,
    /// An item is wanted at the Shop but not yet affordable. Same ring-fence,
    /// smallest stake: a 125g Banner is one Footman, and one Footman is exactly
    /// what continuous production spends it on. Latched only when the bank is
    /// already at least half the price (`item_reserve`), so a poor game never
    /// quietly stops training to save for a consumable.
    item_pending: bool,
    /// Think ticks of "enemy air has been seen" left. Set to `ALERT_TICKS` on
    /// every sighting through our own fog, decremented once per thought. Not a
    /// memory of WHERE — a decaying flag, nothing more.
    air_alert: u32,
    /// Same, for enemy cavalry (Raider or Knight).
    cavalry_alert: u32,
    /// Whether each alert was live at the last thought, so the trace logs the
    /// shift starting and ending once instead of every second.
    air_alert_logged: bool,
    cavalry_alert_logged: bool,
    harvest_counter: u32,
    army_counter: u32,
    /// Catapults queued so far — paced against `army_counter`.
    siege_counter: u32,
    wave_active: bool,
    wave_started: f32,
    wave_target: Vec3,
    next_wave_size: usize,
}

impl AiBrain {
    fn new(team: Team) -> Self {
        AiBrain {
            pending_build: None,
            expansion_pending: false,
            last_halls: 0,
            last_tier: 1,
            tierup_pending: false,
            research_pending: false,
            item_pending: false,
            air_alert: 0,
            cavalry_alert: 0,
            air_alert_logged: false,
            cavalry_alert_logged: false,
            harvest_counter: 0,
            army_counter: 0,
            siege_counter: 0,
            wave_active: false,
            wave_started: 0.0,
            wave_target: team.enemy().base_pos(),
            next_wave_size: FIRST_WAVE_SIZE,
        }
    }
}

#[derive(Resource)]
struct AiState {
    /// Shared across brains: both teams think on the same tick.
    timer: Timer,
    human: AiBrain,
    claude: AiBrain,
}

impl AiState {
    fn brain_mut(&mut self, team: Team) -> &mut AiBrain {
        match team {
            Team::Human => &mut self.human,
            Team::Claude => &mut self.claude,
        }
    }
}

impl Default for AiState {
    fn default() -> Self {
        AiState {
            timer: Timer::from_seconds(THINK_INTERVAL, TimerMode::Repeating),
            human: AiBrain::new(Team::Human),
            claude: AiBrain::new(Team::Claude),
        }
    }
}

/// `WC3_AI_BOTH=1 cargo run` — hand the Human side to the AI as well.
fn ai_apply_env(mut ai_controlled: ResMut<AiControlled>) {
    let enabled = std::env::var(AI_BOTH_ENV)
        .map(|raw| !raw.is_empty() && raw != "0")
        .unwrap_or(false);
    if enabled {
        ai_controlled.human = true;
        info!("{AI_BOTH_ENV}: AI vs AI — the AI is playing Blue too");
    }
    // Announce the probe knobs, and only when they are actually doing
    // something: a run whose trace does not say it was tuned is a run somebody
    // will later read as a baseline.
    if gryphon_bank_gold() != GRYPHON_BANK_GOLD || gryphon_every_nth() != GRYPHON_EVERY_NTH {
        info!(
            "{GRYPHON_BANK_ENV}/{GRYPHON_NTH_ENV}: air branch tuned to bank {}g, every {} \
             Workshop item (defaults {}g / {}) — NOT a baseline run",
            gryphon_bank_gold(),
            gryphon_every_nth(),
            GRYPHON_BANK_GOLD,
            GRYPHON_EVERY_NTH,
        );
    }
}

/// F9 hands Blue to the AI and back. Handing it back is deliberately a no-op
/// beyond the flag: units simply keep their last orders.
fn ai_toggle_hotkey(keys: Res<ButtonInput<KeyCode>>, mut ai_controlled: ResMut<AiControlled>) {
    if !keys.just_pressed(AI_TOGGLE_KEY) {
        return;
    }
    ai_controlled.human = !ai_controlled.human;
    if ai_controlled.human {
        info!("AI took over Blue");
    } else {
        info!("Blue back under player control");
    }
}

/// Coarse view of an order, so the snapshot can be taken read-only.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Tag {
    Idle,
    Move,
    AttackMove,
    Build,
    Busy,
}

fn tag_of(order: &Order) -> Tag {
    match order {
        Order::Idle => Tag::Idle,
        Order::Move(_) => Tag::Move,
        Order::AttackMove(_) => Tag::AttackMove,
        Order::Build { .. } => Tag::Build,
        _ => Tag::Busy,
    }
}

struct UnitInfo {
    entity: Entity,
    kind: UnitKind,
    pos: Vec3,
    tag: Tag,
    moving: bool,
    carrying: bool,
    /// Node this worker was last *told* to harvest. Only a statement of
    /// intent: economy.rs re-targets depleted nodes behind our back without
    /// rewriting the order, so it can name a dead entity.
    harvest_node: Option<Entity>,
}

impl UnitInfo {
    /// Idle, or standing around after a move/attack-move finished.
    fn free(&self) -> bool {
        match self.tag {
            Tag::Idle => true,
            Tag::Move | Tag::AttackMove => !self.moving,
            _ => false,
        }
    }
}

/// One of our living heroes, with everything the ability and shop rules ask of
/// it. Split out from `UnitInfo` because a hero is the only unit the script
/// treats as an individual.
struct HeroInfo {
    entity: Entity,
    pos: Vec3,
    /// Slot-0 ability off cooldown and affordable.
    ready: bool,
    /// Health as a fraction of max — what the potion rule reads.
    frac: f32,
    inventory: Inventory,
}

struct BuildingInfo {
    entity: Entity,
    kind: BuildingKind,
    pos: Vec3,
    done: bool,
    queue_len: usize,
    /// Sorcerers sitting in this building's queue — counted so the script's
    /// caster cap sees production already in flight, not just bodies alive.
    queued_sorcerers: usize,
    /// Already converting to its next tier — not a candidate for another
    /// upgrade order, and the reason the tier-up reserve can be released.
    upgrading: bool,
    /// This forge is mid-job. One research at a time per Blacksmith, so a busy
    /// forge is not a candidate for another order.
    researching: bool,
}

/// A living gold mine, seen from one team's point of view.
struct MineInfo {
    entity: Entity,
    pos: Vec3,
    remaining: u32,
    /// One of our TownHalls (finished or still scaffolding) is close enough
    /// that this mine is already part of our economy.
    claimed: bool,
    /// ...and that hall is finished, so workers sent here have a drop-off.
    has_depot: bool,
}

/// Where the AI wants its next mining base, and why.
struct ExpansionPlan {
    site: Vec3,
    mine_pos: Vec3,
    mine_gold: u32,
    /// Gold left in the mines we already hold — the reason we are moving.
    claimed_gold: u32,
}

// ---------------------------------------------------------------------------
// The think tick
// ---------------------------------------------------------------------------

type UnitQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Unit,
        &'static Team,
        &'static Transform,
        Option<&'static Order>,
        Option<&'static MoveTo>,
        Option<&'static Carrying>,
        Option<&'static Hero>,
        Option<&'static AbilityCooldowns>,
        // Hero-only, both of them: what the shop rules read (units.rs puts an
        // empty `Inventory` on a hero at spawn and on nothing else).
        &'static Health,
        Option<&'static Inventory>,
    ),
>;

type BuildingQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Building,
        &'static Team,
        &'static Transform,
        Option<&'static UnderConstruction>,
        // Read-only since wc3clone-jem: the script asks for a unit with an
        // `Intent::Train` and the compiler does the pushing, so this column is
        // now purely "how deep is the queue" — one of the facts the decision
        // is made from, not a thing being written.
        Option<&'static TrainingQueue>,
        Option<&'static Upgrading>,
        Option<&'static Researching>,
    ),
>;

type NodeQuery<'w, 's> = Query<'w, 's, (Entity, &'static ResourceNode, &'static Transform)>;

/// **Everything the scripted commander can do**, which is: say something.
///
/// One channel, one verb list, one compiler. Wrapped in a struct rather than
/// passed as a bare `EventWriter` so the team is carried alongside it — a
/// brain must never be able to speak for the faction it is playing against,
/// and the way to guarantee that is to make `me` un-passable at the call site.
///
/// The counter is for the report a sim run is read for: how much the script
/// actually says per tick is a number this bead had to measure, and measuring
/// it anywhere else means reconstructing it from the log.
struct Voice<'a, 'w> {
    intents: &'a mut EventWriter<'w, SubmitIntent>,
    me: Team,
    said: u32,
}

impl Voice<'_, '_> {
    /// State one intent, on behalf of this brain's team.
    ///
    /// Nothing comes back. A refusal is not an exception the caller handles:
    /// the compiler logs it, the brain re-thinks in a second, and every
    /// optimistic assumption made around this call (money spent, a build slot
    /// claimed) is re-derived from the world next tick rather than trusted.
    fn say(&mut self, intent: Intent) {
        self.said += 1;
        self.intents.write(SubmitIntent::script(self.me, intent));
    }

    /// **One sentence for a whole group**, skipped when the group is empty.
    ///
    /// For the verbs where naming twelve units and naming one twelve times are
    /// indistinguishable in the world: `harvest` names a node and `return`
    /// names nowhere, so neither passes through `ground_order`'s formation
    /// spread and neither can be changed by how it is phrased. Those get the
    /// phrasing a commander would use, which is also the cheaper one.
    ///
    /// The verbs where the phrasing *does* change the world — `move` and
    /// `attackmove` — deliberately do not come through here. See `say_each`.
    ///
    /// The empty check earns its keep: "nobody needs re-tasking this tick" is
    /// the common case, and a sentence with an empty `units` list is a line in
    /// the replay saying nothing happened.
    fn say_group(&mut self, group: Vec<IntentId>, make: impl FnOnce(Vec<IntentId>) -> Intent) {
        if group.is_empty() {
            return;
        }
        self.say(make(group));
    }

    /// **A ground order, one unit per sentence** — and the one place this bead
    /// deliberately did NOT batch.
    ///
    /// `Intent::Move` / `Intent::AttackMove` are the only verbs whose *result*
    /// depends on how many units are named: `intent::ground_order` spreads a
    /// group over `formation_offset`, so "these twenty units attack-move to X"
    /// puts twenty units on a 2.6-spaced grid around X, while twenty
    /// single-unit sentences put all twenty on X itself. Every other verb the
    /// script uses is geometry-free — `harvest` names a node, `return` names
    /// nowhere — which is why those *are* batched a few lines up.
    ///
    /// The scripted commander has always converged its waves on a single
    /// point, and that is a **tuning decision**, not a spelling one. Measured
    /// on this bead (wc3clone-jem), batching the military branches made the
    /// baseline roughly 40% more lethal — `crossings` fell from ~7.6min to
    /// ~4.75min, out of the documented 5–12min band — because a spread line
    /// engages with more of itself at once. That is a real improvement to how
    /// the script fights, and it would have arrived here disguised as
    /// plumbing, silently invalidating every balance number keyed to the
    /// scripted baseline (docs/TEMPO.md's 45-run sweep among them). So the
    /// script keeps saying what it always said.
    ///
    /// **Nothing is privileged by this.** A human can order units one at a
    /// time, and a commander on the wire can send twenty one-unit `move`s;
    /// the compiler validates each identically and charges each unit its own
    /// link either way (`ground_order` already prices per member). Declining
    /// the formation is a tactic available to all three seats, not a private
    /// door — which is exactly the test this file now has to pass.
    ///
    /// Giving the script the formation is a live option for whoever next
    /// retunes the baseline. It is a balance bead, and it should be measured
    /// as one.
    fn say_each(&mut self, group: Vec<IntentId>, make: impl Fn(Vec<IntentId>) -> Intent) {
        for id in group {
            self.say(make(vec![id]));
        }
    }
}

/// **The scripted commander's roster lookup — one kind per ROLE.**
///
/// This is the whole of ai.rs's race awareness, and it is deliberately a
/// TRANSLATION TABLE rather than a fork. Every `UnitKind::Footman` and
/// `BuildingKind::Barracks` this file used to name became `r.line` and
/// `r.production`; for `Race::Kingdom` those resolve to exactly the kinds that
/// were written there before, so a Kingdom-vs-Kingdom match is
/// instruction-for-instruction the game it was.
///
/// **How dirty this got, honestly.** Three things did not translate, and they
/// are the real cost of race asymmetry in a scripted commander:
///
///  1. **`Option` everywhere.** A race need not have a role. The Horde has no
///     `Barrier` (no wall) and no `Siegeworks` (its siege comes out of its one
///     production building), so eight fields here are `Option` and every build
///     branch that used to be a plain `else if` now has an `is_some()` in it.
///     That is not incidental noise — it is the build order honestly admitting
///     that a branch may not apply to the roster it is playing.
///  2. **Two roles can land on ONE building.** The Kingdom trains siege at a
///     Workshop and casters at a Sanctum; the Horde trains siege at its
///     WarCamp and its flyer at its Spirit Lodge. A `match` on the building's
///     kind can only take one arm, so the production and tech arms grew a
///     "…and if this building ALSO trains the siege/air unit, pace it here"
///     tail. Those tails are dead code for the Kingdom (its Barracks trains no
///     Catapult, its Sanctum no Gryphon), which is exactly why the Kingdom's
///     behaviour is unchanged — and exactly why they are the part of this
///     file most likely to rot.
///  3. **Tech gates had to be asked, not asserted.** `raider_ok` was
///     "a Workshop is standing" and `knight_ok` was "tier >= 3". Both are now
///     `unit_gate_ok`, which asks `unit_requires` the same question
///     economy.rs will ask at the pay-point. Equivalent for the Kingdom,
///     correct for a roster that gates its cavalry on a hall rung instead.
///
/// What did NOT need translating is the more interesting half: waves, retreat,
/// worker assignment, expansion timing, the Slam rule, the item rules, the
/// research ladder and every threat reaction are written against roles,
/// positions and stats already, and not one of them was touched.
#[derive(Clone, Copy)]
struct Roster {
    hall: BuildingKind,
    production: BuildingKind,
    supply: BuildingKind,
    defense: Option<BuildingKind>,
    barrier: Option<BuildingKind>,
    /// The tier-2 caster building.
    tech: Option<BuildingKind>,
    forge: Option<BuildingKind>,
    /// A DEDICATED siege building. `None` for a race whose production building
    /// makes siege itself — see the note above about two roles on one kind.
    siegeworks: Option<BuildingKind>,
    vendor: Option<BuildingKind>,
    worker: UnitKind,
    line: UnitKind,
    ranged: Option<UnitKind>,
    cavalry: Option<UnitKind>,
    anti_cavalry: Option<UnitKind>,
    siege: Option<UnitKind>,
    caster: Option<UnitKind>,
    flyer: Option<UnitKind>,
    shock: Option<UnitKind>,
}

impl Roster {
    fn of(race: Race) -> Roster {
        Roster {
            hall: race_hall(race),
            production: race_building(race, BuildingRole::Production)
                .unwrap_or_else(|| race_hall(race)),
            supply: race_building(race, BuildingRole::Supply)
                .unwrap_or_else(|| race_hall(race)),
            defense: race_building(race, BuildingRole::Defense),
            barrier: race_building(race, BuildingRole::Barrier),
            tech: race_building(race, BuildingRole::Tech),
            forge: race_building(race, BuildingRole::Forge),
            siegeworks: race_building(race, BuildingRole::Siegeworks),
            vendor: race_building(race, BuildingRole::Vendor),
            worker: race_worker(race),
            // The line unit is the only one with no honest fallback: it is
            // what every "I could not afford the good one" branch degrades
            // to, and the loader refuses a race without one.
            line: race_unit(race, UnitRole::Line).unwrap_or_else(|| race_worker(race)),
            ranged: race_unit(race, UnitRole::Ranged),
            cavalry: race_unit(race, UnitRole::Cavalry),
            anti_cavalry: race_unit(race, UnitRole::AntiCavalry),
            siege: race_unit(race, UnitRole::Siege),
            caster: race_unit(race, UnitRole::Caster),
            flyer: race_unit(race, UnitRole::Flyer),
            shock: race_unit(race, UnitRole::Shock),
        }
    }

    /// The hero classes to open slots with, in preference order: the melee
    /// class first, the support class as the second slot a tier-2 hall opens.
    /// Was a `const HERO_PICK_ORDER: [UnitKind; 2]`; the ORDER survives as the
    /// role order, which is the same decision written once for both rosters.
    fn heroes(&self, race: Race) -> Vec<UnitKind> {
        [UnitRole::HeroMelee, UnitRole::HeroSupport]
            .into_iter()
            .filter_map(|role| race_unit(race, role))
            .collect()
    }
}

/// Can this team train `kind` right now, gate-wise? Asks `unit_requires` the
/// same question economy.rs asks when it takes the money, instead of the
/// hand-rolled "a Workshop is standing" / "tier >= 3" tests this replaced.
fn unit_gate_ok(kind: UnitKind, completed: &[BuildingKind]) -> bool {
    requirements_met(unit_requires(kind), completed.iter().copied())
}

/// Drives one think tick for every team the AI is currently playing.
#[allow(clippy::too_many_arguments)]
fn ai_think(
    time: Res<Time>,
    mut state: ResMut<AiState>,
    game_over: Res<GameOver>,
    ai_controlled: Res<AiControlled>,
    economies: Res<Economies>,
    records: Res<HeroRecords>,
    nav: Res<NavGrid>,
    fog: Res<FogGrids>,
    // The whole output surface (see `Voice`). docs/TEMPO.md §3 — "the scripted
    // AI pays latency too, or autopilot becomes a cheat and C1 is violated at
    // the third seat" — used to be satisfied here by reaching for the same
    // `OrderIssuer` the compiler uses. It is satisfied structurally now: the
    // script has no way to issue an order except by asking the compiler to,
    // and the compiler prices every one of them.
    mut intents: EventWriter<SubmitIntent>,
    team_research: Res<TeamResearch>,
    units: UnitQuery,
    buildings: BuildingQuery,
    nodes: NodeQuery,
    // Which roster each team is playing. The one race-dependent input the
    // script has: everything below reaches for a ROLE and this resolves it to
    // a kind (see `Roster`).
    races: Res<Races>,
) {
    if game_over.winner.is_some() {
        return;
    }
    if !state.timer.tick(time.delta()).just_finished() {
        return;
    }

    let now = time.elapsed_secs();
    for team in [Team::Human, Team::Claude] {
        let driving = match team {
            Team::Human => ai_controlled.human,
            Team::Claude => ai_controlled.claude,
        };
        if !driving {
            continue;
        }
        // Each brain sees only its own state; `me` decides every position.
        let brain = state.brain_mut(team);
        let mut voice = Voice {
            intents: &mut intents,
            me: team,
            said: 0,
        };
        think(
            team,
            races.get(team),
            brain,
            now,
            &economies,
            &records,
            &nav,
            fog.get(team),
            &mut voice,
            &team_research,
            &units,
            &buildings,
            &nodes,
        );
        // The log-volume number this bead was asked for, available without
        // parsing anything: `RUST_LOG=debug` on a sim prints one of these per
        // team per second and the total is the intent log's growth rate.
        if voice.said > 0 {
            debug!("[ai {team:?}] said {} intent(s) this tick", voice.said);
        }
    }
}

/// One team's thought. Everything positional is derived from `me`.
#[allow(clippy::too_many_arguments)]
fn think(
    me: Team,
    // The roster this team is playing. Everything race-dependent below goes
    // through `Roster`, built from it once on the next line.
    race: Race,
    brain: &mut AiBrain,
    now: f32,
    economies: &Economies,
    records: &HeroRecords,
    nav: &NavGrid,
    // This team's fog. Every enemy fact below is drawn through it, so the
    // scripted commander plans from the same picture a bridge commander is
    // sent and the same one the player is shown.
    fog: &FogGrid,
    // The only way out of this function. Every decision below ends in a
    // `voice.say(...)`, exactly as a human's right-click and a bridge
    // commander's `move` end in a `SubmitIntent` — same verbs, same compiler,
    // same validation, same link, same replay log.
    voice: &mut Voice,
    team_research: &TeamResearch,
    units: &UnitQuery,
    buildings: &BuildingQuery,
    nodes: &NodeQuery,
) {
    let r = Roster::of(race);

    // --- snapshot the world (read-only) --------------------------------------
    let mut workers: Vec<UnitInfo> = Vec::new();
    let mut army: Vec<UnitInfo> = Vec::new();
    // Enemies that can shoot at a worker (what makes a mine unsafe).
    let mut enemy_combat: Vec<Vec3> = Vec::new();
    // Every enemy, air included — a flyer over the base is still an incursion.
    let mut enemy_any: Vec<Vec3> = Vec::new();
    // Enemies standing ON the ground: the only ones a Slam can touch.
    let mut enemy_ground: Vec<Vec3> = Vec::new();
    // Own living heroes.
    let mut own_heroes: Vec<HeroInfo> = Vec::new();
    // Did we LOOK AT enemy air / enemy cavalry this tick? Two bits, refreshed
    // from scratch every thought; the memory is the decaying counter on the
    // brain, not these.
    let mut saw_air = false;
    let mut saw_cavalry = false;

    for (entity, unit, team, tf, order, move_to, carrying, hero, cooldowns, health, inventory) in
        units.iter()
    {
        let info = UnitInfo {
            entity,
            kind: unit.kind,
            // Flattened to the ground plane. Every comparison below is a
            // ground-plane question ("is it near my base", "has it reached the
            // rally"), and a flyer's altitude would otherwise inflate all of
            // them — a flying unit hovering exactly on the rally point would
            // read as 6 units short of it and never count as arrived.
            pos: Vec3::new(tf.translation.x, 0.0, tf.translation.z),
            tag: order.map(tag_of).unwrap_or(Tag::Idle),
            moving: move_to.is_some(),
            carrying: carrying.is_some(),
            harvest_node: match order {
                Some(Order::Harvest(node)) => Some(*node),
                _ => None,
            },
        };
        if *team == me {
            if let Some(hero) = hero {
                // "Can the Champion slam right now" is asked of the ability
                // table, not of a hard-coded mana number: slot 0 is whatever
                // this hero class's first ability is.
                let ready = abilities_of_unit(unit.kind)
                    .first()
                    .is_some_and(|def| ability_ready(def, Some(hero), cooldowns, 0));
                own_heroes.push(HeroInfo {
                    entity,
                    pos: info.pos,
                    ready,
                    frac: if health.max > 0.0 {
                        health.current / health.max
                    } else {
                        1.0
                    },
                    inventory: inventory.copied().unwrap_or_default(),
                });
            }
            // Everything that isn't a Worker is army: heroes, Archers,
            // Catapults and Raiders all join waves with no extra wiring.
            match unit.kind {
                k if is_worker_kind(k) => workers.push(info),
                _ => army.push(info),
            }
        } else if fog.sees(info.pos) {
            // Enemy units enter the plan only while somebody of ours is
            // looking at them, and are never remembered afterwards. This is
            // the single line that ends the omniscient-commander asymmetry:
            // everything below — defence, worker flight, Slam timing — was
            // reading the ECS directly and now reads what this team can see.
            enemy_any.push(info.pos);
            if is_flying_kind(unit.kind) {
                saw_air = true;
            } else {
                enemy_ground.push(info.pos);
            }
            // The two reactions. Both are pure sight: a kind we are looking at
            // right now, never a kind we were killed by ten seconds ago.
            // By CLASS, not by kind: `TargetClass::Cavalry` is what the
            // Spearman's 5x is keyed off, so "did we see cavalry" and "does
            // the spear line answer it" are now the same question — and a
            // Wolfrider trips it for the same reason a Knight does, with no
            // second race named here.
            if TargetClass::of(Some(unit.kind), false) == Some(TargetClass::Cavalry) {
                saw_cavalry = true;
            }
            // Workers don't hunt, and neither does anything that cannot shoot
            // downward — so neither should make a harvest crew run.
            if !is_worker_kind(unit.kind) && unit_stats(unit.kind).can_hit_ground {
                enemy_combat.push(info.pos);
            }
        }
    }

    // --- reactive alerts (two decaying counters, no planner) ------------------
    // A sighting refills the counter; a thought without one drains it. That is
    // the entire mechanism: the script cannot say where the cavalry was, how
    // much of it there was, or where it is going. It can only say "there was
    // some, recently", and buy accordingly for a while.
    brain.air_alert = tick_alert(brain.air_alert, saw_air);
    brain.cavalry_alert = tick_alert(brain.cavalry_alert, saw_cavalry);
    let air_alert = brain.air_alert > 0;
    let cavalry_alert = brain.cavalry_alert > 0;
    let (archer_nth, spearman_nth) = reactive_cadences(air_alert, cavalry_alert);
    // One line when a shift starts and one when it lapses, so a sim trace shows
    // the reaction without having to be re-derived from the unit mix.
    for (live, logged, what, mix) in [
        (air_alert, &mut brain.air_alert_logged, "enemy AIR", "Archers"),
        (
            cavalry_alert,
            &mut brain.cavalry_alert_logged,
            "enemy CAVALRY",
            "Spearmen",
        ),
    ] {
        if live != *logged {
            if live {
                info!("[ai {me:?}] {what} sighted — shifting the mix toward {mix}");
            } else {
                info!("[ai {me:?}] {what} contact has lapsed — mix back to standard");
            }
            *logged = live;
        }
    }

    let mut own_buildings: Vec<BuildingInfo> = Vec::new();
    let mut enemy_buildings: Vec<Vec3> = Vec::new();
    let mut queued_supply: u32 = 0;
    // Hero CLASSES already in this team's queues. A list, not a flag: slots
    // scale with the hall ladder now, and the lock is per class.
    let mut heroes_queued: Vec<UnitKind> = Vec::new();
    for (entity, building, team, tf, under, queue, upgrading, researching) in buildings.iter() {
        if *team != me {
            // Seen right now. Structures we merely REMEMBER are appended
            // straight after, because a building does not walk away: acting on
            // a scouted barracks long after the scout died is memory, not
            // cheating, and it is exactly what a human does.
            if fog.sees(tf.translation) {
                enemy_buildings.push(tf.translation);
            }
            continue;
        }
        let queue_len = queue.as_ref().map(|q| q.queue.len()).unwrap_or(0);
        let mut queued_sorcerers = 0usize;
        if let Some(q) = queue.as_ref() {
            queued_supply += q.queue.iter().map(|k| unit_stats(*k).supply).sum::<u32>();
            // Hero classes already in flight — one slot each, and no class
            // twice (see `hero_slots`).
            for k in q.queue.iter() {
                if is_hero_kind(*k) {
                    heroes_queued.push(*k);
                }
                if Some(*k) == r.caster {
                    queued_sorcerers += 1;
                }
            }
        }
        own_buildings.push(BuildingInfo {
            entity,
            kind: building.kind,
            pos: tf.translation,
            done: under.is_none(),
            queue_len,
            queued_sorcerers,
            upgrading: upgrading.is_some(),
            researching: researching.is_some(),
        });
    }
    enemy_buildings.extend(fog.ghosts().map(|g| g.pos));

    // Gold mines, tagged with whether our economy already reaches them. Trees
    // are deliberately ignored: lumber clusters are dense and near the base,
    // and running out of them is not what stalls a long game.
    let mut mines: Vec<MineInfo> = Vec::new();
    for (entity, node, tf) in nodes.iter() {
        if node.kind != ResourceKind::Gold || node.remaining == 0 {
            continue;
        }
        let pos = flat(tf.translation);
        let hall_within = |done_only: bool| {
            own_buildings.iter().any(|b| {
                // Any rung of the ladder is a drop-off, so upgrading the hall
                // by a mine never un-claims that mine.
                is_hall(b.kind)
                    && (b.done || !done_only)
                    && xz_dist(b.pos, pos) < MINE_CLAIM_RADIUS
            })
        };
        mines.push(MineInfo {
            entity,
            pos,
            remaining: node.remaining,
            claimed: hall_within(false),
            has_depot: hall_within(true),
        });
    }

    // Gold still standing in the mines our halls can actually reach — the
    // number the expansion logic is really about.
    let claimed_gold: u32 = mines.iter().filter(|m| m.claimed).map(|m| m.remaining).sum();
    let halls = own_buildings
        .iter()
        .filter(|b| is_hall(b.kind) && b.done)
        .count();
    if halls != brain.last_halls {
        info!(
            "[ai {me:?}] town halls: {} -> {} | gold left in reachable mines: {claimed_gold}",
            brain.last_halls, halls
        );
        brain.last_halls = halls;
    }

    let eco = *economies.get(me);
    // Free supply, pessimistically counting units already in production. The
    // shared definition, because `supply_capped` (trigger.rs) now asks the same
    // question and a script that disagreed with the predicate about what
    // "blocked" means would be two economies in one game.
    let mut headroom = supply_headroom(&eco, queued_supply);
    let mut gold = eco.gold;
    let mut lumber = eco.lumber;

    // Clear the in-flight build slot once that worker stopped building.
    if let Some(builder) = brain.pending_build {
        let still_building = workers
            .iter()
            .any(|w| w.entity == builder && w.tag == Tag::Build);
        if !still_building {
            brain.pending_build = None;
            // Either the hall is paid for and going up, or the worker gave up.
            // Both end the ring-fence; a retry re-arms it next tick.
            brain.expansion_pending = false;
        }
    }

    // --- threat assessment ---------------------------------------------------
    let base = me.base_pos();
    let threat = enemy_any
        .iter()
        .copied()
        .filter(|p| p.distance(base) < DEFEND_RADIUS)
        .min_by(|a, b| {
            a.distance(base)
                .partial_cmp(&b.distance(base))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    // --- workers flee melee --------------------------------------------------
    // `fleeing` holds every endangered worker, so the harvest pass leaves them
    // alone until the threat is gone instead of yo-yoing them back into it.
    let mut fleeing: Vec<Entity> = Vec::new();
    for w in &workers {
        let in_danger = enemy_combat
            .iter()
            .any(|e| e.distance(w.pos) < WORKER_FLEE_RADIUS);
        if !in_danger {
            continue;
        }
        fleeing.push(w.entity);
        if w.tag != Tag::Move {
            let a = (w.entity.index() % 8) as f32 * std::f32::consts::TAU / 8.0;
            let safe = base + Vec3::new(a.cos(), 0.0, a.sin()) * 6.0;
            // One worker per sentence, not a group: each is sent to a
            // different spoke of the same ring, so there is no shared
            // destination to batch on. Guarded by `tag != Move` above, which
            // is what keeps a base under siege from restating the same
            // scatter every second.
            voice.say(Intent::Move {
                units: vec![intent_id(w.entity)],
                x: Some(safe.x),
                z: Some(safe.z),
                region: None,
            });
        }
    }

    // --- build order (one command in flight) ---------------------------------
    let mut busy_worker: Option<Entity> = None;
    // Set when an expansion is wanted but not yet paid for, so army production
    // stops eating the down payment (same trick as the Champion reserve).
    let mut saving_for_expansion = false;
    if brain.pending_build.is_none() {
        let count_of = |kind: BuildingKind| own_buildings.iter().filter(|b| b.kind == kind).count();
        // Tech gates count only FINISHED buildings; `count_of` (which includes
        // scaffolding) is what keeps us from ordering two of the same thing.
        let done_count = |kind: BuildingKind| {
            own_buildings.iter().filter(|b| b.kind == kind && b.done).count()
        };
        let under_construction = |kind: BuildingKind| {
            own_buildings.iter().any(|b| b.kind == kind && !b.done)
        };
        // Hall bookkeeping walks the whole ladder: a base whose TownHall
        // became a Keep still has a hall, and must not re-open the "we have no
        // hall, build one" branch or plan a second expansion behind its back.
        let halls_total = own_buildings.iter().filter(|b| is_hall(b.kind)).count();
        let hall_going_up = own_buildings.iter().any(|b| is_hall(b.kind) && !b.done);
        // The rung we actually hold right now — what tier-gated buildings ask.
        let tier_now = own_buildings
            .iter()
            .filter(|b| b.done && is_hall(b.kind))
            .map(|b| building_tier(b.kind))
            .max()
            .unwrap_or(0);

        // A second mining base, planned before it is affordable so the site is
        // already vetted (unclaimed, undefended-by-them, buildable) when the
        // money lands. `None` while a TownHall is already going up: one
        // expansion at a time, like every other line in this build order.
        let expansion = if halls_total > 0 && !hall_going_up {
            plan_expansion(
                me,
                r.hall,
                &own_buildings,
                &enemy_buildings,
                &mines,
                &workers,
                &enemy_combat,
                nav,
            )
        } else {
            None
        };

        // --- static defense ---------------------------------------------------
        // A Keep standing is the "the opening is over" line the whole file
        // already uses; two towers is what the base gets for reaching it, and a
        // third is what an enemy flyer buys itself by being seen. `count_of`
        // (scaffolding included) is what stops a 25s build from being ordered
        // four times while the first one goes up.
        let keep_standing = own_buildings
            .iter()
            .any(|b| b.done && is_hall(b.kind) && building_tier(b.kind) >= 2);
        let towers_standing = r.defense.map_or(0, count_of);
        // Two independent ceilings, and the tighter one wins: the rules' own
        // quota, and the hard cap that exists so no future rule can talk this
        // script into a fortress. See `MAX_TOWERS`.
        let tower_wanted = r.defense.is_some()
            && wants_tower(
            done_count(r.production) >= 1,
            towers_standing,
            keep_standing,
            air_alert,
        );

        // The crossing worth holding, if this map has one and we live by it.
        // Every hall counts, expansions included — that is the point.
        let own_halls: Vec<Vec3> = own_buildings
            .iter()
            .filter(|b| is_hall(b.kind) && b.done)
            .map(|b| b.pos)
            .collect();
        let ford = ford_hold_point(&active_map().chokepoints(), &own_halls);
        // Is the ford already garrisoned? Walls are a screen for a tower that
        // exists, never a fortification on their own.
        let ford_tower = ford.as_ref().is_some_and(|f| {
            own_buildings
                .iter()
                .any(|b| Some(b.kind) == r.defense && xz_dist(b.pos, f.hold) < FORD_AREA)
        });
        let ford_walls = ford.as_ref().map_or(0, |f| {
            own_buildings
                .iter()
                .filter(|b| Some(b.kind) == r.barrier && xz_dist(b.pos, f.hold) < FORD_AREA)
                .count()
        });
        let wall_wanted = r.barrier.is_some()
            && ford_tower
            && ford.as_ref().is_some_and(|f| !ford_wall_sites(f).is_empty())
            && ford_walls < FORD_WALLS
            && gold > TOWER_GOLD;

        // A Shop is a conversion, not production: surplus gold into hero
        // uptime. Bottom of the chain, Keep-gated, bank-gated, one ever.
        let shop_wanted = keep_standing
            && r.vendor.is_some_and(|v| count_of(v) == 0)
            && gold > SHOP_GOLD;

        let want = if halls_total == 0 {
            Some(r.hall)
        } else if headroom < SUPPLY_BUFFER && !under_construction(r.supply) {
            Some(r.supply)
        } else if count_of(r.production) == 0 {
            Some(r.production)
        } else if tier_now >= 2
            && r.tech.is_some_and(|t| count_of(t) == 0)
            && gold > SANCTUM_GOLD
        {
            // The caster branch, and deliberately ABOVE the expansion. That
            // looks like it contradicts "income first", and it does not: this
            // is a ONE-OFF 150g/130l purchase gated behind a Keep the team has
            // already spent 320g/160l reaching, while `expansion` is an
            // unbounded series that re-arms every time a mine is claimed. Put
            // below it, the Sanctum loses every roll forever — five straight
            // sim runs reached tier 2 and finished with no caster ever built,
            // which is a tier-2 unlock that has never been played.
            r.tech
        } else if expansion.is_some() {
            // Above the remaining luxuries (second Barracks, Workshop) and
            // below the army's first Barracks: income outlives any one more
            // Footman, but a base with no defenders never gets to spend it.
            Some(r.hall)
        } else if tower_wanted && air_alert {
            // The reactive Tower, and the only branch in this chain with no
            // gold gate of its own. A flyer cannot be walked around, blocked,
            // or out-ranged by anything the Barracks makes in time; a Tower is
            // the answer that is already standing when it arrives. "We could
            // not afford to answer air" is not a position worth protecting.
            r.defense
        } else if gold > SECOND_BARRACKS_GOLD && count_of(r.production) < MAX_BARRACKS {
            Some(r.production)
        } else if r.siegeworks.is_some_and(|w| count_of(w) == 0)
            && done_count(r.production) >= 1
            && gold > WORKSHOP_GOLD
        {
            // Siege branch. `count_of` (not `done_count`) means one Workshop
            // ever — including one still under construction — so the AI can't
            // spam a second while the first is going up.
            r.siegeworks
        } else if r.forge.is_some_and(|f| done_count(f) == 0 && count_of(f) == 0)
            && own_buildings
                .iter()
                .any(|b| b.done && is_hall(b.kind) && building_tier(b.kind) >= 2)
            && gold > BLACKSMITH_GOLD
        {
            // Research branch, below siege on purpose: a Catapult answers a
            // tower line that a +1 sword never will, and the forge is the more
            // patient purchase of the two. Gated on a STANDING tier-2 hall
            // rather than on `building_requires`, matching how every other
            // branch here spells its prerequisite — ai.rs hand-rolls its gates
            // and economy.rs enforces the real one at placement.
            r.forge
        } else if shop_wanted {
            // 75g/60l, the cheapest thing left on this list, and like the forge
            // above it its benefit travels with the army instead of guarding
            // one patch of dirt. Same argument, same side of the towers.
            r.vendor
        } else if tower_wanted && halls >= 2 && gold > TOWER_GOLD {
            // The baseline tower, at the bottom of the chain. Everything above
            // either earns money, makes soldiers, or makes soldiers better; a
            // Tower does none of the three, and the scripted AI is judged on
            // whether it attacks.
            //
            // `halls >= 2` — fortify only what a second income is paying for.
            // A base that has not expanded yet cannot spare the gold OR the
            // tempo, and buying static defense out of a single mine is how the
            // scripted matchup stops converging (see BASELINE_TOWERS).
            r.defense
        } else if wall_wanted {
            r.barrier
        } else {
            None
        };

        if let Some(kind) = want {
            let stats = building_stats(kind);
            saving_for_expansion = expansion.is_some()
                && kind == r.hall
                && !eco.can_afford(stats.cost_gold, stats.cost_lumber);
            if eco.can_afford(stats.cost_gold, stats.cost_lumber) {
                // Anchor on the town hall, or any surviving building if the
                // main base area has been razed.
                let anchor = own_buildings
                    .iter()
                    .find(|b| is_hall(b.kind))
                    .or_else(|| own_buildings.first())
                    .map(|b| b.pos)
                    .unwrap_or(base);
                // An expansion hall goes next to its mine, not next to home —
                // the whole point is a short haul at the *new* patch.
                let expanding = kind == r.hall && expansion.is_some();
                let footprint = stats.size + BUILD_PADDING;
                // Three siting rules, in order of how much the placement
                // matters: an expansion goes to its mine, an emplacement goes
                // to the ford it is there to hold, everything else goes in a
                // ring around the base like it always did.
                let site = match (&expansion, kind, &ford) {
                    (Some(plan), _, _) if expanding => Some(plan.site),
                    (_, k, Some(f)) if Some(k) == r.defense => pick_spot(nav, f.hold, footprint)
                        .or_else(|| pick_site(nav, anchor, footprint)),
                    (_, k, Some(f)) if Some(k) == r.barrier => ford_wall_sites(f)
                        .into_iter()
                        .find(|p| nav.rect_is_free(*p, footprint)),
                    _ => pick_site(nav, anchor, footprint),
                };
                // **Speak the compiler's lattice.** `Intent::Build` snaps the
                // site so a footprint's edges land on nav-cell boundaries —
                // the same snap the human's placement ghost shows — and THEN
                // checks the ground. A site the script vetted before the snap
                // is a site the compiler might refuse a cell later, so the
                // script snaps first and vets the snapped point.
                //
                // The vet does not have to be repeated: every picker above
                // clears `stats.size + BUILD_PADDING`, the padding is one full
                // cell of slack on each side, and the snap moves the centre by
                // at most half a cell per axis — so the snapped footprint is
                // strictly inside ground already known to be free. That is why
                // this is a `map` and not a `filter`, and why routing build
                // through the compiler costs the script no placements.
                let site = site.map(|p| snap_footprint(p, stats.size));
                if let Some(site) = site {
                    if let Some(builder) = pick_builder(&workers, &fleeing, site) {
                        // Exempt from link latency, exactly as a human's or a
                        // commander's `build` is — because it is now literally
                        // the same arm of the same compiler, rather than this
                        // file remembering to call `issue_instant`.
                        voice.say(Intent::Build {
                            worker: intent_id(builder),
                            kind: building_name(kind).to_string(),
                            x: Some(site.x),
                            z: Some(site.z),
                            region: None,
                        });
                        // Optimistic, and safe to be: if the compiler refuses
                        // the placement the worker never picks up an
                        // `Order::Build`, so next tick's `still_building`
                        // check clears this slot and the expansion ring-fence
                        // with it, and the build order simply retries.
                        brain.pending_build = Some(builder);
                        busy_worker = Some(builder);
                        // economy.rs pays at placement; assume it lands.
                        gold = gold.saturating_sub(stats.cost_gold);
                        lumber = lumber.saturating_sub(stats.cost_lumber);
                        // Every placement the script orders, in one line. The
                        // expansion branch logs its own richer version below;
                        // this is what makes the rest of the build order — the
                        // Workshop, the forge, the farms — visible in a sim
                        // trace at all, instead of having to be inferred from
                        // which units showed up later.
                        debug!(
                            "[ai {me:?}] building {} at ({:.0},{:.0}) for {}g {}l",
                            building_name(kind),
                            site.x,
                            site.z,
                            stats.cost_gold,
                            stats.cost_lumber,
                        );
                        // Emplacements get their own line: which crossing, how
                        // far from its centre, and whether it was the baseline
                        // pair or an answer to something in the air. This is
                        // the evidence a sim run is read for.
                        if Some(kind) == r.defense || Some(kind) == r.barrier {
                            match &ford {
                                Some(f) => info!(
                                    "[ai {me:?}] fortifying the {}: {} at ({:.0},{:.0}), {:.0} back \
                                     from the crossing ({} of {}{})",
                                    f.name,
                                    building_name(kind),
                                    site.x,
                                    site.z,
                                    xz_dist(site, f.hold),
                                    towers_standing + 1,
                                    tower_quota(keep_standing, air_alert),
                                    if air_alert { ", air contact" } else { "" },
                                ),
                                None => info!(
                                    "[ai {me:?}] {} at ({:.0},{:.0}) — no crossing to hold, \
                                     fortifying the base ({} of {})",
                                    building_name(kind),
                                    site.x,
                                    site.z,
                                    towers_standing + 1,
                                    tower_quota(keep_standing, air_alert),
                                ),
                            }
                        }
                        if Some(kind) == r.vendor {
                            info!(
                                "[ai {me:?}] Shop at ({:.0},{:.0}) — hero items are open",
                                site.x, site.z
                            );
                        }
                        if let (true, Some(plan)) = (expanding, &expansion) {
                            brain.expansion_pending = true;
                            info!(
                                "[ai {me:?}] expanding: {} at ({:.0},{:.0}) for the mine at \
                                 ({:.0},{:.0}) holding {} gold — held mines are down to {}",
                                building_name(kind),
                                site.x,
                                site.z,
                                plan.mine_pos.x,
                                plan.mine_pos.z,
                                plan.mine_gold,
                                plan.claimed_gold,
                            );
                        }
                    }
                }
            }
        }
    }

    // --- tier up the main hall ------------------------------------------------
    // The minimal scripted tech ladder: once the opening is over (a Barracks
    // standing and a real army on the field) push one hall to Keep, and much
    // later, if the treasury is genuinely fat, to Castle. Deliberately modest —
    // the point is that tiers actually OCCUR in scripted matches, so tier-gated
    // content has something to gate on and era-validation runs have something
    // to measure. Income comes first: an expansion in flight blocks a tier-up,
    // because a second mine pays for every future Keep and a Keep pays for none.
    let hall_upgrading = own_buildings.iter().any(|b| is_hall(b.kind) && b.upgrading);
    let current_tier = own_buildings
        .iter()
        .filter(|b| is_hall(b.kind) && b.done)
        .map(|b| building_tier(b.kind))
        .max()
        .unwrap_or(0);
    if current_tier != brain.last_tier {
        info!(
            "[ai {me:?}] hall tier: {} -> {current_tier}",
            brain.last_tier
        );
        brain.last_tier = current_tier;
    }

    // The hall we push up the ladder: the highest rung we hold, nearest home
    // among equals — so the tier climbs on one main base instead of creeping
    // sideways across every expansion.
    let mut main_hall: Option<&BuildingInfo> = None;
    for b in &own_buildings {
        if !b.done || !is_hall(b.kind) || b.upgrading {
            continue;
        }
        let better = main_hall.is_none_or(|cur| {
            let (cur_tier, b_tier) = (building_tier(cur.kind), building_tier(b.kind));
            b_tier > cur_tier
                || (b_tier == cur_tier && xz_dist(b.pos, base) < xz_dist(cur.pos, base))
        });
        if better {
            main_hall = Some(b);
        }
    }

    let barracks_up = own_buildings
        .iter()
        .any(|b| b.kind == r.production && b.done);
    let mut tierup_reserve = (0u32, 0u32);
    brain.tierup_pending = false;
    if !hall_upgrading && !saving_for_expansion && !brain.expansion_pending {
        if let Some(hall) = main_hall {
            if let Some((cost_gold, cost_lumber, _)) = upgrade_cost(hall.kind) {
                let tier = building_tier(hall.kind);
                // The two proofs of a durable economy the Castle asks for.
                // Both are things that ALREADY HAPPENED and cost real money —
                // a finished attack rung, or a second mining base standing —
                // which is why either is allowed to stand in for the enormous
                // cash-on-hand test that used to make tier 3 unreachable.
                let attack_researched = team_research.get(me).level(ResearchKind::Attack) >= 1;
                let expanded = halls >= 2;
                let wanted = match tier {
                    // Keep: as soon as the opening is genuinely over.
                    1 => barracks_up && army.len() >= KEEP_MIN_ARMY,
                    // Castle: the ambition, asked without reference to cash so
                    // the reserve below can be what actually accumulates it.
                    _ => castle_is_the_plan(army.len(), attack_researched, expanded),
                };
                // Affordable AND still out of surplus. The Keep has no surplus
                // test — it is the tier-up that pays for itself.
                let payable = match tier {
                    1 => true,
                    _ => wants_castle(army.len(), gold, cost_gold, attack_researched, expanded),
                };
                if wanted {
                    if payable && gold >= cost_gold && lumber >= cost_lumber {
                        gold -= cost_gold;
                        lumber -= cost_lumber;
                        voice.say(Intent::Upgrade {
                            building: intent_id(hall.entity),
                        });
                        info!(
                            "[ai {me:?}] teching up: {} -> {} at ({:.0},{:.0}) for \
                             {cost_gold}g {cost_lumber}l (army {})",
                            building_name(hall.kind),
                            building_name(
                                building_upgrades_to(hall.kind).expect("a cost implies a tier")
                            ),
                            hall.pos.x,
                            hall.pos.z,
                            army.len(),
                        );
                    } else if tier == 1 || gold >= CASTLE_LATCH_GOLD {
                        // Ring-fence: hold the price out of the army's reach
                        // until the deliveries add up, or the Barracks will
                        // keep the treasury a Footman short of it forever.
                        //
                        // The Keep latches unconditionally — it is 320g in the
                        // fourth minute and the script always gets there. The
                        // Castle has to prove it is close first
                        // (`CASTLE_LATCH_GOLD`): 480g/240l held back from a
                        // treasury that will never reach it is not saving, it
                        // is a production halt with good intentions.
                        brain.tierup_pending = true;
                        tierup_reserve = (cost_gold, cost_lumber);
                    }
                }
            }
        }
    }

    // --- research at the forge ------------------------------------------------
    // Modest and strictly out of surplus, in the spirit of the tier-up above:
    // attack first, then armor, one rung at a time, and never while an
    // expansion or a tier-up is already holding money back. Attack leads
    // because the scripted AI attacks — it pushes waves at a fixed cadence, and
    // a wave that kills faster takes fewer losses, which is armor's benefit
    // arriving by a shorter road.
    let mut research_reserve = (0u32, 0u32);
    brain.research_pending = false;
    if !saving_for_expansion
        && !brain.expansion_pending
        && !brain.tierup_pending
        && army.len() >= RESEARCH_MIN_ARMY
    {
        // An idle forge: finished, and not already working. Two Blacksmiths
        // would let the script run both ladders at once; it never builds a
        // second, so in practice this picks the one.
        let forge = own_buildings
            .iter()
            .find(|b| b.done && !b.researching && !building_researches(b.kind).is_empty());
        if let Some(forge) = forge {
            let levels = team_research.get(me);
            // Attack before armor, but the LOWEST rung first: attack 1, armor
            // 1, attack 2, armor 2, and so on. Two reasons. The cheap reason is
            // price — rung 1 of each ladder is 200g/100l for +1/+1, where
            // attack 1-2-3 alone is 525g/300l for +3 and nothing else. The real
            // reason is that flat bonuses compound against each other: +1
            // attack and +1 armor together shift a Footman trade further than
            // +2 attack does, because the first also subtracts from what comes
            // back. Attack still leads every tie, so a script that only ever
            // affords one rung buys the one that makes its waves hit harder.
            let wanted = ALL_RESEARCH_KINDS
                .into_iter()
                .filter_map(|k| levels.next_step(k).map(|step| (k, step)))
                .min_by_key(|(_, step)| step.level);
            if let Some((kind, step)) = wanted {
                // The surplus test is against what is left AFTER the price, so
                // a research rung can never be bought out of the army's budget.
                let spare = gold.saturating_sub(step.cost_gold) >= RESEARCH_SPARE_GOLD;
                if spare && lumber >= step.cost_lumber {
                    gold -= step.cost_gold;
                    lumber -= step.cost_lumber;
                    voice.say(Intent::Research {
                        building: intent_id(forge.entity),
                        // The ladder id the catalog publishes, which is what
                        // `parse_research_kind` reads and what a commander
                        // would have typed.
                        upgrade: kind.id().to_string(),
                    });
                    info!(
                        "[ai {me:?}] researching {} {} at ({:.0},{:.0}) for {}g {}l (army {})",
                        kind.label(),
                        step.level,
                        forge.pos.x,
                        forge.pos.z,
                        step.cost_gold,
                        step.cost_lumber,
                        army.len(),
                    );
                } else if gold >= RESEARCH_SPARE_GOLD {
                    // Close enough to be worth saving for. The gate keeps the
                    // ring-fence from latching in a game where the treasury
                    // never gets near the price at all — a permanently held
                    // reserve would quietly stop army production instead.
                    brain.research_pending = true;
                    research_reserve = (step.cost_gold, step.cost_lumber);
                }
            }
        }
    }

    // --- put idle workers back on resources ----------------------------------
    //
    // Collected before it is spoken, so a crew sent to one patch is ONE
    // sentence ("harvest that mine with these four") instead of four. That is
    // how a commander on the wire would say it, and `harvest` is one of the
    // verbs where the two phrasings are indistinguishable in the world — it
    // names a node, not a point, so there is no formation to spread. See
    // `Voice::say_group` and, for the verbs where it is NOT free, `say_each`.
    //
    // `Vec` rather than a map, keyed by first appearance: the order sentences
    // come out in has to be a function of the world and nothing else, or
    // `WC3_SEED` stops reproducing a match.
    let mut haulers: Vec<IntentId> = Vec::new();
    let mut crews: Vec<(Entity, Vec<IntentId>)> = Vec::new();
    for w in &workers {
        if !w.free() {
            continue;
        }
        if Some(w.entity) == busy_worker
            || Some(w.entity) == brain.pending_build
            || fleeing.contains(&w.entity)
        {
            continue;
        }
        if w.carrying {
            // Stranded with a full load (e.g. after a failed drop-off):
            // deliver it; economy.rs resumes the remembered node afterwards.
            haulers.push(intent_id(w.entity));
            continue;
        }
        brain.harvest_counter = brain.harvest_counter.wrapping_add(1);
        let want_lumber = brain.harvest_counter % LUMBER_EVERY_NTH == 0;
        let first = if want_lumber {
            ResourceKind::Lumber
        } else {
            ResourceKind::Gold
        };
        let node = nearest_node(nodes, w.pos, first)
            .or_else(|| nearest_node(nodes, w.pos, other_resource(first)));
        if let Some(node) = node {
            match crews.iter_mut().find(|(n, _)| *n == node) {
                Some((_, crew)) => crew.push(intent_id(w.entity)),
                None => crews.push((node, vec![intent_id(w.entity)])),
            }
        }
    }
    voice.say_group(haulers, |units| Intent::Return { units });
    for (node, crew) in crews {
        voice.say(Intent::Harvest {
            units: crew,
            target: intent_id(node),
        });
    }

    // --- spread the gold line across the mines we hold -----------------------
    // economy.rs already re-crews a mine the moment it runs dry (danger-aware,
    // map-wide). This is the other half of the job: moving workers onto a mine
    // that is merely *new*, so a finished expansion isn't a hall standing next
    // to an untouched patch while the home crew races the last of the old one.
    // Nothing here fires while only one mine has a drop-off, so it can't fight
    // the depletion rebalance.
    let mut shift_skip: Vec<Entity> = fleeing.clone();
    shift_skip.extend(busy_worker);
    shift_skip.extend(brain.pending_build);
    rebalance_mines(&mines, &workers, &shift_skip, nodes, voice);

    // --- training ------------------------------------------------------------
    let mut worker_count = workers.len();
    let mut orders: Vec<(Entity, UnitKind)> = Vec::new();

    // Hero slots scale with the hall ladder — 1 at TownHall, 2 at Keep, 3 at
    // Castle, distinct classes only (`hero_slots`). The script fills them in a
    // fixed order: Champion first, Priestess as the second slot a Keep opens.
    // Revival of a class it has already lost outranks opening a new one — a
    // level-6 Champion at 250g is the best gold in the game.
    let hero_pick_order = r.heroes(race);
    let mut held_classes: Vec<UnitKind> = army
        .iter()
        .filter(|u| is_hero_kind(u.kind))
        .map(|u| u.kind)
        .collect();
    held_classes.extend(heroes_queued.iter().copied());
    let barracks_standing = own_buildings.iter().any(|b| b.kind == r.production);
    let slots_open = (held_classes.len() as u32) < hero_slots(TechTier::from_level(current_tier));
    // Opening an ADDITIONAL slot is a luxury; filling the first one never was.
    let fighters = army.iter().filter(|u| !is_hero_kind(u.kind)).count();
    let can_open_another = held_classes.is_empty() || fighters >= SECOND_HERO_MIN_ARMY;
    let candidates = |revivals_only: bool| {
        hero_pick_order.iter().copied().find(|k| {
            if held_classes.contains(k) {
                return false;
            }
            let known = records.get(me, *k).is_some();
            if revivals_only {
                // Bringing back a hero this team already owns is always worth
                // it — cheap, and it keeps a level the team already paid for.
                known
            } else {
                // A brand-new hero class waits for a Barracks, exactly as the
                // team's first one always did, and — if it would be the team's
                // second — for an army that can hold the base while it trains.
                !known && can_open_another && barracks_standing
            }
        })
    };
    let mut want_hero = slots_open
        .then(|| candidates(true).or_else(|| candidates(false)))
        .flatten();

    if let Some(hero_kind) = want_hero {
        let (hero_gold, hero_lumber, _) = hero_train_cost(records, me, hero_kind);
        let hero_supply = unit_stats(hero_kind).supply;
        // Hero training and revival happen at any finished rung of the hall
        // ladder — a team that teched to Keep must not lose its hero.
        let hall = own_buildings.iter().find(|b| b.done && is_hall(b.kind));
        if let Some(hall) = hall {
            if gold >= hero_gold && lumber >= hero_lumber && headroom >= hero_supply {
                gold -= hero_gold;
                lumber -= hero_lumber;
                headroom -= hero_supply;
                want_hero = None;
                orders.push((hall.entity, hero_kind));
            }
        }
    }

    // Still saving up? Ring-fence the hero's price so continuous army
    // production doesn't keep the treasury permanently just below it. Supply is
    // deliberately NOT reserved: army units are what drives the farm trigger,
    // and holding 5 supply back would stall the whole build order.
    let (mut reserve_gold, mut reserve_lumber) = match want_hero {
        Some(kind) => {
            let (g, l, _) = hero_train_cost(records, me, kind);
            (g, l)
        }
        None => (0, 0),
    };
    // Same ring-fence for the expansion down payment, held both while saving
    // up and for the whole walk out to the site. Without it the Barracks
    // drains every delivery and a 385g/205l TownHall is never reached — the AI
    // would "want" to expand forever while its last mine ran out.
    if saving_for_expansion || brain.expansion_pending {
        let stats = building_stats(r.hall);
        reserve_gold += stats.cost_gold;
        reserve_lumber += stats.cost_lumber;
    }
    // ...and for a tier-up we have decided on but cannot yet pay for.
    reserve_gold += tierup_reserve.0;
    reserve_lumber += tierup_reserve.1;
    // ...and for a research rung we have decided on but cannot yet pay for.
    reserve_gold += research_reserve.0;
    reserve_lumber += research_reserve.1;

    // --- the Shop ------------------------------------------------------------
    // Four rules, documented on `item_plan`. Placed here, after every other
    // reserve is known, because an item is the smallest deferred purchase in
    // the file and therefore the one most easily eaten: 125 gold is one
    // Footman, so without holding the price back the script would decide to buy
    // a Banner every second for the rest of the match and never own one. Two
    // different questions about money are being asked, deliberately:
    //   * is the BANK healthy enough that a consumable is a reasonable idea
    //     (`gold`, passed to `item_plan` as discretion);
    //   * can we pay for it out of what nothing else has already claimed
    //     (`gold - reserve_gold`).
    // A "yes, no" answer is exactly what the ring-fence exists for.
    brain.item_pending = false;
    let shop = own_buildings
        .iter()
        .find(|b| b.done && Some(b.kind) == r.vendor);
    if let (Some(shop), Some(hero)) = (shop, own_heroes.first()) {
        let tier = tech_tier_for(own_buildings.iter().filter(|b| b.done).map(|b| b.kind));
        // "A real fight", the same shape as the Slam rule: a clump on the hero.
        let engaged = enemy_any
            .iter()
            .filter(|e| e.distance(hero.pos) <= BANNER_RADIUS)
            .count()
            >= BANNER_MIN_TARGETS;
        // "About to march": the wave-launch condition from the military section
        // below, asked one section early. Boots last 15s and the walk is longer,
        // so the only non-wasteful moment to drink them is the moment the army
        // turns around.
        let marching =
            threat.is_none() && !brain.wave_active && army.len() >= brain.next_wave_size;
        match item_plan(tier, gold, hero.frac, hero.inventory, engaged, marching) {
            Some(ItemAction::Use(slot)) => {
                let what = hero.inventory.0[slot].map_or("?", |id| item_def(id).name);
                info!(
                    "[ai {me:?}] hero uses {what} (hp {:.0}%{}{})",
                    hero.frac * 100.0,
                    if engaged { ", in a fight" } else { "" },
                    if marching { ", marching out" } else { "" },
                );
                voice.say(Intent::UseItem {
                    slot,
                    // `hero` named explicitly rather than left to the
                    // compiler's lowest-living-id default: the script picked
                    // THIS hero's bag when it read the inventory, and a team
                    // with two heroes must not drink out of the other one's.
                    hero: Some(intent_id(hero.entity)),
                    // The scripted AI does not pick a hall. `None` is the
                    // nearest one, which is exactly what it got before the
                    // field existed — the baseline this ladder measures
                    // against must not move because a commander gained an
                    // option the script never had.
                    destination: None,
                });
            }
            Some(ItemAction::Buy(id)) => {
                let def = item_def(id);
                if gold.saturating_sub(reserve_gold) >= def.cost_gold {
                    gold -= def.cost_gold;
                    info!(
                        "[ai {me:?}] hero buys {} for {}g (bank {gold}g after)",
                        def.name, def.cost_gold
                    );
                    voice.say(Intent::Buy {
                        shop: intent_id(shop.entity),
                        item: def.name.to_string(),
                        hero: Some(intent_id(hero.entity)),
                    });
                } else {
                    brain.item_pending = true;
                    reserve_gold += def.cost_gold;
                }
            }
            None => {}
        }
    }

    // Sorcerers still owed against `MAX_SORCERERS`, counting the ones already
    // in a queue so a two-Sanctum team can't double-order them.
    let sorcerers_alive = army
        .iter()
        .filter(|u| Some(u.kind) == r.caster)
        .count()
        + own_buildings
            .iter()
            .map(|b| b.queued_sorcerers)
            .sum::<usize>();
    let mut sorcerers_wanted = MAX_SORCERERS.saturating_sub(sorcerers_alive);

    // Production order matters, because this loop spends a shared treasury as
    // it walks and whatever comes last gets what is left. The Sanctum goes
    // FIRST: it wants three units in the whole match against a Barracks that
    // wants one every few seconds, so in build order it always loses the race
    // and the tier-2 unlock the team paid 150g/130l for never produces a
    // single caster. Everything else keeps its historical relative order.
    // Standing, finished buildings — the same list `requirements_met` wants,
    // and what `unit_gate_ok` asks below in place of the hand-rolled "a
    // Workshop is up" / "tier >= 3" tests this file used to carry.
    let completed: Vec<BuildingKind> =
        own_buildings.iter().filter(|b| b.done).map(|b| b.kind).collect();
    let mut production: Vec<&BuildingInfo> = own_buildings.iter().filter(|b| b.done).collect();
    production.sort_by_key(|b| u8::from(Some(b.kind) != r.tech));

    for b in production {
        // A Keep and a Castle are the hall, so worker production keys off the
        // ladder rather than the tier-1 kind — teching up must not stop the
        // economy that paid for it.
        if is_hall(b.kind) {
            if worker_count >= TARGET_WORKERS || b.queue_len > 0 {
                continue;
            }
            let s = unit_stats(r.worker);
            if gold >= s.cost_gold && lumber >= s.cost_lumber && headroom >= s.supply {
                gold -= s.cost_gold;
                lumber -= s.cost_lumber;
                headroom -= s.supply;
                worker_count += 1;
                orders.push((b.entity, r.worker));
            }
            continue;
        }
        match b.kind {
            k if k == r.production => {
                if b.queue_len >= BARRACKS_QUEUE_MAX {
                    continue;
                }
                // Only advance the mix counter when something is actually
                // queued, and fall back to a Footman when the pricier pick
                // (Archer's lumber, Raider's gold) is out of reach.
                let next = brain.army_counter.wrapping_add(1);
                // Raiders are Workshop-gated: queueing one early would park an
                // unpayable item at the front and stall the whole Barracks.
                let raider_ok = r.cavalry.is_some_and(|c| unit_gate_ok(c, &completed));
                // Knights are Castle-gated the same way, and for the same
                // reason: an unpayable item at the front stalls the Barracks.
                // `current_tier` is the highest completed hall rung, so this is
                // exactly the condition `unit_requires` will re-check on pay.
                let knight_ok = r.shock.is_some_and(|k| unit_gate_ok(k, &completed));
                // `archer_nth` / `spearman_nth` are the standing cadences until
                // this team has actually LOOKED at a flyer or a horse, at which
                // point the relevant one tightens for ~50 thoughts and then
                // relaxes again. That is the entire reaction: same catalog,
                // same rule, a different number for a while.
                let mut wanted =
                    pick_army_kind(&r, next, knight_ok, raider_ok, archer_nth, spearman_nth);
                // **The two-roles-on-one-building tail.** A race whose
                // production building also makes siege (the Horde's WarCamp
                // does; the Kingdom's Barracks does not, so this is dead code
                // for it) paces siege here instead of at a Siegeworks, on the
                // same every-Nth beat and against the same counter.
                let affordable = |k: UnitKind| {
                    let s = unit_stats(k);
                    gold.saturating_sub(reserve_gold) >= s.cost_gold
                        && lumber.saturating_sub(reserve_lumber) >= s.cost_lumber
                        && headroom >= s.supply
                };
                if r.siegeworks.is_none() {
                    if let Some(siege) = r.siege.filter(|k| {
                        trainable(b.kind).contains(k) && unit_gate_ok(*k, &completed)
                    }) {
                        // **Both gates, and affordability too.** A Siegeworks
                        // arm can lean on the ratio alone because siege is the
                        // ONLY thing it makes: an unaffordable Catapult just
                        // stalls its own building. Here the same substitution
                        // would eat the army'''s beat — `siege_counter` only
                        // advances when one is actually queued, so a Demolisher
                        // the bank cannot cover would win every beat forever
                        // and the WarCamp would fall back to line units and
                        // never make a Headhunter, an Impaler or a Wolfrider
                        // again. Measured, before this line existed: 13 Grunts,
                        // 1 Headhunter and no cavalry at all in a won game.
                        //
                        // So siege takes the beat only when it is genuinely
                        // due AND payable; otherwise the roster mix stands.
                        let due = brain.siege_counter * CATAPULT_PER_ARMY <= brain.army_counter
                            && next % CATAPULT_PER_ARMY == 0;
                        if due && affordable(siege) {
                            wanted = siege;
                        }
                    }
                }
                let kind = if affordable(wanted) {
                    Some(wanted)
                } else if wanted != r.line && affordable(r.line) {
                    Some(r.line)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    let s = unit_stats(kind);
                    gold -= s.cost_gold;
                    lumber -= s.cost_lumber;
                    headroom -= s.supply;
                    if Some(kind) == r.siege {
                        brain.siege_counter = brain.siege_counter.wrapping_add(1);
                    } else {
                        brain.army_counter = next;
                    }
                    orders.push((b.entity, kind));
                }
            }
            k if Some(k) == r.tech => {
                if b.queue_len >= SANCTUM_QUEUE_MAX {
                    continue;
                }
                // **The other two-roles-on-one-building tail.** A race that
                // trains its flyer at its caster building (the Horde's Spirit
                // Lodge; the Kingdom's Sanctum trains no Gryphon, so again
                // this is dead code for it) buys air out of surplus here, on
                // the same Castle-standing / fat-bank / every-Nth test the
                // Siegeworks arm uses below.
                let air = r.flyer.filter(|k| {
                    r.siegeworks.is_none()
                        && trainable(b.kind).contains(k)
                        && unit_gate_ok(*k, &completed)
                        && gold.saturating_sub(reserve_gold) >= gryphon_bank_gold()
                        && brain.siege_counter % gryphon_every_nth() == 0
                });
                let pick = match air {
                    Some(flyer) => Some(flyer),
                    None if sorcerers_wanted == 0 => None,
                    None => r.caster,
                };
                let Some(pick) = pick else { continue };
                let s = unit_stats(pick);
                if gold.saturating_sub(reserve_gold) >= s.cost_gold
                    && lumber.saturating_sub(reserve_lumber) >= s.cost_lumber
                    && headroom >= s.supply
                {
                    gold -= s.cost_gold;
                    lumber -= s.cost_lumber;
                    headroom -= s.supply;
                    if Some(pick) == r.caster {
                        sorcerers_wanted -= 1;
                    } else {
                        brain.siege_counter = brain.siege_counter.wrapping_add(1);
                    }
                    orders.push((b.entity, pick));
                }
            }
            k if Some(k) == r.siegeworks => {
                if b.queue_len >= WORKSHOP_QUEUE_MAX {
                    continue;
                }
                // Pace siege against line units: skip until the ratio says a
                // catapult is due. The counter only advances on an actual
                // enqueue, so a broke Workshop doesn't bank up credit.
                if brain.siege_counter * CATAPULT_PER_ARMY > brain.army_counter {
                    continue;
                }
                // Air out of surplus only: a Castle standing, the bank fat, and
                // the siege counter on its every-Nth beat. Everything else the
                // Workshop makes is still a Catapult, and a Gryphon the script
                // cannot pay for silently degrades back to one rather than
                // parking an unaffordable item at the front of the queue.
                let want_air = current_tier >= 3
                    && gold.saturating_sub(reserve_gold) >= gryphon_bank_gold()
                    && brain.siege_counter % gryphon_every_nth() == 0;
                let Some(siege) = r.siege else { continue };
                let kind = match r.flyer.filter(|_| want_air) {
                    Some(flyer) => flyer,
                    None => siege,
                };
                let affordable = |k: UnitKind| {
                    let s = unit_stats(k);
                    gold.saturating_sub(reserve_gold) >= s.cost_gold
                        && lumber.saturating_sub(reserve_lumber) >= s.cost_lumber
                        && headroom >= s.supply
                };
                let kind = if affordable(kind) {
                    Some(kind)
                } else if kind != siege && affordable(siege) {
                    Some(siege)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    let s = unit_stats(kind);
                    gold -= s.cost_gold;
                    lumber -= s.cost_lumber;
                    headroom -= s.supply;
                    brain.siege_counter = brain.siege_counter.wrapping_add(1);
                    orders.push((b.entity, kind));
                }
            }
            // Non-producing buildings train nothing — Farms, Towers, Walls,
            // the forge, and the Shop, whose output is bought rather than
            // queued (see the Shop section above, which runs before this loop
            // precisely so its price is already out of the army's budget).
            _ => {}
        }
    }

    // One `train` per item, because that is what the verb is: a commander who
    // wants two Footmen says it twice, and giving the script a plural the
    // other seats do not have would be exactly the kind of quiet privilege
    // this bead exists to remove. The compiler re-checks the tech gate, the
    // hero slots, the queue cap and the price — all of which the loop above
    // already respected, which is why this is a conversion and not a nerf.
    for (entity, kind) in orders {
        voice.say(Intent::Train {
            building: intent_id(entity),
            unit: kind_name(kind).to_string(),
        });
    }

    // --- military ------------------------------------------------------------
    // Slam whenever a worthwhile clump is standing on the Champion. Same event
    // the player's R hotkey sends; combat.rs validates mana and cooldown.
    let slam_radius = hero_ability_radius() + SLAM_RADIUS_SLACK;
    for hero in &own_heroes {
        if !hero.ready {
            continue;
        }
        let pos = &hero.pos;
        // Ground enemies only: the Slam is a ground shockwave, so a clump of
        // flyers overhead must not talk the Champion into spending his mana on
        // an empty patch of dirt.
        let nearby = enemy_ground
            .iter()
            .filter(|e| e.distance(*pos) <= slam_radius)
            .count();
        if nearby >= SLAM_MIN_TARGETS {
            // `ability: None` — slot 0, which is what the script has always
            // fired; and no aim, because the Slam is caster-centred and there
            // is nothing to point it at. The day the script hand-fires a
            // targeted ability it fills in `x`/`z` or `target`, or omits both
            // and lets the compiler's auto-pick aim it, which is the same rule
            // the auto-caster obeys and the same sentence a commander writes.
            //
            // A hero IS a command node, so the link the compiler charges here
            // computes zero and the cast fires in the frame it always did.
            voice.say(Intent::Cast {
                hero: intent_id(hero.entity),
                ability: None,
                x: None,
                z: None,
                target: None,
            });
        }
    }

    let rally = base + (-base.normalize_or_zero()) * RALLY_DIST;

    // Every military branch below states one `attackmove` PER UNIT, all of
    // them naming the same point — the geometry the script has always had.
    // `Voice::say_each` carries the full reasoning; the short version is that
    // batching these would hand the group to `formation_offset` and make the
    // scripted baseline measurably more lethal, which is a balance change and
    // has no business arriving inside a plumbing bead.
    //
    // Three of the four branches are already self-quieting: `wave` stragglers
    // and the `rally` gather speak only for units that are idle, so an army in
    // contact and an army that has arrived both say nothing. `defend` is the
    // one that restates itself every tick, for as long as an enemy is actually
    // inside the base.
    let all: Vec<IntentId> = army.iter().map(|u| intent_id(u.entity)).collect();
    let free_units = |army: &[UnitInfo]| -> Vec<IntentId> {
        army.iter()
            .filter(|u| u.free())
            .map(|u| intent_id(u.entity))
            .collect()
    };

    if let Some(threat_pos) = threat {
        // Defense overrides everything, wave or not.
        voice.say_each(all, |units| Intent::AttackMove {
            units,
            x: Some(threat_pos.x),
            z: Some(threat_pos.z),
            region: None,
        });
        return;
    }

    if brain.wave_active {
        if army.len() < WAVE_ABORT_ARMY {
            brain.wave_active = false;
        } else if now - brain.wave_started > WAVE_TIMEOUT {
            // Stalled push: re-aim at whatever enemy building is closest.
            let centroid = army.iter().fold(Vec3::ZERO, |a, u| a + u.pos) / army.len() as f32;
            brain.wave_target = wave_objective(me, fog, nav, &enemy_buildings, centroid);
            brain.wave_started = now;
            let target = brain.wave_target;
            voice.say_each(all, |units| Intent::AttackMove {
                units,
                x: Some(target.x),
                z: Some(target.z),
                region: None,
            });
        } else {
            // Stragglers rejoin the push. Only the free ones, so a wave in
            // contact says nothing at all — the quiet branch, and the one the
            // army spends most of a push in.
            let target = brain.wave_target;
            voice.say_each(free_units(&army), |units| Intent::AttackMove {
                units,
                x: Some(target.x),
                z: Some(target.z),
                region: None,
            });
        }
    } else if army.len() >= brain.next_wave_size {
        brain.wave_active = true;
        brain.wave_started = now;
        brain.wave_target = wave_objective(me, fog, nav, &enemy_buildings, base);
        brain.next_wave_size = (brain.next_wave_size + WAVE_SIZE_STEP).min(WAVE_SIZE_CAP);
        let target = brain.wave_target;
        voice.say_each(all, |units| Intent::AttackMove {
            units,
            x: Some(target.x),
            z: Some(target.z),
            region: None,
        });
    } else {
        // Gather at the rally point while the army builds up. Whoever is free
        // and not there yet — which empties out as they arrive, and with it
        // this branch's sentence.
        let waiting: Vec<IntentId> = army
            .iter()
            .filter(|u| u.free() && u.pos.distance(rally) > RALLY_ARRIVE_DIST)
            .map(|u| intent_id(u.entity))
            .collect();
        voice.say_each(waiting, |units| Intent::AttackMove {
            units,
            x: Some(rally.x),
            z: Some(rally.z),
            region: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// NOTE: `fn script(what, now) -> Provenance` used to live here, minting a
// `Cause::Script { what }` rung of its own because "ai.rs is not a seat". It
// is one now, so the compiler stamps its orders `order:<verb> by script`
// alongside `by ui` and `by bridge`, and units under autopilot still answer
// that the autopilot moved them — through the ordinary rung rather than a
// bespoke one. What that costs is the free-text `what` ("wave", "flee",
// "rally") on the unit; what it buys is that a panel, a snapshot and the
// replay log describe a script order in the identical words they describe a
// player's, which is the point of the whole exercise. The reasoning behind a
// script order still exists — in this file's `info!` lines and in the intent
// log's sentence.

/// Ground-plane projection — mines and buildings sit at y=0, units do not.
fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

// ---------------------------------------------------------------------------
// Decisions, as pure functions
//
// Everything below is a rule the script follows, written so it can be READ and
// TESTED without an ECS. `think` above is a long function that queries the
// world; these are the parts of it that are actually opinions, and an opinion
// that cannot be unit-tested is an opinion that drifts.
// ---------------------------------------------------------------------------

/// Is the Castle worth buying right now?
///
/// `attack_researched` and `expanded` are the two proofs of a durable economy
/// (see `CASTLE_SPARE_GOLD`); either will do, and without one of them the
/// script stays at Keep no matter how the treasury looks on a single tick. The
/// gold test is against what is LEFT after paying, so tier 3 never comes out of
/// the army's budget — the same shape as every other surplus purchase here.
/// Is a Castle the right AMBITION? Deliberately asks nothing about cash.
///
/// Splitting the ambition from the payment is the whole reason tier 3 became
/// reachable. The old rule folded both into one test, so the Castle was only
/// ever "wanted" on a tick where it was already affordable — which meant the
/// ring-fence below could never fire, which meant the Barracks spent every
/// delivery, which meant the treasury never got there. Exactly the trap the
/// Keep, the expansion and the research rung all have a reserve to escape;
/// tier 3 was simply the one purchase that had been left out of the pattern.
fn castle_is_the_plan(army: usize, attack_researched: bool, expanded: bool) -> bool {
    army >= CASTLE_MIN_ARMY && (attack_researched || expanded)
}

/// ...and may we pay for it out of surplus right now? Against what is LEFT
/// after the price, so a Castle is never the last coin in the bank.
fn wants_castle(
    army: usize,
    gold: u32,
    cost_gold: u32,
    attack_researched: bool,
    expanded: bool,
) -> bool {
    castle_is_the_plan(army, attack_researched, expanded)
        && gold.saturating_sub(cost_gold) >= CASTLE_SPARE_GOLD
}

/// How many Towers the script wants standing, baseline plus reactive.
///
/// The `.min` is the load-bearing line of this whole bead: a Tower costs no
/// supply, so nothing else in the file stops the count from climbing. Callers
/// re-check `MAX_TOWERS` against what is standing anyway — belt and braces on
/// the one number that, if it slipped, would quietly convert the scripted
/// baseline into a turtle.
fn tower_quota(keep_standing: bool, air_alert: bool) -> usize {
    let baseline = if keep_standing { BASELINE_TOWERS } else { 0 };
    let reactive = usize::from(air_alert);
    (baseline + reactive).min(MAX_TOWERS)
}

/// Should the script order another Tower? `towers_standing` counts scaffolding
/// too, so a 25-second build is never ordered twice. The `MAX_TOWERS` test is
/// redundant against `tower_quota` today and stays anyway: this is the
/// predicate an editor will reach for when adding a reactive rule, and it must
/// be impossible to raise the ceiling by accident from here.
fn wants_tower(
    barracks_done: bool,
    towers_standing: usize,
    keep_standing: bool,
    air_alert: bool,
) -> bool {
    barracks_done
        && towers_standing < tower_quota(keep_standing, air_alert)
        && towers_standing < MAX_TOWERS
}

/// One think tick of an alert counter: a sighting refills it, silence drains
/// it one tick at a time. The whole of the script's "memory" of the enemy.
fn tick_alert(alert: u32, seen: bool) -> u32 {
    if seen {
        ALERT_TICKS
    } else {
        alert.saturating_sub(1)
    }
}

/// The Archer and Spearman cadences in force, given the two decaying alerts.
/// Lower is more frequent; these replace `ARCHER_EVERY_NTH` /
/// `SPEARMAN_EVERY_NTH` wholesale while an alert is live.
fn reactive_cadences(air_alert: bool, cavalry_alert: bool) -> (u32, u32) {
    let archer = if air_alert {
        ARCHER_EVERY_NTH_AIR
    } else {
        ARCHER_EVERY_NTH
    };
    let spearman = match (cavalry_alert, air_alert) {
        (true, true) => SPEARMAN_EVERY_NTH_BOTH,
        (true, false) => SPEARMAN_EVERY_NTH_CAVALRY,
        (false, _) => SPEARMAN_EVERY_NTH,
    };
    (archer, spearman)
}

/// What the Barracks queues as its `next`th item. Order is priority order:
/// the tier-3 line-breaker, then cavalry, then the two reactive slots.
/// The cadences are the ROSTER'S, resolved by role: the tier-3 line-breaker,
/// then cavalry, then the two reactive slots, then the line unit. Identical
/// output to the kind-named version it replaced for `Race::Kingdom`, and the
/// only place the Horde's mix is decided too — a race with no `Shock` unit
/// simply never takes the first branch.
fn pick_army_kind(
    r: &Roster,
    next: u32,
    knight_ok: bool,
    raider_ok: bool,
    archer_nth: u32,
    spearman_nth: u32,
) -> UnitKind {
    let pick = if knight_ok && next % KNIGHT_EVERY_NTH == 0 {
        r.shock
    } else if raider_ok && next % RAIDER_EVERY_NTH == 0 {
        r.cavalry
    } else if next % archer_nth == 0 {
        r.ranged
    } else if next % spearman_nth == 0 {
        r.anti_cavalry
    } else {
        None
    };
    pick.unwrap_or(r.line)
}

/// A crossing worth fortifying, and where the emplacement stands.
struct FordHold {
    /// Where the Tower goes: back from the gap, on our side of it.
    hold: Vec3,
    /// Unit vector along the barrier, i.e. across the opening. Wall segments
    /// are spaced along this.
    along: Vec3,
    /// The opening's width, which decides whether walls are allowed at all.
    width: f32,
    name: &'static str,
}

/// Pick the crossing to fortify: the (ford, own hall) pair with the shortest
/// distance between them, provided that distance is inside `FORD_HOLD_RADIUS`.
///
/// "Nearest ford to a hall we already own" rather than "nearest ford to our
/// start" is what keeps this honest about what a script can hold. On
/// `crossings` the flank fords ARE the neutral expansions, so this returns
/// nothing until the expansion lands and then returns the ford that expansion
/// is sitting on — the tower guards the second mine and the crossing with one
/// purchase. `None` on a map with no chokepoints, and `None` when every ford is
/// a long undefended walk away; both fall back to the ordinary base ring.
fn ford_hold_point(chokes: &[crate::terrain::ChokePoint], halls: &[Vec3]) -> Option<FordHold> {
    let mut best: Option<(f32, &crate::terrain::ChokePoint, Vec3)> = None;
    for choke in chokes {
        for hall in halls {
            let d = xz_dist(choke.pos, *hall);
            if d > FORD_HOLD_RADIUS {
                continue;
            }
            if best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, choke, *hall));
            }
        }
    }
    let (_, choke, hall) = best?;
    // Toward our own side of the gap. A hall sitting exactly on the ford (it
    // cannot, the mine is there) would leave no direction; fall back to the
    // gap centre rather than to a NaN.
    let across = (flat(hall) - flat(choke.pos)).normalize_or_zero();
    let hold = flat(choke.pos) + across * FORD_STANDOFF;
    // Perpendicular in the ground plane: the barrier runs across our approach.
    let along = Vec3::new(across.z, 0.0, -across.x);
    Some(FordHold {
        hold,
        along,
        width: choke.width,
        name: choke.name,
    })
}

/// Candidate wall spots beside a ford tower — a short screen on the barrier
/// line, never a plug. Empty for a gap too narrow to give up any of.
fn ford_wall_sites(hold: &FordHold) -> Vec<Vec3> {
    if hold.width < FORD_WALL_MIN_WIDTH {
        return Vec::new();
    }
    FORD_WALL_OFFSETS
        .iter()
        .map(|off| hold.hold + hold.along * *off)
        .collect()
}

/// One thing to do at the Shop this tick.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum ItemAction {
    /// Buy it if we can pay, ring-fence its price if we cannot.
    Buy(ItemId),
    /// Drink/plant what is already in that inventory slot.
    Use(usize),
}

/// The scripted shopping list, in strict priority order. Four rules, no state:
///
/// 1. **Drink at half health.** The one rule that is not about money. A hero is
///    a team's biggest single investment and the only unit that gets *worse*
///    permanently when it dies (revival costs and the level is at risk), so a
///    held potion at 50% is spent, not saved.
/// 2. **Plant the Banner in a real fight** — tier 2 only, and only with a
///    genuine clump on the hero, using the same "is this worth a cooldown"
///    shape as the Slam rule.
/// 3. **Boots when marching**, i.e. on the tick a wave launches: the haste is
///    15 seconds and the walk is longer than that, so spending it at the moment
///    the army turns toward the enemy is the only timing that isn't waste.
/// 4. **Restock**, cheapest need first: always hold a potion; then a Banner if
///    we are tier 2 and rich; then Boots if we are richer still.
///
/// `gold` here gates DISCRETION, not affordability — the caller decides whether
/// to pay or to ring-fence, exactly like the tier-up and research reserves.
fn item_plan(
    tier: TechTier,
    gold: u32,
    hero_frac: f32,
    inv: Inventory,
    engaged: bool,
    marching: bool,
) -> Option<ItemAction> {
    let slot_of = |id: ItemId| inv.0.iter().position(|s| *s == Some(id));

    if hero_frac <= POTION_HP_FRAC {
        if let Some(slot) = slot_of(ItemId::HealingPotion) {
            return Some(ItemAction::Use(slot));
        }
    }
    if engaged {
        if let Some(slot) = slot_of(ItemId::BannerOfCommand) {
            return Some(ItemAction::Use(slot));
        }
    }
    if marching {
        if let Some(slot) = slot_of(ItemId::BootsOfSpeed) {
            return Some(ItemAction::Use(slot));
        }
    }

    // Restocking needs a free slot; two consumables is the whole inventory.
    if inv.0.iter().all(|s| s.is_some()) {
        return None;
    }
    let want = |id: ItemId, rich: u32| {
        item_unlocked(id, tier) && slot_of(id).is_none() && gold >= rich
    };
    if want(ItemId::HealingPotion, POTION_RICH_GOLD) {
        return Some(ItemAction::Buy(ItemId::HealingPotion));
    }
    if want(ItemId::BannerOfCommand, BANNER_RICH_GOLD) {
        return Some(ItemAction::Buy(ItemId::BannerOfCommand));
    }
    if want(ItemId::BootsOfSpeed, BOOTS_RICH_GOLD) {
        return Some(ItemAction::Buy(ItemId::BootsOfSpeed));
    }
    None
}

fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    flat(a).distance(flat(b))
}

/// Decide whether a second (or third) mining base is wanted, and where it goes.
///
/// Two triggers, both dumb on purpose:
/// * the gold left in the mines we already hold has dropped below a threshold
///   chosen for *lead time* — the walk out plus a 40s build has to finish
///   before the old mine dies, not after;
/// * or we are running more workers than a mine can keep busy.
///
/// The target is the nearest live mine nobody has claimed, skipping anything
/// standing in the enemy's front yard or with their army parked on it.
#[allow(clippy::too_many_arguments)]
fn plan_expansion(
    me: Team,
    // This race's tier-1 hall — an expansion is a hall, and which hall depends
    // on who is expanding.
    hall: BuildingKind,
    own_buildings: &[BuildingInfo],
    enemy_buildings: &[Vec3],
    mines: &[MineInfo],
    workers: &[UnitInfo],
    enemy_combat: &[Vec3],
    nav: &NavGrid,
) -> Option<ExpansionPlan> {
    let claimed_gold: u32 = mines.iter().filter(|m| m.claimed).map(|m| m.remaining).sum();
    let claimed_count = mines.iter().filter(|m| m.claimed).count();
    // Rough count of the gold half of the line (every LUMBER_EVERY_NTH worker
    // goes to trees instead).
    let gold_workers = workers.len() - workers.len() / LUMBER_EVERY_NTH as usize;
    let saturated = claimed_count == 0 || gold_workers > WORKERS_PER_MINE * claimed_count;
    if claimed_gold >= EXPAND_GOLD_LEFT && !saturated {
        return None;
    }

    let home = own_buildings
        .iter()
        .find(|b| is_hall(b.kind))
        .map(|b| b.pos)
        .unwrap_or(me.base_pos());
    let enemy_base = me.enemy().base_pos();

    let mut best: Option<(f32, &MineInfo)> = None;
    for mine in mines {
        if mine.claimed {
            continue;
        }
        // Their home mine sits inside this ring. Settling there is a gift.
        if xz_dist(mine.pos, enemy_base) < ENEMY_BASE_KEEPOUT {
            continue;
        }
        // Someone already lives here. Same radius that makes a mine "ours"
        // when the hall is ours: if it works as a drop-off for them, the mine
        // is theirs and contesting it is a fight, not an expansion.
        if enemy_buildings
            .iter()
            .any(|b| xz_dist(*b, mine.pos) < MINE_CLAIM_RADIUS)
        {
            continue;
        }
        if enemy_combat
            .iter()
            .any(|e| xz_dist(*e, mine.pos) < EXPAND_DANGER_RADIUS)
        {
            continue;
        }
        let d = xz_dist(mine.pos, home);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, mine));
        }
    }
    let (_, mine) = best?;

    let footprint = building_stats(hall).size + BUILD_PADDING;
    let site = pick_expansion_site(nav, mine.pos, home, footprint)?;
    Some(ExpansionPlan {
        site,
        mine_pos: mine.pos,
        mine_gold: mine.remaining,
        claimed_gold,
    })
}

/// A hall site hugging `mine`: inner rings first (short haul), and within a
/// ring the spot nearest `home` first, so the hall ends up on our side of the
/// mine — shorter builder walk, shorter answer when it gets raided.
fn pick_expansion_site(nav: &NavGrid, mine: Vec3, home: Vec3, footprint: f32) -> Option<Vec3> {
    let limit = MAP_HALF - footprint;
    for radius in EXPAND_RING_RADII {
        let mut ring: Vec<Vec3> = Vec::new();
        for spoke in 0..BUILD_RING_SPOKES {
            let a = spoke as f32 * std::f32::consts::TAU / BUILD_RING_SPOKES as f32;
            let pos = mine + Vec3::new(a.cos(), 0.0, a.sin()) * radius;
            if pos.x.abs() > limit || pos.z.abs() > limit {
                continue;
            }
            ring.push(flat(pos));
        }
        ring.sort_by(|a, b| {
            xz_dist(*a, home)
                .partial_cmp(&xz_dist(*b, home))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(site) = ring.into_iter().find(|p| nav.rect_is_free(*p, footprint)) {
            return Some(site);
        }
    }
    None
}

/// Which mining post a worker belongs to. Intent first — the node its order
/// names — so workers already walking to a new mine count at the destination
/// instead of their origin; otherwise every tick would peel two more off the
/// old mine and the whole crew would migrate. Position is only the fallback,
/// for workers whose order names a node that no longer exists (economy.rs
/// re-targets those silently). A worker on a live node that isn't a post —
/// a lumberjack, most often — belongs to nobody and is left alone.
fn post_of(posts: &[&MineInfo], worker: &UnitInfo, nodes: &NodeQuery) -> Option<usize> {
    if let Some(node) = worker.harvest_node {
        if let Some(i) = posts.iter().position(|m| m.entity == node) {
            return Some(i);
        }
        if nodes.get(node).is_ok() {
            return None;
        }
    }
    posts
        .iter()
        .enumerate()
        .filter(|(_, m)| xz_dist(m.pos, worker.pos) < MINE_WORKER_RADIUS)
        .min_by(|(_, a), (_, b)| {
            xz_dist(a.pos, worker.pos)
                .partial_cmp(&xz_dist(b.pos, worker.pos))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// Even out the crews across every mine we can actually deliver from, a couple
/// of workers per tick, and only while the gap is worth walking for.
fn rebalance_mines(
    mines: &[MineInfo],
    workers: &[UnitInfo],
    skip: &[Entity],
    nodes: &NodeQuery,
    voice: &mut Voice,
) {
    // A mine with no finished hall near it is not a posting: sending workers
    // there would just make them haul their load back to the old base.
    let posts: Vec<&MineInfo> = mines.iter().filter(|m| m.has_depot).collect();
    if posts.len() < 2 {
        return;
    }

    let mut crew = vec![0usize; posts.len()];
    let mut movable: Vec<Vec<(Entity, Vec3)>> = vec![Vec::new(); posts.len()];
    for w in workers {
        let Some(i) = post_of(&posts, w, nodes) else {
            continue;
        };
        crew[i] += 1;
        // Counted but not moved: a builder mid-order, a worker fleeing, or one
        // holding a load it should bank first.
        if skip.contains(&w.entity) || w.tag == Tag::Build || w.carrying {
            continue;
        }
        movable[i].push((w.entity, w.pos));
    }

    let by_crew = |pick: fn(&usize, &usize) -> bool| -> usize {
        let mut best = 0;
        for i in 1..crew.len() {
            if pick(&crew[i], &crew[best]) {
                best = i;
            }
        }
        best
    };
    let src = by_crew(|a, b| a > b);
    let dst = by_crew(|a, b| a < b);
    if src == dst || crew[src] < crew[dst] + 2 {
        return;
    }
    // Half the gap, so the two crews meet in the middle instead of trading
    // places on the next tick.
    let quota = ((crew[src] - crew[dst]) / 2).min(SHIFT_PER_TICK);

    let target = posts[dst];
    let mut pool = std::mem::take(&mut movable[src]);
    pool.sort_by(|(_, a), (_, b)| {
        xz_dist(*a, target.pos)
            .partial_cmp(&xz_dist(*b, target.pos))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // The shift, as one sentence: same destination, same tick, so it is one
    // order given to two workers rather than two orders that happen to agree.
    let shift: Vec<IntentId> = pool
        .into_iter()
        .take(quota)
        .map(|(worker, _)| intent_id(worker))
        .collect();
    voice.say_group(shift, |units| Intent::Harvest {
        units,
        target: intent_id(target.entity),
    });
}

fn other_resource(kind: ResourceKind) -> ResourceKind {
    match kind {
        ResourceKind::Gold => ResourceKind::Lumber,
        ResourceKind::Lumber => ResourceKind::Gold,
    }
}

fn nearest_node(
    nodes: &NodeQuery,
    from: Vec3,
    kind: ResourceKind,
) -> Option<Entity> {
    nodes
        .iter()
        .filter(|(_, n, _)| n.kind == kind && n.remaining > 0)
        .min_by(|(_, _, a), (_, _, b)| {
            a.translation
                .distance(from)
                .partial_cmp(&b.translation.distance(from))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(e, _, _)| e)
}

fn nearest_pos(candidates: &[Vec3], from: Vec3) -> Option<Vec3> {
    candidates
        .iter()
        .copied()
        .min_by(|a, b| {
            a.distance(from)
                .partial_cmp(&b.distance(from))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// Where a wave marches, in strict order of preference:
///
/// 1. the nearest enemy structure this team KNOWS about — seen right now, or
///    remembered from a scout that has since died;
/// 2. failing that, the opponent's starting base, as long as we have never
///    actually looked at it. It is the one enemy position every player is born
///    knowing, and walking there is both an attack and a scouting run;
/// 3. failing that, the nearest patch of map this team has never seen.
///
/// Clause 3 is what keeps fog from turning into a stalemate machine, and it is
/// the minimal explore behaviour the scripted AI needed to survive this change.
/// Before fog, "attack the enemy base" could never be wrong, because the enemy
/// base was always in the snapshot. Now an opponent that loses its main and
/// lives on an unscouted expansion is genuinely lost — and an army that only
/// ever walks to a place it has already confirmed is empty would never find
/// it, so the match would run to the time cap with both sides intact. Sweeping
/// the unexplored is the cheapest behaviour that makes the win condition
/// reachable again; it is not clever, and it is not meant to be.
fn wave_objective(
    me: Team,
    fog: &FogGrid,
    nav: &NavGrid,
    known_enemy_buildings: &[Vec3],
    from: Vec3,
) -> Vec3 {
    if let Some(target) = nearest_pos(known_enemy_buildings, from) {
        return target;
    }
    let their_home = me.enemy().base_pos();
    if !fog.known(their_home) {
        return their_home;
    }
    nearest_unexplored(fog, from, nav).unwrap_or(their_home)
}

/// Prefer an idle worker, otherwise the nearest one that is merely harvesting.
fn pick_builder(workers: &[UnitInfo], skip: &[Entity], site: Vec3) -> Option<Entity> {
    let mut best: Option<(f32, Entity)> = None;
    let mut best_busy: Option<(f32, Entity)> = None;
    for w in workers {
        if skip.contains(&w.entity) || w.tag == Tag::Build || w.carrying {
            continue;
        }
        let d = w.pos.distance(site);
        let slot = if w.free() { &mut best } else { &mut best_busy };
        let better = match *slot {
            Some((best_d, _)) => d < best_d,
            None => true,
        };
        if better {
            *slot = Some((d, w.entity));
        }
    }
    best.or(best_busy).map(|(_, e)| e)
}

/// The free spot NEAREST `center`, searching outward in tight rings — the
/// placement rule for a structure whose whole value is where it stands. Falls
/// back to `None` if the ground around it is full, and the caller then uses the
/// ordinary base ring.
fn pick_spot(nav: &NavGrid, center: Vec3, footprint: f32) -> Option<Vec3> {
    let limit = MAP_HALF - footprint;
    let ok = |p: Vec3| p.x.abs() <= limit && p.z.abs() <= limit && nav.rect_is_free(p, footprint);
    let center = flat(center);
    if ok(center) {
        return Some(center);
    }
    for radius in EMPLACE_RING_RADII {
        for spoke in 0..BUILD_RING_SPOKES {
            let a = spoke as f32 * std::f32::consts::TAU / BUILD_RING_SPOKES as f32;
            let p = center + Vec3::new(a.cos(), 0.0, a.sin()) * radius;
            if ok(p) {
                return Some(p);
            }
        }
    }
    None
}

/// Rings of candidate spots around `anchor`, nearest-to-map-center first so the
/// base expands outward toward the middle instead of walling itself in.
fn pick_site(nav: &NavGrid, anchor: Vec3, footprint: f32) -> Option<Vec3> {
    let mut candidates: Vec<Vec3> = Vec::new();
    for radius in BUILD_RING_RADII {
        for spoke in 0..BUILD_RING_SPOKES {
            let a = spoke as f32 * std::f32::consts::TAU / BUILD_RING_SPOKES as f32;
            let pos = anchor + Vec3::new(a.cos(), 0.0, a.sin()) * radius;
            let limit = MAP_HALF - footprint;
            if pos.x.abs() > limit || pos.z.abs() > limit {
                continue;
            }
            candidates.push(Vec3::new(pos.x, 0.0, pos.z));
        }
    }
    candidates.sort_by(|a, b| {
        a.length()
            .partial_cmp(&b.length())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.into_iter().find(|p| nav.rect_is_free(*p, footprint))
}

// ---------------------------------------------------------------------------
// Tests
//
// The scripted AI is the baseline every balance run is measured against, so the
// thing worth pinning is not "does it compile" but "does it still hold the
// opinions it was tuned to hold". Each test below names a failure that has
// actually happened, or one the design notes above exist to prevent.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandLatency, CommandNodes, PendingOrder, DEFAULT_HALL_RADIUS};
    use crate::terrain::{ChokePoint, MapKind};

    // -- Castle trigger (wc3clone-0m8) ------------------------------------

    /// The regression this bead exists for: the old gate was `gold - 480 >=
    /// 300`, i.e. 780 gold in hand on a single tick, and no scripted match ever
    /// banked that. Tier 3 must now be reachable at a treasury the script
    /// actually reaches.
    #[test]
    fn castle_is_reachable_at_a_realistic_treasury() {
        let cost = building_stats(BuildingKind::Castle).cost_gold;
        // 560 gold: a real peak for a scripted economy that has already paid
        // for a research rung. Under the old rule this was a hard no.
        assert!(560 < cost + 300, "sanity: this is below the OLD bar");
        assert!(wants_castle(8, 560, cost, true, false));
        assert!(wants_castle(8, 560, cost, false, true));
    }

    /// The bug that made the loosening necessary in the first place, as a test.
    /// The ambition must be expressible while BROKE — that is the only state in
    /// which a reserve does anything, and without a reserve continuous Barracks
    /// production keeps the treasury permanently a Footman short of 480 gold.
    #[test]
    fn the_castle_can_be_wanted_before_it_can_be_afforded() {
        let cost = building_stats(BuildingKind::Castle).cost_gold;
        assert!(castle_is_the_plan(8, true, false));
        assert!(
            !wants_castle(8, CASTLE_LATCH_GOLD, cost, true, false),
            "not payable yet"
        );
        assert!(
            castle_is_the_plan(8, true, false),
            "...but still the plan, which is what arms the ring-fence"
        );
        // And the latch only arms once the bank is genuinely within reach, so a
        // poor game never quietly stops training to save for a hall it will
        // never buy.
        assert!(CASTLE_LATCH_GOLD < cost);
        assert!(CASTLE_LATCH_GOLD >= cost / 2);
    }

    /// ...but not on gold alone. Without a finished research rung or a second
    /// mining base, a one-tick spike in the treasury proves nothing, and a
    /// Castle bought out of a spike is a Castle bought instead of an army.
    #[test]
    fn castle_needs_a_proof_of_durable_income() {
        let cost = building_stats(BuildingKind::Castle).cost_gold;
        assert!(!wants_castle(8, 2000, cost, false, false));
        assert!(!wants_castle(CASTLE_MIN_ARMY - 1, 2000, cost, true, true));
    }

    /// The leftover-gold test survives the loosening: a Castle is still never
    /// the last coin in the bank.
    #[test]
    fn castle_still_comes_out_of_surplus() {
        let cost = building_stats(BuildingKind::Castle).cost_gold;
        assert!(!wants_castle(8, cost, cost, true, true));
        assert!(!wants_castle(8, cost + CASTLE_SPARE_GOLD - 1, cost, true, true));
        assert!(wants_castle(8, cost + CASTLE_SPARE_GOLD, cost, true, true));
    }

    // -- Tower cap (wc3clone-7gv) ------------------------------------------

    /// Towers cost no supply, which is the only reason a cap has to exist at
    /// all. No combination of inputs may ask for more than `MAX_TOWERS`, and no
    /// number of standing towers may talk `wants_tower` into one more.
    #[test]
    fn the_tower_fortress_is_unreachable() {
        for keep in [false, true] {
            for air in [false, true] {
                assert!(tower_quota(keep, air) <= MAX_TOWERS);
                for standing in 0..30usize {
                    let wanted = wants_tower(true, standing, keep, air);
                    assert!(
                        !wanted || standing < MAX_TOWERS,
                        "asked for tower #{} past the cap",
                        standing + 1
                    );
                }
            }
        }
        // 25 towers is the specific failure mode this guards; so is 5.
        assert!(!wants_tower(true, MAX_TOWERS, true, true));
        assert!(!wants_tower(true, 25, true, true));
    }

    /// No towers before there is a Barracks to gate them, none before the Keep
    /// that marks the end of the opening — and exactly one extra for air.
    #[test]
    fn tower_quota_is_baseline_plus_one_for_air() {
        assert_eq!(tower_quota(false, false), 0);
        assert_eq!(tower_quota(true, false), BASELINE_TOWERS);
        assert_eq!(tower_quota(true, true), BASELINE_TOWERS + 1);
        // Air is an emergency: it buys a tower even at tier 1.
        assert_eq!(tower_quota(false, true), 1);
        assert!(!wants_tower(false, 0, true, true), "needs a Barracks first");
        // The cap leaves headroom for exactly one future reactive rule.
        assert!(BASELINE_TOWERS + 1 <= MAX_TOWERS);
    }

    // -- Reactive mix (wc3clone-7gv, wc3clone-20d) -------------------------

    /// A sighting refills the counter; silence drains it one thought at a time
    /// and it lands on zero rather than wrapping.
    #[test]
    fn alerts_refill_on_sight_and_decay_to_nothing() {
        assert_eq!(tick_alert(0, true), ALERT_TICKS);
        assert_eq!(tick_alert(ALERT_TICKS, false), ALERT_TICKS - 1);
        let mut a = tick_alert(0, true);
        for _ in 0..ALERT_TICKS {
            a = tick_alert(a, false);
        }
        assert_eq!(a, 0, "alert must lapse, or the reaction is permanent");
        assert_eq!(tick_alert(0, false), 0, "must not wrap");
    }

    #[test]
    fn cadences_tighten_only_for_what_was_seen() {
        assert_eq!(
            reactive_cadences(false, false),
            (ARCHER_EVERY_NTH, SPEARMAN_EVERY_NTH)
        );
        assert_eq!(
            reactive_cadences(true, false),
            (ARCHER_EVERY_NTH_AIR, SPEARMAN_EVERY_NTH)
        );
        assert_eq!(
            reactive_cadences(false, true),
            (ARCHER_EVERY_NTH, SPEARMAN_EVERY_NTH_CAVALRY)
        );
        // Both at once: the Archer rule already owns every second slot, so the
        // Spearman rule steps aside to 3 rather than being starved to zero.
        assert_eq!(
            reactive_cadences(true, true),
            (ARCHER_EVERY_NTH_AIR, SPEARMAN_EVERY_NTH_BOTH)
        );
    }

    /// Count what 24 Barracks items actually come out as. The point of the
    /// reaction is that the ARMY changes, not that a constant changed.
    fn mix(air: bool, cavalry: bool) -> (usize, usize, usize) {
        let (a_nth, s_nth) = reactive_cadences(air, cavalry);
        let (mut archers, mut spears, mut footmen) = (0, 0, 0);
        for next in 1..=24u32 {
            match pick_army_kind(&Roster::of(Race::Kingdom), next, false, false, a_nth, s_nth) {
                UnitKind::Archer => archers += 1,
                UnitKind::Spearman => spears += 1,
                UnitKind::Footman => footmen += 1,
                other => panic!("ungated {other:?} in a tier-1 mix"),
            }
        }
        (archers, spears, footmen)
    }

    #[test]
    fn air_contact_really_produces_more_archers() {
        let (base_archers, _, _) = mix(false, false);
        let (air_archers, _, _) = mix(true, false);
        assert!(
            air_archers > base_archers,
            "air alert produced {air_archers} archers vs {base_archers} baseline"
        );
        assert_eq!(air_archers, 12, "half the line, out of 24");
    }

    #[test]
    fn cavalry_contact_really_produces_more_spearmen() {
        let (_, base_spears, _) = mix(false, false);
        let (_, cav_spears, _) = mix(false, true);
        assert!(
            cav_spears > base_spears,
            "cavalry alert produced {cav_spears} spearmen vs {base_spears} baseline"
        );
    }

    /// The failure mode of stacking two reactions: one rule eats every slot and
    /// the other silently stops existing. Both must still show up.
    #[test]
    fn both_alerts_at_once_still_produce_both_answers() {
        let (archers, spears, footmen) = mix(true, true);
        assert!(
            archers > 0 && spears > 0,
            "{archers} archers, {spears} spearmen"
        );
        assert!(
            footmen > 0,
            "a reaction must not replace the line entirely — the script is a \
             baseline, not a counter-picker"
        );
    }

    /// Reactions never unlock anything: the tier gates still own the roster.
    #[test]
    fn reactions_cannot_smuggle_in_gated_units() {
        for next in 1..=40u32 {
            let k = pick_army_kind(&Roster::of(Race::Kingdom), next, false, false, 2, 2);
            assert!(!matches!(k, UnitKind::Knight | UnitKind::Raider));
        }
        assert_eq!(
            pick_army_kind(&Roster::of(Race::Kingdom), KNIGHT_EVERY_NTH, true, false, 3, 4),
            UnitKind::Knight
        );
    }

    // -- The air reaction, end to end (wc3clone-il4) ------------------------
    //
    // Everything above this line tests the air reaction as arithmetic:
    // `tick_alert` counts, `reactive_cadences` returns a different number,
    // `tower_quota` returns a bigger one. None of it proves the script ever
    // REACHES those functions, and the sim runs of the era proved it never
    // had: no scripted match built a Gryphon (Castle + Workshop + 700g spare +
    // the every-third beat, all on one tick), so no scripted match ever showed
    // an enemy flyer to anybody, so the archer shift and the reactive Tower
    // were dead code that passed its unit tests.
    //
    // What follows is the missing half: one real `ai_think` tick, on a real
    // World, with a real enemy Gryphon in the fog, asserting the two things the
    // script is supposed to DO about it. The knobs added for the sim
    // (`WC3_AI_GRYPHON_BANK` / `_NTH`) let a headless run reach the same place
    // the slow way; this reaches it in a millisecond and on every CI run.

    /// A world with the scripted commander in it and nothing else: no fog to
    /// walk through, no economy ticking, no units.rs to execute the orders.
    /// One `app.update()` past the think timer is exactly one thought.
    ///
    /// **The compiler is part of the harness now** (wc3clone-jem). The script
    /// states intents and mutates nothing, so a test that ran `ai_think` alone
    /// would watch a world where nothing ever happens. Running the real
    /// `apply_intents` after it is not a concession to the refactor — it is
    /// what makes these assertions stronger than they were, because a Tower in
    /// the world now proves the placement survived validation as well as
    /// intention.
    fn ai_app() -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<AiState>()
            .init_resource::<GameOver>()
            .init_resource::<HeroRecords>()
            .init_resource::<NavGrid>()
            .init_resource::<TeamResearch>()
            // Everything `apply_intents` reads beyond what the script already
            // needed, and the five event channels it writes.
            .init_resource::<TechTiers>()
            .init_resource::<SquadOrders>()
            .init_resource::<GameEvents>()
            .add_event::<CastAbility>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            // Chain of Command, off: this bead is about WHAT the script decides,
            // not how long the decision takes to arrive. With latency on, the
            // Tower order would sit in `PendingOrder` and the assertion below
            // would be testing the wrong module.
            .insert_resource(CommandLatency { on: false, ..Default::default() })
            .insert_resource(CommandNodes {
                nodes: vec![(Team::Claude, Team::Claude.base_pos(), DEFAULT_HALL_RADIUS)],
                ready: true,
            })
            .add_plugins(crate::intent::IntentPlugin)
            // Explicit, because these two sets are only ordered by `SIM_ORDER`
            // in the real schedule and this app does not install it. Thinking
            // and compiling in the same frame IS the shipped frame order
            // (`SimSet::AiThink` sits four sets ahead of `SimSet::Intent`), so
            // pinning it here is a restatement, not a fixture.
            .add_systems(Update, ai_think.before(crate::intent::IntentApply));
        // A unit test must depend on neither `WC3_INTENT_LOG` nor the
        // filesystem, and must leave no file behind.
        app.insert_resource(crate::intent::IntentLog::disabled());
        // Claude only, so the assertions below can name one brain.
        app.insert_resource(AiControlled { human: false, claude: true });
        // Lit, not dark: the subject is what the script does about a flyer it
        // can see, not whether it can see it. `test_dark` would make this test
        // pass for the wrong reason (no sighting, no reaction, no assertion).
        app.insert_resource(FogGrids::test_revealed());
        // Rich enough that no branch below is refused for money, and supplied
        // enough that the Farm branch (which sits above the Tower) stays quiet.
        let mut economies = Economies::default();
        let claude = economies.get_mut(Team::Claude);
        claude.gold = 900;
        claude.lumber = 500;
        claude.supply_used = 10;
        claude.supply_cap = 60;
        app.insert_resource(economies);
        app
    }

    fn spawn_building(app: &mut App, kind: BuildingKind, team: Team, pos: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Building { kind },
                team,
                Transform::from_translation(pos),
                TrainingQueue::default(),
                Health::new(building_stats(kind).hp),
            ))
            .id()
    }

    fn spawn_unit(app: &mut App, kind: UnitKind, team: Team, pos: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind },
                team,
                Transform::from_translation(pos),
                Order::Idle,
                Health::new(unit_stats(kind).hp),
            ))
            .id()
    }

    /// A Claude base that has finished its opening: a hall, a Barracks, and a
    /// worker line. No Tower yet, and the army counter parked one short of a
    /// beat that only the AIR cadence divides.
    fn claude_base(app: &mut App) -> Entity {
        let home = Team::Claude.base_pos();
        spawn_building(app, BuildingKind::TownHall, Team::Claude, home);
        let barracks = spawn_building(
            app,
            BuildingKind::Barracks,
            Team::Claude,
            home + Vec3::new(-12.0, 0.0, 0.0),
        );
        for i in 0..5 {
            spawn_unit(
                app,
                UnitKind::Worker,
                Team::Claude,
                home + Vec3::new(3.0 + i as f32, 0.0, 3.0),
            );
        }
        // next = 4: divisible by ARCHER_EVERY_NTH_AIR (2) but not by
        // ARCHER_EVERY_NTH (3), so the very next Barracks item is an Archer if
        // and only if air has been seen. Absent the alert the same beat falls
        // through to the Spearman rule, which is what the control asserts.
        app.world_mut().resource_mut::<AiState>().claude.army_counter = 3;
        barracks
    }

    fn queued(app: &mut App, building: Entity) -> Vec<UnitKind> {
        app.world()
            .entity(building)
            .get::<TrainingQueue>()
            .map(|q| q.queue.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Is anybody building a Tower — i.e. did the reactive emplacement branch
    /// actually reach a worker?
    fn tower_ordered(app: &mut App) -> bool {
        app.world_mut()
            .query::<&Order>()
            .iter(app.world())
            .any(|o| matches!(o, Order::Build { kind: BuildingKind::Tower, .. }))
    }

    /// THE test this bead exists for. An enemy Gryphon in sight, one thought,
    /// and both halves of the documented reaction have to be visible in the
    /// world: the mix tightens toward Archers, and a Tower goes up.
    #[test]
    fn a_seen_gryphon_tightens_the_archer_cadence_and_buys_a_tower() {
        let mut app = ai_app();
        let barracks = claude_base(&mut app);
        // The flyer, in Claude's face and in Claude's vision.
        spawn_unit(
            &mut app,
            UnitKind::GryphonRider,
            Team::Human,
            Team::Claude.base_pos() + Vec3::new(-20.0, 0.0, -20.0),
        );

        think_once(&mut app);

        // The sighting registered, as a decaying counter and not as a memory
        // of where the thing was.
        let alert = app.world().resource::<AiState>().claude.air_alert;
        assert_eq!(alert, ALERT_TICKS, "the sighting must refill the alert");

        // Reaction one: the Barracks queued the anti-air unit. This is the
        // assertion the arithmetic tests could not make — it is the real
        // `think` walking the real cadence into a real queue.
        assert_eq!(
            queued(&mut app, barracks),
            vec![UnitKind::Archer],
            "air contact must put an Archer in the Barracks, not a Footman"
        );

        // Reaction two: the emplacement. A flyer cannot be blocked or
        // out-walked, so the script buys the thing that is already standing.
        assert!(
            tower_ordered(&mut app),
            "a seen Gryphon must buy a reactive Tower"
        );
    }

    /// The control, and the reason the test above means anything: the SAME
    /// board with a ground unit instead of a flyer produces neither reaction.
    /// Without this, a script that always built Archers and always built
    /// Towers would pass.
    #[test]
    fn a_seen_footman_buys_neither_the_archer_nor_the_tower() {
        let mut app = ai_app();
        let barracks = claude_base(&mut app);
        spawn_unit(
            &mut app,
            UnitKind::Footman,
            Team::Human,
            Team::Claude.base_pos() + Vec3::new(-20.0, 0.0, -20.0),
        );

        think_once(&mut app);

        assert_eq!(app.world().resource::<AiState>().claude.air_alert, 0);
        // The same beat (next = 4) that air contact turns into an Archer falls
        // through to the Spearman rule when the sky is empty. What matters is
        // that it is NOT the anti-air pick.
        let standing = queued(&mut app, barracks);
        assert_eq!(standing, vec![UnitKind::Spearman]);
        assert!(
            !standing.contains(&UnitKind::Archer),
            "no flyer seen, so nothing should have shifted toward anti-air"
        );
        assert!(
            !tower_ordered(&mut app),
            "the reactive Tower is reactive — no flyer, no emplacement"
        );
    }

    /// The alert is a fading memory, not a latch: once the Gryphon is gone the
    /// mix goes back to standard. A permanent reaction would be an AI that
    /// counters whatever it saw once, forever.
    #[test]
    fn the_air_reaction_lapses_when_the_sky_clears() {
        let mut app = ai_app();
        let barracks = claude_base(&mut app);
        let gryphon = spawn_unit(
            &mut app,
            UnitKind::GryphonRider,
            Team::Human,
            Team::Claude.base_pos() + Vec3::new(-20.0, 0.0, -20.0),
        );
        think_once(&mut app);
        assert_eq!(queued(&mut app, barracks), vec![UnitKind::Archer]);

        // It leaves. Nothing else about the board changes.
        app.world_mut().entity_mut(gryphon).despawn();
        app.world_mut().resource_mut::<AiState>().claude.army_counter = 3;
        app.world_mut()
            .entity_mut(barracks)
            .get_mut::<TrainingQueue>()
            .unwrap()
            .queue
            .clear();
        for _ in 0..ALERT_TICKS {
            think_once(&mut app);
        }
        assert_eq!(
            app.world().resource::<AiState>().claude.air_alert,
            0,
            "the alert must drain, or the reaction is permanent"
        );
    }

    /// The other end of the same path: the script actually BUILDING a flyer.
    ///
    /// Four conditions have to be true on one think tick — a Castle standing, a
    /// Workshop with room in its queue, the siege counter on its beat, and the
    /// bank fat after the reserve — and no scripted match has ever had all four
    /// at once, which is why nobody had seen this branch either. A headless run
    /// with `WC3_AI_GRYPHON_BANK=0` still needs the match to LAST long enough to
    /// reach tier 3, and the scripted matchup resolves at tier 2 (verified: both
    /// maps end 6-7 minutes with the loser collapsing before its Castle). So the
    /// board is built here instead of waited for.
    #[test]
    fn a_castle_and_a_fat_bank_put_a_gryphon_in_the_workshop() {
        let mut app = ai_app();
        let home = Team::Claude.base_pos();
        spawn_building(&mut app, BuildingKind::Castle, Team::Claude, home);
        spawn_building(
            &mut app,
            BuildingKind::Barracks,
            Team::Claude,
            home + Vec3::new(-12.0, 0.0, 0.0),
        );
        let workshop = spawn_building(
            &mut app,
            BuildingKind::Workshop,
            Team::Claude,
            home + Vec3::new(0.0, 0.0, -12.0),
        );
        for i in 0..5 {
            spawn_unit(
                &mut app,
                UnitKind::Worker,
                Team::Claude,
                home + Vec3::new(3.0 + i as f32, 0.0, 3.0),
            );
        }
        {
            // A long game's worth of line units behind us, so siege is due.
            let mut state = app.world_mut().resource_mut::<AiState>();
            state.claude.army_counter = 20;
            state.claude.siege_counter = 0;
        }
        // Comfortably past `GRYPHON_BANK_GOLD` even after the reserve.
        app.world_mut()
            .resource_mut::<Economies>()
            .get_mut(Team::Claude)
            .gold = 3000;

        think_once(&mut app);

        assert_eq!(
            queued(&mut app, workshop),
            vec![UnitKind::GryphonRider],
            "Castle + Workshop + a fat bank is the whole air gate"
        );
    }

    /// ...and the gate is a gate. The same board with a thin treasury degrades
    /// to a Catapult rather than parking an unpayable Gryphon at the front of
    /// the queue — which is what the `affordable` fallback in the Workshop arm
    /// is for, and what makes the air branch surplus spending rather than a
    /// commitment.
    #[test]
    fn a_thin_bank_degrades_the_gryphon_back_to_a_catapult() {
        let mut app = ai_app();
        let home = Team::Claude.base_pos();
        spawn_building(&mut app, BuildingKind::Castle, Team::Claude, home);
        spawn_building(
            &mut app,
            BuildingKind::Barracks,
            Team::Claude,
            home + Vec3::new(-12.0, 0.0, 0.0),
        );
        let workshop = spawn_building(
            &mut app,
            BuildingKind::Workshop,
            Team::Claude,
            home + Vec3::new(0.0, 0.0, -12.0),
        );
        for i in 0..5 {
            spawn_unit(
                &mut app,
                UnitKind::Worker,
                Team::Claude,
                home + Vec3::new(3.0 + i as f32, 0.0, 3.0),
            );
        }
        // A hero already on the field. Without one the script ring-fences 400g
        // for the hero it wants, then buys it — and the Workshop is priced
        // against what is left, which would make this test about the hero
        // reserve rather than about the air gate.
        app.world_mut().spawn((
            Unit { kind: UnitKind::Hero },
            Team::Claude,
            Transform::from_translation(home),
            Order::Idle,
            Health::new(600.0),
            Hero { level: 1, xp: 0.0, mana: 80.0 },
        ));
        {
            let mut state = app.world_mut().resource_mut::<AiState>();
            state.claude.army_counter = 20;
            state.claude.siege_counter = 0;
        }
        // One gold under the gate. The script's reserves (a hero slot, a
        // research rung) come off the top before either unit is priced, so this
        // is a treasury that can pay for siege and cannot pay for air — which
        // is exactly the state the fallback exists for.
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let claude = economies.get_mut(Team::Claude);
            claude.gold = GRYPHON_BANK_GOLD - 1;
            claude.lumber = 900;
        }

        think_once(&mut app);

        assert_eq!(queued(&mut app, workshop), vec![UnitKind::Catapult]);
    }

    /// The probe knobs, which exist so a headless sim can reach the air branch
    /// at all. Unset, they must read their constants exactly — a knob that
    /// changes the default is a balance change wearing a debugging hat.
    #[test]
    fn the_gryphon_probe_knobs_default_to_the_shipped_constants() {
        // These are process-wide `OnceLock`s read from the environment, and the
        // test suite does not set them, so this is the shipped behaviour.
        assert_eq!(gryphon_bank_gold(), GRYPHON_BANK_GOLD);
        assert_eq!(gryphon_every_nth(), GRYPHON_EVERY_NTH);
        // Parsing is total: rubbish and empty strings fall back rather than
        // panicking mid-match, and the modulus can never reach zero.
        assert_eq!(env_u32("WC3_AI_NO_SUCH_VAR_HOPEFULLY", 42), 42);
        assert_eq!(env_u32(GRYPHON_NTH_ENV, GRYPHON_EVERY_NTH).max(1).max(1), GRYPHON_EVERY_NTH);
    }

    // -- Ford fortification (wc3clone-j0d) ---------------------------------

    fn ford(name: &'static str, pos: Vec3, width: f32) -> ChokePoint {
        ChokePoint { name, pos, width }
    }

    /// `open` publishes no chokepoints, and the script must then behave exactly
    /// as it did before this bead — base ring, no ford logic.
    #[test]
    fn no_chokepoints_means_no_ford_to_hold() {
        assert!(ford_hold_point(&[], &[HUMAN_BASE]).is_none());
        assert!(MapKind::Open.chokepoints().is_empty());
        assert!(ford_hold_point(&MapKind::Open.chokepoints(), &[HUMAN_BASE]).is_none());
    }

    /// A crossing on the far side of the map is not "ours" just because it is
    /// the nearest one. Walking a lone worker 99 units to plant one tower in
    /// the middle of the map is how a script donates 110 gold.
    #[test]
    fn a_ford_we_do_not_live_by_is_left_alone() {
        let far = ford("centre", Vec3::ZERO, 16.0);
        assert!(ford_hold_point(&[far], &[HUMAN_BASE]).is_none());
        // On the real map, from the real start position: nothing to hold yet.
        assert!(
            ford_hold_point(&MapKind::Crossings.chokepoints(), &[HUMAN_BASE]).is_none(),
            "the centre ford is ~99 from spawn; fortifying it is not a plan"
        );
    }

    /// ...and the moment the expansion lands ON a flank ford — which on
    /// `crossings` is the same decision, because the flank fords ARE the
    /// neutral mines — that ford becomes the one we hold.
    #[test]
    fn the_expansion_ford_is_the_one_we_fortify() {
        let chokes = MapKind::Crossings.chokepoints();
        // An expansion hall by the north-west neutral mine at (-60, 60).
        let expo = Vec3::new(-60.0, 0.0, 72.0);
        let hold = ford_hold_point(&chokes, &[HUMAN_BASE, expo]).expect("a ford to hold");
        let picked = chokes
            .iter()
            .find(|c| c.name == hold.name)
            .expect("named an actual chokepoint");
        assert!(
            xz_dist(picked.pos, expo) <= FORD_HOLD_RADIUS,
            "picked {} at {:?}, {:.0} from the expansion",
            hold.name,
            picked.pos,
            xz_dist(picked.pos, expo)
        );
        // The emplacement stands back from the gap, on OUR side of it.
        assert!((xz_dist(hold.hold, picked.pos) - FORD_STANDOFF).abs() < 0.01);
        assert!(
            xz_dist(hold.hold, expo) < xz_dist(picked.pos, expo),
            "the tower must sit behind the crossing, not in front of it"
        );
        // ...and still covers the gap: a Tower shoots 16.
        let range = building_stats(BuildingKind::Tower)
            .attack
            .expect("towers shoot")
            .range;
        assert!(xz_dist(hold.hold, picked.pos) < range);
    }

    /// Walls narrow a gap, and the gap is also the route our OWN army attacks
    /// through. The narrow centre ford gets a tower and nothing else.
    #[test]
    fn walls_never_plug_a_narrow_crossing() {
        let hall = Vec3::new(-12.0, 0.0, -12.0);
        let narrow = ford_hold_point(&[ford("centre", Vec3::ZERO, 16.0)], &[hall]).unwrap();
        assert!(narrow.width < FORD_WALL_MIN_WIDTH);
        assert!(ford_wall_sites(&narrow).is_empty());

        let wide = ford_hold_point(&[ford("flank", Vec3::ZERO, 30.0)], &[hall]).unwrap();
        let sites = ford_wall_sites(&wide);
        assert_eq!(sites.len(), FORD_WALLS);
        // Beside the tower along the barrier, symmetric, and well inside the
        // gap's half-width so the opening survives them.
        for s in &sites {
            assert!((xz_dist(*s, wide.hold) - FORD_WALL_OFFSETS[1].abs()).abs() < 0.01);
            let along_axis = (*s - wide.hold).normalize().dot(wide.along).abs();
            assert!(
                (along_axis - 1.0).abs() < 0.01,
                "walls line up with the barrier"
            );
        }
        let total: f32 = building_stats(BuildingKind::Wall).size * FORD_WALLS as f32
            + building_stats(BuildingKind::Tower).size;
        assert!(
            total < wide.width * 0.5,
            "the garrison may not eat half the crossing"
        );
    }

    /// An emplacement goes where it is needed, not "somewhere near the base".
    #[test]
    fn emplacements_are_placed_at_the_spot_not_near_it() {
        let nav = NavGrid::default();
        let target = Vec3::new(-30.0, 0.0, 20.0);
        assert_eq!(pick_spot(&nav, target, 5.0), Some(target));
        let mut blocked = NavGrid::default();
        blocked.set_blocked_rect(target, 6.0, true);
        let shifted = pick_spot(&blocked, target, 5.0).expect("a spot nearby");
        assert!(shifted != target);
        assert!(xz_dist(shifted, target) <= EMPLACE_RING_RADII[EMPLACE_RING_RADII.len() - 1] + 0.01);
    }

    // -- Shop and items (wc3clone-vsc) --------------------------------------

    const EMPTY: Inventory = Inventory([None, None]);
    fn held(a: Option<ItemId>, b: Option<ItemId>) -> Inventory {
        Inventory([a, b])
    }

    /// Rule 1, and the only rule that is not about money.
    #[test]
    fn a_held_potion_is_drunk_at_half_health() {
        let inv = held(Some(ItemId::HealingPotion), None);
        assert_eq!(
            item_plan(TechTier::T1, 0, 0.5, inv, false, false),
            Some(ItemAction::Use(0))
        );
        assert_eq!(
            item_plan(TechTier::T1, 0, 0.2, inv, false, false),
            Some(ItemAction::Use(0))
        );
        // A healthy hero holds on to it.
        assert!(!matches!(
            item_plan(TechTier::T1, 0, 0.9, inv, false, false),
            Some(ItemAction::Use(_))
        ));
    }

    /// Drinking outranks shopping: a dying hero does not go window-shopping
    /// with a potion in its pocket.
    #[test]
    fn using_beats_buying() {
        let inv = held(Some(ItemId::HealingPotion), None);
        assert_eq!(
            item_plan(TechTier::T3, 5000, 0.3, inv, true, true),
            Some(ItemAction::Use(0))
        );
    }

    #[test]
    fn the_banner_is_planted_in_a_fight_and_needs_tier_two() {
        let inv = held(Some(ItemId::BannerOfCommand), None);
        assert_eq!(
            item_plan(TechTier::T2, 0, 1.0, inv, true, false),
            Some(ItemAction::Use(0))
        );
        // No fight, no banner.
        assert!(!matches!(
            item_plan(TechTier::T2, 0, 1.0, inv, false, false),
            Some(ItemAction::Use(_))
        ));
        // Tier 1 buys the potion and then stops: nothing else on the shelf is
        // both unlocked and worth the script's gold.
        assert_eq!(
            item_plan(TechTier::T1, 5000, 1.0, EMPTY, false, false),
            Some(ItemAction::Buy(ItemId::HealingPotion)),
        );
        assert_eq!(
            item_plan(
                TechTier::T1,
                BANNER_RICH_GOLD,
                1.0,
                held(Some(ItemId::HealingPotion), None),
                false,
                false
            ),
            None,
            "the Banner is tier 2 — tier 1 must not reach for it"
        );
    }

    #[test]
    fn boots_are_drunk_when_the_wave_marches_out() {
        let inv = held(Some(ItemId::BootsOfSpeed), None);
        assert_eq!(
            item_plan(TechTier::T1, 0, 1.0, inv, false, true),
            Some(ItemAction::Use(0))
        );
        assert!(!matches!(
            item_plan(TechTier::T1, 0, 1.0, inv, false, false),
            Some(ItemAction::Use(_))
        ));
    }

    /// Restocking order is priority order, and every rung has a bank gate: a
    /// Shop must never be the reason a Barracks went quiet.
    #[test]
    fn restocking_is_cheapest_need_first_and_never_broke() {
        // Poor: nothing at all, even with an empty inventory and a Shop up.
        assert_eq!(item_plan(TechTier::T3, 50, 1.0, EMPTY, false, false), None);
        // The potion is the first thing the script ever buys.
        assert_eq!(
            item_plan(TechTier::T3, POTION_RICH_GOLD, 1.0, EMPTY, false, false),
            Some(ItemAction::Buy(ItemId::HealingPotion))
        );
        let stocked = held(Some(ItemId::HealingPotion), None);
        // Then the Banner, once tier 2 and the bank can stand it...
        assert_eq!(
            item_plan(TechTier::T2, BANNER_RICH_GOLD, 1.0, stocked, false, false),
            Some(ItemAction::Buy(ItemId::BannerOfCommand))
        );
        // ...and Boots only when genuinely idle-rich.
        assert_eq!(
            item_plan(TechTier::T1, BOOTS_RICH_GOLD, 1.0, stocked, false, false),
            Some(ItemAction::Buy(ItemId::BootsOfSpeed))
        );
        assert!(BOOTS_RICH_GOLD > item_def(ItemId::BootsOfSpeed).cost_gold);
        assert!(POTION_RICH_GOLD > item_def(ItemId::HealingPotion).cost_gold);
    }

    /// Two slots is the whole inventory; a full hero stops shopping.
    #[test]
    fn a_full_inventory_buys_nothing() {
        let full = held(Some(ItemId::HealingPotion), Some(ItemId::BootsOfSpeed));
        assert_eq!(item_plan(TechTier::T3, 9999, 1.0, full, false, false), None);
        // ...but still uses what it holds.
        assert_eq!(
            item_plan(TechTier::T3, 9999, 0.4, full, false, false),
            Some(ItemAction::Use(0))
        );
    }

    /// The script never buys the two map-control items. Both are commander
    /// tools — a Town Portal is a decision about when to leave a fight, and the
    /// Scroll is a decision about where the whole army should be. A dumb
    /// baseline that owned them would be spending them at random.
    #[test]
    fn the_script_leaves_the_commander_items_on_the_shelf() {
        for gold in [200, 500, 1000, 5000] {
            for tier in [TechTier::T1, TechTier::T2, TechTier::T3] {
                let plan = item_plan(tier, gold, 1.0, EMPTY, false, false);
                assert!(!matches!(
                    plan,
                    Some(ItemAction::Buy(ItemId::TownPortal))
                        | Some(ItemAction::Buy(ItemId::ScrollOfMassTeleport))
                ));
            }
        }
    }

    // -- Chain of Command, third seat (docs/TEMPO.md §3) -------------------

    /// A world with just enough around it for one scripted think tick, and a
    /// Claude team whose only command node is its own hall.
    fn ai_world(latency_on: bool) -> App {
        let mut app = App::new();
        // Races: `CorePlugin` supplies this in a real match; a hand-built
        // test app must too, or any system reading it panics inside Bevy's
        // worker pool and the test HANGS rather than fails.
        app.init_resource::<Races>();
        app.init_resource::<Time>()
            .init_resource::<AiState>()
            .init_resource::<GameOver>()
            .insert_resource(AiControlled { human: false, claude: true })
            .init_resource::<Economies>()
            .init_resource::<HeroRecords>()
            .init_resource::<NavGrid>()
            .init_resource::<FogGrids>()
            .init_resource::<TeamResearch>()
            .init_resource::<TechTiers>()
            .init_resource::<SquadOrders>()
            .init_resource::<GameEvents>()
            .add_event::<CastAbility>()
            .add_event::<UpgradeBuilding>()
            .add_event::<StartResearch>()
            .add_event::<BuyItem>()
            .add_event::<UseItem>()
            .insert_resource(CommandLatency { on: latency_on, ..Default::default() })
            .insert_resource(CommandNodes {
                nodes: vec![(Team::Claude, Team::Claude.base_pos(), DEFAULT_HALL_RADIUS)],
                ready: true,
            })
            .add_plugins(crate::intent::IntentPlugin)
            .add_systems(Update, ai_think.before(crate::intent::IntentApply));
        app.insert_resource(crate::intent::IntentLog::disabled());
        app
    }

    /// One lone soldier, standing in the enemy's half of the map — as far from
    /// its own chain of command as the map allows.
    fn spawn_far_soldier(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Footman },
                Team::Claude,
                Transform::from_translation(Team::Human.base_pos()),
                Order::Idle,
                // `UnitQuery` requires it — a unit with no `Health` is
                // invisible to the scripted commander entirely.
                Health::new(100.0),
            ))
            .id()
    }

    fn think_once(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(THINK_INTERVAL + 0.05));
        app.update();
    }

    /// **All three seats pay.** docs/TEMPO.md §3 requires it in as many words —
    /// "the scripted AI pays latency too, or autopilot becomes a cheat and C1
    /// is violated at the third seat". This used to be the test that kept an
    /// exception honest, because `ai.rs` reached for `OrderIssuer` by hand and
    /// nothing but this assertion stopped somebody deleting the call.
    ///
    /// It now tests something better: the script speaks through the compiler
    /// like everyone else, so an order it gives a unit on the far side of the
    /// map is held in transit *because there is no other path it could have
    /// taken*. `WC3_COMMAND_LATENCY=1` reaches the third seat through the same
    /// `ground_order` arm that prices the first two.
    #[test]
    fn the_scripted_ai_pays_latency_like_everybody_else() {
        let mut app = ai_world(true);
        let soldier = spawn_far_soldier(&mut app);

        think_once(&mut app);

        let pending = app
            .world()
            .entity(soldier)
            .get::<PendingOrder>()
            .unwrap_or_else(|| {
                panic!("the scripted AI's order landed instantly — autopilot is cheating")
            });
        assert!(
            pending.link() > 0.0,
            "the AI was charged a zero link from the wrong side of the map"
        );
        // And, like anyone else's, the order it was already carrying is
        // undisturbed until the new one arrives.
        assert!(
            matches!(app.world().entity(soldier).get::<Order>(), Some(Order::Idle)),
            "an in-transit order must not change what the unit is doing yet"
        );
    }

    /// **The script does not ride the trigger exemption.**
    ///
    /// `wc3clone-pec` gave `apply_intents` a second issuer: anything carrying
    /// `SubmitIntent::trigger` gets `CommandLink::exempt_issuer` and pays
    /// nothing, because a rule's author paid the reach when they armed it.
    /// That is a correct rule and a live hazard for this seat — a scripted
    /// think tick submitting with `trigger: Some(..)` would be autopilot
    /// commanding at zero latency, which is docs/TEMPO.md §3's cheat by
    /// another door.
    ///
    /// Pinned at the field rather than at the timing, because the field is
    /// what decides it and a timing assertion would pass for the wrong reason
    /// on a unit that happened to be standing on a command node.
    #[test]
    fn the_script_never_claims_the_trigger_link_exemption() {
        let spoken = SubmitIntent::script(Team::Claude, Intent::Return { units: vec![] });
        assert!(
            spoken.trigger.is_none(),
            "a scripted decision is a commander deciding now, not a rule firing — \
             it pays the link like the other two seats"
        );
        assert_eq!(spoken.source, IntentSource::Script);
    }

    /// The off-flag identity at the third seat: with `WC3_COMMAND_LATENCY`
    /// unset the scripted AI writes `Order`s exactly where it always did, and
    /// no `PendingOrder` can exist anywhere.
    #[test]
    fn the_scripted_ai_is_unchanged_with_the_flag_off() {
        let mut app = ai_world(false);
        let soldier = spawn_far_soldier(&mut app);

        think_once(&mut app);

        assert!(
            app.world().entity(soldier).get::<PendingOrder>().is_none(),
            "the feature is off; nothing may be in transit"
        );
        assert!(
            matches!(
                app.world().entity(soldier).get::<Order>(),
                Some(Order::AttackMove(_))
            ),
            "with latency off the script's order must land in the same frame it always did"
        );
    }

    // -- The third seat (wc3clone-jem) -------------------------------------

    /// **The whole bead in one assertion.** A thought becomes a `SubmitIntent`
    /// with `IntentSource::Script`, the compiler validates and applies it in
    /// the same frame, the unit it moved answers `order:attackmove by script`,
    /// and the sentence is in the journal that feeds the replay — all of it
    /// through the code path `ui` and `bridge` use, with nothing in it that
    /// knows the script is special.
    ///
    /// The provenance string is the load-bearing part. It is the join key
    /// between the intent log and a snapshot's `units[].why`, so a replay of a
    /// scripted match can now be read the same way a replay of a played one
    /// is: every order in it names an author, and there are three of them.
    #[test]
    fn a_script_order_flows_through_the_compiler_and_says_so() {
        let mut app = ai_world(false);
        let soldier = spawn_far_soldier(&mut app);

        think_once(&mut app);

        let why = app
            .world()
            .entity(soldier)
            .get::<Provenance>()
            .expect("an order the compiler applied carries the reason it applied it")
            .why();
        assert!(
            why.starts_with("order:attackmove by script t="),
            "a scripted order must read like every other seat's: got {why:?}"
        );

        // ...and the same sentence is in the record both seats and the replay
        // read, attributed to the third seat rather than to nobody.
        let journal = app.world().resource::<IntentJournal>();
        let spoken = journal.get(Team::Claude);
        assert!(
            spoken
                .iter()
                .any(|e| e.source == IntentSource::Script && e.verb == "attackmove" && e.ok),
            "the script's sentence never reached the journal: {:?}",
            spoken.iter().map(|e| (e.source, e.verb)).collect::<Vec<_>>()
        );
    }

    /// **A refusal is the script's problem and nobody else's.**
    ///
    /// The compiler may now say no to the scripted commander — a dead target,
    /// a site that filled up, a price that moved. Two things have to be true
    /// when it does. The refusal must be recorded (the journal entry says
    /// `ok: false`, and the intent log carries the string), and it must NOT be
    /// posted to the team's error channel: bridge.rs ships that list to
    /// whichever seat is reading, and a commander handed failures it did not
    /// cause would be debugging the autopilot instead of playing.
    #[test]
    fn a_refused_script_intent_stays_out_of_the_seats_error_channel() {
        let mut app = ai_world(false);
        // An id nothing has ever had: the shape of every rejection the script
        // can actually hit — it named something the world no longer agrees is
        // there.
        app.world_mut().send_event(SubmitIntent::script(
            Team::Claude,
            Intent::Train {
                building: 999_999_999,
                unit: "Footman".to_string(),
            },
        ));
        app.update();

        assert!(
            app.world().resource::<IntentErrors>().get(Team::Claude).is_empty(),
            "a script rejection must not be posted to a seat's error channel"
        );
        let journal = app.world().resource::<IntentJournal>();
        assert!(
            journal
                .get(Team::Claude)
                .iter()
                .any(|e| e.source == IntentSource::Script && e.verb == "train" && !e.ok),
            "the refusal still has to be on the record"
        );
    }

    /// **No wedge.** The one piece of optimistic bookkeeping a rejection could
    /// strand is the build slot: `pending_build` is claimed at the moment the
    /// intent is spoken, and if the compiler refuses the placement the worker
    /// never picks up an `Order::Build` to clear it with.
    ///
    /// It clears itself from the world instead of from the assumption — the
    /// slot is released the moment no worker is actually building — and it
    /// releases the expansion ring-fence with it, so a refused hall cannot
    /// quietly halt army production for the rest of the match.
    #[test]
    fn a_build_that_never_landed_releases_the_slot_and_the_ring_fence() {
        let mut app = ai_world(false);
        let idler = app
            .world_mut()
            .spawn((
                Unit { kind: UnitKind::Worker },
                Team::Claude,
                Transform::from_translation(Team::Claude.base_pos()),
                Order::Idle,
                Health::new(100.0),
            ))
            .id();
        {
            let mut state = app.world_mut().resource_mut::<AiState>();
            state.claude.pending_build = Some(idler);
            state.claude.expansion_pending = true;
        }

        think_once(&mut app);

        let state = app.world().resource::<AiState>();
        assert_eq!(
            state.claude.pending_build, None,
            "a build that never became an Order must not hold the slot forever"
        );
        assert!(
            !state.claude.expansion_pending,
            "nor keep the expansion down payment ring-fenced behind it"
        );
    }
}
