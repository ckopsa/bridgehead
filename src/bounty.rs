//! bounty.rs — neutral treasure caches in the contested middle of the map.
//!
//! Owns the whole life cycle of the `Bounty` component declared in shared.rs:
//!   * spawning one on a timer (first at `BOUNTY_FIRST_AT`, then every
//!     `BOUNTY_INTERVAL`) at a random free spot on the contested ring,
//!   * the glowing chest-and-orb visual that marks it on the ground,
//!   * claim detection — the nearest living unit of *either* team inside
//!     `BOUNTY_CLAIM_RADIUS` takes it, which emits `BountyClaim`,
//!   * quiet expiry once `Bounty::expires_at` passes.
//!
//! This module never touches money: it only writes `BountyClaim` and
//! economy.rs banks the gold (untaxed — see shared.rs). ui.rs draws the
//! minimap dot and bridge.rs reports the caches to external commanders.
//!
//! Placement uses runtime randomness (`rand::thread_rng`) rather than the
//! map's fixed seed: bounty spots are meant to be unpredictable from one match
//! to the next, and nothing else in the sim depends on them being reproducible.

use bevy::prelude::*;
use rand::Rng;

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Game seconds between claim sweeps. Bounties are few and the claim radius is
/// generous, so there is no point testing every unit every frame.
const CLAIM_SWEEP: f32 = 0.25;
/// How many random ring positions to try before giving up for this cycle.
const PLACEMENT_TRIES: usize = 24;

/// Height the orb hovers at, and how far it bobs either side of it.
const ORB_HEIGHT: f32 = 2.2;
const ORB_BOB: f32 = 0.3;
/// Radians per second of the hover/pulse sine.
const PULSE_SPEED: f32 = 2.2;
/// Peak extra scale at the top of the pulse.
const PULSE_SCALE: f32 = 0.15;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct BountyPlugin;

impl Plugin for BountyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BountySchedule>()
            .add_systems(Startup, setup_bounty_assets)
            .add_systems(
                Update,
                (spawn_bounty, claim_bounties, expire_bounties, pulse_orbs).chain(),
            );
    }
}

/// When the next cache is due, in game seconds.
#[derive(Resource)]
struct BountySchedule {
    next_at: f32,
}

impl Default for BountySchedule {
    fn default() -> Self {
        BountySchedule {
            next_at: BOUNTY_FIRST_AT,
        }
    }
}

/// Meshes/materials shared by every cache — built once, cloned per spawn.
#[derive(Resource)]
struct BountyAssets {
    chest: Handle<Mesh>,
    orb: Handle<Mesh>,
    chest_mat: Handle<StandardMaterial>,
    orb_mat: Handle<StandardMaterial>,
}

/// The hovering orb above a chest. Carries its rest height so the pulse can be
/// a pure function of game time rather than an accumulating drift.
#[derive(Component)]
struct BountyOrb {
    base_y: f32,
}

fn setup_bounty_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(BountyAssets {
        chest: meshes.add(Cuboid::new(1.6, 1.0, 1.1)),
        orb: meshes.add(Sphere::new(0.45)),
        chest_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(0.72, 0.54, 0.14),
            emissive: LinearRgba::new(0.20, 0.13, 0.0, 1.0),
            metallic: 0.8,
            perceptual_roughness: 0.35,
            ..default()
        }),
        orb_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.86, 0.35),
            // Bright enough to read as treasure from the default camera height.
            emissive: LinearRgba::new(2.4, 1.7, 0.25, 1.0),
            metallic: 0.6,
            perceptual_roughness: 0.2,
            ..default()
        }),
    });
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// Drop a cache on the contested ring whenever one is due. If the ring somehow
/// has no free cell at any of the angles we try, the cycle is skipped silently
/// and the next one comes around on schedule.
fn spawn_bounty(
    mut commands: Commands,
    time: Res<Time>,
    nav: Res<NavGrid>,
    assets: Option<Res<BountyAssets>>,
    mut schedule: ResMut<BountySchedule>,
    game_over: Res<GameOver>,
) {
    let now = time.elapsed_secs();
    if now < schedule.next_at || game_over.0.is_some() {
        return;
    }
    // Whether or not a spot was found, the clock moves on.
    schedule.next_at = now + BOUNTY_INTERVAL;
    let Some(assets) = assets else {
        return; // Startup hasn't run yet; try again next interval.
    };

    let Some(pos) = free_ring_spot(&nav) else {
        debug!("bounty: no free spot on the ring this cycle — skipping");
        return;
    };

    let gold = bounty_value(now);
    info!("bounty: {gold}g at ({:.0},{:.0})", pos.x, pos.z);
    commands
        .spawn((
            Bounty {
                gold,
                expires_at: now + BOUNTY_LIFETIME,
            },
            Transform::from_translation(pos),
            Visibility::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(assets.chest.clone()),
                MeshMaterial3d(assets.chest_mat.clone()),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            parent.spawn((
                Mesh3d(assets.orb.clone()),
                MeshMaterial3d(assets.orb_mat.clone()),
                Transform::from_xyz(0.0, ORB_HEIGHT, 0.0),
                BountyOrb { base_y: ORB_HEIGHT },
            ));
        });
}

