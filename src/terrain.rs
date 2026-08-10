//! Terrain module: ground, lighting, doodads, map layout (impassable terrain),
//! resource nodes (gold mines and trees) and the RTS camera.
//!
//! Gameplay is strictly flat: everything lives on the Y=0 plane. Any bump or
//! rock spawned here is decoration only and never touches the `NavGrid`.
//! Gold mines block a 6x6 footprint, trees block their single 2x2 nav cell.
//!
//! **Map layouts.** `WC3_MAP` picks one (`open`, the historical layout, is the
//! default; `crossings` adds a real barrier). The layout is not a secret
//! setting: every bridge snapshot carries the map's name, one-line summary and
//! the exact position/width of every chokepoint, and the same facts are logged
//! at startup, so a commander reading JSON and a human looking at the screen
//! learn the same thing about the ground they are fighting over.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::pbr::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::OnceLock;

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Deterministic map seed — the world is identical every run.
const MAP_SEED: u64 = 0xC1A0_DE_5EED;

const SKY_COLOR: Color = Color::srgb(0.42, 0.62, 0.88);

/// Gold mine footprint edge length (world units) blocked in the nav grid.
const MINE_FOOTPRINT: f32 = 6.0;
// Tuned down twice (10k → 5k → 3.5k), then back UP to 5k after round 9: the
// map's total gold sets the game's length, and 3.5k was cut when games ran 60
// minutes — but the production-only win, uncapped bounty escalation, and the
// smarter scripted AI have since shortened everything. At 3.5k a saturated
// mine died around minute 4-5, deciding commander matches before tier 2
// existed. 5k restores a mid-game without restoring the stall.
const MINE_GOLD: u32 = 5_000;
const TREE_LUMBER: u32 = 150;

/// Camera pitch (radians below the horizon) and fixed yaw.
const CAM_PITCH: f32 = 0.90; // ~51.5 degrees
const CAM_YAW: f32 = std::f32::consts::FRAC_PI_4; // look from SW toward NE
const CAM_MIN_HEIGHT: f32 = 25.0;
const CAM_MAX_HEIGHT: f32 = 120.0;
const CAM_START_HEIGHT: f32 = 70.0;
/// How far past the map edge the camera focus may wander.
const CAM_FOCUS_LIMIT: f32 = MAP_HALF + 12.0;
/// Cursor distance (px) from a window edge that triggers edge panning.
const EDGE_PAN_MARGIN: f32 = 15.0;

// ---------------------------------------------------------------------------
// Map layouts
// ---------------------------------------------------------------------------

/// Which layout the world is generated from.
///
/// Selected once per process by `WC3_MAP` (unset or unknown -> `Open`, so every
/// existing script, sim and replay keeps the map it was tuned on).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapKind {
    /// The original layout: flat field, forest strip with wide lanes. Walls are
    /// decorative here — there is nothing to plug.
    Open,
    /// A canyon river splits the map along the diagonal between the two bases.
    /// Three fords are the only way across; two of them are the neutral gold
    /// expansions, so taking a second mine and holding a crossing are the same
    /// decision.
    Crossings,
}

impl MapKind {
    /// Every layout, in the order they are offered — this is the list a player
    /// (of either kind) can choose from with `WC3_MAP`.
    pub const ALL: [MapKind; 2] = [MapKind::Open, MapKind::Crossings];

    /// The `WC3_MAP` value that selects this map, and its snapshot name.
    pub fn id(self) -> &'static str {
        match self {
            MapKind::Open => "open",
            MapKind::Crossings => "crossings",
        }
    }

    fn from_id(s: &str) -> Option<MapKind> {
        MapKind::ALL.into_iter().find(|m| m.id() == s)
    }

    /// One line of ground truth, phrased the way a commander needs it. Shipped
    /// verbatim in the bridge snapshot and logged at startup.
    pub fn summary(self) -> &'static str {
        match self {
            MapKind::Open => {
                "Open field. Forest strip across the middle with wide lanes; \
                 armies can march anywhere between the bases."
            }
            MapKind::Crossings => {
                "A canyon river runs corner to corner between the bases. It is \
                 impassable: every army, worker and expansion must use one of \
                 three fords, and the two flank fords are the neutral gold \
                 mines. Ground held at a ford is ground the enemy cannot walk \
                 around."
            }
        }
    }

    /// The gaps in this map's impassable terrain. Empty on `Open`.
    pub fn chokepoints(self) -> Vec<ChokePoint> {
        match self {
            MapKind::Open => Vec::new(),
            MapKind::Crossings => FORDS
                .iter()
                .map(|&(name, along, half_width)| ChokePoint {
                    name,
                    pos: channel_point(along, 0.0),
                    width: half_width * 2.0,
                })
                .collect(),
        }
    }
}

/// A gap through impassable terrain: where it is and how wide the opening is.
#[derive(Clone, Copy, Debug)]
pub struct ChokePoint {
    pub name: &'static str,
    /// Centre of the gap on the ground plane.
    pub pos: Vec3,
    /// Opening width in world units, measured along the barrier. Gold mines or
    /// buildings standing inside a gap narrow it further.
    pub width: f32,
}

