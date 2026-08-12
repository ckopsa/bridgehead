//! Economy module: buildings, construction, the harvest loop and training queues.
//!
//! Owns:
//!   * `SpawnBuildingEvent` -> procedural, team-tinted building entities + nav blocking
//!   * `UnderConstruction` progress (visual growth + additive health)
//!   * `Order::Build` follow-through for workers (walk, pay, place)
//!   * `Order::Harvest` / `Order::ReturnResources` gather loop
//!   * `TrainingQueue` processing (pays when an item becomes the front item)
//!   * `StartResearch` at a Blacksmith (pays, then ticks `Researching` and
//!     raises the team's `TeamResearch` level when the clock runs out)
//!   * `BuyItem` at a Shop (pays and fills a hero's inventory slot)
//!
//! All money in the game is spent here: `ui.rs` / `ai.rs` only check affordability.

use bevy::prelude::*;
use std::collections::HashMap;

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Seconds spent standing at a node per trip.
const GATHER_TIME: f32 = 1.5;
/// Resources taken per trip.
const CARRY_AMOUNT: u32 = 10;
/// How many times a worker will re-issue a `MoveTo` before giving up on a job.
const MAX_ATTEMPTS: u32 = 8;
/// Auto-rebalance skips nodes with enemy combat units within this range.
const NODE_DANGER_RADIUS: f32 = 16.0;
/// Buildings start at this fraction of their final height.
const BUILD_START_SCALE: f32 = 0.4;
/// Buildings start at this fraction of their max HP.
const BUILD_START_HP: f32 = 0.1;
/// How far a building's roofline sinks while it is upgrading in place. Far
/// gentler than `BUILD_START_SCALE`: the building is still standing and still
/// providing supply, it is merely under scaffolding.
const UPGRADE_SCAFFOLD_SCALE: f32 = 0.85;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct EconomyPlugin;

impl Plugin for EconomyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_economy_assets).add_systems(
            Update,
            (
                // Banking a bounty has nothing to do with the harvest loop
                // that follows, but it does touch the same `Economies`, so it
                // can no longer float: two systems that both write a team's
                // gold must have a stated order or the total depends on the
                // executor. It goes FIRST so treasure claimed this frame is
                // spendable this frame — bounty.rs sits immediately upstream.
                bank_bounties,
                spawn_buildings,
                construction_progress,
                start_upgrades,
                upgrade_progress,
                start_research,
                research_progress,
                order_changed,
                build_sites,
                harvest_loop,
                training_queues,
                buy_items,
                // Last in the chain, so a sample describes the frame that just
                // finished paying and banking rather than the one before it.
                sample_gold_flow,
            )
                .chain()
                .in_set(SimSet::Economy),
        );
    }
}

/// Pay out a claimed treasure cache. Unlike a gold delivery this is NOT taxed
/// by upkeep (documented in shared.rs): treasure rewards the bold, and a big
/// army is exactly what it takes to hold the middle.
fn bank_bounties(mut claims: EventReader<BountyClaim>, mut economies: ResMut<Economies>) {
    for claim in claims.read() {
        let economy = economies.get_mut(claim.team);
        economy.earn(claim.gold);
        debug!(
            "bounty banked: {:?} +{}g (untaxed) at ({:.0},{:.0}) -> {}g",
            claim.team, claim.gold, claim.pos.x, claim.pos.z, economy.gold
        );
    }
}

/// **The gold runway, sampled.** Both derivatives of the bank, once per game
/// second, for whichever renderer asks.
///
/// Two numbers, and they are different kinds of thing:
///
///   * `income_per_min` is MEASURED — the difference of `Economy::earned` over
///     a trailing [`INCOME_WINDOW_S`] window. Measured rather than modelled
///     because the model ("workers × trips × carry") is wrong about every
///     interesting case: a crew walking past a raid, a mine that just ran out,
///     an upkeep bracket the team crossed on the last supply. A commander that
///     is being taxed 40% wants the taxed number, and that is the one that
///     lands in the bank.
///   * `commit_per_min` is PROJECTED — what the standing training queues will
///     demand if they run as scheduled. This is r36's missing fact: three
///     Barracks each cycling a 135g Footman every 20s want 1,215 gold a minute,
///     and until now the only thing that ever said so was a stream of "cannot
///     afford" bounces on a bank that never went up.
///
/// The projection deliberately prices heroes off the flat catalog row rather
/// than through `hero_train_cost`, so a team's *first* hero — which is free —
/// is counted at full fare for as long as it sits in a queue. Overstating a
/// commitment by one waiver is the safe direction for a number whose whole job
/// is to say "you have promised more than you earn", and it keeps this system
/// off `HeroRecords`.
///
/// Sampled on a **game**-time cadence rather than `on_timer`'s virtual clock,
/// so two runs of one seed at one fixed dt sample at identical game times and
/// publish identical rates.
fn sample_gold_flow(
    time: Res<Time>,
    economies: Res<Economies>,
    mut flow: ResMut<GoldFlow>,
    mut next_at: Local<f32>,
    queues: Query<(&Team, &TrainingQueue), Without<UnderConstruction>>,
) {
    let now = time.elapsed_secs();
    if now < *next_at {
        return;
    }
    // Advance to the next whole sample boundary at or after `now`, so a long
    // frame skips a sample instead of firing a burst of them.
    *next_at = (now / INCOME_SAMPLE_S).floor() * INCOME_SAMPLE_S + INCOME_SAMPLE_S;

    for team in [Team::Human, Team::Claude] {
        // PER BUILDING and then summed, because that is how production
        // actually parallelises: one building trains its queue serially (so
        // its own rate is the queue's cost over the queue's time), and three
        // buildings draw on the bank at once. Pooling the numerator and the
        // denominator across buildings would have reported three Barracks as
        // one, which is the exact reading r36 was missing.
        let mut commit = 0.0f32;
        for (owner, queue) in queues.iter() {
            if *owner != team || queue.queue.is_empty() {
                continue;
            }
            let mut cost = 0.0f32;
            let mut secs = 0.0f32;
            for kind in &queue.queue {
                let stats = unit_stats(*kind);
                cost += stats.cost_gold as f32;
                secs += stats.train_time.max(0.1);
            }
            if secs > 0.0 {
                commit += cost * 60.0 / secs;
            }
        }
        flow.observe(team, now, economies.get(team).earned, commit);
    }
}

// ---------------------------------------------------------------------------
// Cached procedural meshes / materials
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Part {
    mesh: Handle<Mesh>,
    tf: Transform,
    /// Index into a team's material palette: 0 body, 1 accent, 2 trim.
    mat: usize,
}

#[derive(Resource, Default)]
struct EconomyAssets {
    parts: HashMap<BuildingKind, Vec<Part>>,
    team_mats: HashMap<Team, [Handle<StandardMaterial>; 3]>,
    /// Muted variant of `team_mats`, used by walls: a dozen of them in a row at
    /// full team saturation turns the base into a solid slab of colour.
    wall_mats: HashMap<Team, [Handle<StandardMaterial>; 3]>,
    carry_mesh: Handle<Mesh>,
    gold_mat: Handle<StandardMaterial>,
    lumber_mat: Handle<StandardMaterial>,
}

impl EconomyAssets {
    /// Which palette a kind renders with (see `wall_mats`).
    fn palette(&self, kind: BuildingKind, team: Team) -> Option<&[Handle<StandardMaterial>; 3]> {
        match kind {
            BuildingKind::Wall => self.wall_mats.get(&team),
            _ => self.team_mats.get(&team),
        }
    }
}

/// Body / accent / trim materials derived from one base tint.
fn palette_from(
    materials: &mut Assets<StandardMaterial>,
    c: Srgba,
) -> [Handle<StandardMaterial>; 3] {
    let body = materials.add(StandardMaterial {
        base_color: Color::srgb(
            c.red * 0.55 + 0.18,
            c.green * 0.55 + 0.18,
            c.blue * 0.55 + 0.18,
        ),
        perceptual_roughness: 0.9,
        ..default()
    });
    let accent = materials.add(StandardMaterial {
        base_color: Color::srgb(c.red * 0.9, c.green * 0.9, c.blue * 0.9),
        perceptual_roughness: 0.8,
        ..default()
    });
    let trim = materials.add(StandardMaterial {
        base_color: Color::srgb(
            (c.red * 1.3).min(1.0),
            (c.green * 1.3).min(1.0),
            (c.blue * 1.3).min(1.0),
        ),
        perceptual_roughness: 0.6,
        ..default()
    });
    [body, accent, trim]
}

