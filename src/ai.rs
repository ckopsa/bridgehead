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
            .add_systems(Update, (ai_toggle_hotkey, ai_think));
    }
}

/// Everything one team's brain remembers between thoughts. Two of these live
/// side by side; a brain never reads or writes the other team's copy.
struct AiBrain {
    /// Worker we last handed an `Order::Build` to (one build in flight).
    pending_build: Option<Entity>,
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
    mut commands: Commands,
    mut casts: EventWriter<CastAbility>,
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
            &mut commands,
            &mut casts,
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
    commands: &mut Commands,
    casts: &mut EventWriter<CastAbility>,
    units: &UnitQuery,
    buildings: &mut BuildingQuery,
    nodes: &NodeQuery,
) {
    // --- snapshot the world (read-only) --------------------------------------
    let mut workers: Vec<UnitInfo> = Vec::new();
    let mut army: Vec<UnitInfo> = Vec::new();
    let mut enemy_combat: Vec<Vec3> = Vec::new();
    let mut enemy_any: Vec<Vec3> = Vec::new();
    // Own living heroes: (entity, position, ability ready).
    let mut own_heroes: Vec<(Entity, Vec3, bool)> = Vec::new();

    for (entity, unit, team, tf, order, move_to, carrying, hero) in units.iter() {
        let info = UnitInfo {
            entity,
            pos: tf.translation,
            tag: order.map(tag_of).unwrap_or(Tag::Idle),
            moving: move_to.is_some(),
            carrying: carrying.is_some(),
        };
        if *team == me {
            if let Some(hero) = hero {
                own_heroes.push((entity, info.pos, hero.ability_ready()));
            }
            // Everything that isn't a Worker is army: heroes, Archers,
            // Catapults and Raiders all join waves with no extra wiring.
            match unit.kind {
                UnitKind::Worker => workers.push(info),
                _ => army.push(info),
            }
        } else {
            enemy_any.push(info.pos);
            if unit.kind != UnitKind::Worker {
                enemy_combat.push(info.pos);
            }
        }
    }

    let mut own_buildings: Vec<BuildingInfo> = Vec::new();
    let mut enemy_buildings: Vec<Vec3> = Vec::new();
    let mut queued_supply: u32 = 0;
    let mut hero_queued = false;
    for (entity, building, team, tf, under, queue) in buildings.iter() {
        if *team != me {
            enemy_buildings.push(tf.translation);
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
        });
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

        let want = if count_of(BuildingKind::TownHall) == 0 {
            Some(BuildingKind::TownHall)
        } else if headroom < SUPPLY_BUFFER && !under_construction(BuildingKind::Farm) {
            Some(BuildingKind::Farm)
        } else if count_of(BuildingKind::Barracks) == 0 {
            Some(BuildingKind::Barracks)
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
        } else {
            None
        };

        if let Some(kind) = want {
            let stats = building_stats(kind);
            if eco.can_afford(stats.cost_gold, stats.cost_lumber) {
                // Anchor on the town hall, or any surviving building if the
                // main base area has been razed.
                let anchor = own_buildings
                    .iter()
                    .find(|b| b.kind == BuildingKind::TownHall)
                    .or_else(|| own_buildings.first())
                    .map(|b| b.pos)
                    .unwrap_or(base);
                if let Some(site) = pick_site(nav, anchor, stats.size + BUILD_PADDING) {
                    if let Some(builder) = pick_builder(&workers, &fleeing, site) {
                        commands
                            .entity(builder)
                            .try_insert(Order::Build { kind, pos: site });
                        brain.pending_build = Some(builder);
                        busy_worker = Some(builder);
                        // economy.rs pays at placement; assume it lands.
                        gold = gold.saturating_sub(stats.cost_gold);
                        lumber = lumber.saturating_sub(stats.cost_lumber);
                    }
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
        let hall = own_buildings
            .iter()
            .find(|b| b.done && b.kind == BuildingKind::TownHall);
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
    let (reserve_gold, reserve_lumber) = if want_hero {
        let (g, l, _) = hero_train_cost(records, me);
        (g, l)
    } else {
        (0, 0)
    };

    for b in &own_buildings {
        if !b.done {
            continue;
        }
        match b.kind {
            BuildingKind::TownHall => {
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
            }
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
        if let Ok((_, _, _, _, _, Some(mut queue))) = buildings.get_mut(entity) {
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
        let nearby = enemy_any
            .iter()
            .filter(|e| e.distance(*pos) <= slam_radius)
            .count();
        if nearby >= SLAM_MIN_TARGETS {
            casts.write(CastAbility { caster: *hero });
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
            brain.wave_target =
                nearest_pos(&enemy_buildings, centroid).unwrap_or(me.enemy().base_pos());
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
        brain.wave_target = nearest_pos(&enemy_buildings, base).unwrap_or(me.enemy().base_pos());
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