/// The map this process is playing on. Read from `WC3_MAP` exactly once.
pub fn active_map() -> MapKind {
    static ACTIVE: OnceLock<MapKind> = OnceLock::new();
    *ACTIVE.get_or_init(|| match std::env::var("WC3_MAP") {
        Ok(raw) if !raw.is_empty() => MapKind::from_id(raw.trim()).unwrap_or_else(|| {
            let known: Vec<&str> = MapKind::ALL.iter().map(|m| m.id()).collect();
            warn!(
                "WC3_MAP=\"{raw}\" is not a map (known: {}) — using \"open\"",
                known.join(", ")
            );
            MapKind::Open
        }),
        _ => MapKind::Open,
    })
}

// ---- The "crossings" canyon -----------------------------------------------

/// Half-thickness of the impassable channel. 10 world units is 5 nav cells —
/// far too thick for 8-connected A* to slip through diagonally, and wide enough
/// to read as a canyon from the default camera height.
const CHANNEL_HALF: f32 = 5.0;

/// How far the channel runs either side of the map centre. The centre line is
/// the NW–SE diagonal, so it reaches the map corners at ±141.4 and is sealed by
/// the map bounds at both ends.
const CHANNEL_REACH: f32 = 142.0;

/// The fords, as `(name, distance along the channel from map centre, half
/// width)`. Negative is toward the NW corner, positive toward the SE.
///
/// The flank fords sit exactly on `GOLD_MINE_POSITIONS[2]` / `[3]`: the mine's
/// 6x6 footprint stands in the middle of the opening and splits it into two
/// lanes, which is the point — the expansion *is* the chokepoint.
const FORDS: [(&str, f32, f32); 3] = [
    ("northwest ford", -84.85, 15.0),
    ("center ford", 0.0, 8.0),
    ("southeast ford", 84.85, 15.0),
];

/// Unit vector along the channel (NW -> SE), i.e. perpendicular to the SW->NE
/// axis the two bases sit on.
fn channel_along() -> Vec3 {
    Vec3::new(std::f32::consts::FRAC_1_SQRT_2, 0.0, -std::f32::consts::FRAC_1_SQRT_2)
}

/// Unit vector across the channel (SW -> NE), the direction armies travel.
fn channel_across() -> Vec3 {
    Vec3::new(std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2)
}

/// World point at channel coordinates `(along, across)`.
fn channel_point(along: f32, across: f32) -> Vec3 {
    channel_along() * along + channel_across() * across
}

/// Channel coordinates of a world point: `(along the channel, signed distance
/// across it)`. `across` is negative on the human/SW half of the map.
fn channel_coords(p: Vec3) -> (f32, f32) {
    const INV: f32 = std::f32::consts::FRAC_1_SQRT_2;
    ((p.x - p.z) * INV, (p.x + p.z) * INV)
}

/// Is `along` inside one of the fords?
fn in_ford(along: f32) -> bool {
    FORDS
        .iter()
        .any(|&(_, centre, half)| (along - centre).abs() < half)
}

/// Is this world point inside impassable terrain on the given map? The nav grid
/// and every visual are generated from this one predicate, so what a unit can
/// walk on and what a player can see never drift apart.
pub fn terrain_blocks(map: MapKind, p: Vec3) -> bool {
    match map {
        MapKind::Open => false,
        MapKind::Crossings => {
            let (along, across) = channel_coords(p);
            across.abs() <= CHANNEL_HALF && !in_ford(along)
        }
    }
}

