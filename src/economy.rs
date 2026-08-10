//! Economy module: buildings, construction, the harvest loop and training queues.
//!
//! Owns:
//!   * `SpawnBuildingEvent` -> procedural, team-tinted building entities + nav blocking
//!   * `UnderConstruction` progress (visual growth + additive health)
//!   * `Order::Build` follow-through for workers (walk, pay, place)
//!   * `Order::Harvest` / `Order::ReturnResources` gather loop
//!   * `TrainingQueue` processing (pays when an item becomes the front item)
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
                spawn_buildings,
                construction_progress,
                start_upgrades,
                upgrade_progress,
                order_changed,
                build_sites,
                harvest_loop,
                training_queues,
                buy_items,
            )
                .chain(),
        )
        // Banking a bounty has nothing to do with the harvest loop above; it
        // runs on its own so an ordering change there can never drop gold.
        .add_systems(Update, bank_bounties);
    }
}

/// Pay out a claimed treasure cache. Unlike a gold delivery this is NOT taxed
/// by upkeep (documented in shared.rs): treasure rewards the bold, and a big
/// army is exactly what it takes to hold the middle.
fn bank_bounties(mut claims: EventReader<BountyClaim>, mut economies: ResMut<Economies>) {
    for claim in claims.read() {
        let economy = economies.get_mut(claim.team);
        economy.gold += claim.gold;
        debug!(
            "bounty banked: {:?} +{}g (untaxed) at ({:.0},{:.0}) -> {}g",
            claim.team, claim.gold, claim.pos.x, claim.pos.z, economy.gold
        );
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

#[derive(Resource)]
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

/// True once the front item of a building's `TrainingQueue` has been paid for.
#[derive(Component)]
struct PaidFront(bool);

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
                .insert((TrainingQueue::default(), PaidFront(false)));
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
                    .try_insert((TrainingQueue::default(), PaidFront(false)));
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
// 3./4. Order dispatch — set up module-private job state on order changes
// ---------------------------------------------------------------------------

fn order_changed(
    mut commands: Commands,
    mut units: Query<
        (
            Entity,
            &Order,
            &Transform,
            Option<&mut HarvestJob>,
            Option<&RememberedNode>,
            Option<&Carrying>,
        ),
        (Changed<Order>, With<Unit>),
    >,
    nodes: Query<(&ResourceNode, &Transform)>,
) {
    for (entity, order, tf, job, remembered, carrying) in &mut units {
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
    mut economies: ResMut<Economies>,
    mut spawn_events: EventWriter<SpawnBuildingEvent>,
    mut workers: Query<(Entity, &Transform, &Team, &mut BuildSite, Option<&MoveTo>)>,
    // Tech tree: only FINISHED buildings count toward requirements.
    completed: Query<(&Building, &Team), Without<UnderConstruction>>,
) {
    let owned = completed_kinds(&completed);

    for (entity, tf, team, mut site, moving) in &mut workers {
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
            let paid = free
                && tech_ok
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
            // Unreachable site — give up.
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

fn harvest_loop(
    time: Res<Time>,
    mut commands: Commands,
    mut nav: ResMut<NavGrid>,
    mut economies: ResMut<Economies>,
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
    halls: Query<(&Transform, &Team, &Building), Without<UnderConstruction>>,
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
        .filter(|(_, _, u)| u.kind != UnitKind::Worker && unit_stats(u.kind).can_hit_ground)
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
                    commands.entity(node_e).try_despawn();
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
                            economy.gold += taxed.max(1);
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
        if is_hero_kind(front) && paid.is_some_and(|p| p.0) {
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
        let mut paid_front = paid.map(|p| p.0).unwrap_or(false);

        let Some(&front) = queue.queue.front() else {
            queue.progress = 0.0;
            if paid_front {
                commands.entity(entity).try_insert(PaidFront(false));
            }
            continue;
        };

        let stats = unit_stats(front);
        // Heroes of either class are priced (and timed) by `hero_train_cost`:
        // full price for the team's first one, revival price afterwards.
        let (cost_gold, cost_lumber, train_time) = if is_hero_kind(front) {
            hero_train_cost(&records, *team, front)
        } else {
            (stats.cost_gold, stats.cost_lumber, stats.train_time)
        };

        if is_hero_kind(front) && !paid_front {
            // THE hero-slot rule, asked in the one place it lives. Both
            // refusals — a duplicate class, and a full slate — drop the item
            // unpaid so the queue keeps moving, exactly the treatment the old
            // one-hero rule gave it. The slot count is read from the team's
            // LIVE tier, so losing the Keep closes the second slot for FUTURE
            // heroes and never confiscates one already standing.
            let held = held_by(&hero_committed, *team);
            if hero_slot_check(&held, front, tiers.get(*team)) != HeroSlotVerdict::Ok {
                queue.queue.pop_front();
                queue.progress = 0.0;
                continue;
            }
        }

        // Pay the moment this item becomes the active front item. If we can't
        // afford it (or supply is blocked) the item simply waits in the queue.
        if !paid_front {
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
            paid_front = true;
            queue.progress = 0.0;
            commands.entity(entity).try_insert(PaidFront(true));
            if is_hero_kind(front) {
                // Later buildings in this same pass must see the commitment.
                hero_committed.push((*team, front));
            }
        }

        queue.progress += dt;
        if queue.progress >= train_time {
            queue.queue.pop_front();
            queue.progress = 0.0;
            commands.entity(entity).try_insert(PaidFront(false));

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
