//! Terrain module: ground, lighting, doodads, resource nodes (gold mines and
//! trees) and the RTS camera.
//!
//! Gameplay is strictly flat: everything lives on the Y=0 plane. Any bump or
//! rock spawned here is decoration only and never touches the `NavGrid`.
//! Gold mines block a 6x6 footprint, trees block their single 2x2 nav cell.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::pbr::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Deterministic map seed — the world is identical every run.
const MAP_SEED: u64 = 0xC1A0_DE_5EED;

const SKY_COLOR: Color = Color::srgb(0.42, 0.62, 0.88);

/// Gold mine footprint edge length (world units) blocked in the nav grid.
const MINE_FOOTPRINT: f32 = 6.0;
// Tuned down twice (10k → 5k → 3.5k): the map's total gold sets the game's
// length. At 3.5k the mines die around minute 10-12, forcing the decisive
// phase into the target 10-20 minute window.
const MINE_GOLD: u32 = 3_500;
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
// Plugin
// ---------------------------------------------------------------------------

pub struct TerrainPlugin {
    /// Headless sims keep map generation (nav blocking, resource nodes are
    /// gameplay) but skip camera, lighting, and sky — nothing renders anyway.
    pub headless: bool,
}

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .add_systems(Startup, (setup_ground, setup_resource_nodes));
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

/// Is this spot allowed to hold a tree? Keeps the bases, the gold mines and
/// the worker approach lanes to the home mines completely clear.
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