/// Impassable stretches of the channel as `(along_start, along_end)` intervals —
/// the complement of the fords, clipped to the channel's reach. Visuals are
/// built from these so the rock always ends exactly where a ford begins.
fn barrier_intervals() -> Vec<(f32, f32)> {
    let mut gaps: Vec<(f32, f32)> = FORDS
        .iter()
        .map(|&(_, c, half)| (c - half, c + half))
        .collect();
    gaps.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut out = Vec::new();
    let mut cursor = -CHANNEL_REACH;
    for (start, end) in gaps {
        if start > cursor {
            out.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < CHANNEL_REACH {
        out.push((cursor, CHANNEL_REACH));
    }
    out
}

/// Nav cells made impassable by terrain on the active map, as world positions.
/// ui.rs paints these on the minimap so the human sees the barrier at a glance,
/// the same fact the bridge snapshot hands a commander as chokepoint geometry.
pub fn barrier_cells() -> Vec<Vec3> {
    let map = active_map();
    let mut out = Vec::new();
    if map == MapKind::Open {
        return out;
    }
    for cz in 0..GRID_DIM {
        for cx in 0..GRID_DIM {
            let c = NavGrid::cell_to_world(cx, cz);
            if terrain_blocks(map, c) {
                out.push(c);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct TerrainPlugin {
    /// Headless sims keep map generation (nav blocking, resource nodes are
    /// gameplay) but skip camera, lighting, and sky — nothing renders anyway.
    pub headless: bool,
}

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        // Chained: the barrier must own its nav cells before the forest asks
        // "is this cell already taken?", or trees would grow in the canyon.
        app.init_resource::<CameraRig>()
            .add_systems(
                Startup,
                (setup_ground, setup_barriers, setup_resource_nodes).chain(),
            );
        if !self.headless {
            app.insert_resource(ClearColor(SKY_COLOR))
                .insert_resource(AmbientLight {
                    color: Color::srgb(0.80, 0.88, 1.0),
                    brightness: 300.0,
                    ..default()
                })
                .add_systems(Startup, (setup_lighting, setup_camera))
                .add_systems(Update, camera_control);
        }
    }
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

fn setup_lighting(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 11_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(60.0, 120.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: 4,
            first_cascade_far_bound: 40.0,
            maximum_distance: 400.0,
            ..default()
        }
        .build(),
    ));
}

// ---------------------------------------------------------------------------
// Ground
// ---------------------------------------------------------------------------

fn setup_ground(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Main gameplay plane.
    let plane = meshes.add(Plane3d::default().mesh().size(MAP_HALF * 2.0, MAP_HALF * 2.0));
    let grass = materials.add(StandardMaterial {
        base_color: Color::srgb(0.24, 0.45, 0.20),
        perceptual_roughness: 0.95,
        reflectance: 0.03,
        ..default()
    });
    commands.spawn((
        Mesh3d(plane),
        MeshMaterial3d(grass),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    let mut rng = StdRng::seed_from_u64(MAP_SEED ^ 0x9E37_79B9);

    // A handful of large overlapping discs give the grass some tonal variety.
    // Purely cosmetic, a few centimeters above the plane to avoid z-fighting.
    let patch_materials: Vec<Handle<StandardMaterial>> = [
        Color::srgb(0.28, 0.50, 0.22),
        Color::srgb(0.21, 0.40, 0.18),
        Color::srgb(0.32, 0.48, 0.24),
        Color::srgb(0.35, 0.44, 0.22),
    ]
    .into_iter()
    .map(|c| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 1.0,
            reflectance: 0.02,
            ..default()
        })
    })
    .collect();

    for i in 0..26 {
        let radius = rng.gen_range(12.0f32..34.0);
        let disc = meshes.add(Cylinder::new(radius, 0.04));
        let x = rng.gen_range(-MAP_HALF..MAP_HALF);
        let z = rng.gen_range(-MAP_HALF..MAP_HALF);
        let y = 0.02 + (i as f32) * 0.001; // stable draw order between patches
        commands.spawn((
            Mesh3d(disc),
            MeshMaterial3d(patch_materials[i % patch_materials.len()].clone()),
            Transform::from_xyz(x, y, z),
        ));
    }

    // Decorative rocks / dirt mounds. Not in the nav grid, kept away from the
    // bases and the marching corridor so they never look like obstacles.
    let rock_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.42, 0.42, 0.45),
        perceptual_roughness: 0.9,
        ..default()
    });
    let mound_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.36, 0.31, 0.22),
        perceptual_roughness: 1.0,
        ..default()
    });
    for _ in 0..40 {
        let p = Vec3::new(
            rng.gen_range(-(MAP_HALF - 4.0)..(MAP_HALF - 4.0)),
            0.0,
            rng.gen_range(-(MAP_HALF - 4.0)..(MAP_HALF - 4.0)),
        );
        if p.distance(HUMAN_BASE) < 26.0 || p.distance(CLAUDE_BASE) < 26.0 {
            continue;
        }
        if GOLD_MINE_POSITIONS.iter().any(|m| p.distance(*m) < 14.0) {
            continue;
        }
        if point_segment_distance(p, HUMAN_BASE, CLAUDE_BASE) < 12.0 {
            continue;
        }
        // Nothing decorative in or beside the canyon: the barrier's own rock is
        // the only thing allowed to look like an obstacle there.
        if near_barrier(p, 3.0) {
            continue;
        }
        if rng.gen_bool(0.5) {
            let r = rng.gen_range(0.5f32..1.4);
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(r))),
                MeshMaterial3d(rock_mat.clone()),
                Transform::from_xyz(p.x, r * 0.35, p.z)
                    .with_scale(Vec3::new(1.0, rng.gen_range(0.5f32..0.9), 1.0)),
            ));
        } else {
            let r = rng.gen_range(1.5f32..4.0);
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(r))),
                MeshMaterial3d(mound_mat.clone()),
                Transform::from_xyz(p.x, -r * 0.75, p.z)
                    .with_scale(Vec3::new(1.0, rng.gen_range(0.25f32..0.5), 1.0)),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Map barriers: the impassable canyon and its fords
// ---------------------------------------------------------------------------