fn setup_economy_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut parts: HashMap<BuildingKind, Vec<Part>> = HashMap::new();

    // --- Town hall: big keep + smaller upper tier + spire ------------------
    parts.insert(
        BuildingKind::TownHall,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(8.0, 3.6, 8.0)),
                tf: Transform::from_xyz(0.0, 1.8, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(5.0, 2.0, 5.0)),
                tf: Transform::from_xyz(0.0, 4.6, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(1.2, 1.8, 1.2)),
                tf: Transform::from_xyz(0.0, 6.5, 0.0),
                mat: 2,
            },
        ],
    );

    // --- Keep: the town hall grown a storey, with four corner turrets ------
    // Same 8-wide footprint as the TownHall (an upgrade must never need ground
    // the original did not already hold) but half again as tall, and the
    // turrets are the read at a glance: this base has teched.
    {
        let mut keep = vec![
            Part {
                mesh: meshes.add(Cuboid::new(8.0, 4.4, 8.0)),
                tf: Transform::from_xyz(0.0, 2.2, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(5.6, 2.6, 5.6)),
                tf: Transform::from_xyz(0.0, 5.7, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(1.4, 2.6, 1.4)),
                tf: Transform::from_xyz(0.0, 8.3, 0.0),
                mat: 2,
            },
        ];
        let turret = meshes.add(Cylinder::new(0.8, 6.0));
        let cap = meshes.add(Cylinder::new(1.0, 0.5));
        for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            keep.push(Part {
                mesh: turret.clone(),
                tf: Transform::from_xyz(3.2 * sx, 3.0, 3.2 * sz),
                mat: 0,
            });
            keep.push(Part {
                mesh: cap.clone(),
                tf: Transform::from_xyz(3.2 * sx, 6.25, 3.2 * sz),
                mat: 2,
            });
        }
        parts.insert(BuildingKind::Keep, keep);
    }

    // --- Castle: skirted curtain wall, tall corner towers, central spire ---
    // Still 8 wide on the grid; ~12 tall, which makes it the highest thing on
    // the field by a clear margin. Tier is meant to be legible from the camera.
    {
        let mut castle = vec![
            // Skirt: a wider, low plinth reading as a curtain wall.
            Part {
                mesh: meshes.add(Cuboid::new(8.4, 1.4, 8.4)),
                tf: Transform::from_xyz(0.0, 0.7, 0.0),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cuboid::new(7.2, 5.0, 7.2)),
                tf: Transform::from_xyz(0.0, 3.2, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(5.0, 3.0, 5.0)),
                tf: Transform::from_xyz(0.0, 7.2, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(1.6, 3.2, 1.6)),
                tf: Transform::from_xyz(0.0, 10.4, 0.0)
                    .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
                mat: 2,
            },
        ];
        let tower = meshes.add(Cylinder::new(0.95, 9.0));
        let cap = meshes.add(Cylinder::new(1.15, 0.6));
        for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            castle.push(Part {
                mesh: tower.clone(),
                tf: Transform::from_xyz(3.3 * sx, 4.5, 3.3 * sz),
                mat: 0,
            });
            castle.push(Part {
                mesh: cap.clone(),
                tf: Transform::from_xyz(3.3 * sx, 9.3, 3.3 * sz),
                mat: 2,
            });
        }
        parts.insert(BuildingKind::Castle, castle);
    }

    // --- Barracks: wide hall + flat roof slab + banner pole ----------------
    parts.insert(
        BuildingKind::Barracks,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(6.0, 3.2, 6.0)),
                tf: Transform::from_xyz(0.0, 1.6, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(6.8, 0.5, 6.8)),
                tf: Transform::from_xyz(0.0, 3.45, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.3, 2.2, 1.6)),
                tf: Transform::from_xyz(2.4, 4.8, 0.0),
                mat: 2,
            },
        ],
    );

    // --- Farm: small hut + 45-degree rotated cuboid "prism" roof + chimney -
    parts.insert(
        BuildingKind::Farm,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(4.0, 1.6, 4.0)),
                tf: Transform::from_xyz(0.0, 0.8, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(2.6, 2.6, 4.4)),
                tf: Transform::from_xyz(0.0, 2.3, 0.0)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.5, 1.2, 0.5)),
                tf: Transform::from_xyz(1.2, 3.4, 1.2),
                mat: 2,
            },
        ],
    );

    // --- Tower: narrow shaft + overhanging platform + crystal on top --------
    // Footprint is only 3 wide, so it reads as height. combat.rs fires its
    // bolts from y ≈ 5, i.e. off the platform.
    parts.insert(
        BuildingKind::Tower,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(1.4, 5.0, 1.4)),
                tf: Transform::from_xyz(0.0, 2.5, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(2.4, 0.7, 2.4)),
                tf: Transform::from_xyz(0.0, 5.15, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.55, 1.1, 0.55)),
                tf: Transform::from_xyz(0.0, 6.0, 0.0)
                    .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
                mat: 2,
            },
        ],
    );

    // --- Wall: chunky block with a tapered cap — palisade segment -----------
    parts.insert(
        BuildingKind::Wall,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(2.0, 1.8, 2.0)),
                tf: Transform::from_xyz(0.0, 0.9, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(1.5, 0.45, 1.5)),
                tf: Transform::from_xyz(0.0, 2.0, 0.0),
                mat: 1,
            },
        ],
    );

    // --- Workshop: squat industrial hall + side cog wheel + stub chimney ----
    // Footprint 5 wide but deliberately LOW (2.2 tall vs the Barracks' 3.2), so
    // the siege works reads as a machine shed, not another troop hall. The cog
    // is a flat cylinder laid on its side against the +X wall.
    parts.insert(
        BuildingKind::Workshop,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(5.0, 2.2, 5.0)),
                tf: Transform::from_xyz(0.0, 1.1, 0.0),
                mat: 0,
            },
            // Roof slab, slightly overhanging.
            Part {
                mesh: meshes.add(Cuboid::new(5.6, 0.4, 5.6)),
                tf: Transform::from_xyz(0.0, 2.4, 0.0),
                mat: 1,
            },
            // The cog: a wide, thin cylinder standing proud of the side wall.
            Part {
                mesh: meshes.add(Cylinder::new(1.5, 0.5)),
                tf: Transform::from_xyz(2.6, 1.5, 0.0)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
                mat: 2,
            },
            // Cog hub, so it reads as machinery rather than a plain disc.
            Part {
                mesh: meshes.add(Cylinder::new(0.45, 0.8)),
                tf: Transform::from_xyz(2.9, 1.5, 0.0)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
                mat: 1,
            },
            // Chimney at the far corner.
            Part {
                mesh: meshes.add(Cuboid::new(0.6, 1.6, 0.6)),
                tf: Transform::from_xyz(-1.6, 3.3, -1.6),
                mat: 2,
            },
        ],
    );

    // --- Shop: market stall — low counter, tilted striped awning, crate -----
    // Footprint 4 wide and deliberately the LOWEST silhouette on the field
    // (nothing above ~2.6): a vendor's stall, not a fortification. The awning
    // is a thin slab tipped forward over the counter, with two trim stripes
    // laid on it so it reads as canvas rather than another roof.
    parts.insert(
        BuildingKind::Shop,
        vec![
            // Counter block.
            Part {
                mesh: meshes.add(Cuboid::new(3.4, 1.0, 2.0)),
                tf: Transform::from_xyz(0.0, 0.5, 0.0),
                mat: 0,
            },
            // Overhanging counter top — the "serving" surface.
            Part {
                mesh: meshes.add(Cuboid::new(3.8, 0.2, 2.4)),
                tf: Transform::from_xyz(0.0, 1.1, 0.0),
                mat: 2,
            },
            // Awning: thin slab tilted forward over the counter.
            Part {
                mesh: meshes.add(Cuboid::new(4.0, 0.16, 2.2)),
                tf: Transform::from_xyz(0.0, 2.3, -0.3)
                    .with_rotation(Quat::from_rotation_x(-0.32)),
                mat: 1,
            },
            // Two stripes across the awning.
            Part {
                mesh: meshes.add(Cuboid::new(0.7, 0.16, 2.3)),
                tf: Transform::from_xyz(-1.1, 2.38, -0.3)
                    .with_rotation(Quat::from_rotation_x(-0.32)),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.7, 0.16, 2.3)),
                tf: Transform::from_xyz(1.1, 2.38, -0.3)
                    .with_rotation(Quat::from_rotation_x(-0.32)),
                mat: 2,
            },
            // Corner post holding the awning up.
            Part {
                mesh: meshes.add(Cuboid::new(0.2, 2.2, 0.2)),
                tf: Transform::from_xyz(1.75, 1.1, -1.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.2, 2.2, 0.2)),
                tf: Transform::from_xyz(-1.75, 1.1, -1.0),
                mat: 0,
            },
            // Goods crate beside the stall, kicked off-axis.
            Part {
                mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
                tf: Transform::from_xyz(-1.4, 0.5, 1.4)
                    .with_rotation(Quat::from_rotation_y(0.4)),
                mat: 1,
            },
        ],
    );

    // --- Arcane Sanctum: narrow tower + ring + floating capstone -----------
    // Footprint 5 like the Workshop, but the exact inverse silhouette: the
    // Workshop is the widest low thing on the field, the Sanctum the narrowest
    // TALL one (~6.4 to the capstone — above everything but the town hall
    // spire). A tier-2 building has to be legible from across the map, because
    // "they have a Sanctum" means "their army now has Slow in it" and that is
    // information a scout must be able to bring home at a glance.
    parts.insert(
        BuildingKind::Sanctum,
        vec![
            // Stepped base.
            Part {
                mesh: meshes.add(Cuboid::new(4.6, 0.8, 4.6)),
                tf: Transform::from_xyz(0.0, 0.4, 0.0),
                mat: 0,
            },
            // The tower proper — an octagonal shaft, so it reads as masonry
            // rather than another crate.
            Part {
                mesh: meshes.add(Cylinder::new(1.5, 4.2)),
                tf: Transform::from_xyz(0.0, 2.9, 0.0),
                mat: 0,
            },
            // Balcony ring two thirds of the way up.
            Part {
                mesh: meshes.add(Torus::new(0.22, 1.75)),
                tf: Transform::from_xyz(0.0, 4.1, 0.0),
                mat: 2,
            },
            // Capstone: a floating cube tipped onto a corner, hovering a hand
            // above the shaft. Nothing else on the map is detached from the
            // ground, which is the tell.
            Part {
                mesh: meshes.add(Cuboid::new(1.2, 1.2, 1.2)),
                tf: Transform::from_xyz(0.0, 6.0, 0.0).with_rotation(
                    Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)
                        * Quat::from_rotation_x(std::f32::consts::FRAC_PI_4),
                ),
                mat: 2,
            },
            // Two buttresses, so the shaft does not read as a lone pillar.
            Part {
                mesh: meshes.add(Cuboid::new(0.5, 2.6, 0.5)),
                tf: Transform::from_xyz(1.7, 1.5, 1.7),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.5, 2.6, 0.5)),
                tf: Transform::from_xyz(-1.7, 1.5, -1.7),
                mat: 1,
            },
        ],
    );

    // --- Blacksmith: forge hall + fat chimney + anvil on a stump -------------
    // Footprint 5, like the Workshop, and deliberately distinguished from it by
    // VERTICAL mass rather than plan: the Workshop is a low shed with a cog on
    // its flank, the Blacksmith is a squat hall with the tallest chimney on the
    // field. At a glance across a base the two never read as each other, which
    // matters because they are the two tier-gated support buildings and a
    // player checking "did I build the forge yet" is doing it from the camera.
    parts.insert(
        BuildingKind::Blacksmith,
        vec![
            // Forge hall.
            Part {
                mesh: meshes.add(Cuboid::new(4.6, 2.6, 4.6)),
                tf: Transform::from_xyz(0.0, 1.3, 0.0),
                mat: 0,
            },
            // Roof slab.
            Part {
                mesh: meshes.add(Cuboid::new(5.2, 0.4, 5.2)),
                tf: Transform::from_xyz(0.0, 2.8, 0.0),
                mat: 1,
            },
            // The chimney: broad and tall, the building's whole silhouette.
            Part {
                mesh: meshes.add(Cuboid::new(1.4, 3.0, 1.4)),
                tf: Transform::from_xyz(-1.3, 4.3, -1.3),
                mat: 2,
            },
            // Chimney cap, so the stack reads as a stack and not a pillar.
            Part {
                mesh: meshes.add(Cuboid::new(1.8, 0.3, 1.8)),
                tf: Transform::from_xyz(-1.3, 5.9, -1.3),
                mat: 1,
            },
            // Anvil: a small block on a cylindrical stump, out front.
            Part {
                mesh: meshes.add(Cylinder::new(0.45, 0.7)),
                tf: Transform::from_xyz(1.8, 0.35, 2.0),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cuboid::new(1.1, 0.45, 0.5)),
                tf: Transform::from_xyz(1.8, 0.92, 2.0),
                mat: 1,
            },
        ],
    );

    // =======================================================================
    // Horde
    // =======================================================================
    // One silhouette rule governs every block below, so the individual blocks
    // can stay terse. The Kingdom is RECTILINEAR AND VERTICAL: square keeps,
    // prism roofs, spires, straight shafts — right angles stacked upward. The
    // Horde is LOW, WIDE AND ANGULAR: squat masses under tapered cone roofs,
    // slabs pitched off-axis, stakes and totems where the Kingdom puts spires.
    // The point is a scouting one. A player sweeping the camera over a base
    // must be able to say "that is not mine" before reading a single shape in
    // detail, and team tint cannot carry that alone (it only has three
    // materials and both races use all three). So the races are separated by
    // POSTURE — height and taper — which survives at any zoom.
    //
    // The three hall rungs still have to read as a ladder among themselves, on
    // the same terms the TownHall -> Keep -> Castle ladder does: each rung is
    // visibly taller and heavier than the last (4.6 -> 7.0 -> 9.6 to the roof
    // tip), while every rung stays under its Kingdom opposite number.

    // --- Stronghold: one squat 8-wide mass under a wide conical roof -------
    // Tier 1. Fills the whole 8.0 footprint but tops out at 4.6, well under the
    // TownHall's 7.4 spire: the Horde hall sprawls where the Kingdom's climbs.
    parts.insert(
        BuildingKind::Stronghold,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(8.0, 2.2, 8.0)),
                tf: Transform::from_xyz(0.0, 1.1, 0.0),
                mat: 0,
            },
            // Cone radius 4.0 is exactly half the footprint — the roof reaches
            // the walls and no further.
            Part {
                mesh: meshes.add(Cone::new(4.0, 2.4)),
                tf: Transform::from_xyz(0.0, 3.4, 0.0),
                mat: 1,
            },
            // Two stakes driven into the roof line, on the diagonal where the
            // cone has already fallen away from the corners.
            Part {
                mesh: meshes.add(Cone::new(0.3, 1.8)),
                tf: Transform::from_xyz(3.2, 3.1, 3.2),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cone::new(0.3, 1.8)),
                tf: Transform::from_xyz(-3.2, 3.1, -3.2),
                mat: 2,
            },
        ],
    );

    // --- Fortress: the Stronghold grown a tier, ringed with corner stakes ---
    // Same 8.0 footprint as the Stronghold, for the Keep's reason: an upgrade
    // must never demand ground the original did not already hold. The stakes
    // are the tell that this base has teched, the way the Keep's turrets are.
    {
        let mut fortress = vec![
            Part {
                mesh: meshes.add(Cuboid::new(8.0, 3.0, 8.0)),
                tf: Transform::from_xyz(0.0, 1.5, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(5.6, 1.8, 5.6)),
                tf: Transform::from_xyz(0.0, 3.9, 0.0),
                mat: 1,
            },
            // Roof overhangs the upper tier by a shade, so the eave line stays
            // visible from the camera's angle.
            Part {
                mesh: meshes.add(Cone::new(3.0, 2.2)),
                tf: Transform::from_xyz(0.0, 5.9, 0.0),
                mat: 2,
            },
        ];
        // Stakes stand on the lower roof, outside the upper tier's 2.8 half
        // width and inside the 4.0 footprint edge.
        let stake = meshes.add(Cone::new(0.35, 2.6));
        for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            fortress.push(Part {
                mesh: stake.clone(),
                tf: Transform::from_xyz(3.2 * sx, 4.3, 3.2 * sz),
                mat: 2,
            });
        }
        parts.insert(BuildingKind::Fortress, fortress);
    }

    // --- Hold: skirted earthwork, four corner totems, big tapered roof -----
    // Tier 3, and the tallest thing the Horde builds at 9.6 — still short of
    // the Castle's ~12, which is the race read holding even at the top rung.
    {
        let mut hold = vec![
            // Earth skirt at the full 8.0 footprint: the Castle's curtain wall
            // answered with a rampart rather than masonry.
            Part {
                mesh: meshes.add(Cuboid::new(8.0, 1.2, 8.0)),
                tf: Transform::from_xyz(0.0, 0.6, 0.0),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cuboid::new(7.0, 3.4, 7.0)),
                tf: Transform::from_xyz(0.0, 2.9, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(4.8, 2.2, 4.8)),
                tf: Transform::from_xyz(0.0, 5.7, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cone::new(2.8, 2.8)),
                tf: Transform::from_xyz(0.0, 8.2, 0.0),
                mat: 2,
            },
        ];
        // Totems on the skirt corners, clear of the 3.5 half width of the main
        // mass; the spike caps stop exactly at the 4.0 footprint edge.
        let totem = meshes.add(Cylinder::new(0.38, 4.6));
        let spike = meshes.add(Cone::new(0.5, 1.4));
        for (sx, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            hold.push(Part {
                mesh: totem.clone(),
                tf: Transform::from_xyz(3.5 * sx, 3.5, 3.5 * sz),
                mat: 0,
            });
            hold.push(Part {
                mesh: spike.clone(),
                tf: Transform::from_xyz(3.5 * sx, 6.5, 3.5 * sz),
                mat: 2,
            });
        }
        parts.insert(BuildingKind::Hold, hold);
    }

    // --- WarCamp: low pen under two pitched hide slabs + flank totem -------
    // The Barracks' opposite: 6.0 of footprint spent on a 2.4-tall pen with a
    // ridged roof lashed over it, topping out at 4.2 against the Barracks'
    // 5.9. The one martial building the Horde has should read as a camp.
    parts.insert(
        BuildingKind::WarCamp,
        vec![
            // 5.4 rather than the full 6.0, so the totem can stand proud of a
            // wall instead of being swallowed by it.
            Part {
                mesh: meshes.add(Cuboid::new(5.4, 2.4, 5.4)),
                tf: Transform::from_xyz(0.0, 1.2, 0.0),
                mat: 0,
            },
            // Two slabs pitched off-axis, meeting in a ridge over the middle —
            // the angular answer to the Barracks' flat roof slab.
            Part {
                mesh: meshes.add(Cuboid::new(6.0, 0.35, 3.3)),
                tf: Transform::from_xyz(0.0, 3.3, 1.35)
                    .with_rotation(Quat::from_rotation_x(0.55)),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cuboid::new(6.0, 0.35, 3.3)),
                tf: Transform::from_xyz(0.0, 3.3, -1.35)
                    .with_rotation(Quat::from_rotation_x(-0.55)),
                mat: 1,
            },
            // Totem lashed to the +X flank, where the Barracks flies a banner.
            Part {
                mesh: meshes.add(Cylinder::new(0.2, 3.6)),
                tf: Transform::from_xyz(2.75, 1.8, 1.9),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Sphere::new(0.35)),
                tf: Transform::from_xyz(2.75, 3.75, 1.9),
                mat: 2,
            },
        ],
    );

    // --- Burrow: earth mound with a dark mouth and two spears in it --------
    // A hole in the ground, not a hut: nothing here clears 2.0, where the Farm
    // reaches 3.6. That silhouette IS the building's rules text — the Burrow
    // pays supply like a Farm but sits in the dirt and stabs at whatever walks
    // past, so it must not read as another barn from across the map.
    parts.insert(
        BuildingKind::Burrow,
        vec![
            // Radius 2.0 fills the 4.0 footprint exactly: all width, no height.
            Part {
                mesh: meshes.add(Cylinder::new(2.0, 0.6)),
                tf: Transform::from_xyz(0.0, 0.3, 0.0),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cone::new(1.8, 1.4)),
                tf: Transform::from_xyz(0.0, 1.3, 0.0),
                mat: 0,
            },
            // The mouth: accent material set into body-toned earth, so it reads
            // as a shadowed opening rather than a door.
            Part {
                mesh: meshes.add(Cuboid::new(1.3, 0.9, 0.8)),
                tf: Transform::from_xyz(0.0, 0.45, 1.5),
                mat: 1,
            },
            // Two spears angled out of the mouth — the weak attack, made
            // visible. Tips stop at z 1.71, inside the footprint.
            Part {
                mesh: meshes.add(Cone::new(0.14, 2.0)),
                tf: Transform::from_xyz(0.5, 1.25, 0.9)
                    .with_rotation(Quat::from_rotation_x(0.95)),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cone::new(0.14, 2.0)),
                tf: Transform::from_xyz(-0.5, 1.25, 0.9)
                    .with_rotation(Quat::from_rotation_x(0.95)),
                mat: 2,
            },
        ],
    );

    // --- Watchtower: lashed stake tower with an open platform --------------
    // Footprint is 3, same as the Tower, but the shaft is 0.9 against the
    // Tower's 1.4 and is braced by two splayed legs instead of standing
    // straight: a thing hammered together out of stakes, not built out of
    // stone. combat.rs fires from y = 5.0 (`TOWER_MUZZLE_HEIGHT`), so the
    // platform sits at 4.75 and the spears go up around it.
    parts.insert(
        BuildingKind::Watchtower,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(0.9, 4.6, 0.9)),
                tf: Transform::from_xyz(0.0, 2.3, 0.0),
                mat: 0,
            },
            // Splayed legs: feet at x ±1.2, inside the 1.5 footprint edge.
            Part {
                mesh: meshes.add(Cuboid::new(0.22, 3.6, 0.22)),
                tf: Transform::from_xyz(0.7, 1.75, 0.0)
                    .with_rotation(Quat::from_rotation_z(0.22)),
                mat: 0,
            },
            Part {
                mesh: meshes.add(Cuboid::new(0.22, 3.6, 0.22)),
                tf: Transform::from_xyz(-0.7, 1.75, 0.0)
                    .with_rotation(Quat::from_rotation_z(-0.22)),
                mat: 0,
            },
            // The firing platform, just under the muzzle height.
            Part {
                mesh: meshes.add(Cuboid::new(2.4, 0.3, 2.4)),
                tf: Transform::from_xyz(0.0, 4.75, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cone::new(0.18, 1.4)),
                tf: Transform::from_xyz(0.85, 5.6, 0.85),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cone::new(0.18, 1.4)),
                tf: Transform::from_xyz(-0.85, 5.6, -0.85),
                mat: 2,
            },
        ],
    );

    // --- Spirit Lodge: round hut under a broad conical thatch + totem ------
    // The exact inverse of the Sanctum it answers: the Sanctum is the narrowest
    // TALL thing on the field (6.6 to its floating capstone), the Lodge a wide
    // low cone stopping at 4.9. Both still have to be legible from across the
    // map for the same reason — seeing one means casters are coming.
    parts.insert(
        BuildingKind::SpiritLodge,
        vec![
            Part {
                mesh: meshes.add(Cylinder::new(1.9, 2.0)),
                tf: Transform::from_xyz(0.0, 1.0, 0.0),
                mat: 0,
            },
            // Radius 2.5 is the whole 5.0 footprint: the thatch overhangs the
            // hut on every side, which is what makes the shape read as squat.
            Part {
                mesh: meshes.add(Cone::new(2.5, 1.8)),
                tf: Transform::from_xyz(0.0, 2.9, 0.0),
                mat: 1,
            },
            Part {
                mesh: meshes.add(Cone::new(0.25, 1.1)),
                tf: Transform::from_xyz(0.0, 4.35, 0.0),
                mat: 2,
            },
            // Fetish pole through the eave — the caster tell at ground level,
            // where the Sanctum puts a hovering cube overhead.
            Part {
                mesh: meshes.add(Cylinder::new(0.2, 3.4)),
                tf: Transform::from_xyz(2.2, 1.7, 0.0),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Sphere::new(0.35)),
                tf: Transform::from_xyz(2.2, 3.55, 0.0),
                mat: 2,
            },
        ],
    );

    // --- WarMill: low forge under a single-pitch roof + leaning stack ------
    // Same 5.0 footprint and the same job as the Blacksmith, and distinguished
    // from it exactly the way the Blacksmith is distinguished from the
    // Workshop — by the stack. The Blacksmith's chimney is broad, vertical and
    // the tallest thing in a Kingdom base at ~6.0; the WarMill's leans, and
    // stops at 4.8. Nothing about this building stands up straight.
    parts.insert(
        BuildingKind::WarMill,
        vec![
            Part {
                mesh: meshes.add(Cuboid::new(4.2, 2.0, 4.2)),
                tf: Transform::from_xyz(0.0, 1.0, 0.0),
                mat: 0,
            },
            // One pitched slab instead of a flat roof slab: the whole roof
            // slopes one way, which is the read at any zoom.
            Part {
                mesh: meshes.add(Cuboid::new(4.8, 0.4, 4.6)),
                tf: Transform::from_xyz(0.0, 2.4, 0.0)
                    .with_rotation(Quat::from_rotation_x(-0.35)),
                mat: 1,
            },
            // The stack, tipped off vertical and punched through the roof.
            Part {
                mesh: meshes.add(Cylinder::new(0.5, 3.2)),
                tf: Transform::from_xyz(-1.3, 3.3, -1.3)
                    .with_rotation(Quat::from_rotation_z(0.28)),
                mat: 2,
            },
            Part {
                mesh: meshes.add(Cuboid::new(1.3, 0.28, 1.3)),
                tf: Transform::from_xyz(-1.74, 4.84, -1.3)
                    .with_rotation(Quat::from_rotation_z(0.28)),
                mat: 1,
            },
            // Slag heap spilling out of the far corner, half sunk in the
            // ground — the Blacksmith's tidy anvil-on-a-stump, gone feral.
            Part {
                mesh: meshes.add(Sphere::new(0.45)),
                tf: Transform::from_xyz(2.0, 0.3, 2.0),
                mat: 2,
            },
        ],
    );

    let mut team_mats: HashMap<Team, [Handle<StandardMaterial>; 3]> = HashMap::new();
    let mut wall_mats: HashMap<Team, [Handle<StandardMaterial>; 3]> = HashMap::new();
    for team in [Team::Human, Team::Claude] {
        let c = team.color().to_srgba();
        team_mats.insert(team, palette_from(&mut materials, c));
        // Walls: pull the tint most of the way toward its own luminance so a
        // long palisade reads as stone/timber with a hint of team colour.
        let lum = 0.30 * c.red + 0.59 * c.green + 0.11 * c.blue;
        let muted = |x: f32| lum * 0.6 + x * 0.4;
        wall_mats.insert(
            team,
            palette_from(
                &mut materials,
                Srgba::new(muted(c.red), muted(c.green), muted(c.blue), 1.0),
            ),
        );
    }

    commands.insert_resource(EconomyAssets {
        parts,
        team_mats,
        wall_mats,
        carry_mesh: meshes.add(Cuboid::new(0.55, 0.55, 0.55)),
        gold_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.84, 0.15),
            emissive: LinearRgba::rgb(0.35, 0.28, 0.02),
            ..default()
        }),
        lumber_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.28, 0.12),
            ..default()
        }),
    });
}