/// A random unblocked cell on the contested ring, or `None` if every attempt
/// landed on trees, mines or a building footprint.
fn free_ring_spot(nav: &NavGrid) -> Option<Vec3> {
    let mut rng = rand::thread_rng();
    for _ in 0..PLACEMENT_TRIES {
        let angle = rng.gen_range(0.0..std::f32::consts::TAU);
        let radius = rng.gen_range(BOUNTY_RING_MIN..BOUNTY_RING_MAX);
        let raw = Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
        // Snap to the nav cell so the chest sits where units can actually walk.
        let Some((cx, cz)) = NavGrid::world_to_cell(raw) else {
            continue;
        };
        if nav.is_blocked(cx, cz) {
            continue;
        }
        return Some(NavGrid::cell_to_world(cx, cz));
    }
    None
}

// ---------------------------------------------------------------------------
// Claiming
// ---------------------------------------------------------------------------

/// Every `CLAIM_SWEEP` seconds: whichever living unit is nearest a cache, and
/// inside `BOUNTY_CLAIM_RADIUS`, wins it for its team. Nearest breaks ties, so
/// two armies arriving on the same tick is decided by who actually got there.
fn claim_bounties(
    mut commands: Commands,
    time: Res<Time>,
    mut next_sweep: Local<f32>,
    mut claims: EventWriter<BountyClaim>,
    bounties: Query<(Entity, &Bounty, &Transform)>,
    units: Query<(&Team, &Transform, &Health), With<Unit>>,
) {
    let now = time.elapsed_secs();
    if now < *next_sweep {
        return;
    }
    *next_sweep = now + CLAIM_SWEEP;

    for (entity, bounty, tf) in &bounties {
        let pos = tf.translation;
        let mut best: Option<(f32, Team)> = None;
        // AIR CLAIMS: DELIBERATELY ALLOWED. `flat_dist` ignores height and the
        // query has no kind filter, so a flyer passing over a cache takes it.
        // Kept that way on purpose — a fast, terrain-ignoring raider that can
        // reach the contested ring first turns every escalating cache into a
        // race the ground army has to answer, and the answer (archers, towers
        // near the ring) is exactly the anti-air investment flyers are meant
        // to provoke. Contesting bounties is the flyer's economic role, not a
        // loophole.
        for (team, unit_tf, health) in &units {
            if health.current <= 0.0 {
                continue; // dying this frame; apply_death hasn't run yet
            }
            let d = flat_dist(pos, unit_tf.translation);
            if d > BOUNTY_CLAIM_RADIUS {
                continue;
            }
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, *team));
            }
        }
        let Some((_, team)) = best else {
            continue;
        };
        info!(
            "bounty claimed: {:?} +{}g at ({:.0},{:.0})",
            team, bounty.gold, pos.x, pos.z
        );
        claims.write(BountyClaim {
            team,
            gold: bounty.gold,
            pos,
        });
        commands.entity(entity).despawn();
    }
}

/// Unclaimed caches vanish on their own — no event, no gold, just a note in the
/// debug log for anyone balancing the ring.
fn expire_bounties(
    mut commands: Commands,
    time: Res<Time>,
    bounties: Query<(Entity, &Bounty, &Transform)>,
) {
    let now = time.elapsed_secs();
    for (entity, bounty, tf) in &bounties {
        if now >= bounty.expires_at {
            debug!(
                "bounty expired: {}g at ({:.0},{:.0})",
                bounty.gold, tf.translation.x, tf.translation.z
            );
            commands.entity(entity).despawn();
        }
    }
}

/// Hover + breathe. One sine drives both the height and the scale so the orb
/// reads as a single pulse rather than two independent wobbles.
fn pulse_orbs(time: Res<Time>, mut orbs: Query<(&mut Transform, &BountyOrb)>) {
    let s = (time.elapsed_secs() * PULSE_SPEED).sin();
    for (mut tf, orb) in &mut orbs {
        tf.translation.y = orb.base_y + s * ORB_BOB;
        tf.scale = Vec3::splat(1.0 + s * PULSE_SCALE);
    }
}

/// Ground-plane distance: height never matters for a claim.
fn flat_dist(a: Vec3, b: Vec3) -> f32 {
    (a.x - b.x).hypot(a.z - b.z)
}