/// Blocks the nav grid and builds the barrier's look. Runs on every map: on
/// `open` it only announces the layout, which keeps the startup log a reliable
/// place to learn what you are playing on.
fn setup_barriers(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut nav: ResMut<NavGrid>,
) {
    let map = active_map();
    info!("map \"{}\": {}", map.id(), map.summary());
    for choke in map.chokepoints() {
        info!(
            "map \"{}\": {} at ({:.0},{:.0}), {:.0} wide",
            map.id(),
            choke.name,
            choke.pos.x,
            choke.pos.z,
            choke.width
        );
    }
    if map == MapKind::Open {
        return;
    }

    // ---- Nav: one pass over the grid, straight from the predicate ----------
    let mut blocked = 0usize;
    for cz in 0..GRID_DIM {
        for cx in 0..GRID_DIM {
            if terrain_blocks(map, NavGrid::cell_to_world(cx, cz)) {
                nav.blocked[NavGrid::idx(cx, cz)] = true;
                blocked += 1;
            }
        }
    }
    info!("map \"{}\": {blocked} nav cells are impassable terrain", map.id());

    // ---- Visuals ----------------------------------------------------------
    // The canyon reads as: dark water in a trench, grey rock walls along both
    // banks, and pale stone ramps at the fords. Same primitive vocabulary as
    // the rest of the map (cuboids, spheres, cylinders) — no new tricks.
    let mut rng = StdRng::seed_from_u64(MAP_SEED ^ 0x0BAD_F00D);
    let yaw = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);

    let water_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.07, 0.20, 0.34),
        perceptual_roughness: 0.15,
        reflectance: 0.55,
        ..default()
    });
    let cliff_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.31, 0.30, 0.33),
        perceptual_roughness: 0.95,
        ..default()
    });
    let cliff_top_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.40, 0.39, 0.40),
        perceptual_roughness: 1.0,
        ..default()
    });
    let ford_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.58, 0.53, 0.41),
        perceptual_roughness: 1.0,
        reflectance: 0.05,
        ..default()
    });

    // Trench floor + banks, segment by segment along every impassable stretch.
    const SEGMENT: f32 = 5.0;
    for (start, end) in barrier_intervals() {
        let span = end - start;
        let count = (span / SEGMENT).ceil().max(1.0);
        let step = span / count;
        for i in 0..count as usize {
            let along = start + step * (i as f32 + 0.5);
            let centre = channel_point(along, 0.0);
            // Past the corners the channel is outside the world; nothing to draw.
            if centre.x.abs() > MAP_HALF + 2.0 || centre.z.abs() > MAP_HALF + 2.0 {
                continue;
            }
            // Water sits a little below the grass: a trench, not a puddle.
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(step * 1.02, 0.6, CHANNEL_HALF * 2.0))),
                MeshMaterial3d(water_mat.clone()),
                Transform::from_translation(centre + Vec3::Y * -0.25).with_rotation(yaw),
            ));
            for side in [-1.0f32, 1.0] {
                let height = rng.gen_range(2.0f32..3.4);
                let bank = channel_point(along, side * (CHANNEL_HALF - 0.8));
                commands.spawn((
                    Mesh3d(meshes.add(Cuboid::new(step * 1.04, height, 1.8))),
                    MeshMaterial3d(cliff_mat.clone()),
                    Transform::from_translation(bank + Vec3::Y * (height * 0.5 - 0.5))
                        .with_rotation(yaw),
                ));
                // A boulder every few segments breaks the wall's straight line.
                if rng.gen_bool(0.4) {
                    let r = rng.gen_range(0.9f32..1.8);
                    let spot = channel_point(
                        along + rng.gen_range(-1.5f32..1.5),
                        side * (CHANNEL_HALF - rng.gen_range(0.0f32..2.0)),
                    );
                    commands.spawn((
                        Mesh3d(meshes.add(Sphere::new(r))),
                        MeshMaterial3d(cliff_top_mat.clone()),
                        Transform::from_translation(spot + Vec3::Y * (r * 0.3)),
                    ));
                }
            }
        }
    }

    // Fords: a pale stone ramp filling the gap, flanked by cairns standing on
    // the headlands so the opening is obvious from any camera height.
    for &(_, centre_along, half) in FORDS.iter() {
        let centre = channel_point(centre_along, 0.0);
        if centre.x.abs() > MAP_HALF || centre.z.abs() > MAP_HALF {
            continue;
        }
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(half * 2.0, 0.3, CHANNEL_HALF * 2.0 + 3.0))),
            MeshMaterial3d(ford_mat.clone()),
            Transform::from_translation(centre + Vec3::Y * 0.02).with_rotation(yaw),
        ));
        for end in [-1.0f32, 1.0] {
            for side in [-1.0f32, 1.0] {
                let pillar = channel_point(
                    centre_along + end * (half + 1.4),
                    side * (CHANNEL_HALF - 1.2),
                );
                if pillar.x.abs() > MAP_HALF || pillar.z.abs() > MAP_HALF {
                    continue;
                }
                commands.spawn((
                    Mesh3d(meshes.add(Cylinder::new(1.0, 4.0))),
                    MeshMaterial3d(cliff_top_mat.clone()),
                    Transform::from_translation(pillar + Vec3::Y * 1.6),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Resource nodes: gold mines and trees
// ---------------------------------------------------------------------------

fn setup_resource_nodes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut nav: ResMut<NavGrid>,
) {
    // ---- Gold mines -------------------------------------------------------
    let mound_mesh = meshes.add(Cuboid::new(5.0, 2.2, 5.0));
    let rim_mesh = meshes.add(Cuboid::new(6.0, 0.6, 6.0));
    let nugget_mesh = meshes.add(Sphere::new(0.7));
    let crystal_mesh = meshes.add(Cuboid::new(0.9, 2.6, 0.9));

    let rock_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.22, 0.20, 0.19),
        perceptual_roughness: 0.95,
        ..default()
    });
    let rim_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.33, 0.28, 0.22),
        perceptual_roughness: 1.0,
        ..default()
    });
    let gold_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.75, 0.15),
        emissive: LinearRgba::new(0.25, 0.17, 0.0, 1.0),
        metallic: 0.85,
        perceptual_roughness: 0.25,
        ..default()
    });

    for (i, pos) in GOLD_MINE_POSITIONS.iter().enumerate() {
        let ground = Vec3::new(pos.x, 0.0, pos.z);
        commands
            .spawn((
                ResourceNode {
                    kind: ResourceKind::Gold,
                    remaining: MINE_GOLD,
                },
                Transform::from_translation(ground)
                    .with_rotation(Quat::from_rotation_y(i as f32 * 0.4)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Mesh3d(rim_mesh.clone()),
                    MeshMaterial3d(rim_mat.clone()),
                    Transform::from_xyz(0.0, 0.3, 0.0),
                ));
                parent.spawn((
                    Mesh3d(mound_mesh.clone()),
                    MeshMaterial3d(rock_mat.clone()),
                    Transform::from_xyz(0.0, 1.4, 0.0),
                ));
                // Golden crystal spike in the middle...
                parent.spawn((
                    Mesh3d(crystal_mesh.clone()),
                    MeshMaterial3d(gold_mat.clone()),
                    Transform::from_xyz(0.0, 3.2, 0.0)
                        .with_rotation(Quat::from_rotation_z(0.15)),
                ));
                // ...plus nuggets scattered around the rim.
                for k in 0..6 {
                    let a = k as f32 * std::f32::consts::TAU / 6.0;
                    let r = 2.0;
                    parent.spawn((
                        Mesh3d(nugget_mesh.clone()),
                        MeshMaterial3d(gold_mat.clone()),
                        Transform::from_xyz(a.cos() * r, 2.7, a.sin() * r)
                            .with_scale(Vec3::splat(if k % 2 == 0 { 1.0 } else { 0.7 })),
                    ));
                }
            });

        nav.set_blocked_rect(ground, MINE_FOOTPRINT, true);
    }

    // ---- Trees ------------------------------------------------------------
    let mut rng = StdRng::seed_from_u64(MAP_SEED);

    let trunk_mesh = meshes.add(Cylinder::new(0.32, 2.4));
    let cone_mesh = meshes.add(Cone {
        radius: 1.7,
        height: 4.0,
    });
    let blob_mesh = meshes.add(Sphere::new(1.7));

    let bark_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.21, 0.13),
        perceptual_roughness: 1.0,
        ..default()
    });
    let leaf_mats: Vec<Handle<StandardMaterial>> = [
        Color::srgb(0.11, 0.34, 0.14),
        Color::srgb(0.15, 0.42, 0.18),
        Color::srgb(0.09, 0.28, 0.13),
        Color::srgb(0.19, 0.40, 0.15),
    ]
    .into_iter()
    .map(|c| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 0.95,
            reflectance: 0.02,
            ..default()
        })
    })
    .collect();

    for pos in tree_positions(&mut rng) {
        // A tree occupies exactly one nav cell; skip anything already blocked
        // (mines, other trees) so the forest never double-books a cell.
        if nav.is_blocked_world(pos) {
            continue;
        }
        nav.set_blocked_rect(pos, 2.0, true);

        let scale = rng.gen_range(0.8f32..1.25);
        let lean = Quat::from_rotation_y(rng.gen_range(0.0f32..std::f32::consts::TAU));
        let leaf = leaf_mats[rng.gen_range(0..leaf_mats.len())].clone();
        let pine = rng.gen_bool(0.6);

        commands
            .spawn((
                ResourceNode {
                    kind: ResourceKind::Lumber,
                    remaining: TREE_LUMBER,
                },
                Transform::from_translation(pos)
                    .with_rotation(lean)
                    .with_scale(Vec3::splat(scale)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Mesh3d(trunk_mesh.clone()),
                    MeshMaterial3d(bark_mat.clone()),
                    Transform::from_xyz(0.0, 1.2, 0.0),
                ));
                if pine {
                    parent.spawn((
                        Mesh3d(cone_mesh.clone()),
                        MeshMaterial3d(leaf.clone()),
                        Transform::from_xyz(0.0, 4.0, 0.0),
                    ));
                    parent.spawn((
                        Mesh3d(cone_mesh.clone()),
                        MeshMaterial3d(leaf),
                        Transform::from_xyz(0.0, 2.5, 0.0).with_scale(Vec3::splat(0.8)),
                    ));
                } else {
                    parent.spawn((
                        Mesh3d(blob_mesh.clone()),
                        MeshMaterial3d(leaf.clone()),
                        Transform::from_xyz(0.0, 3.4, 0.0)
                            .with_scale(Vec3::new(1.0, 1.15, 1.0)),
                    ));
                    parent.spawn((
                        Mesh3d(blob_mesh.clone()),
                        MeshMaterial3d(leaf),
                        Transform::from_xyz(0.55, 2.7, -0.3).with_scale(Vec3::splat(0.65)),
                    ));
                }
            });
    }
}