// ---------------------------------------------------------------------------
// Module-private components
// ---------------------------------------------------------------------------

/// A worker that has been told to build and is walking to the site.
#[derive(Component)]
struct BuildSite {
    kind: BuildingKind,
    pos: Vec3,
    attempts: u32,
}

/// **The one sentence an abandoned foundation gets.**
///
/// Construction has two failure windows and only the first one ever spoke. The
/// compiler refuses an illegal `build` in words at command time; what happened
/// *after* an accepted one — the worker re-tasked, killed, or turned away at
/// the site — was pure silence, and the commander's only evidence was a
/// building that never appeared. r25-red sent three accepted `build`s and got
/// no Barracks and no error; r26-blue lost three expansions and a Blacksmith
/// the same way, and diagnosed it by diffing `buildings[]` by hand. This is
/// that window learning to talk.
///
/// **The economics, stated because they are the surprising part.** Nothing is
/// spent. `build_sites` below is the only place a building's price is ever
/// paid, and it pays at the moment the worker breaks ground — so an order that
/// dies before then costs its team no gold and no lumber, and there is nothing
/// to refund. What it costs is *time*, and a commander who thinks it cost gold
/// will bank against a bill that never arrives. So the line says so out loud.
///
/// Edge-triggered, and terminal: an abandonment happens once, at the moment the
/// site stops being anybody's job (tools/BUILDER_BRIEF.md §6.11).
fn report_abandoned_build(
    feed: &mut GameEvents,
    team: Team,
    now: f32,
    site: &BuildSite,
    why: &str,
) {
    feed.push(
        team,
        now,
        format!(
            "build abandoned: {} at ({:.0}, {:.0}) — {why}; nothing was spent \
             (a build is paid for when the worker breaks ground)",
            building_name(site.kind),
            site.pos.x,
            site.pos.z,
        ),
        EventSeverity::Warning,
        Some(site.pos),
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HarvestPhase {
    ToNode,
    Gathering,
    ToDepot,
}

/// Active harvest job on a worker.
#[derive(Component)]
struct HarvestJob {
    node: Option<Entity>,
    node_pos: Vec3,
    kind: ResourceKind,
    phase: HarvestPhase,
    timer: f32,
    attempts: u32,
}

/// Last node this worker worked on, kept across order overrides so
/// `Order::ReturnResources` can resume harvesting afterwards.
#[derive(Component, Clone, Copy)]
struct RememberedNode {
    node: Option<Entity>,
    node_pos: Vec3,
    kind: ResourceKind,
}

/// The little floating cube shown over a carrying worker.
#[derive(Component)]
struct CarryVisual(Entity);

/// **What this building has already paid for at the front of its queue**, if
/// anything.
///
/// This was a bare `bool` — "the front item is paid" — and the bool was a bug
/// with two faces. A `cancel` of the front item removes the item and leaves
/// the flag standing, so the NEXT thing in the queue inherited a payment it
/// never made: queue a Footman, let it pay, cancel it, and the Champion behind
/// it trained for free *and* skipped the hero-slot check, because both gates
/// live behind `if !paid_front`. The mirror of that bug is the money: a
/// cancelled item's gold was never handed back, because nothing recorded how
/// much had been spent or on what.
///
/// Recording the KIND and the AMOUNT fixes both at once and keeps economy.rs
/// the single owner of every payment — intent.rs still only edits the queue,
/// exactly as `cancel` always did, and this system notices on the next tick
/// that what it paid for is no longer at the front and gives the money back.
///
/// The amount is stored rather than recomputed on purpose: a hero's price
/// depends on whether its class has a record, and a hero that DIED while its
/// revival sat in a queue would otherwise be refunded at a different price
/// from the one it was charged.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
struct PaidFront(Option<PaidItem>);

/// One paid-for, not-yet-delivered queue item.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PaidItem {
    kind: UnitKind,
    gold: u32,
    lumber: u32,
}

impl PaidFront {
    /// The payment standing against `front`, if this building's paid item IS
    /// what is at the front of its queue now.
    ///
    /// Compared by KIND, not by identity: cancelling one of two queued Footmen
    /// leaves a Footman at the front, and the payment rightly carries to it —
    /// one Footman was bought and one Footman is being built. It is only when
    /// the front becomes a DIFFERENT kind (or nothing at all) that a purchase
    /// has been abandoned.
    fn covers(&self, front: UnitKind) -> Option<PaidItem> {
        self.0.filter(|item| item.kind == front)
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

fn xz_dist(a: Vec3, b: Vec3) -> f32 {
    flat(a - b).length()
}

/// A point `radius` away from `target`, on the side `from` is standing.
fn approach_point(from: Vec3, target: Vec3, radius: f32) -> Vec3 {
    let d = flat(from - target);
    let dir = if d.length_squared() > 0.0001 {
        d.normalize()
    } else {
        Vec3::X
    };
    let p = target + dir * radius;
    Vec3::new(
        p.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
        0.0,
        p.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
    )
}

/// Rough visual radius of a resource node (used for stand-off distance).
fn node_radius(kind: ResourceKind) -> f32 {
    match kind {
        ResourceKind::Gold => 3.0,
        ResourceKind::Lumber => 1.0,
    }
}

/// The other half of the resource pair — the fallback when one runs out globally.
fn other_resource(kind: ResourceKind) -> ResourceKind {
    match kind {
        ResourceKind::Gold => ResourceKind::Lumber,
        ResourceKind::Lumber => ResourceKind::Gold,
    }
}

/// Footprint terrain.rs blocks for each node type; we unblock the same on depletion.
fn node_block_size(kind: ResourceKind) -> f32 {
    match kind {
        ResourceKind::Gold => 6.0,
        ResourceKind::Lumber => 2.0,
    }
}

/// A building gets a production queue iff it can actually train something —
/// derived from the shared table, so new trainers (or new inert buildings like
/// Towers and Walls) need no change here.
fn is_production(kind: BuildingKind) -> bool {
    !trainable(kind).is_empty()
}

/// Snapshot of every team's COMPLETED buildings — the input to the tech tree.
/// Collected once per system run because `requirements_met` wants a `Clone`
/// iterator, which a live query iterator is not.
fn completed_kinds(
    query: &Query<(&Building, &Team), Without<UnderConstruction>>,
) -> Vec<(BuildingKind, Team)> {
    query.iter().map(|(b, t)| (b.kind, *t)).collect()
}

/// The kinds in that snapshot belonging to one team.
fn owned_by(
    owned: &[(BuildingKind, Team)],
    team: Team,
) -> impl Iterator<Item = BuildingKind> + Clone + '_ {
    owned
        .iter()
        .filter(move |(_, t)| *t == team)
        .map(|(kind, _)| *kind)
}

// ---------------------------------------------------------------------------
// 1. Building spawning
// ---------------------------------------------------------------------------

fn spawn_buildings(
    mut commands: Commands,
    mut events: EventReader<SpawnBuildingEvent>,
    mut nav: ResMut<NavGrid>,
    assets: Res<EconomyAssets>,
) {
    for ev in events.read() {
        let stats = building_stats(ev.kind);
        let pos = Vec3::new(ev.pos.x, 0.0, ev.pos.z);

        let mut tf = Transform::from_translation(pos);
        if !ev.completed {
            tf.scale.y = BUILD_START_SCALE;
        }

        let hp = if ev.completed {
            stats.hp
        } else {
            stats.hp * BUILD_START_HP
        };

        let root = commands
            .spawn((
                Building { kind: ev.kind },
                ev.team,
                Health {
                    current: hp,
                    max: stats.hp,
                },
                tf,
                // Same one-frame propagation hole as units (see units.rs):
                // GlobalTransform is only filled in during PostUpdate, so a
                // building spawned in Update reads as being at the world
                // origin for the rest of the frame. Buildings are roots, so
                // their GlobalTransform is just their Transform — seed it.
                GlobalTransform::from(tf),
                Visibility::default(),
            ))
            .id();

        spawn_body(&mut commands, &assets, root, ev.kind, ev.team);

        if !ev.completed {
            commands.entity(root).insert(UnderConstruction {
                remaining: stats.build_time,
            });
        }
        if is_production(ev.kind) {
            commands
                .entity(root)
                .insert((TrainingQueue::default(), PaidFront(None)));
        }

        nav.set_blocked_rect(pos, stats.size, true);
    }
}

/// Attach a kind's team-tinted procedural body to a building root. Used at
/// spawn and again when an upgrade swaps one silhouette for the next.
fn spawn_body(
    commands: &mut Commands,
    assets: &EconomyAssets,
    root: Entity,
    kind: BuildingKind,
    team: Team,
) {
    let palette = assets
        .palette(kind, team)
        .expect("team materials initialised in setup");
    let Some(parts) = assets.parts.get(&kind) else {
        return;
    };
    for part in parts {
        let child = commands
            .spawn((
                Mesh3d(part.mesh.clone()),
                MeshMaterial3d(palette[part.mat].clone()),
                part.tf,
            ))
            .id();
        commands.entity(root).add_child(child);
    }
}

// ---------------------------------------------------------------------------
// 2. Construction progress
// ---------------------------------------------------------------------------

fn construction_progress(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &Building,
        &mut UnderConstruction,
        &mut Transform,
        &mut Health,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, building, mut uc, mut tf, mut health) in &mut query {
        let stats = building_stats(building.kind);
        let build_time = stats.build_time.max(0.01);

        let step = dt.min(uc.remaining.max(0.0));
        uc.remaining -= dt;

        // Additive healing only — combat owns subtraction.
        let heal = stats.hp * (1.0 - BUILD_START_HP) * (step / build_time);
        health.current = (health.current + heal).min(health.max);

        if uc.remaining <= 0.0 {
            tf.scale.y = 1.0;
            commands.entity(entity).remove::<UnderConstruction>();
        } else {
            let frac = (1.0 - uc.remaining / build_time).clamp(0.0, 1.0);
            tf.scale.y = BUILD_START_SCALE + (1.0 - BUILD_START_SCALE) * frac;
        }
    }
}

// ---------------------------------------------------------------------------
// 2b. In-place upgrades (TownHall -> Keep -> Castle)
// ---------------------------------------------------------------------------
//
// An upgrade is a construction that happens to already have a building on the
// site. It keeps the entity, the position, the footprint, the rally point, the
// doctrine template and — deliberately — the training QUEUE: paying to tech up
// must never cost a player the four Footmen they had lined up. What it does
// cost is the TIME: `training_queues` skips an `Upgrading` building entirely,
// so the queue and its progress freeze on the spot and thaw untouched when the
// scaffolding comes down. That pause is the real price of teching mid-fight.
//
// Money changes hands once, here, the instant the order is accepted — unlike
// `Order::Build`, no worker has to walk anywhere first, so there is no window
// in which a training queue could spend the down payment.

/// `UpgradeBuilding` -> validate, pay, start the conversion.
///
/// Rejections are `debug!` and nothing else: ui.rs, bridge.rs and ai.rs all
/// pre-check, so anything that lands here failed because the world moved
/// between the request and this frame.
#[allow(clippy::too_many_arguments)]
fn start_upgrades(
    time: Res<Time>,
    mut commands: Commands,
    mut events: EventReader<UpgradeBuilding>,
    mut economies: ResMut<Economies>,
    mut feed: ResMut<GameEvents>,
    buildings: Query<(
        &Building,
        &Team,
        &Transform,
        Option<&UnderConstruction>,
        Option<&Upgrading>,
    )>,
) {
    for ev in events.read() {
        let Ok((building, team, tf, under, upgrading)) = buildings.get(ev.building) else {
            debug!("UpgradeBuilding: {:?} is not a building", ev.building);
            continue;
        };
        if under.is_some() {
            debug!("UpgradeBuilding: {:?} is still under construction", ev.building);
            continue;
        }
        if upgrading.is_some() {
            debug!("UpgradeBuilding: {:?} is already upgrading", ev.building);
            continue;
        }
        let Some(to) = building_upgrades_to(building.kind) else {
            debug!(
                "UpgradeBuilding: {} is at the top of its ladder",
                building_name(building.kind)
            );
            continue;
        };
        let (cost_gold, cost_lumber, upgrade_time) =
            upgrade_cost(building.kind).expect("a next tier implies a cost");
        if !economies.get_mut(*team).pay(cost_gold, cost_lumber) {
            debug!(
                "UpgradeBuilding: {:?} cannot afford {} ({cost_gold}g {cost_lumber}l)",
                team,
                building_name(to)
            );
            continue;
        }

        commands.entity(ev.building).try_insert(Upgrading {
            to,
            remaining: upgrade_time,
            total: upgrade_time,
        });
        let pos = flat(tf.translation);
        info!(
            "[{:?}] {} -> {} upgrade started at ({:.0},{:.0}) — {cost_gold}g {cost_lumber}l, \
             {upgrade_time:.0}s (training paused)",
            team,
            building_name(building.kind),
            building_name(to),
            pos.x,
            pos.z
        );
        // Own feed only. The enemy learns a base teched by looking at it.
        feed.push(
            *team,
            time.elapsed_secs(),
            format!("{} upgrade started @({:.1},{:.1})", building_name(to), pos.x, pos.z),
            EventSeverity::Info,
            Some(pos),
        );
    }
}

/// Tick every conversion; on completion swap the kind, the body and the HP
/// pool, and tell the owner.
#[allow(clippy::too_many_arguments)]
fn upgrade_progress(
    time: Res<Time>,
    mut commands: Commands,
    mut nav: ResMut<NavGrid>,
    assets: Res<EconomyAssets>,
    mut feed: ResMut<GameEvents>,
    mut query: Query<(
        Entity,
        &mut Building,
        &Team,
        &mut Upgrading,
        &mut Transform,
        &mut Health,
        Option<&Children>,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, mut building, team, mut upgrading, mut tf, mut health, children) in &mut query {
        upgrading.remaining -= dt;
        if upgrading.remaining > 0.0 {
            // Scaffolding: the roofline sits low and rises back as the work
            // finishes, so an upgrade in progress is visible on the field and
            // not only in the HUD.
            let frac = (1.0 - upgrading.remaining / upgrading.total.max(0.01)).clamp(0.0, 1.0);
            tf.scale.y = UPGRADE_SCAFFOLD_SCALE + (1.0 - UPGRADE_SCAFFOLD_SCALE) * frac;
            continue;
        }

        let from = building.kind;
        let to = upgrading.to;
        let old_stats = building_stats(from);
        let new_stats = building_stats(to);

        // Carry the damage across as a FRACTION: a hall at half health becomes
        // a Keep at half health. Upgrading is not a repair, and a player who
        // starts one under fire does not get to un-take the damage.
        let frac = if health.max > 0.0 {
            (health.current / health.max).clamp(0.0, 1.0)
        } else {
            1.0
        };
        health.max = new_stats.hp;
        health.current = new_stats.hp * frac;

        // Footprints are equal all the way up today, but re-blocking honestly
        // costs nothing and means a future ladder with a wider top rung works.
        if (new_stats.size - old_stats.size).abs() > f32::EPSILON {
            nav.set_blocked_rect(tf.translation, old_stats.size, false);
            nav.set_blocked_rect(tf.translation, new_stats.size, true);
        }

        // New silhouette: drop the old body, raise the new one.
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).try_despawn();
            }
        }
        building.kind = to;
        tf.scale.y = 1.0;
        spawn_body(&mut commands, &assets, entity, to, *team);

        // A rung that trains nothing would have to shed its queue; nothing on
        // today's ladder does, but deriving it keeps the invariant honest.
        if is_production(to) {
            if !is_production(from) {
                commands
                    .entity(entity)
                    .try_insert((TrainingQueue::default(), PaidFront(None)));
            }
        } else {
            commands
                .entity(entity)
                .try_remove::<TrainingQueue>()
                .try_remove::<PaidFront>();
        }

        commands.entity(entity).try_remove::<Upgrading>();
        let pos = flat(tf.translation);
        info!(
            "[{:?}] {} upgrade complete at ({:.0},{:.0}) — tier {}, {:.0} HP",
            team,
            building_name(to),
            pos.x,
            pos.z,
            building_tier(to),
            new_stats.hp
        );
        feed.push(
            *team,
            time.elapsed_secs(),
            format!("{} upgrade complete @({:.1},{:.1})", building_name(to), pos.x, pos.z),
            EventSeverity::Info,
            Some(pos),
        );
    }
}

