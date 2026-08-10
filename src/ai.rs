//! ai.rs — the scripted RTS brain. Always drives the Claude faction (red, NE
//! base) and optionally the Human one too (blue, SW base) for AI-vs-AI
//! spectating: launch with `WC3_AI_BOTH=1`, or press F9 at runtime.
//!
//! A macro-focused RTS AI that plays strictly through the same primitives the
//! human UI uses: it writes `Order` components on its own units, pushes
//! `UnitKind`s onto its own buildings' `TrainingQueue`, and issues
//! `Order::Build` on its workers. It never spawns anything, never touches
//! `Health`, and only *reads* `Economies` (economy.rs does all the paying).
//! Because of that, sharing a team with the player is harmless: the AI simply
//! reassigns whatever it finds idle on its next tick.
//!
//! Everything runs from one `ai_think` system on a ~1s timer, which runs the
//! same `think` body once per AI-controlled team against that team's own
//! `AiBrain`. Nothing is positional: every base/target lookup is derived from
//! the team being thought for. All difficulty knobs live in the const block
//! below.

use crate::shared::*;
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

/// Siege. A Workshop is a luxury: only once a Barracks stands and the treasury
/// is comfortably ahead of army production does the AI branch into siege.
// 350, not 500: sim runs showed peak treasury in short games (~320) never
// clears a 500 gate, so siege only appeared in long games — the opposite of
// its purpose.
const WORKSHOP_GOLD: u32 = 350;
const WORKSHOP_QUEUE_MAX: usize = 2;
/// Target mix: one Catapult per this many Barracks-produced line units.
const CATAPULT_PER_ARMY: u32 = 4;

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
/// ...and this much gold still in the bank AFTER paying for it. Tuned down
/// from 400 against sims: games converge in 8-12 minutes and the mines run dry
/// before that, so a stricter test meant tier 3 simply never happened.
const CASTLE_SPARE_GOLD: u32 = 300;
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
/// Slack added to `HERO_ABILITY_RADIUS` when counting slam targets.
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
            .add_systems(Update, (ai_toggle_hotkey, ai_think.after(FogSet)));
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

struct BuildingInfo {
    entity: Entity,
    kind: BuildingKind,
    pos: Vec3,
    done: bool,
    queue_len: usize,
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
        Option<&'static mut TrainingQueue>,
        Option<&'static Upgrading>,
        Option<&'static Researching>,
    ),
>;

type NodeQuery<'w, 's> = Query<'w, 's, (Entity, &'static ResourceNode, &'static Transform)>;

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
    mut commands: Commands,
    mut casts: EventWriter<CastAbility>,
    mut upgrades: EventWriter<UpgradeBuilding>,
    mut start_research: EventWriter<StartResearch>,
    team_research: Res<TeamResearch>,
    units: UnitQuery,
    mut buildings: BuildingQuery,
    nodes: NodeQuery,
) {
    if game_over.0.is_some() {
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
        think(
            team,
            brain,
            now,
            &economies,
            &records,
            &nav,
            fog.get(team),
            &mut commands,
            &mut casts,
            &mut upgrades,
            &mut start_research,
            &team_research,
            &units,
            &mut buildings,
            &nodes,
        );
    }
}