/// Distance from `p` to the segment `a`-`b`, in the XZ plane.
fn point_segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let p = Vec2::new(p.x, p.z);
    let a = Vec2::new(a.x, a.z);
    let b = Vec2::new(b.x, b.z);
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Within `margin` of the canyon, counting the fords: the corridor itself, its
/// banks, and a clear apron at each ford mouth. Nothing else may grow or be
/// scattered there — a choke only means something if its approaches are open.
fn near_barrier(p: Vec3, margin: f32) -> bool {
    match active_map() {
        MapKind::Open => false,
        MapKind::Crossings => {
            let (along, across) = channel_coords(p);
            if across.abs() <= CHANNEL_HALF + margin {
                return true;
            }
            FORDS.iter().any(|&(_, centre, half)| {
                (along - centre).abs() < half + margin
                    && across.abs() <= CHANNEL_HALF + margin + 6.0
            })
        }
    }
}

/// Is this spot allowed to hold a tree? Keeps the bases, the gold mines, the
/// canyon corridor and the worker approach lanes to the home mines clear.
fn spot_is_free(p: Vec3) -> bool {
    if p.x.abs() > MAP_HALF - 3.0 || p.z.abs() > MAP_HALF - 3.0 {
        return false;
    }
    if p.distance(HUMAN_BASE) < 24.0 || p.distance(CLAUDE_BASE) < 24.0 {
        return false;
    }
    if GOLD_MINE_POSITIONS.iter().any(|m| p.distance(*m) < 13.0) {
        return false;
    }
    if near_barrier(p, 3.0) {
        return false;
    }
    // Worker walking lanes: base -> its own gold mine.
    if point_segment_distance(p, HUMAN_BASE, GOLD_MINE_POSITIONS[0]) < 7.0 {
        return false;
    }
    if point_segment_distance(p, CLAUDE_BASE, GOLD_MINE_POSITIONS[1]) < 7.0 {
        return false;
    }
    true
}