// ---------------------------------------------------------------------------
// 2b. Research — the bank-to-power conversion
// ---------------------------------------------------------------------------
//
// Structurally identical to the tier-up above: an event asks, this module
// validates and pays, a component ticks, and completion is announced to the
// owner's feed only. The one difference is where the result lands — a tier-up
// changes the BUILDING, a research changes the TEAM, and the building it was
// bought at is thereafter irrelevant (it can be razed; the levels stay).

/// Take the money and start the clock. Re-validates everything intent.rs
/// checked, because an event can be stale by a frame and because ai.rs writes
/// this event directly — the same reason `start_upgrades` re-validates.
///
/// **This system, not the compiler, is the authority on "one job at a time."**
/// intent.rs checks `Researching` too, but that component is inserted through
/// `Commands` and therefore does not exist until the next flush — so a batch
/// that says `research attack` and `research armor` in the same frame passes
/// the compiler's check twice and arrives here as two events. Observed live
/// through the bridge: a five-command batch charged the team for three rungs
/// and delivered one. The two `claimed`/`started` sets below are what close
/// that, and they are the reason the money is spent here rather than there.
#[allow(clippy::too_many_arguments)]
fn start_research(
    time: Res<Time>,
    mut commands: Commands,
    mut events: EventReader<StartResearch>,
    mut economies: ResMut<Economies>,
    mut feed: ResMut<GameEvents>,
    research: Res<TeamResearch>,
    buildings: Query<(
        &Building,
        &Team,
        &Transform,
        Option<&UnderConstruction>,
        Option<&Upgrading>,
        Option<&Researching>,
    )>,
) {
    // Forges already given a job by an earlier event in THIS drain, and
    // (team, ladder) pairs already started in it. Both are needed: the first
    // stops one forge taking two jobs, the second stops two forges buying the
    // same level twice — `research` is read-only here, so without it both
    // would resolve `next_step` to the same rung and the team would pay
    // 100 + 100 for a level that should have cost 100 + 175.
    let mut claimed: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    let mut started: std::collections::HashSet<(Team, ResearchKind)> =
        std::collections::HashSet::new();
    for ev in events.read() {
        let Ok((building, team, tf, under, upgrading, busy)) = buildings.get(ev.building) else {
            debug!("StartResearch: {:?} is not a building", ev.building);
            continue;
        };
        if !building_researches(building.kind).contains(&ev.kind) {
            debug!(
                "StartResearch: {} cannot research {}",
                building_name(building.kind),
                ev.kind.id()
            );
            continue;
        }
        if under.is_some() || upgrading.is_some() {
            debug!("StartResearch: {:?} is not ready to work", ev.building);
            continue;
        }
        // `busy` is last frame's truth; `claimed` is this frame's. Both are
        // tested here and only RECORDED after the payment succeeds, so an
        // unaffordable request never locks a forge out for its own frame.
        if busy.is_some() || claimed.contains(&ev.building) {
            debug!("StartResearch: {:?} is already researching", ev.building);
            continue;
        }
        if started.contains(&(*team, ev.kind)) {
            debug!(
                "StartResearch: {:?} already started {} this frame",
                team,
                ev.kind.id()
            );
            continue;
        }
        // The level is resolved HERE, at the instant of payment, not on the
        // event — so two events arriving in one frame cannot both buy level 2.
        // (The second finds the forge busy on the next frame; a second forge
        // would find the level already advanced.)
        let Some(step) = research.get(*team).next_step(ev.kind) else {
            debug!(
                "StartResearch: {:?} {} is already at max level",
                team,
                ev.kind.id()
            );
            continue;
        };
        if !economies.get_mut(*team).pay(step.cost_gold, step.cost_lumber) {
            debug!(
                "StartResearch: {:?} cannot afford {} {} ({}g {}l)",
                team,
                ev.kind.id(),
                step.level,
                step.cost_gold,
                step.cost_lumber
            );
            continue;
        }

        // Paid for and committed: only now do the frame-local guards close.
        claimed.insert(ev.building);
        started.insert((*team, ev.kind));
        commands.entity(ev.building).try_insert(Researching {
            kind: ev.kind,
            to_level: step.level,
            remaining: step.research_time,
            total: step.research_time,
        });
        let pos = flat(tf.translation);
        info!(
            "[{:?}] research started: {} {} at ({:.0},{:.0}) — {}g {}l, {:.0}s",
            team,
            ev.kind.label(),
            step.level,
            pos.x,
            pos.z,
            step.cost_gold,
            step.cost_lumber,
            step.research_time
        );
        // Own feed only, exactly like a tier-up: the enemy learns you upgraded
        // by being hit harder, not by being told.
        feed.push(
            *team,
            time.elapsed_secs(),
            format!("{} {} research started", ev.kind.label(), step.level),
            EventSeverity::Info,
            Some(pos),
        );
    }
}

/// Tick every forge; on completion raise the team's level and announce it.
fn research_progress(
    time: Res<Time>,
    mut commands: Commands,
    mut research: ResMut<TeamResearch>,
    mut feed: ResMut<GameEvents>,
    mut query: Query<(Entity, &Team, &Transform, &mut Researching)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, team, tf, mut job) in &mut query {
        job.remaining -= dt;
        if job.remaining > 0.0 {
            continue;
        }
        commands.entity(entity).try_remove::<Researching>();
        // `advance` is the only writer of a research level in the whole
        // codebase, and it saturates — so even a duplicated completion cannot
        // push a ladder past its cap.
        let Some(level) = research.get_mut(*team).advance(job.kind) else {
            debug!(
                "research: {:?} {} completed but was already capped",
                team,
                job.kind.id()
            );
            continue;
        };
        let pos = flat(tf.translation);
        let bonus = research_bonus(job.kind, level);
        info!(
            "[{:?}] research complete: {} level {level} (+{bonus:.0}) — applies to every unit, \
             now and forever",
            team,
            job.kind.label()
        );
        feed.push(
            *team,
            time.elapsed_secs(),
            format!(
                "{} {level} complete: {} to every unit",
                job.kind.label(),
                match job.kind {
                    ResearchKind::Attack => format!("+{bonus:.0} damage"),
                    ResearchKind::Armor => format!("-{bonus:.0} damage taken"),
                }
            ),
            EventSeverity::Info,
            Some(pos),
        );
    }
}

// ---------------------------------------------------------------------------
// 3./4. Order dispatch — set up module-private job state on order changes
// ---------------------------------------------------------------------------