/// One team's thought. Everything positional is derived from `me`.
#[allow(clippy::too_many_arguments)]
fn think(
    me: Team,
    brain: &mut AiBrain,
    now: f32,
    economies: &Economies,
    records: &HeroRecords,
    nav: &NavGrid,
    // This team's fog. Every enemy fact below is drawn through it, so the
    // scripted commander plans from the same picture a bridge commander is
    // sent and the same one the player is shown.
    fog: &FogGrid,
    commands: &mut Commands,
    casts: &mut EventWriter<CastAbility>,
    upgrades: &mut EventWriter<UpgradeBuilding>,
    start_research: &mut EventWriter<StartResearch>,
    team_research: &TeamResearch,
    units: &UnitQuery,
    buildings: &mut BuildingQuery,
    nodes: &NodeQuery,
) {
    // --- snapshot the world (read-only) --------------------------------------
    let mut workers: Vec<UnitInfo> = Vec::new();
    let mut army: Vec<UnitInfo> = Vec::new();
    // Enemies that can shoot at a worker (what makes a mine unsafe).
    let mut enemy_combat: Vec<Vec3> = Vec::new();
    // Every enemy, air included — a flyer over the base is still an incursion.
    let mut enemy_any: Vec<Vec3> = Vec::new();
    // Enemies standing ON the ground: the only ones a Slam can touch.
    let mut enemy_ground: Vec<Vec3> = Vec::new();
    // Own living heroes: (entity, position, ability ready).
    let mut own_heroes: Vec<(Entity, Vec3, bool)> = Vec::new();

    for (entity, unit, team, tf, order, move_to, carrying, hero, cooldowns) in units.iter() {
        let info = UnitInfo {
            entity,
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
                own_heroes.push((entity, info.pos, ready));
            }
            // Everything that isn't a Worker is army: heroes, Archers,
            // Catapults and Raiders all join waves with no extra wiring.
            match unit.kind {
                UnitKind::Worker => workers.push(info),
                _ => army.push(info),
            }
        } else if fog.sees(info.pos) {
            // Enemy units enter the plan only while somebody of ours is
            // looking at them, and are never remembered afterwards. This is
            // the single line that ends the omniscient-commander asymmetry:
            // everything below — defence, worker flight, Slam timing — was
            // reading the ECS directly and now reads what this team can see.
            enemy_any.push(info.pos);
            if !is_flying_kind(unit.kind) {
                enemy_ground.push(info.pos);
            }
            // Workers don't hunt, and neither does anything that cannot shoot
            // downward — so neither should make a harvest crew run.
            if unit.kind != UnitKind::Worker && unit_stats(unit.kind).can_hit_ground {
                enemy_combat.push(info.pos);
            }
        }
    }

    let mut own_buildings: Vec<BuildingInfo> = Vec::new();
    let mut enemy_buildings: Vec<Vec3> = Vec::new();
    let mut queued_supply: u32 = 0;
    let mut hero_queued = false;
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
        if let Some(q) = queue.as_ref() {
            queued_supply += q.queue.iter().map(|k| unit_stats(*k).supply).sum::<u32>();
            // Either hero class occupies the team's single hero slot.
            hero_queued |= q.queue.iter().any(|k| is_hero_kind(*k));
        }
        own_buildings.push(BuildingInfo {
            entity,
            kind: building.kind,
            pos: tf.translation,
            done: under.is_none(),
            queue_len,
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
    // Free supply, pessimistically counting units already in production.
    let mut headroom = eco
        .supply_cap
        .saturating_sub(eco.supply_used + queued_supply);
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
            commands.entity(w.entity).try_insert(Order::Move(safe));
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

        // A second mining base, planned before it is affordable so the site is
        // already vetted (unclaimed, undefended-by-them, buildable) when the
        // money lands. `None` while a TownHall is already going up: one
        // expansion at a time, like every other line in this build order.
        let expansion = if halls_total > 0 && !hall_going_up {
            plan_expansion(
                me,
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

        let want = if halls_total == 0 {
            Some(BuildingKind::TownHall)
        } else if headroom < SUPPLY_BUFFER && !under_construction(BuildingKind::Farm) {
            Some(BuildingKind::Farm)
        } else if count_of(BuildingKind::Barracks) == 0 {
            Some(BuildingKind::Barracks)
        } else if expansion.is_some() {
            // Above the luxuries (second Barracks, Workshop) and below the
            // army's first Barracks: income outlives any one more Footman, but
            // a base with no defenders never gets to spend it.
            Some(BuildingKind::TownHall)
        } else if gold > SECOND_BARRACKS_GOLD && count_of(BuildingKind::Barracks) < MAX_BARRACKS {
            Some(BuildingKind::Barracks)
        } else if done_count(BuildingKind::Barracks) >= 1
            && count_of(BuildingKind::Workshop) == 0
            && gold > WORKSHOP_GOLD
        {
            // Siege branch. `count_of` (not `done_count`) means one Workshop
            // ever — including one still under construction — so the AI can't
            // spam a second while the first is going up.
            Some(BuildingKind::Workshop)
        } else if done_count(BuildingKind::Blacksmith) == 0
            && count_of(BuildingKind::Blacksmith) == 0
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
            Some(BuildingKind::Blacksmith)
        } else {
            None
        };

        if let Some(kind) = want {
            let stats = building_stats(kind);
            saving_for_expansion = expansion.is_some()
                && kind == BuildingKind::TownHall
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
                let expanding = kind == BuildingKind::TownHall && expansion.is_some();
                let site = match &expansion {
                    Some(plan) if expanding => Some(plan.site),
                    _ => pick_site(nav, anchor, stats.size + BUILD_PADDING),
                };
                if let Some(site) = site {
                    if let Some(builder) = pick_builder(&workers, &fleeing, site) {
                        commands
                            .entity(builder)
                            .try_insert(Order::Build { kind, pos: site });
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
                        if let (true, Some(plan)) = (expanding, &expansion) {
                            brain.expansion_pending = true;
                            info!(
                                "[ai {me:?}] expanding: TownHall at ({:.0},{:.0}) for the mine at \
                                 ({:.0},{:.0}) holding {} gold — held mines are down to {}",
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
        .any(|b| b.kind == BuildingKind::Barracks && b.done);
    let mut tierup_reserve = (0u32, 0u32);
    brain.tierup_pending = false;
    if !hall_upgrading && !saving_for_expansion && !brain.expansion_pending {
        if let Some(hall) = main_hall {
            if let Some((cost_gold, cost_lumber, _)) = upgrade_cost(hall.kind) {
                let tier = building_tier(hall.kind);
                let wanted = match tier {
                    // Keep: as soon as the opening is genuinely over.
                    1 => barracks_up && army.len() >= KEEP_MIN_ARMY,
                    // Castle: a late luxury, and only out of surplus — the
                    // gold test is against what is left AFTER the price, so a
                    // Castle never comes out of the army budget.
                    _ => {
                        army.len() >= CASTLE_MIN_ARMY
                            && gold.saturating_sub(cost_gold) >= CASTLE_SPARE_GOLD
                    }
                };
                if wanted {
                    if gold >= cost_gold && lumber >= cost_lumber {
                        gold -= cost_gold;
                        lumber -= cost_lumber;
                        upgrades.write(UpgradeBuilding {
                            building: hall.entity,
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
                    } else {
                        // Ring-fence: hold the price out of the army's reach
                        // until the deliveries add up, or the Barracks will
                        // keep the treasury a Footman short of it forever.
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
                    start_research.write(StartResearch {
                        building: forge.entity,
                        kind,
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
            commands.entity(w.entity).try_insert(Order::ReturnResources);
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
            commands.entity(w.entity).try_insert(Order::Harvest(node));
        }
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
    rebalance_mines(&mines, &workers, &shift_skip, nodes, commands);

    // --- training ------------------------------------------------------------
    let mut worker_count = workers.len();
    let mut orders: Vec<(Entity, UnitKind)> = Vec::new();

    // One Champion per team: train the first once a Barracks is up, and revive
    // a fallen one as soon as the (cheaper) price is affordable.
    let mut want_hero = own_heroes.is_empty()
        && !hero_queued
        && (records.get(me).is_some()
            || own_buildings.iter().any(|b| b.kind == BuildingKind::Barracks));
    // The script plays the Champion, but a team's class is locked by whichever
    // hero it fielded first (a commander sharing this team could have picked
    // the Priestess). Queuing the locked class keeps economy.rs from dropping
    // the item unpaid at the front of the queue.
    let hero_kind = records.get(me).map_or(UnitKind::Hero, |rec| rec.kind);
    let hero_supply = unit_stats(hero_kind).supply;

    if want_hero {
        let (hero_gold, hero_lumber, _) = hero_train_cost(records, me);
        // Hero training and revival happen at any finished rung of the hall
        // ladder — a team that teched to Keep must not lose its hero.
        let hall = own_buildings.iter().find(|b| b.done && is_hall(b.kind));
        if let Some(hall) = hall {
            if gold >= hero_gold && lumber >= hero_lumber && headroom >= hero_supply {
                gold -= hero_gold;
                lumber -= hero_lumber;
                headroom -= hero_supply;
                want_hero = false;
                orders.push((hall.entity, hero_kind));
            }
        }
    }

    // Still saving up? Ring-fence the Champion's price so continuous army
    // production doesn't keep the treasury permanently just below it. Supply is
    // deliberately NOT reserved: army units are what drives the farm trigger,
    // and holding 5 supply back would stall the whole build order.
    let (mut reserve_gold, mut reserve_lumber) = if want_hero {
        let (g, l, _) = hero_train_cost(records, me);
        (g, l)
    } else {
        (0, 0)
    };
    // Same ring-fence for the expansion down payment, held both while saving
    // up and for the whole walk out to the site. Without it the Barracks
    // drains every delivery and a 385g/205l TownHall is never reached — the AI
    // would "want" to expand forever while its last mine ran out.
    if saving_for_expansion || brain.expansion_pending {
        let stats = building_stats(BuildingKind::TownHall);
        reserve_gold += stats.cost_gold;
        reserve_lumber += stats.cost_lumber;
    }
    // ...and for a tier-up we have decided on but cannot yet pay for.
    reserve_gold += tierup_reserve.0;
    reserve_lumber += tierup_reserve.1;
    // ...and for a research rung we have decided on but cannot yet pay for.
    reserve_gold += research_reserve.0;
    reserve_lumber += research_reserve.1;

    for b in &own_buildings {
        if !b.done {
            continue;
        }
        // A Keep and a Castle are the hall, so worker production keys off the
        // ladder rather than the tier-1 kind — teching up must not stop the
        // economy that paid for it.
        if is_hall(b.kind) {
            if worker_count >= TARGET_WORKERS || b.queue_len > 0 {
                continue;
            }
            let s = unit_stats(UnitKind::Worker);
            if gold >= s.cost_gold && lumber >= s.cost_lumber && headroom >= s.supply {
                gold -= s.cost_gold;
                lumber -= s.cost_lumber;
                headroom -= s.supply;
                worker_count += 1;
                orders.push((b.entity, UnitKind::Worker));
            }
            continue;
        }
        match b.kind {
            BuildingKind::Barracks => {
                if b.queue_len >= BARRACKS_QUEUE_MAX {
                    continue;
                }
                // Only advance the mix counter when something is actually
                // queued, and fall back to a Footman when the pricier pick
                // (Archer's lumber, Raider's gold) is out of reach.
                let next = brain.army_counter.wrapping_add(1);
                // Raiders are Workshop-gated: queueing one early would park an
                // unpayable item at the front and stall the whole Barracks.
                let raider_ok = own_buildings
                    .iter()
                    .any(|ob| ob.kind == BuildingKind::Workshop && ob.done);
                let wanted = if raider_ok && next % RAIDER_EVERY_NTH == 0 {
                    UnitKind::Raider
                } else if next % ARCHER_EVERY_NTH == 0 {
                    UnitKind::Archer
                } else if next % SPEARMAN_EVERY_NTH == 0 {
                    UnitKind::Spearman
                } else {
                    UnitKind::Footman
                };
                let affordable = |k: UnitKind| {
                    let s = unit_stats(k);
                    gold.saturating_sub(reserve_gold) >= s.cost_gold
                        && lumber.saturating_sub(reserve_lumber) >= s.cost_lumber
                        && headroom >= s.supply
                };
                let kind = if affordable(wanted) {
                    Some(wanted)
                } else if wanted != UnitKind::Footman && affordable(UnitKind::Footman) {
                    Some(UnitKind::Footman)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    let s = unit_stats(kind);
                    gold -= s.cost_gold;
                    lumber -= s.cost_lumber;
                    headroom -= s.supply;
                    brain.army_counter = next;
                    orders.push((b.entity, kind));
                }
            }
            BuildingKind::Workshop => {
                if b.queue_len >= WORKSHOP_QUEUE_MAX {
                    continue;
                }
                // Pace siege against line units: skip until the ratio says a
                // catapult is due. The counter only advances on an actual
                // enqueue, so a broke Workshop doesn't bank up credit.
                if brain.siege_counter * CATAPULT_PER_ARMY > brain.army_counter {
                    continue;
                }
                let s = unit_stats(UnitKind::Catapult);
                if gold.saturating_sub(reserve_gold) >= s.cost_gold
                    && lumber.saturating_sub(reserve_lumber) >= s.cost_lumber
                    && headroom >= s.supply
                {
                    gold -= s.cost_gold;
                    lumber -= s.cost_lumber;
                    headroom -= s.supply;
                    brain.siege_counter = brain.siege_counter.wrapping_add(1);
                    orders.push((b.entity, UnitKind::Catapult));
                }
            }
            // Non-producing buildings (and any future kinds the scripted AI
            // doesn't know how to use) train nothing. Deliberately included:
            // the Shop — items, like Call to Arms militia, are commander
            // tools; the script never builds one and never shops.
            _ => {}
        }
    }

    for (entity, kind) in orders {
        if let Ok((_, _, _, _, _, Some(mut queue), _, _)) = buildings.get_mut(entity) {
            queue.queue.push_back(kind);
        }
    }

    // --- military ------------------------------------------------------------
    // Slam whenever a worthwhile clump is standing on the Champion. Same event
    // the player's R hotkey sends; combat.rs validates mana and cooldown.
    let slam_radius = HERO_ABILITY_RADIUS + SLAM_RADIUS_SLACK;
    for (hero, pos, ready) in &own_heroes {
        if !ready {
            continue;
        }
        // Ground enemies only: the Slam is a ground shockwave, so a clump of
        // flyers overhead must not talk the Champion into spending his mana on
        // an empty patch of dirt.
        let nearby = enemy_ground
            .iter()
            .filter(|e| e.distance(*pos) <= slam_radius)
            .count();
        if nearby >= SLAM_MIN_TARGETS {
            casts.write(CastAbility::new(*hero));
        }
    }

    let rally = base + (-base.normalize_or_zero()) * RALLY_DIST;

    if let Some(threat_pos) = threat {
        // Defense overrides everything, wave or not.
        for u in &army {
            commands
                .entity(u.entity)
                .try_insert(Order::AttackMove(threat_pos));
        }
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
            for u in &army {
                commands
                    .entity(u.entity)
                    .try_insert(Order::AttackMove(brain.wave_target));
            }
        } else {
            // Stragglers rejoin the push.
            let target = brain.wave_target;
            for u in &army {
                if u.free() {
                    commands.entity(u.entity).try_insert(Order::AttackMove(target));
                }
            }
        }
    } else if army.len() >= brain.next_wave_size {
        brain.wave_active = true;
        brain.wave_started = now;
        brain.wave_target = wave_objective(me, fog, nav, &enemy_buildings, base);
        brain.next_wave_size = (brain.next_wave_size + WAVE_SIZE_STEP).min(WAVE_SIZE_CAP);
        let target = brain.wave_target;
        for u in &army {
            commands.entity(u.entity).try_insert(Order::AttackMove(target));
        }
    } else {
        // Gather at the rally point while the army builds up.
        for u in &army {
            if u.free() && u.pos.distance(rally) > RALLY_ARRIVE_DIST {
                commands.entity(u.entity).try_insert(Order::AttackMove(rally));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Ground-plane projection — mines and buildings sit at y=0, units do not.
fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
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

    let footprint = building_stats(BuildingKind::TownHall).size + BUILD_PADDING;
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
    commands: &mut Commands,
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
    for (worker, _) in pool.into_iter().take(quota) {
        commands
            .entity(worker)
            .try_insert(Order::Harvest(target.entity));
    }
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