/// Deterministic tree layout: edge forests, a diagonal strip between the two
/// bases (with wide gaps left open as marching lanes) and scattered groves.
fn tree_positions(rng: &mut StdRng) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::new();

    // --- Border forests: a band hugging each map edge. ---
    let edge = MAP_HALF - 2.0;
    for e in 0..4 {
        for _ in 0..36 {
            let depth = rng.gen_range(0.5f32..12.0);
            let along = rng.gen_range(-(MAP_HALF - 3.0)..(MAP_HALF - 3.0));
            let p = match e {
                0 => Vec3::new(-edge + depth, 0.0, along), // west
                1 => Vec3::new(edge - depth, 0.0, along),  // east
                2 => Vec3::new(along, 0.0, -edge + depth), // north
                _ => Vec3::new(along, 0.0, edge - depth),  // south
            };
            if spot_is_free(p) {
                out.push(p);
            }
        }
    }

    // --- Diagonal forest strip across the map, perpendicular to the SW->NE
    // base axis. Gaps at |t| < 20 (the central highway) and around |t| ~ 62
    // (two flanking lanes) keep armies able to march. ---
    let along = Vec3::new(1.0, 0.0, -1.0).normalize();
    let across = Vec3::new(1.0, 0.0, 1.0).normalize();
    for i in 0..80 {
        let t = -96.0 + i as f32 * (192.0 / 80.0);
        if t.abs() < 20.0 || (t.abs() - 62.0).abs() < 9.0 {
            continue;
        }
        let p = along * t
            + across * rng.gen_range(-6.5f32..6.5)
            + along * rng.gen_range(-1.5f32..1.5);
        if spot_is_free(p) {
            out.push(p);
        }
    }

    // --- Scattered groves in the open field, kept off the main corridor. ---
    let mut groves = 0;
    let mut attempts = 0;
    while groves < 11 && attempts < 400 {
        attempts += 1;
        let center = Vec3::new(
            rng.gen_range(-(MAP_HALF - 12.0)..(MAP_HALF - 12.0)),
            0.0,
            rng.gen_range(-(MAP_HALF - 12.0)..(MAP_HALF - 12.0)),
        );
        if !spot_is_free(center)
            || point_segment_distance(center, HUMAN_BASE, CLAUDE_BASE) < 16.0
        {
            continue;
        }
        groves += 1;
        for _ in 0..rng.gen_range(6..11) {
            let p = center
                + Vec3::new(
                    rng.gen_range(-6.0f32..6.0),
                    0.0,
                    rng.gen_range(-6.0f32..6.0),
                );
            if spot_is_free(p) && point_segment_distance(p, HUMAN_BASE, CLAUDE_BASE) > 12.0 {
                out.push(p);
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// RTS camera
// ---------------------------------------------------------------------------

/// Where the camera is looking and how high it sits. Yaw/pitch are fixed like
/// in classic RTS games; only the ground focus point and the zoom move.
#[derive(Resource)]
struct CameraRig {
    focus: Vec3,
    height: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        // Start over the human base, nudged toward the center of the map so
        // the town hall sits comfortably in frame.
        let toward_center = -HUMAN_BASE.normalize_or_zero();
        CameraRig {
            focus: HUMAN_BASE + toward_center * 12.0,
            height: CAM_START_HEIGHT,
        }
    }
}

/// Flat (ground-plane) direction the camera faces.
fn cam_forward() -> Vec3 {
    Vec3::new(CAM_YAW.sin(), 0.0, CAM_YAW.cos())
}

fn rig_transform(rig: &CameraRig) -> Transform {
    let forward = cam_forward();
    let back = rig.height / CAM_PITCH.tan();
    let pos = rig.focus - forward * back + Vec3::Y * rig.height;
    Transform::from_translation(pos).looking_at(rig.focus, Vec3::Y)
}

fn setup_camera(mut commands: Commands, rig: Res<CameraRig>) {
    commands.spawn((Camera3d::default(), rig_transform(&rig), MainCamera));
}

#[allow(clippy::too_many_arguments)]
fn camera_control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scroll: EventReader<MouseWheel>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut rig: ResMut<CameraRig>,
    mut focus_events: EventReader<CameraFocus>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
) {
    let Ok(mut cam_tf) = cam.single_mut() else {
        return;
    };
    let dt = time.delta_secs();

    // ---- Zoom ----
    let mut zoom = 0.0;
    for ev in scroll.read() {
        zoom += match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y * 0.05,
        };
    }
    if zoom != 0.0 {
        rig.height = (rig.height * (1.0 - zoom * 0.12)).clamp(CAM_MIN_HEIGHT, CAM_MAX_HEIGHT);
    }

    // ---- Pan (screen relative, on the ground plane) ----
    let forward = cam_forward();
    let right = forward.cross(Vec3::Y);
    let mut dir = Vec3::ZERO;

    // Letters are reserved for command hotkeys (WC3-style): the camera pans
    // with arrow keys, screen edges, and minimap clicks only.
    if keys.pressed(KeyCode::ArrowUp) {
        dir += forward;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        dir -= forward;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        dir += right;
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        dir -= right;
    }

    // ---- Edge-of-screen panning ----
    if let Ok(window) = windows.single() {
        if let Some(cursor) = window.cursor_position() {
            let w = window.width();
            let h = window.height();
            if cursor.x <= EDGE_PAN_MARGIN {
                dir -= right;
            } else if cursor.x >= w - EDGE_PAN_MARGIN {
                dir += right;
            }
            // Screen-space Y grows downward: top of the screen pans forward.
            if cursor.y <= EDGE_PAN_MARGIN {
                dir += forward;
            } else if cursor.y >= h - EDGE_PAN_MARGIN {
                dir -= forward;
            }
        }
    }

    if dir != Vec3::ZERO {
        // Panning gets faster the further out you are zoomed.
        let speed = 30.0 + rig.height * 0.75;
        rig.focus += dir.normalize() * speed * dt;
    }

    // ---- Minimap / programmatic focus jumps ----
    if let Some(event) = focus_events.read().last() {
        rig.focus = event.pos;
    }

    // Keep the view from wandering off the map.
    rig.focus.x = rig.focus.x.clamp(-CAM_FOCUS_LIMIT, CAM_FOCUS_LIMIT);
    rig.focus.z = rig.focus.z.clamp(-CAM_FOCUS_LIMIT, CAM_FOCUS_LIMIT);
    rig.focus.y = 0.0;

    *cam_tf = rig_transform(&rig);
}