fn order_changed(
    mut commands: Commands,
    time: Res<Time>,
    mut feed: ResMut<GameEvents>,
    mut units: Query<
        (
            Entity,
            &Order,
            &Transform,
            &Team,
            Option<&mut HarvestJob>,
            Option<&RememberedNode>,
            Option<&Carrying>,
            // The site this worker was walking to before the new order
            // arrived, if there was one. Read only to say goodbye to it.
            Option<&BuildSite>,
        ),
        (Changed<Order>, With<Unit>),
    >,
    nodes: Query<(&ResourceNode, &Transform)>,
) {
    let now = time.elapsed_secs();
    for (entity, order, tf, team, job, remembered, carrying, site) in &mut units {
        // **The re-task window, spoken.** Every arm below that is not
        // `Order::Build` removes `BuildSite`, and until now it did so without
        // a word: the foundation simply stopped existing. A worker handed a new
        // `Order::Build` is not an abandonment of the *worker's* job in the
        // sense that matters — it is one build superseding another on the same
        // body, which is still one lost foundation, so it is reported too.
        if let Some(site) = site {
            // Re-issuing the SAME build on the same body is a no-op, not an
            // abandonment. `try_insert` marks `Order` changed whether or not
            // the value moved, and the scripted commander re-thinks every
            // second — without this guard the feed would carry a line a second
            // for a build that is going perfectly well. Nothing else about the
            // re-issue changes: the arm below still re-seats `BuildSite` and
            // the approach `MoveTo` exactly as it always did.
            let same_build = matches!(
                order,
                Order::Build { kind, pos } if *kind == site.kind && flat(*pos) == site.pos
            );
            if !same_build {
                let why = match order {
                    Order::Build { .. } => "the worker was given a different build",
                    Order::Harvest(_) | Order::ReturnResources => {
                        "the worker was sent back to gathering before breaking ground"
                    }
                    _ => "the worker was re-tasked before breaking ground",
                };
                report_abandoned_build(&mut feed, *team, now, site, why);
            }
        }
        match order {
            Order::Build { kind, pos } => {
                let site = Vec3::new(pos.x, 0.0, pos.z);
                let stats = building_stats(*kind);
                commands
                    .entity(entity)
                    .remove::<HarvestJob>()
                    .try_insert(BuildSite {
                        kind: *kind,
                        pos: site,
                        attempts: 1,
                    })
                    .try_insert(MoveTo {
                        target: approach_point(tf.translation, site, stats.size * 0.5 + 1.5),
                    });
            }
            Order::Harvest(node) => {
                let Ok((res, node_tf)) = nodes.get(*node) else {
                    // Node already gone — nothing to do.
                    commands
                        .entity(entity)
                        .remove::<HarvestJob>()
                        .try_insert(Order::Idle);
                    continue;
                };
                let node_pos = Vec3::new(node_tf.translation.x, 0.0, node_tf.translation.z);
                let kind = res.kind;
                // Already carrying a full load? Deliver first, then work this node.
                let phase = if carrying.is_some() {
                    HarvestPhase::ToDepot
                } else {
                    HarvestPhase::ToNode
                };
                commands
                    .entity(entity)
                    .remove::<BuildSite>()
                    .try_insert(HarvestJob {
                        node: Some(*node),
                        node_pos,
                        kind,
                        phase,
                        timer: 0.0,
                        attempts: 0,
                    })
                    .try_insert(RememberedNode {
                        node: Some(*node),
                        node_pos,
                        kind,
                    });
            }
            Order::ReturnResources => {
                commands.entity(entity).remove::<BuildSite>();
                if let Some(mut job) = job {
                    job.phase = HarvestPhase::ToDepot;
                    job.attempts = 0;
                    job.timer = 0.0;
                } else {
                    let (node, node_pos, kind) = match remembered {
                        Some(r) => (r.node, r.node_pos, r.kind),
                        None => (
                            None,
                            tf.translation,
                            carrying.map(|c| c.kind).unwrap_or(ResourceKind::Gold),
                        ),
                    };
                    commands.entity(entity).try_insert(HarvestJob {
                        node,
                        node_pos,
                        kind,
                        phase: HarvestPhase::ToDepot,
                        timer: 0.0,
                        attempts: 0,
                    });
                }
            }
            _ => {
                // Any other order abandons economy jobs (Carrying is kept).
                if let Some(job) = job {
                    commands.entity(entity).try_insert(RememberedNode {
                        node: job.node,
                        node_pos: job.node_pos,
                        kind: job.kind,
                    });
                }
                commands
                    .entity(entity)
                    .remove::<HarvestJob>()
                    .remove::<BuildSite>();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Order::Build follow-through
// ---------------------------------------------------------------------------

fn build_sites(
    mut commands: Commands,
    nav: Res<NavGrid>,
    time: Res<Time>,
    mut feed: ResMut<GameEvents>,
    mut economies: ResMut<Economies>,
    mut spawn_events: EventWriter<SpawnBuildingEvent>,
    mut workers: Query<(
        Entity,
        &Transform,
        &Team,
        &mut BuildSite,
        Option<&MoveTo>,
        // Dead-but-not-yet-despawned. Combat subtracts health in
        // `SimSet::Combat`, which is upstream of `SimSet::Economy`, and
        // `apply_death` despawns at the top of the NEXT frame — so this system
        // sees a killed builder exactly once, with its site still readable.
        // That one pass is the only chance anybody gets to say the foundation
        // died with the worker.
        &Health,
    )>,
    // Tech tree: only FINISHED buildings count toward requirements.
    completed: Query<(&Building, &Team), Without<UnderConstruction>>,
    races: Res<Races>,
) {
    let owned = completed_kinds(&completed);
    let now = time.elapsed_secs();

    for (entity, tf, team, mut site, moving, health) in &mut workers {
        if health.current <= 0.0 {
            report_abandoned_build(
                &mut feed,
                *team,
                now,
                &site,
                "the worker was killed before breaking ground",
            );
            continue;
        }
        if moving.is_some() {
            continue; // still walking
        }
        let stats = building_stats(site.kind);
        let dist = xz_dist(tf.translation, site.pos);

        if dist <= stats.size * 0.5 + 4.0 {
            let free = nav.rect_is_free(site.pos, stats.size);
            // Requirements are re-checked here, at the one place money changes
            // hands: the Barracks that unlocked this Tower may have died while
            // the worker walked over. Unmet -> refuse, exactly like being broke.
            let tech_ok = requirements_met(building_requires(site.kind), owned_by(&owned, *team));
            // ...and so is the ROSTER, at the same one place money changes
            // hands. The build card only ever offers a team its own race and
            // the intent compiler refuses the rest with an error string, but
            // this is the check that makes those two a convenience rather than
            // the rule: nothing anywhere can spend a Horde bank on a Barracks.
            let race_ok = race_has_building(races.get(*team), site.kind);
            let paid = free
                && tech_ok
                && race_ok
                && economies
                    .get_mut(*team)
                    .pay(stats.cost_gold, stats.cost_lumber);

            if paid {
                spawn_events.write(SpawnBuildingEvent {
                    kind: site.kind,
                    team: *team,
                    pos: site.pos,
                    completed: false,
                });
                // Step out of the footprint we are about to block.
                let out = approach_point(tf.translation, site.pos, stats.size * 0.5 + 2.5);
                commands.entity(entity).try_insert(MoveTo { target: out });
            } else {
                // **Arrived, and turned away.** This is r25-red's failure
                // exactly: the command was accepted, the worker walked the
                // whole way, and the site was refused at the pay-point with
                // nothing said. The compiler checked all four of these when the
                // sentence was written; the walk is where they go stale, and
                // this is the one place that knows which one did.
                let why = if !free {
                    "the ground was no longer clear when the worker arrived".to_string()
                } else if !race_ok {
                    "it is not in your roster".to_string()
                } else if !tech_ok {
                    "its requirements were no longer met when the worker arrived".to_string()
                } else {
                    format!(
                        "you could not afford it when the worker arrived ({}g {}l)",
                        stats.cost_gold, stats.cost_lumber
                    )
                };
                report_abandoned_build(&mut feed, *team, now, &site, &why);
            }
            commands
                .entity(entity)
                .remove::<BuildSite>()
                .try_insert(Order::Idle);
        } else if site.attempts < MAX_ATTEMPTS {
            site.attempts += 1;
            commands.entity(entity).try_insert(MoveTo {
                target: approach_point(tf.translation, site.pos, stats.size * 0.5 + 1.5),
            });
        } else {
            // Unreachable site — give up. The third silent window, and the one
            // a commander has least chance of guessing: the order was legal,
            // the worker set off, and the pathing never got there.
            report_abandoned_build(
                &mut feed,
                *team,
                now,
                &site,
                "the worker could not reach the site after several tries",
            );
            commands
                .entity(entity)
                .remove::<BuildSite>()
                .try_insert(Order::Idle);
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Harvest loop
// ---------------------------------------------------------------------------

/// Every standing (completed) building, which is how the harvest loop finds a
/// drop-off point and how it decides whose mine just ran out. Named because two
/// things now take it.
type HallQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Team,
        &'static Building,
    ),
    Without<UnderConstruction>,
>;

/// **A mine your hall works has hit zero.** One line, once, on the owning
/// team's own feed.
///
/// r23's ask, finished. The level has been right since a dry mine stopped
/// being despawned — `mines[].remaining` reads `0` in every snapshot from here
/// on, `TriggerWhen::MineDry` sees it, `income_alarm` counts it. What none of
/// those is, is an *interruption*: a commander parked in `bridge_wait` learns
/// its income ended whenever it next looks, which for an LLM seat is the length
/// of one whole decision. Levels are status, transitions are events
/// (BUILDER_BRIEF §6.11), and this transition happens exactly once in a match
/// per mine, in the one statement that takes the last gold out of it.
///
/// **There is no clearing edge and none is owed.** The rule in §6.11 is that an
/// entry edge obliges an exit edge because the reader would otherwise have to
/// poll to learn it recovered. A mine does not recover: `remaining` is
/// monotonically decreasing and zero is terminal, so there is nothing to wait
/// for and nothing to poll. What *can* recover is the income, and that is the
/// `IncomeCollapse` alarm's business — it already raises and clears on its own
/// grace window.
///
/// **"Yours" is geometry**, the same definition `TriggerWhen::MineDry` and
/// `alarm::income_alarm` use: a completed hall of yours inside
/// `MINE_HOME_RADIUS`. Mines are neutral; the hall you placed to work one is
/// the only honest reading of the mine you are losing. Both teams can qualify
/// for one mine (two expansions on one hole), and both are told, each on its
/// own feed — pushed to a team about that team's own hall, never to
/// `team.enemy()`.
///
/// Fog-legal by construction, and doubly so. The fact is about your own
/// building's neighbourhood, and the underlying number is public geography
/// anyway: bridge.rs ships `mines` (position and remaining) unfiltered to both
/// seats, as its header says. This event hands out no fact a snapshot did not
/// already carry; it changes only *when* the reader finds out.
fn announce_mine_dry(feed: &mut GameEvents, halls: &HallQuery, pos: Vec3, now: f32) {
    // A fixed team order, not the query's: two lines minted in one statement
    // must take their `seq` in the same order on every run of one seed.
    for team in [Team::Human, Team::Claude] {
        let works_it = halls.iter().any(|(hall_tf, hall_team, building)| {
            *hall_team == team
                && is_hall(building.kind)
                && xz_dist(hall_tf.translation, pos) <= MINE_HOME_RADIUS
        });
        if !works_it {
            continue;
        }
        feed.push(
            team,
            now,
            format!("the {} your hall works has run dry", mine_place_name(pos)),
            // Warning and not Critical: your income just took a real cut, which
            // is the severity's own definition ("something of yours is being
            // spent"), but nothing is burning and the answer — expand, or move
            // the crew — is a decision rather than a reflex.
            EventSeverity::Warning,
            Some(pos),
        );
    }
}

fn harvest_loop(
    time: Res<Time>,
    mut commands: Commands,
    mut nav: ResMut<NavGrid>,
    mut economies: ResMut<Economies>,
    // The exhaustion EDGE (shared.rs § "Levels are status; transitions are
    // events"). `mines[].remaining` is the level and it was always there; this
    // is the once-ever transition, so a seat asleep in `bridge_wait` wakes on
    // the frame its income ended instead of on its next poll.
    //
    // A third writer of `GameEvents` after `announce_bounty_claims` and
    // `produce_game_events`, and `seq` stays deterministic without a new edge
    // because this one is in `SimSet::Economy`, which `SIM_ORDER` already
    // chains ahead of `SimSet::Feed` where both of those live.
    mut feed: ResMut<GameEvents>,
    assets: Res<EconomyAssets>,
    mut workers: Query<(
        Entity,
        &Transform,
        &Team,
        &mut HarvestJob,
        Option<&MoveTo>,
        Option<&Carrying>,
        Option<&CarryVisual>,
    )>,
    mut nodes: Query<(Entity, &mut ResourceNode, &Transform)>,
    halls: HallQuery,
    hostiles: Query<(&Transform, &Team, &Unit)>,
) {
    let dt = time.delta_secs();

    // Enemy combat presence, snapshotted once: auto-rebalanced workers must
    // not be marched into an occupied mine (playtest: 9 workers dead in 2s
    // when the last mine on the map sat inside enemy lines). Explicit Harvest
    // orders from the commander still go wherever they're told.
    let combat_presence: Vec<(Vec3, Team)> = hostiles
        .iter()
        // "Dangerous to a worker" means it can actually shoot one. Flyers that
        // strafe the ground count and will chase a crew off a mine like
        // anything else; a hypothetical air-only interceptor would not.
        .filter(|(_, _, u)| !is_worker_kind(u.kind) && unit_stats(u.kind).can_hit_ground)
        .map(|(tf, t, _)| (tf.translation, *t))
        .collect();

    for (entity, tf, team, mut job, moving, carrying, carry_visual) in &mut workers {
        let pos = tf.translation;

        match job.phase {
            // ---------------------------------------------------------------
            HarvestPhase::ToNode => {
                // Re-target if the node is gone / depleted.
                let mut valid = false;
                if let Some(node_e) = job.node {
                    if let Ok((_, res, node_tf)) = nodes.get(node_e) {
                        if res.remaining > 0 {
                            job.node_pos = flat(node_tf.translation);
                            valid = true;
                        }
                    }
                }
                if !valid {
                    // Auto-rebalance: a depleted node sends the worker to the
                    // nearest live node of the SAME kind anywhere on the map —
                    // no radius cap, because a mined-out expansion used to leave
                    // its whole crew idling next to the hole while other mines
                    // still had gold. Only if that kind is exhausted map-wide do
                    // we retrain the worker onto the other resource; Idle means
                    // the map itself is empty.
                    job.node = None;
                    let nearest_of = |want: ResourceKind| {
                        let mut best: Option<(f32, Entity, Vec3)> = None;
                        for (cand, res, node_tf) in nodes.iter() {
                            if res.kind != want || res.remaining == 0 {
                                continue;
                            }
                            // Never auto-assign into enemy-held ground.
                            let dangerous = combat_presence.iter().any(|(p, t)| {
                                *t != *team
                                    && xz_dist(*p, node_tf.translation) < NODE_DANGER_RADIUS
                            });
                            if dangerous {
                                continue;
                            }
                            let d = xz_dist(node_tf.translation, pos);
                            if best.is_none_or(|(bd, _, _)| d < bd) {
                                best = Some((d, cand, flat(node_tf.translation)));
                            }
                        }
                        best
                    };
                    let picked = nearest_of(job.kind)
                        .map(|b| (job.kind, b))
                        .or_else(|| {
                            let other = other_resource(job.kind);
                            nearest_of(other).map(|b| (other, b))
                        });
                    match picked {
                        Some((kind, (_, cand, cand_pos))) => {
                            job.node = Some(cand);
                            job.node_pos = cand_pos;
                            job.kind = kind;
                            job.attempts = 0;
                            commands.entity(entity).try_insert(RememberedNode {
                                node: Some(cand),
                                node_pos: cand_pos,
                                kind,
                            });
                        }
                        None => {
                            // Map truly exhausted — nothing left to gather.
                            commands
                                .entity(entity)
                                .remove::<HarvestJob>()
                                .try_insert(Order::Idle);
                            continue;
                        }
                    }
                }

                if moving.is_some() {
                    continue;
                }
                // Must exceed the approach radius (+1.8) by more than the
                // pathfinder's arrival slop (1.5) plus separation jostling.
                let reach = node_radius(job.kind) + 4.5;
                if xz_dist(pos, job.node_pos) <= reach {
                    job.phase = HarvestPhase::Gathering;
                    job.timer = 0.0;
                    job.attempts = 0;
                } else if job.attempts < MAX_ATTEMPTS {
                    job.attempts += 1;
                    let target = approach_point(pos, job.node_pos, node_radius(job.kind) + 1.8);
                    commands.entity(entity).try_insert(MoveTo { target });
                } else {
                    commands
                        .entity(entity)
                        .remove::<HarvestJob>()
                        .try_insert(Order::Idle);
                }
            }

            // ---------------------------------------------------------------
            HarvestPhase::Gathering => {
                job.timer += dt;
                if job.timer < GATHER_TIME {
                    continue;
                }
                job.timer = 0.0;

                let Some(node_e) = job.node else {
                    job.phase = HarvestPhase::ToNode;
                    continue;
                };
                let Ok((_, mut res, node_tf)) = nodes.get_mut(node_e) else {
                    job.node = None;
                    job.phase = HarvestPhase::ToNode;
                    continue;
                };
                if res.remaining == 0 {
                    job.node = None;
                    job.phase = HarvestPhase::ToNode;
                    continue;
                }

                let kind = res.kind;
                let node_pos = flat(node_tf.translation);
                let amount = res.remaining.min(CARRY_AMOUNT);
                res.remaining -= amount;
                let depleted = res.remaining == 0;

                if depleted {
                    nav.set_blocked_rect(node_pos, node_block_size(kind), false);
                    match kind {
                        // A felled tree is gone. There are thousands of them,
                        // nothing asks after one by name, and a stump is not a
                        // fact anybody reasons about.
                        ResourceKind::Lumber => {
                            commands.entity(node_e).try_despawn();
                        }
                        // A mined-out gold mine STAYS, as a dry mine. Mines are
                        // geography: a fixed handful of named places, shipped in
                        // every snapshot with their remaining gold, and the
                        // thing a hall is placed to work. Half this codebase is
                        // already written against a dry mine you can look at —
                        // `TriggerWhen::MineDry` asks for a node with
                        // `remaining == 0`, `alarm::income_alarm` counts live
                        // mines against the mines near your halls to say "the
                        // one gold mine your hall works is dry",
                        // `intent::nearest_node` skips the empty ones, and the
                        // snapshot's `mines[].remaining` is documented in
                        // COMMANDER_BRIEF as the thing `mine_dry` saves you
                        // watching. Despawning it satisfied none of them:
                        // arena r23's blue armed the expand trigger, emptied
                        // both home mines, and the rule never fired because
                        // `remaining == 0` was never a state of the world — the
                        // entity died in the same statement that reached zero.
                        //
                        // Only the visuals collapse. The node keeps its
                        // Transform and its `ResourceNode`, so it is still a
                        // place with a reading; the ground it stood on was just
                        // unblocked above, so an expansion can be built over it
                        // exactly as before.
                        ResourceKind::Gold => {
                            commands.entity(node_e).despawn_related::<Children>();
                            announce_mine_dry(&mut feed, &halls, node_pos, time.elapsed_secs());
                        }
                    }
                    job.node = None;
                }

                // Carried-resource visual.
                if let Some(v) = carry_visual {
                    commands.entity(v.0).try_despawn();
                }
                let mat = match kind {
                    ResourceKind::Gold => assets.gold_mat.clone(),
                    ResourceKind::Lumber => assets.lumber_mat.clone(),
                };
                let bob = commands
                    .spawn((
                        Mesh3d(assets.carry_mesh.clone()),
                        MeshMaterial3d(mat),
                        // Local child coords on a UNIT_SCALE'd worker.
                        Transform::from_xyz(0.0, 1.05, 0.0),
                    ))
                    .id();
                commands.entity(entity).add_child(bob);
                commands
                    .entity(entity)
                    .try_insert(Carrying { kind, amount })
                    .try_insert(CarryVisual(bob));

                job.node_pos = node_pos;
                job.phase = HarvestPhase::ToDepot;
                job.attempts = 0;
            }

            // ---------------------------------------------------------------
            HarvestPhase::ToDepot => {
                let Some(load) = carrying.map(|c| (c.kind, c.amount)) else {
                    // Nothing to deliver — go back to work.
                    job.phase = HarvestPhase::ToNode;
                    job.attempts = 0;
                    continue;
                };

                // Any rung of the hall ladder takes a delivery: upgrading the
                // TownHall a worker crew depends on must not strand the crew.
                let mut best: Option<(f32, Vec3, f32)> = None;
                for (hall_tf, hall_team, building) in &halls {
                    if hall_team != team || !is_hall(building.kind) {
                        continue;
                    }
                    let d = xz_dist(pos, hall_tf.translation);
                    if best.map_or(true, |(bd, _, _)| d < bd) {
                        best = Some((
                            d,
                            flat(hall_tf.translation),
                            building_stats(building.kind).size,
                        ));
                    }
                }
                let Some((dist, hall_pos, hall_size)) = best else {
                    // No drop-off point; hold the load and idle.
                    commands
                        .entity(entity)
                        .remove::<HarvestJob>()
                        .try_insert(Order::Idle);
                    continue;
                };

                if dist <= hall_size * 0.5 + 5.0 {
                    let economy = economies.get_mut(*team);
                    match load.0 {
                        // Gold pays upkeep: big standing armies take a cut of
                        // every delivery. Lumber is untaxed (WC3-style).
                        ResourceKind::Gold => {
                            let taxed =
                                (load.1 as f32 * upkeep_rate(economy.supply_used)).round() as u32;
                            // Through `earn`, so the income meter sees exactly
                            // the gold the team actually banks — after upkeep,
                            // which is the number a runway is measured in.
                            economy.earn(taxed.max(1));
                        }
                        ResourceKind::Lumber => economy.lumber += load.1,
                    }
                    commands.entity(entity).remove::<Carrying>();
                    if let Some(v) = carry_visual {
                        commands.entity(v.0).try_despawn();
                        commands.entity(entity).remove::<CarryVisual>();
                    }
                    job.phase = HarvestPhase::ToNode;
                    job.attempts = 0;
                } else if moving.is_some() {
                    continue;
                } else if job.attempts < MAX_ATTEMPTS {
                    job.attempts += 1;
                    let target = approach_point(pos, hall_pos, hall_size * 0.5 + 1.5);
                    commands.entity(entity).try_insert(MoveTo { target });
                } else {
                    commands
                        .entity(entity)
                        .remove::<HarvestJob>()
                        .try_insert(Order::Idle);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Training queues
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn training_queues(
    time: Res<Time>,
    mut commands: Commands,
    nav: Res<NavGrid>,
    mut economies: ResMut<Economies>,
    mut feed: ResMut<GameEvents>,
    records: Res<HeroRecords>,
    // How many heroes each team may field: `hero_slots` of its live tier.
    tiers: Res<TechTiers>,
    mut spawn_events: EventWriter<SpawnUnitEvent>,
    mut buildings: Query<
        (
            Entity,
            &Building,
            &Team,
            &Transform,
            &mut TrainingQueue,
            Option<&mut PaidFront>,
            Option<&RallyPoint>,
            // Present while the building is converting to its next tier. NOT
            // filtered out of the query: a hall that already PAID for a hero
            // at its queue front still holds the team's one hero slot while it
            // upgrades, and dropping it from this iteration would let a second
            // hall pay for a second hero.
            Option<&Upgrading>,
        ),
        Without<UnderConstruction>,
    >,
    // Read-only, and heroes are units — never buildings — so this can't alias
    // the mutable building query above. `Unit` comes along because the rule is
    // now per CLASS as well as per count.
    living_heroes: Query<(&Team, &Unit, &Health), With<Hero>>,
    // Read-only view of the same buildings for the tech tree; reads never
    // conflict with the mutable queue access above.
    completed: Query<(&Building, &Team), Without<UnderConstruction>>,
) {
    let dt = time.delta_secs();
    let owned = completed_kinds(&completed);

    // Hero slots, counted across every training building. A CLASS is
    // "committed" to a team while it fields a living hero of that class, or
    // while some building has already PAID for one at its queue front. Without
    // the second half, two halls could each pay for a hero in the same frame.
    //
    // This replaces the old one-hero-per-team boolean with a list, because
    // slots now scale with the hall ladder (`hero_slots`): TownHall 1, Keep 2,
    // Castle 3. Two rules come out of the same list — the LENGTH is checked
    // against the slot count, and MEMBERSHIP is the class lock, which no
    // longer means "no second hero" but "no second hero of the same class".
    let mut hero_committed: Vec<(Team, UnitKind)> = living_heroes
        .iter()
        .filter(|(_, _, hp)| hp.current > 0.0)
        .map(|(t, u, _)| (*t, u.kind))
        .collect();
    for (_, _, team, _, queue, paid, _, _) in buildings.iter() {
        let Some(&front) = queue.queue.front() else { continue };
        // `covers`, not a bare flag: a payment left over from a CANCELLED hero
        // must not keep that class's slot hostage. It is refunded below, and
        // it holds nothing in the meantime.
        if is_hero_kind(front) && paid.is_some_and(|p| p.covers(front).is_some()) {
            hero_committed.push((*team, front));
        }
    }
    let held_by = |list: &[(Team, UnitKind)], team: Team| -> Vec<UnitKind> {
        list.iter()
            .filter(|(t, _)| *t == team)
            .map(|(_, k)| *k)
            .collect()
    };

    for (entity, building, team, tf, mut queue, paid, rally, upgrading) in &mut buildings {
        // Training PAUSES for the duration of an in-place upgrade. Nothing is
        // popped, nothing is refunded and `queue.progress` is left exactly
        // where it was, so the queue resumes mid-item when the scaffolding
        // comes down. `continue` before the progress tick is the whole
        // mechanism.
        if upgrading.is_some() {
            continue;
        }
        let paid_state = paid.map(|p| *p).unwrap_or(PaidFront(None));
        let front = queue.queue.front().copied();

        // **Refund an abandoned purchase.** Whatever we paid for is either
        // still at the front of the queue or it has been cancelled out from
        // under us; in the second case the money goes back to the treasury
        // before anything else happens this tick. This is the ONLY refund path
        // in the game, and it is here rather than in intent.rs's `cancel`
        // because economy.rs owns every payment in both directions.
        //
        // What comes back is what went out, which for a hero is the number
        // that matters: cancelling your free first Champion refunds 0, and
        // cancelling a revival refunds the whole revival price.
        if let Some(item) = paid_state.0.filter(|i| front != Some(i.kind)) {
            if item.gold > 0 || item.lumber > 0 {
                economies.get_mut(*team).refund(item.gold, item.lumber);
                feed.push(
                    *team,
                    time.elapsed_secs(),
                    format!(
                        "{} cancelled — {}g {}l refunded",
                        kind_name(item.kind),
                        item.gold,
                        item.lumber
                    ),
                    EventSeverity::Info,
                    Some(flat(tf.translation)),
                );
            }
            commands.entity(entity).try_insert(PaidFront(None));
        }

        let Some(front) = front else {
            queue.progress = 0.0;
            continue;
        };
        let mut paid_item = paid_state.covers(front);

        let stats = unit_stats(front);
        // Every hero class this team is holding — living heroes plus whatever
        // an earlier building in THIS pass has already paid for. One list,
        // asked twice: `hero_train_cost` prices off it and `hero_slot_check`
        // gates off it, which is how "the second hero in flight is not free"
        // and "the second hero in flight fills a slot" stay the same fact.
        let held = is_hero_kind(front)
            .then(|| held_by(&hero_committed, *team))
            .unwrap_or_default();
        // Heroes of either class are priced (and timed) by `hero_train_cost`:
        // free for the team's first ever, full fare for everything after it.
        let (cost_gold, cost_lumber, train_time) = if is_hero_kind(front) {
            hero_train_cost(&records, *team, front, &held)
        } else {
            (stats.cost_gold, stats.cost_lumber, stats.train_time)
        };

        if is_hero_kind(front) && paid_item.is_none() {
            // THE hero-slot rule, asked in the one place it lives. Both
            // refusals — a duplicate class, and a full slate — drop the item
            // unpaid so the queue keeps moving, exactly the treatment the old
            // one-hero rule gave it. The slot count is read from the team's
            // LIVE tier, so losing the Keep closes the second slot for FUTURE
            // heroes and never confiscates one already standing.
            if hero_slot_check(&held, front, tiers.get(*team)) != HeroSlotVerdict::Ok {
                queue.queue.pop_front();
                queue.progress = 0.0;
                continue;
            }
        }

        // Pay the moment this item becomes the active front item. If we can't
        // afford it (or supply is blocked) the item simply waits in the queue.
        if paid_item.is_none() {
            // Tech gate: an item whose requirements aren't met yet WAITS at the
            // front of the queue (like an unaffordable one) instead of being
            // dropped, so finishing the missing building resumes production.
            if !requirements_met(unit_requires(front), owned_by(&owned, *team)) {
                continue;
            }
            let economy = economies.get(*team);
            if economy.supply_used + stats.supply > economy.supply_cap {
                continue;
            }
            if !economies.get_mut(*team).pay(cost_gold, cost_lumber) {
                continue;
            }
            let item = PaidItem { kind: front, gold: cost_gold, lumber: cost_lumber };
            paid_item = Some(item);
            queue.progress = 0.0;
            commands.entity(entity).try_insert(PaidFront(Some(item)));
            if is_hero_kind(front) {
                // Later buildings in this same pass must see the commitment.
                hero_committed.push((*team, front));
                // The one purchase in the game whose price depends on the
                // team's history rather than on a table, logged with the
                // reason it cost what it cost. A tier-up and a research rung
                // already announce their price here; a hero is dearer than
                // either, and "was that one free?" was previously a question
                // no arena log could answer.
                let why = if records.get(*team, front).is_some() {
                    "revival, level preserved"
                } else if cost_gold == 0 && cost_lumber == 0 {
                    "free — this team's first hero"
                } else {
                    "second hero: the free one is already spent"
                };
                info!(
                    "[{:6.1}s] [{:?}] {} fielded at ({:.0},{:.0}) — \
                     {cost_gold}g {cost_lumber}l ({why})",
                    time.elapsed_secs(),
                    team,
                    kind_name(front),
                    tf.translation.x,
                    tf.translation.z,
                );
            }
        }

        queue.progress += dt;
        if queue.progress >= train_time {
            queue.queue.pop_front();
            queue.progress = 0.0;
            commands.entity(entity).try_insert(PaidFront(None));

            let size = building_stats(building.kind).size;
            let pos = free_spawn_spot(&nav, flat(tf.translation), size, is_flying_kind(front));
            spawn_events.write(SpawnUnitEvent {
                kind: front,
                team: *team,
                pos,
                // units.rs turns the building's rally into the unit's first order.
                rally: rally.map(|r| r.target),
                // …and stamps this building's `DoctrineTemplate`, if it has one.
                source: Some(entity),
                // Trained, not called.
                summoned: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Shop purchases
// ---------------------------------------------------------------------------

/// `BuyItem` -> validate, pay, fill a hero inventory slot.
///
/// This is the only place item gold changes hands. Every failure is a plain
/// debug log, never a warning: ui.rs and bridge.rs pre-validate affordability,
/// so a rejection here means the world moved between request and resolution
/// (shop died, hero died, treasury drained by a training queue that ran first)
/// — a race, not an error.
fn buy_items(
    mut events: EventReader<BuyItem>,
    mut economies: ResMut<Economies>,
    tiers: Res<TechTiers>,
    shops: Query<(&Building, &Team, Option<&UnderConstruction>)>,
    // Gated on `Inventory`, which units.rs only puts on heroes.
    mut heroes: Query<(&Team, &Health, &mut Inventory)>,
) {
    for ev in events.read() {
        let Ok((shop, shop_team, under)) = shops.get(ev.shop) else {
            debug!("BuyItem: {:?} is not a building", ev.shop);
            continue;
        };
        if shop.kind != BuildingKind::Shop {
            debug!("BuyItem: {:?} is a {:?}, not a Shop", ev.shop, shop.kind);
            continue;
        }
        if under.is_some() {
            debug!("BuyItem: shop {:?} is still under construction", ev.shop);
            continue;
        }

        let Ok((hero_team, health, mut inventory)) = heroes.get_mut(ev.hero) else {
            debug!("BuyItem: {:?} has no hero inventory", ev.hero);
            continue;
        };
        if hero_team != shop_team {
            debug!("BuyItem: {:?} does not own shop {:?}", ev.hero, ev.shop);
            continue;
        }
        if health.current <= 0.0 {
            debug!("BuyItem: hero {:?} is dead", ev.hero);
            continue;
        }
        let Some(slot) = inventory.0.iter().position(|s| s.is_none()) else {
            debug!("BuyItem: hero {:?} inventory is full", ev.hero);
            continue;
        };

        let def = item_def(ev.item);
        // The shelf is tiered. This is the authoritative check — the command
        // card greys the button and the bridge validator explains the refusal,
        // but a team that has just LOST its Castle stops being able to buy the
        // scroll here, on the same frame, without anyone telling it.
        if !item_unlocked(ev.item, tiers.get(*hero_team)) {
            debug!(
                "BuyItem: {:?} is {} but {} needs {}",
                hero_team,
                tiers.get(*hero_team).name(),
                def.name,
                def.tier.name()
            );
            continue;
        }
        if !economies.get_mut(*hero_team).pay(def.cost_gold, 0) {
            debug!(
                "BuyItem: {:?} cannot afford {} ({} gold)",
                hero_team, def.name, def.cost_gold
            );
            continue;
        }
        inventory.0[slot] = Some(ev.item);
        debug!(
            "BuyItem: {:?} bought {} for {} gold (slot {slot})",
            hero_team, def.name, def.cost_gold
        );
    }
}

/// A free-ish rally spot just outside the footprint, biased toward map center.
/// `flying` units skip the walkability search entirely — every spot is free
/// airspace, so a packed base can never stall an air factory the way it can
/// stall a barracks.
fn free_spawn_spot(nav: &NavGrid, center: Vec3, size: f32, flying: bool) -> Vec3 {
    let toward_center = {
        let d = flat(-center);
        if d.length_squared() > 0.0001 {
            d.normalize()
        } else {
            Vec3::X
        }
    };
    let mut fallback = center + toward_center * (size * 0.5 + 2.0);
    fallback = Vec3::new(
        fallback.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
        0.0,
        fallback.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
    );
    if flying {
        return fallback;
    }

    for ring in 0..3 {
        let radius = size * 0.5 + 2.0 + ring as f32 * 2.0;
        for i in 0..12 {
            // Sweep outward from the center-facing direction, alternating sides.
            let step = ((i + 1) / 2) as f32 * std::f32::consts::TAU / 12.0;
            let angle = if i % 2 == 0 { step } else { -step };
            let dir = Quat::from_rotation_y(angle) * toward_center;
            let spot = center + dir * radius;
            let spot = Vec3::new(
                spot.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
                0.0,
                spot.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0),
            );
            if nav.rect_is_free(spot, 1.5) {
                return spot;
            }
        }
    }
    fallback
}

// ---------------------------------------------------------------------------
// Tests: the training queue's payment ledger
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A world with one hall that can train, and a treasury we can read.
    /// `training_queues` is the only system under test — spawning is observed
    /// through the `SpawnUnitEvent` it writes, so nothing here needs units.rs,
    /// rendering, or a nav grid with real terrain.
    fn app_with_hall(team: Team, gold: u32, lumber: u32) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Economies>()
            .init_resource::<HeroRecords>()
            .init_resource::<TechTiers>()
            .init_resource::<NavGrid>()
            .init_resource::<GameEvents>()
            .add_event::<SpawnUnitEvent>()
            .add_systems(Update, training_queues);
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let e = economies.get_mut(team);
            e.gold = gold;
            e.lumber = lumber;
            e.supply_cap = 100;
            e.supply_used = 0;
        }
        let hall = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::TownHall },
                team,
                Transform::from_translation(Vec3::ZERO),
                TrainingQueue::default(),
                PaidFront(None),
            ))
            .id();
        (app, hall)
    }

    /// Advance the world by `secs`, in one tick.
    fn tick(app: &mut App, secs: f32) {
        let mut time = app.world_mut().resource_mut::<Time>();
        time.advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    fn queue(app: &mut App, hall: Entity, kinds: &[UnitKind]) {
        let mut q = app.world_mut().get_mut::<TrainingQueue>(hall).unwrap();
        for &k in kinds {
            q.queue.push_back(k);
        }
    }

    fn cancel(app: &mut App, hall: Entity, index: usize) {
        // Exactly what intent.rs's `cancel` verb does: edit the queue and
        // nothing else. Every consequence is economy.rs's to work out.
        let mut q = app.world_mut().get_mut::<TrainingQueue>(hall).unwrap();
        q.queue.remove(index);
        if index == 0 {
            q.progress = 0.0;
        }
    }

    fn queue_len(app: &App, hall: Entity) -> usize {
        app.world().get::<TrainingQueue>(hall).unwrap().queue.len()
    }
    fn gold(app: &App, team: Team) -> u32 {
        app.world().resource::<Economies>().get(team).gold
    }
    fn lumber(app: &App, team: Team) -> u32 {
        app.world().resource::<Economies>().get(team).lumber
    }

    /// **Your first hero of a class is free.** Not discounted, not deferred:
    /// the treasury is untouched, and the 25 seconds are still spent.
    #[test]
    fn a_first_hero_trains_free_and_still_takes_its_full_time() {
        for (hero, race) in [(UnitKind::Hero, "Kingdom"), (UnitKind::Warchief, "Horde")] {
            let team = Team::Human;
            let (mut app, hall) = app_with_hall(team, 0, 0);
            queue(&mut app, hall, &[hero]);

            // Zero gold, zero lumber, and it starts anyway.
            tick(&mut app, 0.5);
            assert_eq!(gold(&app, team), 0, "{race}: a free hero costs no gold");
            assert_eq!(lumber(&app, team), 0, "{race}: and no lumber");
            assert!(
                app.world().get::<PaidFront>(hall).unwrap().0.is_some(),
                "{race}: free still means PAID — the item is committed"
            );

            // Free is not instant. The train time is the row's, untouched.
            let train_time = unit_stats(hero).train_time;
            tick(&mut app, train_time - 2.0);
            assert_eq!(queue_len(&app, hall), 1, "{race}: free is not instant");
            tick(&mut app, 3.0);
            assert_eq!(queue_len(&app, hall), 0, "{race}: and it does finish");
        }
    }

    /// ...and the bill arrives on death. A class with a record costs its
    /// revival price, in gold AND lumber, on both rosters.
    #[test]
    fn reviving_a_recorded_hero_costs_the_revival_price_in_full() {
        for hero in [
            UnitKind::Hero,
            UnitKind::Priestess,
            UnitKind::Warchief,
            UnitKind::FarSeer,
        ] {
            let team = Team::Claude;
            let (mut app, hall) = app_with_hall(team, 1000, 1000);
            // A record is what "this class has died at least once" means.
            app.world_mut().resource_mut::<HeroRecords>().set(
                team,
                HeroRecord { level: 5, xp: 3.0, kind: hero },
            );
            queue(&mut app, hall, &[hero]);
            tick(&mut app, 0.5);

            let s = unit_stats(hero);
            assert_eq!(gold(&app, team), 1000 - s.revive_gold, "{hero:?} revival gold");
            assert_eq!(gold(&app, team), 600, "{hero:?} revival is 400g flat");
            assert_eq!(lumber(&app, team), 1000 - s.revive_lumber, "{hero:?} lumber");
            assert_eq!(lumber(&app, team), 900, "{hero:?} revival is 100l flat");
        }
    }

    /// **Cancelling a free hero refunds nothing, because nothing was paid.**
    /// The interesting half is that the treasury must not GAIN either — a
    /// zero-price item is still a paid item, and a refund path that handed
    /// back "the catalog price" would mint gold on every cancel.
    #[test]
    fn cancelling_a_free_hero_refunds_nothing() {
        let team = Team::Human;
        let (mut app, hall) = app_with_hall(team, 500, 500);
        queue(&mut app, hall, &[UnitKind::Hero]);
        tick(&mut app, 0.5);
        assert_eq!(gold(&app, team), 500, "nothing was paid");

        cancel(&mut app, hall, 0);
        tick(&mut app, 0.5);
        assert_eq!(gold(&app, team), 500, "so nothing comes back");
        assert_eq!(lumber(&app, team), 500);
        assert!(app.world().get::<PaidFront>(hall).unwrap().0.is_none());
    }

    /// **Cancelling a revival refunds the revival price**, exactly and once.
    /// This is now the largest single refundable purchase in the game, and the
    /// r18 ledger records a commander queueing a hero and cancelling it
    /// mid-push for line units — so it is a button that gets pressed.
    #[test]
    fn cancelling_a_revival_refunds_the_whole_revival_price() {
        let team = Team::Human;
        let (mut app, hall) = app_with_hall(team, 500, 500);
        app.world_mut().resource_mut::<HeroRecords>().set(
            team,
            HeroRecord { level: 4, xp: 1.0, kind: UnitKind::Hero },
        );
        queue(&mut app, hall, &[UnitKind::Hero]);
        tick(&mut app, 0.5);
        assert_eq!((gold(&app, team), lumber(&app, team)), (100, 400), "charged");

        cancel(&mut app, hall, 0);
        tick(&mut app, 0.5);
        assert_eq!(
            (gold(&app, team), lumber(&app, team)),
            (500, 500),
            "the whole revival price comes back"
        );
        // ...and only once, however many ticks pass.
        tick(&mut app, 5.0);
        assert_eq!((gold(&app, team), lumber(&app, team)), (500, 500), "not twice");
    }

    /// **A cancelled payment is never inherited by the next item in the
    /// queue.** This was a live exploit: `PaidFront` was a bare bool, so
    /// cancelling a paid Footman left the flag standing and whatever came next
    /// trained for free — and, because the hero-slot check also sits behind
    /// `if !paid_front`, a hero sliding into that slot skipped the slot rule
    /// on the way past.
    #[test]
    fn a_cancelled_payment_does_not_buy_the_next_item() {
        let team = Team::Human;
        let footman = unit_stats(UnitKind::Footman).cost_gold;
        let (mut app, hall) = app_with_hall(team, 500, 500);
        app.world_mut().resource_mut::<HeroRecords>().set(
            team,
            HeroRecord { level: 6, xp: 9.0, kind: UnitKind::Hero },
        );
        queue(&mut app, hall, &[UnitKind::Footman, UnitKind::Hero]);
        tick(&mut app, 0.5);
        assert_eq!(gold(&app, team), 500 - footman, "the Footman paid");

        cancel(&mut app, hall, 0);
        tick(&mut app, 0.5);
        // Footman money back, then the revival charged on its own merits.
        assert_eq!(
            gold(&app, team),
            100,
            "the Champion must buy its own revival, not ride the Footman's receipt"
        );
        assert_eq!(lumber(&app, team), 400);
    }

    /// Cancelling one of two identical queued units is not an abandoned
    /// purchase — one body was bought and one body is being built, so the
    /// receipt carries across and no refund is due.
    #[test]
    fn cancelling_one_of_two_identical_units_carries_the_receipt() {
        let team = Team::Human;
        let cost = unit_stats(UnitKind::Footman).cost_gold;
        let (mut app, hall) = app_with_hall(team, 500, 500);
        queue(&mut app, hall, &[UnitKind::Footman, UnitKind::Footman]);
        tick(&mut app, 0.5);
        assert_eq!(gold(&app, team), 500 - cost);

        cancel(&mut app, hall, 0);
        tick(&mut app, 0.5);
        assert_eq!(
            gold(&app, team),
            500 - cost,
            "one paid, one building — refunding here would make units free"
        );
    }

    /// A broke team cannot revive, and that is the whole mechanism: the hero
    /// waits at the front of the queue instead of being dropped, so income
    /// resumes it. The free FIRST hero, by contrast, never waits for anything.
    #[test]
    fn a_revival_waits_for_money_that_a_first_hero_never_needed() {
        let team = Team::Human;
        let (mut app, hall) = app_with_hall(team, 0, 0);
        app.world_mut().resource_mut::<HeroRecords>().set(
            team,
            HeroRecord { level: 2, xp: 0.0, kind: UnitKind::Hero },
        );
        queue(&mut app, hall, &[UnitKind::Hero]);
        tick(&mut app, 30.0);
        assert_eq!(queue_len(&app, hall), 1, "no money, no revival — but still queued");
        assert!(app.world().get::<PaidFront>(hall).unwrap().0.is_none());

        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let e = economies.get_mut(team);
            e.gold = 400;
            e.lumber = 100;
        }
        tick(&mut app, HERO_REVIVE_TIME + 1.0);
        assert_eq!(queue_len(&app, hall), 0, "paid, and out it comes");
        assert_eq!((gold(&app, team), lumber(&app, team)), (0, 0));
    }

    /// **The waiver is one per team, and the pay-point is where that is true.**
    /// A Keep opens a second slot; it does not open a second free hero. The
    /// first class is standing (queued, in this harness — see the queue-edge
    /// test below for why that is the same thing), and the second class is
    /// charged 400g/100l on both rosters.
    ///
    /// This is the spike the bead exists to kill: after the hall ladder started
    /// granting slots, teching to a Keep quietly posted a fully-levelling
    /// Priestess to your army for nothing.
    #[test]
    fn a_second_hero_class_costs_the_full_fielding_price_on_both_rosters() {
        for (first, second, race) in [
            (UnitKind::Hero, UnitKind::Priestess, "Kingdom"),
            (UnitKind::Warchief, UnitKind::FarSeer, "Horde"),
        ] {
            let team = Team::Human;
            let (mut app, hall) = app_with_hall(team, 1000, 1000);
            // A Keep's worth of slots, which is the only thing that makes a
            // second hero legal at all.
            app.world_mut()
                .resource_mut::<TechTiers>()
                .set(team, TechTier::T2);

            queue(&mut app, hall, &[first, second]);
            tick(&mut app, 0.5);
            assert_eq!(
                (gold(&app, team), lumber(&app, team)),
                (1000, 1000),
                "{race}: the FIRST hero is still free"
            );

            // Out comes the first — and in the real frame order it is standing
            // on the map before `training_queues` runs again (`spawn_units` is
            // in `SimSet::Movement`, which precedes `SimSet::Economy`), so the
            // harness puts the body down by hand. Nothing else here needs
            // units.rs.
            tick(&mut app, unit_stats(first).train_time + 1.0);
            assert_eq!(queue_len(&app, hall), 1, "{race}: the first one is out");
            app.world_mut().spawn((
                Unit { kind: first },
                Hero::from_record(None),
                Health::new(100.0),
                team,
                Transform::from_translation(Vec3::ZERO),
            ));

            // Now the second, at full fare.
            tick(&mut app, 0.5);
            let s = unit_stats(second);
            assert_eq!(
                (gold(&app, team), lumber(&app, team)),
                (1000 - s.revive_gold, 1000 - s.revive_lumber),
                "{race}: the second class pays its own fielding price"
            );
            assert_eq!(
                (gold(&app, team), lumber(&app, team)),
                (600, 900),
                "{race}: 400g/100l — the same number its revival costs"
            );
            // ...on a fresh hero's clock, not a revival's: nothing was bought
            // back, so `train_time` is what it takes.
            tick(&mut app, HERO_REVIVE_TIME + 1.0);
            assert_eq!(
                queue_len(&app, hall),
                1,
                "{race}: a second hero is TRAINED, not revived — full train time"
            );
            tick(&mut app, s.train_time);
            assert_eq!(queue_len(&app, hall), 0, "{race}: and then it finishes");
        }
    }

    /// **The queue edge: a first hero still in training has already spent the
    /// waiver.** It has no `HeroRecord` — nothing has spawned — so the only
    /// witness to its existence is the in-flight list, and the price rule reads
    /// that list for the same reason the slot rule does. Two halls queueing a
    /// hero each in the same breath must not both come out free.
    #[test]
    fn a_hero_still_in_training_already_prices_the_next_one() {
        let team = Team::Claude;
        let (mut app, hall_a) = app_with_hall(team, 1000, 1000);
        app.world_mut()
            .resource_mut::<TechTiers>()
            .set(team, TechTier::T2);
        let hall_b = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::TownHall },
                team,
                Transform::from_translation(Vec3::new(20.0, 0.0, 0.0)),
                TrainingQueue::default(),
                PaidFront(None),
            ))
            .id();

        queue(&mut app, hall_a, &[UnitKind::Hero]);
        queue(&mut app, hall_b, &[UnitKind::Priestess]);
        tick(&mut app, 0.5);

        // Both are in flight, neither is alive, and no record exists for
        // either. Exactly one of them was free.
        assert!(app.world().resource::<HeroRecords>().list(team).is_empty());
        assert_eq!(
            (gold(&app, team), lumber(&app, team)),
            (600, 900),
            "one free hero and one at 400g/100l — not two free heroes"
        );
        assert!(app.world().get::<PaidFront>(hall_a).unwrap().0.is_some());
        assert!(app.world().get::<PaidFront>(hall_b).unwrap().0.is_some());
    }

    /// A team whose only hero is DEAD has still had one. The waiver is spent by
    /// having fielded a hero, not by having one — so a fresh second class after
    /// a funeral is full fare, and so is buying the dead one back. There is no
    /// order of operations that gets a team two free heroes.
    #[test]
    fn a_dead_first_hero_leaves_no_freebie_behind() {
        let team = Team::Human;
        let (mut app, hall) = app_with_hall(team, 1000, 1000);
        app.world_mut().resource_mut::<HeroRecords>().set(
            team,
            HeroRecord { level: 3, xp: 2.0, kind: UnitKind::Hero },
        );
        // Nothing is held — the Champion is dead — so the Priestess is legal
        // even at a TownHall's single slot. She is not, however, free.
        queue(&mut app, hall, &[UnitKind::Priestess]);
        tick(&mut app, 0.5);
        assert_eq!(
            (gold(&app, team), lumber(&app, team)),
            (600, 900),
            "the team has had a hero; the next one is bought"
        );
    }

    // -----------------------------------------------------------------------
    // Tests: the build lifecycle between "accepted" and "ground broken"
    // (wc3clone-phc, from arena r25/r26)
    // -----------------------------------------------------------------------

    /// A world where a build can be ordered, walked and lost: the two systems
    /// that own the window, and nothing else.
    fn app_with_builds(gold: u32, lumber: u32) -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<NavGrid>()
            .init_resource::<Economies>()
            .init_resource::<Races>()
            .init_resource::<GameEvents>()
            .add_event::<SpawnBuildingEvent>()
            .add_systems(Update, (order_changed, build_sites).chain());
        {
            let mut economies = app.world_mut().resource_mut::<Economies>();
            let e = economies.get_mut(Team::Human);
            e.gold = gold;
            e.lumber = lumber;
        }
        app
    }

    fn builder_at(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Unit { kind: UnitKind::Worker },
                Team::Human,
                Transform::from_translation(at),
                Order::Idle,
                Health::new(50.0),
            ))
            .id()
    }

    /// Order a Farm and let `order_changed` install the site. Returns once the
    /// worker is walking.
    fn order_a_farm(app: &mut App, worker: Entity, site: Vec3) {
        app.world_mut().entity_mut(worker).insert(Order::Build {
            kind: BuildingKind::Farm,
            pos: site,
        });
        app.update();
        assert!(
            app.world().get::<BuildSite>(worker).is_some(),
            "the order must have become a site"
        );
    }

    fn abandonments(app: &App) -> Vec<String> {
        feed_lines(app, Team::Human)
            .into_iter()
            .filter(|l| l.starts_with("build abandoned"))
            .collect()
    }

    /// **The r26-blue failure, spoken.** A worker re-tasked before it breaks
    /// ground used to lose the foundation in complete silence — the whole
    /// diagnostic difficulty of the bead. Now it says so, and it says the true
    /// economics: nothing was spent, because economy.rs pays at the site.
    #[test]
    fn a_retasked_builder_says_the_foundation_is_gone_and_that_nothing_was_spent() {
        let mut app = app_with_builds(1000, 1000);
        let worker = builder_at(&mut app, Vec3::ZERO);
        order_a_farm(&mut app, worker, Vec3::new(0.0, 0.0, 30.0));
        assert_eq!(gold(&app, Team::Human), 1000, "a build charges on arrival");
        assert!(abandonments(&app).is_empty(), "ordering one is not losing one");

        // The re-task: any other order at all.
        app.world_mut().entity_mut(worker).insert(Order::Idle);
        app.update();

        let said = abandonments(&app);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("build abandoned: Farm at (0, 30)"), "{}", said[0]);
        assert!(said[0].contains("re-tasked before breaking ground"), "{}", said[0]);
        assert!(said[0].contains("nothing was spent"), "{}", said[0]);
        assert_eq!(
            gold(&app, Team::Human),
            1000,
            "and the sentence is true — there is nothing to refund"
        );
        assert!(app.world().get::<BuildSite>(worker).is_none());
    }

    /// **The r25-red failure, spoken.** Accepted, walked the whole way, refused
    /// at the pay-point — three times, with no error. The line names which of
    /// the four re-checks went stale, because "it did not work" is the part the
    /// commander could already see.
    #[test]
    fn a_build_refused_on_arrival_names_the_reason_it_went_stale() {
        let mut app = app_with_builds(0, 0);
        let site = Vec3::new(0.0, 0.0, 4.0);
        let worker = builder_at(&mut app, site);
        order_a_farm(&mut app, worker, site);
        // Standing on the site: drop the approach walk and let it arrive.
        app.world_mut().entity_mut(worker).remove::<MoveTo>();
        app.update();

        let said = abandonments(&app);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("could not afford it"), "{}", said[0]);
        let stats = building_stats(BuildingKind::Farm);
        assert!(
            said[0].contains(&format!("{}g {}l", stats.cost_gold, stats.cost_lumber)),
            "{}",
            said[0]
        );
        assert!(app.world().get::<BuildSite>(worker).is_none());
    }

    /// A build that WORKS says nothing. The channel is edge-triggered and the
    /// edge is the loss; a foundation going up is what `buildings[]` is for.
    #[test]
    fn a_build_that_breaks_ground_is_not_an_abandonment() {
        let mut app = app_with_builds(1000, 1000);
        let site = Vec3::new(0.0, 0.0, 4.0);
        let worker = builder_at(&mut app, site);
        order_a_farm(&mut app, worker, site);
        app.world_mut().entity_mut(worker).remove::<MoveTo>();
        app.update();

        assert!(abandonments(&app).is_empty(), "{:?}", abandonments(&app));
        let stats = building_stats(BuildingKind::Farm);
        assert_eq!(
            gold(&app, Team::Human),
            1000 - stats.cost_gold,
            "ground-break is where the money goes"
        );
    }

    /// A builder killed on the way loses the foundation too, and that is the
    /// one case nobody can infer: the worker is simply not in `units[]` next
    /// poll, and the build was never in `buildings[]` at all.
    #[test]
    fn a_builder_killed_on_the_walk_still_reports_its_lost_site() {
        let mut app = app_with_builds(1000, 1000);
        let worker = builder_at(&mut app, Vec3::ZERO);
        order_a_farm(&mut app, worker, Vec3::new(0.0, 0.0, 30.0));
        app.world_mut().get_mut::<Health>(worker).unwrap().current = 0.0;
        app.update();

        let said = abandonments(&app);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("killed before breaking ground"), "{}", said[0]);
    }

    /// **Not every `Changed<Order>` is an abandonment.** The scripted commander
    /// re-issues the same build every think, and `try_insert` marks the
    /// component changed whether or not the value moved. A line a second for a
    /// build that is going fine is the fire-hose r17 lost a match to
    /// (tools/BUILDER_BRIEF.md §6.11).
    #[test]
    fn re_issuing_the_same_build_is_not_an_abandonment() {
        let mut app = app_with_builds(1000, 1000);
        let worker = builder_at(&mut app, Vec3::ZERO);
        let site = Vec3::new(0.0, 0.0, 30.0);
        order_a_farm(&mut app, worker, site);
        for _ in 0..3 {
            app.world_mut().entity_mut(worker).insert(Order::Build {
                kind: BuildingKind::Farm,
                pos: site,
            });
            app.update();
        }
        assert!(abandonments(&app).is_empty(), "{:?}", abandonments(&app));

        // A DIFFERENT build on the same body is one lost foundation, and says so.
        app.world_mut().entity_mut(worker).insert(Order::Build {
            kind: BuildingKind::Farm,
            pos: Vec3::new(30.0, 0.0, 0.0),
        });
        app.update();
        let said = abandonments(&app);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("given a different build"), "{}", said[0]);
    }

    // -----------------------------------------------------------------------
    // Tests: what is left behind when a node runs out
    // -----------------------------------------------------------------------

    /// A world where a worker can actually empty a node: the real
    /// `harvest_loop` over a nav grid and a treasury, and nothing else.
    fn app_with_harvest() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<NavGrid>()
            .init_resource::<Economies>()
            .init_resource::<EconomyAssets>()
            .init_resource::<GameEvents>()
            .add_systems(Update, harvest_loop);
        app
    }

    /// Every line on one team's feed, oldest first.
    fn feed_lines(app: &App, team: Team) -> Vec<String> {
        app.world()
            .resource::<GameEvents>()
            .feed(team)
            .iter()
            .map(|e| e.message.clone())
            .collect()
    }

    fn spawn_hall(app: &mut App, team: Team, at: Vec3) {
        app.world_mut().spawn((
            Building { kind: BuildingKind::TownHall },
            team,
            Transform::from_translation(at),
            Health::new(1500.0),
        ));
    }

    fn spawn_node(app: &mut App, kind: ResourceKind, remaining: u32, pos: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                ResourceNode::full(kind, remaining),
                Transform::from_translation(pos),
            ))
            .id()
    }

    /// A worker mid-swing at `node`, one gather short of emptying it.
    fn swinging_at(app: &mut App, team: Team, node: Entity, kind: ResourceKind, pos: Vec3) {
        app.world_mut().spawn((
            team,
            Transform::from_translation(pos),
            HarvestJob {
                node: Some(node),
                node_pos: pos,
                kind,
                phase: HarvestPhase::Gathering,
                timer: 0.0,
                attempts: 0,
            },
        ));
    }

    /// **A mined-out gold mine stays on the board; a felled tree does not.**
    ///
    /// The two are different kinds of thing. A tree is scenery that is consumed;
    /// a gold mine is *geography* — a named place, a fixed position every
    /// snapshot ships, and the thing a hall was placed to work. "That mine is
    /// dry" is a fact about the map that every reader of the world needs to be
    /// able to observe: `TriggerWhen::MineDry` asks for a node with
    /// `remaining == 0`, `alarm::income_alarm` counts live mines against mines
    /// near home, `intent::nearest_node` skips the empty ones, and the bridge
    /// snapshot ships `mines[].remaining`. All four are written against a dry
    /// mine that is still there to be looked at. Despawning it does not make it
    /// dry, it makes it *absent*, and absence is a different sentence.
    #[test]
    fn an_emptied_mine_stays_on_the_board_and_an_emptied_tree_does_not() {
        let mut app = app_with_harvest();
        let at_mine = Vec3::new(-60.0, 0.0, -60.0);
        let at_tree = Vec3::new(10.0, 0.0, 10.0);
        let mine = spawn_node(&mut app, ResourceKind::Gold, CARRY_AMOUNT, at_mine);
        let tree = spawn_node(&mut app, ResourceKind::Lumber, CARRY_AMOUNT, at_tree);
        swinging_at(&mut app, Team::Human, mine, ResourceKind::Gold, at_mine);
        swinging_at(&mut app, Team::Human, tree, ResourceKind::Lumber, at_tree);

        tick(&mut app, GATHER_TIME + 0.1);

        let dry = app.world().get::<ResourceNode>(mine);
        assert!(
            dry.is_some(),
            "the mine ran dry — it did not stop being a place"
        );
        assert_eq!(dry.unwrap().remaining, 0, "and it reads as dry");
        assert!(
            app.world().get::<ResourceNode>(tree).is_none(),
            "a felled tree is gone; there are thousands of them and nothing asks after one"
        );
    }

    /// **r23, the whole bug in one test.** Blue armed
    /// `when a mine at our base runs dry: build a TownHall`, watched both home
    /// mines empty, and the rule sat `armed` to the end of the match. The
    /// predicate was right and the world never showed it a dry mine: the node
    /// was despawned in the same statement that took its last gold, so the only
    /// state that could have satisfied `remaining == 0` existed for zero frames
    /// of the evaluator's sweep.
    ///
    /// The two systems are the two ends of that seam — `harvest_loop` empties
    /// the mine in `SimSet::Economy`, `evaluate_triggers` sweeps in the next
    /// frame's `SimSet::Think` — so the test runs both and ticks twice, which
    /// is exactly the distance the real schedule puts between them.
    #[test]
    fn the_mine_running_dry_at_our_hall_fires_the_expand_trigger() {
        let mut app = app_with_harvest();
        app.init_resource::<Races>()
            .init_resource::<Triggers>()
            .init_resource::<Regions>()
            .init_resource::<TechTiers>()
            .init_resource::<GameEvents>()
            .add_event::<SubmitIntent>()
            .add_systems(Update, crate::trigger::evaluate_triggers);
        app.insert_resource(FogGrids::test_dark());

        // Blue's opening geometry: a finished hall, and the mine it was placed
        // to work, well inside `MINE_HOME_RADIUS`.
        let hall = Vec3::new(-66.0, 0.0, -66.0);
        let at_mine = Vec3::new(-60.0, 0.0, -60.0);
        spawn_hall(&mut app, Team::Human, hall);
        let mine = spawn_node(&mut app, ResourceKind::Gold, CARRY_AMOUNT, at_mine);
        swinging_at(&mut app, Team::Human, mine, ResourceKind::Gold, at_mine);

        app.world_mut()
            .resource_mut::<Triggers>()
            .get_mut(Team::Human)
            .push(TriggerRule {
                name: TriggerName::new("expand").expect("legal name"),
                when: TriggerWhen::MineDry,
                then: Intent::Stop { units: vec![], select: None },
                repeat: None,
                source: IntentSource::Bridge,
                armed: true,
                last_fired: None,
            });

        tick(&mut app, GATHER_TIME + 0.1); // the mine empties
        tick(&mut app, 0.1); // the sweep reads the world it left

        let fired: Vec<SubmitIntent> = app
            .world_mut()
            .resource_mut::<Events<SubmitIntent>>()
            .drain()
            .collect();
        assert_eq!(
            fired.len(),
            1,
            "the mine at our hall ran dry and the rule was armed for exactly that"
        );
        assert!(
            !app.world().resource::<Triggers>().get(Team::Human)[0].armed,
            "a once-trigger spends itself when it fires"
        );
    }

    /// **The edge, not just the level** (wc3clone-q90, r23's AAR).
    ///
    /// The mine reading `0` is a status a reader has to go and look at. The
    /// moment it hits zero is an interruption, and it is the interruption that
    /// wakes `bridge_wait` — which is the whole point, because the commander
    /// this was written for was asleep in exactly that call while its income
    /// ended.
    #[test]
    fn a_mine_your_hall_works_announces_the_moment_it_runs_dry() {
        let mut app = app_with_harvest();
        let at_mine = Vec3::new(-60.0, 0.0, -60.0);
        spawn_hall(&mut app, Team::Human, Vec3::new(-66.0, 0.0, -66.0));
        let mine = spawn_node(&mut app, ResourceKind::Gold, CARRY_AMOUNT * 2, at_mine);
        swinging_at(&mut app, Team::Human, mine, ResourceKind::Gold, at_mine);

        // Two loads, so the level falls before it hits the floor — and while
        // it is merely falling, nothing is said. A mine with gold in it is not
        // news. (The whole gather/deliver/return cycle runs, which is why this
        // counts frames rather than assuming a fixed number of them.)
        let mut emptied = false;
        for step in 0..12 {
            let left = app.world().get::<ResourceNode>(mine).map_or(0, |n| n.remaining);
            if left == 0 {
                emptied = true;
                break;
            }
            assert!(
                feed_lines(&app, Team::Human).is_empty(),
                "step {step}: {left}g still in the ground is not an event: {:?}",
                feed_lines(&app, Team::Human)
            );
            tick(&mut app, GATHER_TIME + 0.1);
        }
        assert!(emptied, "the worker never emptied the mine");

        assert_eq!(
            feed_lines(&app, Team::Human),
            vec!["the southwest mine your hall works has run dry".to_string()],
            "the transition to zero is announced once, naming the place"
        );

        // And it stays once. The mine is still on the board reading `0`, so a
        // level-triggered producer would re-say this every frame forever.
        tick(&mut app, GATHER_TIME * 3.0);
        assert_eq!(
            feed_lines(&app, Team::Human).len(),
            1,
            "an edge fires on the transition, not for as long as it is true"
        );
    }

    /// Whose mine it was is geometry, and the enemy is not told. A hall of
    /// theirs forty units away would earn them their own line; a hall across
    /// the map earns them nothing, because the mine they are losing is the one
    /// their hall was placed to work.
    #[test]
    fn the_exhaustion_line_goes_to_the_hall_that_works_the_mine_and_no_further() {
        let mut app = app_with_harvest();
        let at_mine = Vec3::new(-60.0, 0.0, -60.0);
        spawn_hall(&mut app, Team::Human, Vec3::new(-66.0, 0.0, -66.0));
        spawn_hall(&mut app, Team::Claude, Vec3::new(66.0, 0.0, 66.0));
        let mine = spawn_node(&mut app, ResourceKind::Gold, CARRY_AMOUNT, at_mine);
        swinging_at(&mut app, Team::Human, mine, ResourceKind::Gold, at_mine);

        tick(&mut app, GATHER_TIME + 0.1);

        assert_eq!(feed_lines(&app, Team::Human).len(), 1, "our mine, our news");
        assert!(
            feed_lines(&app, Team::Claude).is_empty(),
            "the enemy's halls are nowhere near it: {:?}",
            feed_lines(&app, Team::Claude)
        );
    }

    /// A tree is not geography and nobody is told about one.
    #[test]
    fn a_felled_tree_is_not_an_event() {
        let mut app = app_with_harvest();
        let at_tree = Vec3::new(-62.0, 0.0, -62.0);
        spawn_hall(&mut app, Team::Human, Vec3::new(-66.0, 0.0, -66.0));
        let tree = spawn_node(&mut app, ResourceKind::Lumber, CARRY_AMOUNT, at_tree);
        swinging_at(&mut app, Team::Human, tree, ResourceKind::Lumber, at_tree);

        tick(&mut app, GATHER_TIME + 0.1);

        assert!(
            feed_lines(&app, Team::Human).is_empty(),
            "there are thousands of trees; a stump is not a fact anybody reasons about"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: the gold runway (income measured, commitment projected)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod flow_tests {
    use super::*;

    /// The sampler alone, on a hand-driven clock.
    fn flow_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Economies>()
            .init_resource::<GoldFlow>()
            .add_systems(Update, sample_gold_flow);
        app
    }

    fn tick(app: &mut App, secs: f32) {
        let mut time = app.world_mut().resource_mut::<Time>();
        time.advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    fn rate(app: &App, team: Team) -> f32 {
        app.world().resource::<GoldFlow>().get(team).income_per_min
    }

    /// Income is the DIFFERENCE of what was banked, not the level of the bank:
    /// a team that earns 100 and spends 100 has an income of 100, and a team
    /// that spends nothing and earns nothing has an income of zero. Reading the
    /// level would have called the first one broke and the second one rich.
    #[test]
    fn income_measures_gold_banked_and_never_gold_held() {
        let mut app = flow_app();
        tick(&mut app, 0.0);
        for _ in 0..30 {
            {
                let mut economies = app.world_mut().resource_mut::<Economies>();
                let e = economies.get_mut(Team::Human);
                e.earn(10);
                // ...and spend every penny of it, which must not show up here.
                e.pay(10, 0);
            }
            tick(&mut app, 1.0);
        }
        let r = rate(&app, Team::Human);
        assert!(
            (r - 600.0).abs() < 30.0,
            "10g a second is 600g a minute, spent or not — got {r}"
        );
        assert_eq!(rate(&app, Team::Claude), 0.0, "and it is per team");
    }

    /// A refund is money you already had. Counting it would let a commander
    /// queue and cancel its way to an imaginary economy.
    #[test]
    fn a_refund_is_not_income() {
        let mut app = flow_app();
        tick(&mut app, 0.0);
        for _ in 0..30 {
            app.world_mut()
                .resource_mut::<Economies>()
                .get_mut(Team::Human)
                .refund(50, 0);
            tick(&mut app, 1.0);
        }
        assert_eq!(rate(&app, Team::Human), 0.0);
    }

    /// r36's fact: three trainers on one bank. The rate is per BUILDING and
    /// summed, because that is how they draw — pooling the queues would have
    /// reported three Barracks as one and hidden the whole problem.
    #[test]
    fn three_standing_trainers_commit_three_trainers_worth_of_gold() {
        let mut app = flow_app();
        let stats = unit_stats(UnitKind::Footman);
        let one = stats.cost_gold as f32 * 60.0 / stats.train_time;
        for _ in 0..3 {
            let mut queue = TrainingQueue::default();
            queue.queue.push_back(UnitKind::Footman);
            queue.queue.push_back(UnitKind::Footman);
            app.world_mut().spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                queue,
            ));
        }
        tick(&mut app, 1.0);
        let commit = app.world().resource::<GoldFlow>().get(Team::Human).commit_per_min;
        assert!(
            (commit - 3.0 * one).abs() < 1.0,
            "three trainers want three times one trainer: got {commit}, one is {one}"
        );

        // A building still going up commits nothing — it cannot train yet.
        let site = app
            .world_mut()
            .spawn((
                Building { kind: BuildingKind::Barracks },
                Team::Human,
                Transform::from_translation(Vec3::ZERO),
                {
                    let mut q = TrainingQueue::default();
                    q.queue.push_back(UnitKind::Footman);
                    q
                },
                UnderConstruction { remaining: 10.0 },
            ))
            .id();
        tick(&mut app, 1.0);
        let with_site = app.world().resource::<GoldFlow>().get(Team::Human).commit_per_min;
        assert!(
            (with_site - 3.0 * one).abs() < 1.0,
            "a foundation is not a trainer"
        );
        let _ = site;
    }

    /// Empty queues are a commitment of nothing, and the snapshot skips the
    /// key entirely rather than reporting a zero somebody has to interpret.
    #[test]
    fn nothing_queued_commits_nothing() {
        let mut app = flow_app();
        app.world_mut().spawn((
            Building { kind: BuildingKind::Barracks },
            Team::Human,
            Transform::from_translation(Vec3::ZERO),
            TrainingQueue::default(),
        ));
        tick(&mut app, 1.0);
        assert_eq!(
            app.world().resource::<GoldFlow>().get(Team::Human).commit_per_min,
            0.0
        );
    }
}