// ---------------------------------------------------------------------------
// Map layout tests
// ---------------------------------------------------------------------------
//
// These check the property the whole layout rests on: the canyon is a *real*
// barrier (armies cannot walk around it) and the fords are *real* openings
// (armies can walk through them, and so can anything that spawns in the
// contested middle). All of it is pure geometry, so it runs without a World.

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Nav cells blocked by terrain alone (no trees, mines or buildings).
    fn terrain_grid(map: MapKind) -> Vec<bool> {
        let mut blocked = vec![false; GRID_DIM * GRID_DIM];
        for cz in 0..GRID_DIM {
            for cx in 0..GRID_DIM {
                if terrain_blocks(map, NavGrid::cell_to_world(cx, cz)) {
                    blocked[NavGrid::idx(cx, cz)] = true;
                }
            }
        }
        blocked
    }

    /// The same grid with the fords filled in — "what if nobody could cross?".
    fn sealed_grid() -> Vec<bool> {
        let mut blocked = vec![false; GRID_DIM * GRID_DIM];
        for cz in 0..GRID_DIM {
            for cx in 0..GRID_DIM {
                let (_, across) = channel_coords(NavGrid::cell_to_world(cx, cz));
                if across.abs() <= CHANNEL_HALF {
                    blocked[NavGrid::idx(cx, cz)] = true;
                }
            }
        }
        blocked
    }

    /// Flood fill with the movement rules units.rs actually uses: 8-connected,
    /// no cutting the corner between two blocked cells.
    fn reachable_from(blocked: &[bool], start: Vec3) -> Vec<bool> {
        let mut seen = vec![false; GRID_DIM * GRID_DIM];
        let (sx, sz) = NavGrid::world_to_cell(start).expect("start is on the map");
        assert!(!blocked[NavGrid::idx(sx, sz)], "start cell is blocked");
        seen[NavGrid::idx(sx, sz)] = true;
        let mut queue = VecDeque::from([(sx as i32, sz as i32)]);
        while let Some((cx, cz)) = queue.pop_front() {
            for (dx, dz) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let (nx, nz) = (cx + dx, cz + dz);
                if nx < 0 || nz < 0 || nx >= GRID_DIM as i32 || nz >= GRID_DIM as i32 {
                    continue;
                }
                let (nxu, nzu) = (nx as usize, nz as usize);
                if blocked[NavGrid::idx(nxu, nzu)] || seen[NavGrid::idx(nxu, nzu)] {
                    continue;
                }
                if dx != 0
                    && dz != 0
                    && (blocked[NavGrid::idx(nxu, cz as usize)]
                        || blocked[NavGrid::idx(cx as usize, nzu)])
                {
                    continue;
                }
                seen[NavGrid::idx(nxu, nzu)] = true;
                queue.push_back((nx, nz));
            }
        }
        seen
    }

    fn is_reachable(seen: &[bool], p: Vec3) -> bool {
        NavGrid::world_to_cell(p).is_some_and(|(cx, cz)| seen[NavGrid::idx(cx, cz)])
    }

    #[test]
    fn open_map_has_no_impassable_terrain() {
        assert!(terrain_grid(MapKind::Open).iter().all(|b| !b));
        assert!(MapKind::Open.chokepoints().is_empty());
    }

    #[test]
    fn crossings_canyon_would_cut_the_map_in_two() {
        // Fords closed: the bases must end up on different continents, or the
        // "barrier" is decoration and the fords are not chokepoints at all.
        let seen = reachable_from(&sealed_grid(), HUMAN_BASE);
        assert!(!is_reachable(&seen, CLAUDE_BASE));
    }

    #[test]
    fn crossings_fords_connect_the_bases_and_the_mines() {
        let seen = reachable_from(&terrain_grid(MapKind::Crossings), HUMAN_BASE);
        assert!(is_reachable(&seen, CLAUDE_BASE));
        for mine in GOLD_MINE_POSITIONS {
            // Mines stand on blocked footprints of their own in a real game;
            // here we only assert the ground beside them is on the road network.
            assert!(
                is_reachable(&seen, mine + Vec3::new(0.0, 0.0, 6.0))
                    || is_reachable(&seen, mine + Vec3::new(6.0, 0.0, 0.0)),
                "mine at {mine:?} is cut off"
            );
        }
    }

    #[test]
    fn crossings_leaves_no_unreachable_pocket_on_the_bounty_ring() {
        // bounty.rs drops caches on any free cell of the contested ring; a cell
        // walled off from the rest of the map would be gold nobody can claim.
        let blocked = terrain_grid(MapKind::Crossings);
        let seen = reachable_from(&blocked, HUMAN_BASE);
        for cz in 0..GRID_DIM {
            for cx in 0..GRID_DIM {
                let p = NavGrid::cell_to_world(cx, cz);
                let radius = p.length();
                if blocked[NavGrid::idx(cx, cz)]
                    || radius < BOUNTY_RING_MIN
                    || radius > BOUNTY_RING_MAX
                {
                    continue;
                }
                assert!(seen[NavGrid::idx(cx, cz)], "ring cell {p:?} is a pocket");
            }
        }
    }

    #[test]
    fn crossings_flank_fords_sit_on_the_neutral_expansions() {
        let chokes = MapKind::Crossings.chokepoints();
        assert_eq!(chokes.len(), 3);
        for mine in [GOLD_MINE_POSITIONS[2], GOLD_MINE_POSITIONS[3]] {
            assert!(
                chokes.iter().any(|c| c.pos.distance(mine) < 1.0),
                "no ford on the expansion at {mine:?}"
            );
        }
        // The centre ford is the direct road between the bases.
        assert!(chokes.iter().any(|c| c.pos.length() < 1.0));
    }
}
