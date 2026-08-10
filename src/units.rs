//! units.rs — unit spawning, order→movement translation, A* pathfinding over the
//! `NavGrid`, steering, local separation and repathing.
//!
//! Cross-module contract (see DESIGN.md):
//!   * Only this module mutates unit `Transform`s.
//!   * Other modules insert `MoveTo { target }`; this module pathfinds, steers and
//!     REMOVES `MoveTo` on arrival (or when the target is unreachable). Absence of
//!     `MoveTo` is the "arrived / not moving" signal everyone else relies on.

use bevy::prelude::*;
use std::collections::BinaryHeap;
use std::collections::HashMap;

use crate::shared::*;

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// How close (XZ) to the requested target counts as "arrived".
const ARRIVE_RADIUS: f32 = 1.5;
/// How close to an intermediate waypoint before advancing to the next one.
const WAYPOINT_RADIUS: f32 = 0.55;
// Collision radius comes from shared::UNIT_RADIUS (scaled by UNIT_SCALE).
/// Units closer than this push each other apart.
const SEPARATION_DIST: f32 = UNIT_RADIUS * 2.0;
/// Max world units/second a unit can be displaced by separation.
const SEPARATION_SPEED: f32 = 2.5;
/// Radians/second-ish turn responsiveness.
const TURN_RATE: f32 = 12.0;
/// Seconds of "barely moved" before we force a repath.
const STUCK_TIME: f32 = 1.5;
/// How often the stuck detector samples position.
const STUCK_SAMPLE: f32 = 0.5;
/// Distance considered "actually moving" between two samples.
const STUCK_EPSILON: f32 = 0.25;
/// Give up (drop MoveTo) after this many consecutive stuck repaths.
const MAX_REPATHS: u32 = 3;
/// Safety cap on A* node expansions.
const MAX_EXPANSIONS: u32 = 20_000;
/// `Order::Follow`: XZ distance at which a follower stops chasing.
const FOLLOW_STOP_DIST: f32 = 4.5;
/// `Order::Follow`: seconds between re-targets while chasing (~3x/sec).
const FOLLOW_INTERVAL: f32 = 0.33;
/// `Order::Follow`: followee displacement that forces an early re-target.
const FOLLOW_MOVE_EPSILON: f32 = 2.0;
/// Heroes stand a head taller than the rank and file: their root uses
/// `UNIT_SCALE * HERO_SCALE` (body radii stay on the shared grid).
const HERO_SCALE: f32 = 1.15;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct UnitsPlugin;

impl Plugin for UnitsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PathScratch>()
            .add_systems(Startup, setup_unit_assets)
            .add_systems(
                Update,
                (
                    spawn_units,
                    handle_teleports,
                    orders_to_movement,
                    follow_orders,
                    compute_paths,
                    steer_units,
                    separate_units,
                    cleanup_orphan_paths,
                )
                    .chain()
                    // `steer_units` and `separate_units` both hold
                    // `&mut Transform`, and so does combat.rs's `engagement`.
                    // Before `SimSet` nothing ordered this chain against
                    // combat's, so which one moved a unit first was decided by
                    // whichever thread got there. Now movement finishes before
                    // combat begins: a unit shoots from where it now stands.
                    .in_set(SimSet::Movement),
            );
    }
}

// ---------------------------------------------------------------------------
// Module-private components
// ---------------------------------------------------------------------------

/// The pathfollowing state of a unit that currently has a `MoveTo`.
/// Created/replaced by `compute_paths`, consumed by `steer_units`, removed
/// together with `MoveTo` on arrival.
#[derive(Component)]
struct PathFollow {
    waypoints: Vec<Vec3>,
    index: usize,
    /// Stuck detection.
    last_sample_pos: Vec3,
    sample_timer: f32,
    stuck_time: f32,
    repaths: u32,
}

/// Marker for the accent child meshes (shield / bow / backpack / head).
#[derive(Component)]
struct UnitAccent;

/// Throttle state for a unit currently under `Order::Follow`. Removed as soon
/// as the order changes to anything else.
#[derive(Component)]
struct FollowState {
    /// Seconds left before the next re-target.
    cooldown: f32,
    /// Followee position at the last re-target.
    last_target_pos: Vec3,
}

// ---------------------------------------------------------------------------
// Cached meshes & materials
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct UnitAssets {
    worker_body: Handle<Mesh>,
    worker_pack: Handle<Mesh>,
    footman_body: Handle<Mesh>,
    footman_shield: Handle<Mesh>,
    archer_body: Handle<Mesh>,
    archer_bow: Handle<Mesh>,
    hero_body: Handle<Mesh>,
    hero_crown: Handle<Mesh>,
    hero_blade: Handle<Mesh>,
    catapult_body: Handle<Mesh>,
    catapult_wheel: Handle<Mesh>,
    catapult_arm: Handle<Mesh>,
    catapult_panel: Handle<Mesh>,
    raider_mount: Handle<Mesh>,
    raider_leg: Handle<Mesh>,
    raider_rider: Handle<Mesh>,
    spearman_body: Handle<Mesh>,
    spearman_shaft: Handle<Mesh>,
    spearman_head: Handle<Mesh>,
    priestess_body: Handle<Mesh>,
    priestess_hood: Handle<Mesh>,
    priestess_staff: Handle<Mesh>,
    priestess_tip: Handle<Mesh>,
    sorcerer_body: Handle<Mesh>,
    sorcerer_hood: Handle<Mesh>,
    sorcerer_orb: Handle<Mesh>,
    knight_mount: Handle<Mesh>,
    knight_leg: Handle<Mesh>,
    knight_rider: Handle<Mesh>,
    knight_lance: Handle<Mesh>,
    gryphon_body: Handle<Mesh>,
    gryphon_wing: Handle<Mesh>,
    gryphon_tail: Handle<Mesh>,
    gryphon_rider: Handle<Mesh>,
    head: Handle<Mesh>,
    human_mat: Handle<StandardMaterial>,
    claude_mat: Handle<StandardMaterial>,
    metal_mat: Handle<StandardMaterial>,
    wood_mat: Handle<StandardMaterial>,
    dark_mat: Handle<StandardMaterial>,
    skin_mat: Handle<StandardMaterial>,
    gold_mat: Handle<StandardMaterial>,
    /// Pale accent trim for the Priestess (hood).
    robe_mat: Handle<StandardMaterial>,
    /// Emissive staff tip.
    glow_mat: Handle<StandardMaterial>,
    /// The Sorcerer's floating orb: violet, and deliberately the same hue as
    /// `StatusKind::Slow`'s ground ring — the unit and the debuff it puts on
    /// the field read as one thing at a glance.
    arcane_mat: Handle<StandardMaterial>,
}

impl UnitAssets {
    fn team_mat(&self, team: Team) -> Handle<StandardMaterial> {
        match team {
            Team::Human => self.human_mat.clone(),
            Team::Claude => self.claude_mat.clone(),
        }
    }
}

fn setup_unit_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Hero trim: shinier and faintly glowing so the Champion reads at a glance.
    let gold_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.78, 0.22),
        emissive: LinearRgba::new(0.25, 0.17, 0.02, 1.0),
        perceptual_roughness: 0.35,
        metallic: 0.8,
        ..default()
    });

    // The Priestess's staff tip: a small lamp of light at the top of the shaft.
    let glow_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.95, 1.0),
        emissive: LinearRgba::new(1.6, 2.2, 3.0, 1.0),
        unlit: false,
        ..default()
    });

    // Violet arcane light, matched to `StatusKind::Slow.tint()` so the caster
    // and its debuff ring are visibly the same magic.
    let arcane_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.45, 1.0),
        emissive: LinearRgba::new(0.9, 0.6, 2.4, 1.0),
        ..default()
    });

    let mut solid = |color: Color| {
        materials.add(StandardMaterial {
            base_color: color,
            perceptual_roughness: 0.85,
            ..default()
        })
    };

    commands.insert_resource(UnitAssets {
        // Worker: short stubby capsule (~1.2 tall) + backpack.
        worker_body: meshes.add(Capsule3d::new(0.30, 0.60)),
        worker_pack: meshes.add(Cuboid::new(0.38, 0.42, 0.24)),
        // Footman: taller capsule (~1.5 tall) + shield.
        footman_body: meshes.add(Capsule3d::new(0.34, 0.80)),
        footman_shield: meshes.add(Cuboid::new(0.10, 0.62, 0.46)),
        // Archer: slim capsule (~1.3 tall) + thin bow.
        archer_body: meshes.add(Capsule3d::new(0.26, 0.78)),
        archer_bow: meshes.add(Cuboid::new(0.07, 0.95, 0.10)),
        // Champion: broad capsule (~1.7 tall) + gold crown and greatsword.
        hero_body: meshes.add(Capsule3d::new(0.42, 0.86)),
        hero_crown: meshes.add(Torus::new(0.13, 0.21)),
        hero_blade: meshes.add(Cuboid::new(0.10, 1.15, 0.20)),
        // Catapult: a wide flat chassis on four small wheels with a throwing
        // arm cocked over the back — low, broad, and obviously not a person.
        catapult_body: meshes.add(Cuboid::new(1.05, 0.32, 1.35)),
        catapult_wheel: meshes.add(Cylinder::new(0.25, 0.14)),
        catapult_arm: meshes.add(Cuboid::new(0.14, 0.95, 0.14)),
        catapult_panel: meshes.add(Cuboid::new(0.90, 0.26, 0.08)),
        // Raider: a dark beast — capsule lying along Z on four stubby legs —
        // with a small team-coloured rider sitting on its back.
        raider_mount: meshes.add(Capsule3d::new(0.30, 0.80)),
        // 0.34 tall: the legs reach the ground plane at local y = -0.75 from
        // their centre at -0.58 without punching through it.
        raider_leg: meshes.add(Cylinder::new(0.09, 0.34)),
        raider_rider: meshes.add(Capsule3d::new(0.19, 0.30)),
        // Spearman: the tallest infantry silhouette and the thinnest — a
        // narrow capsule under a spear that stands a full body-length above
        // the head. At RTS camera distance the vertical line is the whole
        // read: if you can see spears over the front rank, that rank is a
        // wall, and you do not ride into it.
        spearman_body: meshes.add(Capsule3d::new(0.25, 1.06)),
        spearman_shaft: meshes.add(Cylinder::new(0.045, 1.75)),
        spearman_head: meshes.add(Cone::new(0.10, 0.26)),
        // Priestess: slim robe, oversized hood, tall staff with a lit tip.
        // Total 1.60 = the kind's height, so she stands on the ground plane.
        priestess_body: meshes.add(Capsule3d::new(0.27, 1.06)),
        priestess_hood: meshes.add(Sphere::new(0.26)),
        priestess_staff: meshes.add(Cuboid::new(0.07, 1.30, 0.07)),
        priestess_tip: meshes.add(Sphere::new(0.11)),
        // Sorcerer: the slightest silhouette on the field — a narrow robe
        // under a pointed hood, with an untethered orb floating at shoulder
        // height instead of a weapon. Nothing it carries is a weapon, which is
        // the read: this thing does not fight, it does something to you.
        // Total 1.44 = the kind's height.
        sorcerer_body: meshes.add(Capsule3d::new(0.23, 0.92)),
        sorcerer_hood: meshes.add(Cone::new(0.27, 0.44)),
        sorcerer_orb: meshes.add(Sphere::new(0.14)),
        // Knight: the Raider's silhouette rebuilt in steel and scaled up — a
        // heavier barded mount, thicker legs, an armoured rider, and a lance
        // couched forward past the horse's head. The read from the RTS camera
        // is "the same shape as a Raider, but bigger and metal", which is
        // exactly what it is: cavalry, so the spear line still answers it.
        knight_mount: meshes.add(Capsule3d::new(0.36, 0.92)),
        // 0.42 tall so the legs meet the ground from their centre at -0.62.
        knight_leg: meshes.add(Cylinder::new(0.11, 0.42)),
        knight_rider: meshes.add(Capsule3d::new(0.23, 0.40)),
        knight_lance: meshes.add(Cylinder::new(0.055, 2.10)),
        // Gryphon Rider: a wide horizontal wingspan is the whole silhouette.
        // Nothing else on the field is broader than it is tall, so at altitude
        // the shape alone says "you cannot reach this".
        gryphon_body: meshes.add(Capsule3d::new(0.30, 0.86)),
        gryphon_wing: meshes.add(Cuboid::new(1.55, 0.07, 0.62)),
        gryphon_tail: meshes.add(Cone::new(0.16, 0.70)),
        gryphon_rider: meshes.add(Capsule3d::new(0.20, 0.34)),
        head: meshes.add(Sphere::new(0.20)),
        human_mat: solid(Team::Human.color()),
        claude_mat: solid(Team::Claude.color()),
        metal_mat: solid(Color::srgb(0.28, 0.29, 0.33)),
        wood_mat: solid(Color::srgb(0.42, 0.28, 0.14)),
        dark_mat: solid(Color::srgb(0.13, 0.11, 0.10)),
        skin_mat: solid(Color::srgb(0.86, 0.72, 0.58)),
        gold_mat,
        robe_mat: solid(Color::srgb(0.88, 0.86, 0.95)),
        glow_mat,
        arcane_mat,
    });
}

/// Total height of a unit's body mesh.
fn unit_height(kind: UnitKind) -> f32 {
    match kind {
        UnitKind::Worker => 1.20,
        UnitKind::Footman => 1.48,
        UnitKind::Archer => 1.30,
        UnitKind::Hero => 1.70,
        // Low and wide: the siege engine is a wagon, not a soldier.
        UnitKind::Catapult => 1.10,
        // Mounted: long and low, rider's head about where a footman's is.
        UnitKind::Raider => 1.50,
        UnitKind::Priestess => 1.60,
        // Taller than a Footman and much narrower — before the spear is even
        // drawn, the silhouette says "reach, not shoulders".
        UnitKind::Spearman => 1.56,
        // Shortest adult on the field: a scholar, not a soldier.
        UnitKind::Sorcerer => 1.44,
        // Taller and heavier than the Raider's 1.50 — armoured horse, armoured
        // man. The extra 0.2 is the whole visual claim of a tier-3 unit.
        UnitKind::Knight => 1.70,
        // Measured through the body, not the wings: what makes the Gryphon
        // unmistakable is width and altitude, not height.
        UnitKind::GryphonRider => 1.40,
    }
}

/// World scale applied to a unit's root transform.
fn unit_root_scale(kind: UnitKind) -> f32 {
    if is_hero_kind(kind) {
        UNIT_SCALE * HERO_SCALE
    } else {
        UNIT_SCALE
    }
}

/// Constant Y the unit's origin is kept at (mesh centre = half height).
/// Flying kinds ride `FLYER_ALTITUDE` above that — the one place altitude is
/// decided, so every mover and teleport lands a flyer at the same height.
fn unit_y(kind: UnitKind) -> f32 {
    let ground = unit_height(kind) * 0.5 * unit_root_scale(kind);
    if is_flying_kind(kind) {
        ground + FLYER_ALTITUDE
    } else {
        ground
    }
}

// ---------------------------------------------------------------------------
// 1. Spawning
// ---------------------------------------------------------------------------

fn spawn_units(
    mut commands: Commands,
    mut events: EventReader<SpawnUnitEvent>,
    time: Res<Time>,
    assets: Option<Res<UnitAssets>>,
    nav: Res<NavGrid>,
    records: Res<HeroRecords>,
    nodes: Query<&Transform, With<ResourceNode>>,
    live_units: Query<Entity, With<Unit>>,
    // The producing building, for both halves of what it stamps: the doctrine
    // template, and the name a trained unit gives when asked who sent it.
    producers: Query<(&Building, Option<&DoctrineTemplate>)>,
) {
    let Some(assets) = assets else {
        return;
    };
    let now = time.elapsed_secs();

    for ev in events.read() {
        let stats = unit_stats(ev.kind);

        // Nudge out of blocked / out-of-bounds cells. Flyers skip the nudge:
        // an occupied cell is not occupied airspace, so a Gryphon may perfectly
        // well be born hovering over the tree its barracks backs onto.
        let mut pos = ev.pos;
        pos.x = pos.x.clamp(-MAP_HALF + 1.0, MAP_HALF - 1.0);
        pos.z = pos.z.clamp(-MAP_HALF + 1.0, MAP_HALF - 1.0);
        if !stats.flying && nav.is_blocked_world(pos) {
            if let Some(cell) = NavGrid::world_to_cell(pos) {
                if let Some(free) = nearest_free_cell(&nav, cell) {
                    pos = NavGrid::cell_to_world(free.0, free.1);
                }
            }
        }
        pos.y = unit_y(ev.kind);

        let team_mat = assets.team_mat(ev.team);
        let body = match ev.kind {
            UnitKind::Worker => assets.worker_body.clone(),
            UnitKind::Footman => assets.footman_body.clone(),
            UnitKind::Archer => assets.archer_body.clone(),
            UnitKind::Hero => assets.hero_body.clone(),
            UnitKind::Catapult => assets.catapult_body.clone(),
            // The raider's ROOT is the rider (so it wears the team colour and
            // faces where the unit faces); the dark mount is a child below it.
            UnitKind::Raider => assets.raider_rider.clone(),
            UnitKind::Priestess => assets.priestess_body.clone(),
            UnitKind::Spearman => assets.spearman_body.clone(),
            UnitKind::Sorcerer => assets.sorcerer_body.clone(),
            // Both mounted kinds put the RIDER at the root for the same reason
            // the Raider does: the root wears the team colour and faces where
            // the unit faces, and the beast underneath is a child.
            UnitKind::Knight => assets.knight_rider.clone(),
            UnitKind::GryphonRider => assets.gryphon_rider.clone(),
        };
        // Everyone wears their team's colour; the catapult is a wooden machine
        // that merely carries a painted panel (spawned as a child below).
        let body_mat = match ev.kind {
            UnitKind::Catapult => assets.wood_mat.clone(),
            _ => team_mat.clone(),
        };

        // Hero classes resume their team's progression; a fresh one starts at
        // level 1. Either way they arrive at full (class- and level-scaled) HP.
        // Per CLASS: a team that fields a Champion and a Priestess keeps two
        // independent progressions, and each revival restores its own.
        let hero =
            is_hero_kind(ev.kind).then(|| Hero::from_record(records.get(ev.team, ev.kind)));
        let health = match hero {
            Some(hero) => {
                let max = Hero::max_hp_for(ev.kind, hero.level);
                Health { current: max, max }
            }
            None => Health::new(stats.hp),
        };

        // Face roughly toward the middle of the map so fresh units don't all
        // stare along -Z.
        let facing = Vec3::new(-pos.x, 0.0, -pos.z).normalize_or_zero();
        let mut transform =
            Transform::from_translation(pos).with_scale(Vec3::splat(unit_root_scale(ev.kind)));
        if facing.length_squared() > 0.0 {
            transform.look_to(facing, Vec3::Y);
        }

        // The rider's head sits on top of the rider capsule, not on top of the
        // (much taller) mounted silhouette.
        let head_y = match ev.kind {
            UnitKind::Raider => 0.45,
            // Same reason as the Raider: the head belongs on the rider capsule,
            // not on top of the whole mounted (or airborne) silhouette.
            UnitKind::Knight => 0.50,
            UnitKind::GryphonRider => 0.42,
            _ => unit_height(ev.kind) * 0.5 - 0.05,
        };

        // Producing building's rally point (if any) becomes the first order.
        // Anything stale (depleted node, dead followee) degrades to Idle.
        let order = match ev.rally {
            None => Order::default(),
            Some(RallyTarget::Ground(p)) => Order::Move(Vec3::new(p.x, 0.0, p.z)),
            Some(RallyTarget::Node(node)) => match nodes.get(node) {
                Ok(_) if ev.kind == UnitKind::Worker => Order::Harvest(node),
                // Non-workers can't gather — just gather *near* the node.
                Ok(node_tf) => Order::Move(Vec3::new(
                    node_tf.translation.x,
                    0.0,
                    node_tf.translation.z,
                )),
                Err(_) => Order::default(),
            },
            Some(RallyTarget::Unit(followee)) => {
                if live_units.get(followee).is_ok() {
                    Order::Follow(followee)
                } else {
                    Order::default()
                }
            }
        };

        // Who sent it, in the words it will use when asked.
        let producer = ev.source.and_then(|src| producers.get(src).ok().map(|p| (src, p)));
        let template = producer.and_then(|(_, (_, tmpl))| tmpl);
        let why = spawn_provenance(
            producer.map(|(entity, (building, _))| (entity, building.kind)),
            template.is_some(),
            // A rally that produced a real first order. A stale one (depleted
            // node, dead followee) degraded to Idle above and is no reason.
            !matches!(order, Order::Idle),
            now,
        );

        let mut entity = commands.spawn((
            Unit { kind: ev.kind },
            ev.team,
            health,
            order,
            why,
            Mesh3d(body),
            MeshMaterial3d(body_mat),
            transform,
            // Bevy only propagates Transform -> GlobalTransform in PostUpdate,
            // so a unit spawned during Update would read as sitting at the
            // world origin for the rest of that frame. Combat reads positions
            // through GlobalTransform (it needs a read-only alias while it
            // mutates attacker Transforms), which made every fresh spawn look
            // like it had teleported to (0,0,0) — towers near the origin got a
            // free bolt on it. A unit is always a root entity, so its
            // GlobalTransform simply IS its Transform; seed it here and the
            // hole never opens.
            GlobalTransform::from(transform),
            Name::new(match ev.kind {
                UnitKind::Worker => "Worker",
                UnitKind::Footman => "Footman",
                UnitKind::Archer => "Archer",
                UnitKind::Hero => "Champion",
                UnitKind::Catapult => "Catapult",
                UnitKind::Raider => "Raider",
                UnitKind::Priestess => "Priestess",
                UnitKind::Spearman => "Spearman",
                UnitKind::Sorcerer => "Sorcerer",
                UnitKind::Knight => "Knight",
                UnitKind::GryphonRider => "Gryphon Rider",
            }),
        ));

        // Every hero class carries progression state and an (empty) item bag.
        if let Some(hero) = hero {
            entity.insert((hero, Inventory::default()));
        }

        // Set when a `DoctrineTemplate` had an explicit opinion about
        // auto-cast, so the per-kind default below does not overrule it.
        let mut stamped_autocast = false;

        // Standing doctrine from the producing building, applied verbatim.
        // Deliberately touches nothing but doctrine components — the rally
        // point above already decided this unit's initial `Order`.
        if let Some(template) = template {
            if let Some(squad) = template.squad {
                entity.insert(SquadId(squad));
            }
            if let Some(retreat) = template.retreat {
                entity.insert(retreat);
            }
            if let Some(priority) = &template.priority {
                entity.insert(TargetPriority(priority.clone()));
            }
            // Anything with an ability can auto-cast it — heroes were merely
            // the only casters that existed when this was written, and the
            // Sorcerer's whole value is a spell it fires on its own.
            if !abilities_of_unit(ev.kind).is_empty() {
                if let Some(min_enemies) = template.autocast {
                    // Templates speak the v1 language (one number); it lands on
                    // the trained unit's first ability slot.
                    entity.insert(AutoCastPolicy::first(min_enemies));
                    stamped_autocast = true;
                }
            }
        }

        // Kinds whose ability is meant to be on by default (the Sorcerer's
        // Slow) are born with it. A template that said something about
        // auto-cast wins — that is a standing order the player actually gave.
        if !stamped_autocast {
            if let Some((slot, min_enemies)) = default_autocast(ev.kind) {
                let mut policy = AutoCastPolicy::default();
                policy.set(slot, min_enemies);
                entity.insert(policy);
            }
        }

        entity.with_children(|parent| {
            // Head (every kind that is a person — the catapult is a machine).
            if ev.kind != UnitKind::Catapult {
                parent.spawn((
                    Mesh3d(assets.head.clone()),
                    MeshMaterial3d(assets.skin_mat.clone()),
                    Transform::from_xyz(0.0, head_y, 0.0),
                    UnitAccent,
                ));
            }

            match ev.kind {
                UnitKind::Worker => {
                    // Backpack behind the body (+Z is "back" for look_to facing).
                    parent.spawn((
                        Mesh3d(assets.worker_pack.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.0, 0.10, 0.34),
                        UnitAccent,
                    ));
                }
                UnitKind::Footman => {
                    // Shield strapped to the left arm, tilted slightly forward.
                    parent.spawn((
                        Mesh3d(assets.footman_shield.clone()),
                        MeshMaterial3d(assets.metal_mat.clone()),
                        Transform::from_xyz(-0.40, 0.08, -0.10)
                            .with_rotation(Quat::from_rotation_z(-0.18)),
                        UnitAccent,
                    ));
                }
                UnitKind::Archer => {
                    // Thin bow held out to the right side.
                    parent.spawn((
                        Mesh3d(assets.archer_bow.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.34, 0.05, -0.06)
                            .with_rotation(Quat::from_rotation_x(0.25)),
                        UnitAccent,
                    ));
                }
                UnitKind::Hero => {
                    // Gold crown sitting on the head.
                    parent.spawn((
                        Mesh3d(assets.hero_crown.clone()),
                        MeshMaterial3d(assets.gold_mat.clone()),
                        Transform::from_xyz(0.0, head_y + 0.16, 0.0),
                        UnitAccent,
                    ));
                    // Greatsword shouldered on the right.
                    parent.spawn((
                        Mesh3d(assets.hero_blade.clone()),
                        MeshMaterial3d(assets.gold_mat.clone()),
                        Transform::from_xyz(0.46, 0.22, 0.02)
                            .with_rotation(Quat::from_rotation_z(-0.30)),
                        UnitAccent,
                    ));
                }
                UnitKind::Catapult => {
                    // Four small dark wheels, axles along X (cylinders point
                    // up by default, so roll them a quarter turn about Z).
                    for (x, z) in [(-0.50, -0.45), (0.50, -0.45), (-0.50, 0.45), (0.50, 0.45)] {
                        parent.spawn((
                            Mesh3d(assets.catapult_wheel.clone()),
                            MeshMaterial3d(assets.dark_mat.clone()),
                            Transform::from_xyz(x, -0.30, z).with_rotation(
                                Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                            ),
                            UnitAccent,
                        ));
                    }
                    // Throwing arm, cocked back over the chassis (-Z is front).
                    parent.spawn((
                        Mesh3d(assets.catapult_arm.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.0, 0.16, 0.18)
                            .with_rotation(Quat::from_rotation_x(0.80)),
                        UnitAccent,
                    ));
                    // Team-coloured plate bolted across the front of the frame.
                    parent.spawn((
                        Mesh3d(assets.catapult_panel.clone()),
                        MeshMaterial3d(team_mat.clone()),
                        Transform::from_xyz(0.0, 0.02, -0.70),
                        UnitAccent,
                    ));
                }
                UnitKind::Raider => {
                    // The mount: a capsule laid down along Z (front-to-back),
                    // slung low so the rider straddles it.
                    parent.spawn((
                        Mesh3d(assets.raider_mount.clone()),
                        MeshMaterial3d(assets.dark_mat.clone()),
                        Transform::from_xyz(0.0, -0.15, 0.0)
                            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                        UnitAccent,
                    ));
                    // Four short legs under the barrel of the mount.
                    for (x, z) in [(-0.20, -0.36), (0.20, -0.36), (-0.20, 0.36), (0.20, 0.36)] {
                        parent.spawn((
                            Mesh3d(assets.raider_leg.clone()),
                            MeshMaterial3d(assets.dark_mat.clone()),
                            Transform::from_xyz(x, -0.58, z),
                            UnitAccent,
                        ));
                    }
                }
                UnitKind::Spearman => {
                    // The spear itself: a thin shaft held upright at the right
                    // shoulder, deliberately overlong so a block of them reads
                    // as a hedge from the RTS camera.
                    parent.spawn((
                        Mesh3d(assets.spearman_shaft.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.30, 0.52, -0.04),
                        UnitAccent,
                    ));
                    // Leaf-blade point on top of the shaft.
                    parent.spawn((
                        Mesh3d(assets.spearman_head.clone()),
                        MeshMaterial3d(assets.metal_mat.clone()),
                        Transform::from_xyz(0.30, 1.52, -0.04),
                        UnitAccent,
                    ));
                }
                UnitKind::Sorcerer => {
                    // Pointed hood over the head — the one shape on the field
                    // that comes to a point.
                    parent.spawn((
                        Mesh3d(assets.sorcerer_hood.clone()),
                        MeshMaterial3d(assets.dark_mat.clone()),
                        Transform::from_xyz(0.0, head_y + 0.20, 0.0),
                        UnitAccent,
                    ));
                    // The orb: unattached, hanging off the right shoulder.
                    // No haft, no string, no blade — this unit carries no
                    // weapon at all, and that is the silhouette's whole job.
                    parent.spawn((
                        Mesh3d(assets.sorcerer_orb.clone()),
                        MeshMaterial3d(assets.arcane_mat.clone()),
                        Transform::from_xyz(0.38, 0.30, -0.12),
                        UnitAccent,
                    ));
                }
                UnitKind::Knight => {
                    // The barded warhorse: a steel-grey capsule laid along Z,
                    // slung under the rider exactly like the Raider's mount but
                    // heavier and metal rather than beast-dark.
                    parent.spawn((
                        Mesh3d(assets.knight_mount.clone()),
                        MeshMaterial3d(assets.metal_mat.clone()),
                        Transform::from_xyz(0.0, -0.18, 0.0)
                            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                        UnitAccent,
                    ));
                    // Four heavy legs under the barrel.
                    for (x, z) in [(-0.23, -0.40), (0.23, -0.40), (-0.23, 0.40), (0.23, 0.40)] {
                        parent.spawn((
                            Mesh3d(assets.knight_leg.clone()),
                            MeshMaterial3d(assets.dark_mat.clone()),
                            Transform::from_xyz(x, -0.62, z),
                            UnitAccent,
                        ));
                    }
                    // The lance, couched under the right arm and levelled along
                    // -Z (the facing direction), reaching well past the horse's
                    // head. This is the line-breaker's whole story in one prop.
                    parent.spawn((
                        Mesh3d(assets.knight_lance.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.34, 0.02, -0.55)
                            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                        UnitAccent,
                    ));
                }
                UnitKind::GryphonRider => {
                    // The beast: a capsule laid along Z under the rider.
                    parent.spawn((
                        Mesh3d(assets.gryphon_body.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.0, -0.20, 0.05)
                            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                        UnitAccent,
                    ));
                    // Wings: two broad slabs swept out and up from the
                    // shoulders. Nearly 3.5 units across at unit scale, which
                    // is wider than any ground unit is tall — the silhouette IS
                    // the unit's rules, readable from the camera at altitude.
                    for (x, roll) in [(-0.85_f32, 0.30_f32), (0.85, -0.30)] {
                        parent.spawn((
                            Mesh3d(assets.gryphon_wing.clone()),
                            MeshMaterial3d(assets.robe_mat.clone()),
                            Transform::from_xyz(x, -0.02, 0.05)
                                .with_rotation(Quat::from_rotation_z(roll)),
                            UnitAccent,
                        ));
                    }
                    // Tail streaming behind (+Z is "back" for look_to facing).
                    parent.spawn((
                        Mesh3d(assets.gryphon_tail.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.0, -0.20, 0.72)
                            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                        UnitAccent,
                    ));
                }
                UnitKind::Priestess => {
                    // Hood: a pale sphere pulled over the head.
                    parent.spawn((
                        Mesh3d(assets.priestess_hood.clone()),
                        MeshMaterial3d(assets.robe_mat.clone()),
                        Transform::from_xyz(0.0, head_y + 0.04, 0.02),
                        UnitAccent,
                    ));
                    // Staff held out to the right...
                    parent.spawn((
                        Mesh3d(assets.priestess_staff.clone()),
                        MeshMaterial3d(assets.wood_mat.clone()),
                        Transform::from_xyz(0.36, 0.15, -0.04),
                        UnitAccent,
                    ));
                    // ...capped with a lit crystal.
                    parent.spawn((
                        Mesh3d(assets.priestess_tip.clone()),
                        MeshMaterial3d(assets.glow_mat.clone()),
                        Transform::from_xyz(0.36, 0.86, -0.04),
                        UnitAccent,
                    ));
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// 1b. Teleports (Town Portal)
// ---------------------------------------------------------------------------

/// `TeleportRequest`: yank `center` and every same-team unit within `radius`
/// (XZ, measured from the centre's PRE-teleport position) to `dest` — minus
/// workers, when the request says `army_only`.
///
/// This module owns Transforms, so the whole thing happens here. Each unit
/// keeps its own `unit_y` and its relative offset to the centre (clamped, so an
/// arriving group lands as a loose blob instead of one stack), and everyone
/// moved has `MoveTo` + path state cleared — they arrive standing still.
/// `pub(crate)` for one reason: a destination is now a DECISION, and the only
/// test that can check the decision was honoured is one that runs the item and
/// the move together — combat.rs picks the hall, this system is what actually
/// puts the army on it. See the probe in combat.rs's test module.
pub(crate) fn handle_teleports(
    mut commands: Commands,
    mut events: EventReader<TeleportRequest>,
    nav: Res<NavGrid>,
    mut units: Query<(
        Entity,
        &Unit,
        &Team,
        &Health,
        &mut Transform,
        &mut GlobalTransform,
    )>,
) {
    /// How far from the destination a passenger may be placed.
    const SPREAD: f32 = 3.0;

    for ev in events.read() {
        // The caster may have died between the request and now.
        let Ok((_, _, center_team, center_health, center_tf, _)) = units.get(ev.center) else {
            continue;
        };
        if center_health.current <= 0.0 {
            continue;
        }
        let team = *center_team;
        let origin = center_tf.translation;

        let mut dest = ev.dest;
        dest.x = dest.x.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0);
        dest.z = dest.z.clamp(-MAP_HALF + 2.0, MAP_HALF - 2.0);

        // Snapshot the passenger list first (the query is borrowed mutably below).
        let riders: Vec<(Entity, Vec3)> = units
            .iter()
            .filter(|(entity, unit, unit_team, health, tf, _)| {
                // The caster always rides its own teleport.
                if *entity == ev.center {
                    return true;
                }
                if **unit_team != team || health.current <= 0.0 {
                    return false;
                }
                // `army_only` is what makes a MAP-WIDE recall (Scroll of Mass
                // Teleport, radius > the map) an army move instead of an
                // economy wipe: workers keep mining. A Town Portal sets it
                // false and behaves exactly as it always has.
                if ev.army_only && unit.kind == UnitKind::Worker {
                    return false;
                }
                Vec2::new(tf.translation.x - origin.x, tf.translation.z - origin.z).length()
                    <= ev.radius
            })
            .map(|(entity, _, _, _, tf, _)| {
                let rel = Vec3::new(tf.translation.x - origin.x, 0.0, tf.translation.z - origin.z);
                let len = rel.length();
                let offset = if len > SPREAD { rel * (SPREAD / len) } else { rel };
                (entity, offset)
            })
            .collect();

        for (entity, offset) in riders {
            let mut spot = Vec3::new(dest.x + offset.x, 0.0, dest.z + offset.z);
            spot.x = spot.x.clamp(-MAP_HALF + 1.0, MAP_HALF - 1.0);
            spot.z = spot.z.clamp(-MAP_HALF + 1.0, MAP_HALF - 1.0);
            // Never materialise inside a building or a tree — unless you fly
            // over both anyway.
            let flying = units
                .get(entity)
                .map(|(_, unit, ..)| is_flying_kind(unit.kind))
                .unwrap_or(false);
            if !flying && nav.is_blocked_world(spot) {
                if let Some(cell) = NavGrid::world_to_cell(spot) {
                    if let Some(free) = nearest_free_cell(&nav, cell) {
                        spot = NavGrid::cell_to_world(free.0, free.1);
                    }
                }
            }
            if let Ok((_, unit, _, _, mut tf, mut gt)) = units.get_mut(entity) {
                tf.translation = Vec3::new(spot.x, unit_y(unit.kind), spot.z);
                // Same hole as a fresh spawn, in reverse: Bevy only propagates
                // Transform -> GlobalTransform in PostUpdate, so for the rest
                // of THIS frame combat.rs (which reads positions through
                // GlobalTransform) would still see the passenger standing
                // where it used to be — a hero town-portalling out of a fight
                // could be struck once at the position it had already left.
                // A unit is a root entity, so its GlobalTransform simply IS
                // its Transform; write it here and the hole never opens.
                *gt = GlobalTransform::from(*tf);
            }
            // Whatever they were walking toward is on the other side of the map.
            commands
                .entity(entity)
                .try_remove::<(MoveTo, PathFollow, FollowState)>();
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Order execution (movement only)
// ---------------------------------------------------------------------------

fn orders_to_movement(
    mut commands: Commands,
    query: Query<(Entity, &Order), (Changed<Order>, With<Unit>)>,
) {
    for (entity, order) in &query {
        match order {
            Order::Move(target) | Order::AttackMove(target) => {
                commands
                    .entity(entity)
                    .try_insert(MoveTo { target: *target });
            }
            // Follow is re-evaluated continuously by `follow_orders` below.
            // Attack -> combat.rs, Harvest/ReturnResources/Build -> economy.rs.
            _ => {}
        }
    }
}

/// `Order::Follow(target)`: stay loosely glued to another unit.
///
/// Re-issues `MoveTo` at most `FOLLOW_INTERVAL` apart (or immediately once the
/// followee has drifted more than `FOLLOW_MOVE_EPSILON`), stops inside
/// `FOLLOW_STOP_DIST`, and degrades to `Idle` when the followee dies.
fn follow_orders(
    mut commands: Commands,
    time: Res<Time>,
    // `&mut FollowState` makes this query mutable, but Transform is read-only
    // in both queries below, so they can never alias.
    mut followers: Query<
        (
            Entity,
            &Order,
            &Transform,
            Option<&mut FollowState>,
            Has<MoveTo>,
        ),
        With<Unit>,
    >,
    followees: Query<&Transform, With<Unit>>,
) {
    let dt = time.delta_secs();

    for (entity, order, tf, state, moving) in &mut followers {
        let Order::Follow(followee) = *order else {
            // Order changed away from Follow — drop the throttle state.
            if state.is_some() {
                commands.entity(entity).try_remove::<FollowState>();
            }
            continue;
        };

        // Followee despawned (or is us) — nothing to follow any more.
        let target_tf = match followees.get(followee) {
            Ok(target_tf) if followee != entity => target_tf,
            _ => {
                commands
                    .entity(entity)
                    .try_remove::<(FollowState, MoveTo)>()
                    .try_insert(Order::Idle);
                continue;
            }
        };

        let target_pos = Vec3::new(target_tf.translation.x, 0.0, target_tf.translation.z);
        let pos = Vec3::new(tf.translation.x, 0.0, tf.translation.z);

        // --- throttle -----------------------------------------------------
        match state {
            Some(mut state) => {
                state.cooldown -= dt;
                let drifted = state.last_target_pos.distance(target_pos) > FOLLOW_MOVE_EPSILON;
                if state.cooldown > 0.0 && !drifted {
                    continue;
                }
                state.cooldown = FOLLOW_INTERVAL;
                state.last_target_pos = target_pos;
            }
            None => {
                // First tick of this Follow: act now, throttle from here on.
                commands.entity(entity).try_insert(FollowState {
                    cooldown: FOLLOW_INTERVAL,
                    last_target_pos: target_pos,
                });
            }
        }

        // --- chase / hold --------------------------------------------------
        if pos.distance(target_pos) > FOLLOW_STOP_DIST {
            // Aim for a spot just short of the followee so groups don't all
            // converge on the exact same point.
            let away = (pos - target_pos).normalize_or_zero();
            let mut stand = target_pos + away * (FOLLOW_STOP_DIST * 0.6);
            stand.x = stand.x.clamp(-MAP_HALF + 1.0, MAP_HALF - 1.0);
            stand.z = stand.z.clamp(-MAP_HALF + 1.0, MAP_HALF - 1.0);
            commands.entity(entity).try_insert(MoveTo { target: stand });
        } else if moving {
            // Close enough: release the movement request.
            commands.entity(entity).try_remove::<(MoveTo, PathFollow)>();
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Pathfinding
// ---------------------------------------------------------------------------

/// Reusable A* scratch buffers (avoids allocating 10k-entry vectors per query).
#[derive(Resource)]
struct PathScratch {
    g: Vec<f32>,
    came: Vec<u32>,
    /// Generation stamp per cell so we never have to clear `g`/`came`.
    seen: Vec<u32>,
    closed: Vec<u32>,
    generation: u32,
    open: BinaryHeap<std::cmp::Reverse<(u32, u32)>>,
}

impl Default for PathScratch {
    fn default() -> Self {
        let n = GRID_DIM * GRID_DIM;
        PathScratch {
            g: vec![0.0; n],
            came: vec![u32::MAX; n],
            seen: vec![0; n],
            closed: vec![0; n],
            generation: 0,
            open: BinaryHeap::new(),
        }
    }
}

fn compute_paths(
    mut commands: Commands,
    nav: Res<NavGrid>,
    mut scratch: ResMut<PathScratch>,
    query: Query<
        (Entity, &Unit, &Transform, &MoveTo, Option<&PathFollow>),
        Or<(Changed<MoveTo>, Without<PathFollow>)>,
    >,
) {
    for (entity, unit, transform, move_to, existing) in &query {
        let repaths = existing.map(|p| p.repaths).unwrap_or(0);
        let from = transform.translation;

        // Flying: no grid, no search, no unreachable. One waypoint — the
        // destination — and the steering below flies the straight line to it,
        // over walls, forests, mines and the enemy's whole tower net. This is
        // the entire "bypass" in one branch; everything else about a flyer is
        // an ordinary unit.
        let plan = if is_flying_kind(unit.kind) {
            Some(vec![clamp_to_map_xz(move_to.target)])
        } else {
            plan_path(&nav, &mut scratch, from, move_to.target)
        };

        match plan {
            Some(waypoints) if !waypoints.is_empty() => {
                commands.entity(entity).try_insert(PathFollow {
                    waypoints,
                    index: 0,
                    last_sample_pos: from,
                    sample_timer: 0.0,
                    stuck_time: 0.0,
                    repaths,
                });
            }
            _ => {
                // Unreachable (or already there): drop MoveTo — that is the
                // cross-module "not moving any more" signal.
                commands
                    .entity(entity)
                    .try_remove::<(MoveTo, PathFollow)>();
            }
        }
    }
}

/// Flatten to the ground plane and keep inside the map bounds. Flying paths
/// use this in place of the whole A* pipeline.
fn clamp_to_map_xz(p: Vec3) -> Vec3 {
    Vec3::new(
        p.x.clamp(-MAP_HALF + 0.5, MAP_HALF - 0.5),
        0.0,
        p.z.clamp(-MAP_HALF + 0.5, MAP_HALF - 0.5),
    )
}

/// Full plan: resolve start/goal cells, run A*, convert to world waypoints and
/// string-pull them. Returns `None` when no path exists.
fn plan_path(nav: &NavGrid, scratch: &mut PathScratch, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
    let mut target = to;
    target.x = target.x.clamp(-MAP_HALF + 0.5, MAP_HALF - 0.5);
    target.z = target.z.clamp(-MAP_HALF + 0.5, MAP_HALF - 0.5);

    let start_cell = NavGrid::world_to_cell(from)?;
    let goal_cell_raw = NavGrid::world_to_cell(target)?;

    // If the unit is standing inside a blocked cell (e.g. a building was just
    // dropped on it) start from the closest free cell instead.
    let start_cell = if nav.is_blocked(start_cell.0, start_cell.1) {
        nearest_free_cell(nav, start_cell)?
    } else {
        start_cell
    };

    // If the destination is blocked, aim for the nearest free cell to it.
    let (goal_cell, goal_was_blocked) = if nav.is_blocked(goal_cell_raw.0, goal_cell_raw.1) {
        (nearest_free_cell(nav, goal_cell_raw)?, true)
    } else {
        (goal_cell_raw, false)
    };

    // Already in the goal cell: single direct waypoint (no A* needed).
    if start_cell == goal_cell {
        let end = if goal_was_blocked {
            NavGrid::cell_to_world(goal_cell.0, goal_cell.1)
        } else {
            Vec3::new(target.x, 0.0, target.z)
        };
        return Some(vec![end]);
    }

    let cells = astar(nav, scratch, start_cell, goal_cell)?;

    // Cell centres -> world waypoints; the last one snaps to the true target
    // when that spot is actually walkable.
    let mut points: Vec<Vec3> = cells
        .iter()
        .map(|&(cx, cz)| NavGrid::cell_to_world(cx, cz))
        .collect();
    if !goal_was_blocked {
        if let Some(last) = points.last_mut() {
            *last = Vec3::new(target.x, 0.0, target.z);
        }
    }

    Some(simplify_path(nav, from, points))
}

/// 8-connected A* with no corner cutting. Returns the cell path *excluding* the
/// start cell.
fn astar(
    nav: &NavGrid,
    scratch: &mut PathScratch,
    start: (usize, usize),
    goal: (usize, usize),
) -> Option<Vec<(usize, usize)>> {
    const DIAG: f32 = std::f32::consts::SQRT_2;
    const NEIGHBORS: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];

    scratch.generation = scratch.generation.wrapping_add(1);
    if scratch.generation == 0 {
        // Wrapped around: hard reset so stale stamps can't alias.
        scratch.seen.iter_mut().for_each(|s| *s = 0);
        scratch.closed.iter_mut().for_each(|s| *s = 0);
        scratch.generation = 1;
    }
    let gen = scratch.generation;
    scratch.open.clear();

    let goal_idx = NavGrid::idx(goal.0, goal.1);
    let start_idx = NavGrid::idx(start.0, start.1);

    scratch.g[start_idx] = 0.0;
    scratch.came[start_idx] = u32::MAX;
    scratch.seen[start_idx] = gen;
    scratch
        .open
        .push(std::cmp::Reverse((score(octile(start, goal)), start_idx as u32)));

    let mut expansions = 0u32;
    let mut found = false;

    while let Some(std::cmp::Reverse((_, idx))) = scratch.open.pop() {
        let idx = idx as usize;
        if scratch.closed[idx] == gen {
            continue;
        }
        scratch.closed[idx] = gen;

        if idx == goal_idx {
            found = true;
            break;
        }

        expansions += 1;
        if expansions > MAX_EXPANSIONS {
            break;
        }

        let cx = (idx % GRID_DIM) as i32;
        let cz = (idx / GRID_DIM) as i32;
        let base_g = scratch.g[idx];

        for (dx, dz) in NEIGHBORS {
            let nx = cx + dx;
            let nz = cz + dz;
            if nx < 0 || nz < 0 || nx >= GRID_DIM as i32 || nz >= GRID_DIM as i32 {
                continue;
            }
            let (nxu, nzu) = (nx as usize, nz as usize);
            if nav.is_blocked(nxu, nzu) {
                continue;
            }
            let diagonal = dx != 0 && dz != 0;
            if diagonal {
                // No corner cutting: both orthogonal neighbours must be free.
                if nav.is_blocked((cx + dx) as usize, cz as usize)
                    || nav.is_blocked(cx as usize, (cz + dz) as usize)
                {
                    continue;
                }
            }
            let n_idx = NavGrid::idx(nxu, nzu);
            if scratch.closed[n_idx] == gen {
                continue;
            }
            let tentative = base_g + if diagonal { DIAG } else { 1.0 };
            if scratch.seen[n_idx] == gen && scratch.g[n_idx] <= tentative {
                continue;
            }
            scratch.seen[n_idx] = gen;
            scratch.g[n_idx] = tentative;
            scratch.came[n_idx] = idx as u32;
            let f = tentative + octile((nxu, nzu), goal);
            scratch
                .open
                .push(std::cmp::Reverse((score(f), n_idx as u32)));
        }
    }

    if !found {
        return None;
    }

    // Walk the parent chain back to the start.
    let mut path = Vec::new();
    let mut cur = goal_idx;
    while cur != start_idx {
        path.push((cur % GRID_DIM, cur / GRID_DIM));
        let parent = scratch.came[cur];
        if parent == u32::MAX {
            break;
        }
        cur = parent as usize;
        if path.len() > GRID_DIM * GRID_DIM {
            return None;
        }
    }
    path.reverse();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Octile heuristic in cell units.
fn octile(a: (usize, usize), b: (usize, usize)) -> f32 {
    let dx = (a.0 as f32 - b.0 as f32).abs();
    let dz = (a.1 as f32 - b.1 as f32).abs();
    let (lo, hi) = if dx < dz { (dx, dz) } else { (dz, dx) };
    hi - lo + std::f32::consts::SQRT_2 * lo
}

/// Fixed-point key so f32 scores can live in a `BinaryHeap`.
fn score(f: f32) -> u32 {
    (f.max(0.0) * 64.0) as u32
}

/// Breadth-ish spiral search for the closest walkable cell.
fn nearest_free_cell(nav: &NavGrid, from: (usize, usize)) -> Option<(usize, usize)> {
    if !nav.is_blocked(from.0, from.1) {
        return Some(from);
    }
    let (fx, fz) = (from.0 as i32, from.1 as i32);
    for radius in 1..=25i32 {
        let mut best: Option<((usize, usize), i32)> = None;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                // Only the ring border.
                if dx.abs() != radius && dz.abs() != radius {
                    continue;
                }
                let (nx, nz) = (fx + dx, fz + dz);
                if nx < 0 || nz < 0 || nx >= GRID_DIM as i32 || nz >= GRID_DIM as i32 {
                    continue;
                }
                let (nxu, nzu) = (nx as usize, nz as usize);
                if nav.is_blocked(nxu, nzu) {
                    continue;
                }
                let d2 = dx * dx + dz * dz;
                if best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
                    best = Some(((nxu, nzu), d2));
                }
            }
        }
        if let Some((cell, _)) = best {
            return Some(cell);
        }
    }
    None
}

/// Grid raycast: true when the straight segment a→b only crosses free cells.
fn has_line_of_sight(nav: &NavGrid, a: Vec3, b: Vec3) -> bool {
    let delta = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let dist = delta.length();
    if dist < 1e-4 {
        return true;
    }
    let dir = delta / dist;
    // Sample the centre line plus a body-width offset on each side so a
    // shortcut never grazes a building corner.
    let side = Vec3::new(-dir.z, 0.0, dir.x) * (UNIT_RADIUS * 0.8);
    let step = CELL * 0.4;
    let steps = (dist / step).ceil().max(1.0) as i32;
    for i in 0..=steps {
        let t = (i as f32 / steps as f32).min(1.0);
        let p = Vec3::new(a.x + delta.x * t, 0.0, a.z + delta.z * t);
        if nav.is_blocked_world(p) || nav.is_blocked_world(p + side) || nav.is_blocked_world(p - side)
        {
            return false;
        }
    }
    true
}

/// String-pull: drop waypoints that can be skipped with clear line of sight.
fn simplify_path(nav: &NavGrid, from: Vec3, points: Vec<Vec3>) -> Vec<Vec3> {
    if points.len() <= 1 {
        return points;
    }
    let mut out: Vec<Vec3> = Vec::with_capacity(points.len());
    let mut anchor = Vec3::new(from.x, 0.0, from.z);
    let mut i = 0usize;
    while i < points.len() {
        // Farthest reachable point from the current anchor (bounded lookahead).
        let limit = (i + 24).min(points.len() - 1);
        let mut best = i;
        let mut j = limit;
        while j > i {
            if has_line_of_sight(nav, anchor, points[j]) {
                best = j;
                break;
            }
            j -= 1;
        }
        out.push(points[best]);
        anchor = points[best];
        i = best + 1;
    }
    // Always keep the true destination.
    if let (Some(last_in), Some(last_out)) = (points.last(), out.last()) {
        if last_in.distance_squared(*last_out) > 1e-4 {
            out.push(*last_in);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 4. Steering
// ---------------------------------------------------------------------------

fn steer_units(
    mut commands: Commands,
    time: Res<Time>,
    nav: Res<NavGrid>,
    mut query: Query<(
        Entity,
        &Unit,
        &mut Transform,
        &mut MoveTo,
        &mut PathFollow,
        Option<&StatusEffects>,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (entity, unit, mut transform, mut move_to, mut path, status) in &mut query {
        let stats = unit_stats(unit.kind);
        // Move speed is asked for, never looked up: Slow and Haste both land
        // here, through shared.rs's one modifier function.
        let effective = effective_unit_stats(unit.kind, status);
        let y = unit_y(unit.kind);
        let pos = transform.translation;
        let flat = Vec3::new(pos.x, 0.0, pos.z);
        let target = Vec3::new(move_to.target.x, 0.0, move_to.target.z);

        // --- arrival on the real target ---------------------------------
        if flat.distance(target) <= ARRIVE_RADIUS {
            commands
                .entity(entity)
                .try_remove::<(MoveTo, PathFollow)>();
            continue;
        }

        // --- advance through waypoints ----------------------------------
        while path.index < path.waypoints.len()
            && flat.distance(path.waypoints[path.index]) <= WAYPOINT_RADIUS
        {
            path.index += 1;
        }
        let Some(&waypoint) = path.waypoints.get(path.index) else {
            // Reached the end of the path (target may have been a blocked cell).
            commands
                .entity(entity)
                .try_remove::<(MoveTo, PathFollow)>();
            continue;
        };

        // --- repath if the way ahead became blocked ---------------------
        // Airspace is never blocked, so a flyer never repaths for terrain —
        // and a wall thrown up across its route is simply not its problem.
        if !stats.flying && nav.is_blocked_world(waypoint) {
            move_to.set_changed();
            continue;
        }

        // --- move ---------------------------------------------------------
        let to_wp = waypoint - flat;
        let dist = to_wp.length();
        if dist > 1e-4 {
            let dir = to_wp / dist;
            let step = (effective.speed * dt).min(dist);
            transform.translation.x += dir.x * step;
            transform.translation.z += dir.z * step;

            // Face the direction of travel.
            let mut wanted = *transform;
            wanted.look_to(dir, Vec3::Y);
            let t = (TURN_RATE * dt).clamp(0.0, 1.0);
            transform.rotation = transform.rotation.slerp(wanted.rotation, t);
        }
        transform.translation.y = y;

        // --- stuck detection ---------------------------------------------
        path.sample_timer += dt;
        if path.sample_timer >= STUCK_SAMPLE {
            let moved = Vec3::new(transform.translation.x, 0.0, transform.translation.z)
                .distance(Vec3::new(path.last_sample_pos.x, 0.0, path.last_sample_pos.z));
            let elapsed = path.sample_timer;
            path.sample_timer = 0.0;
            path.last_sample_pos = transform.translation;

            if moved < STUCK_EPSILON {
                path.stuck_time += elapsed;
            } else {
                path.stuck_time = 0.0;
                if moved > 1.0 {
                    path.repaths = 0;
                }
            }

            if path.stuck_time >= STUCK_TIME {
                path.stuck_time = 0.0;
                if path.repaths >= MAX_REPATHS {
                    // Hopelessly stuck — release the movement request so other
                    // modules stop waiting on it.
                    commands
                        .entity(entity)
                        .try_remove::<(MoveTo, PathFollow)>();
                } else {
                    path.repaths += 1;
                    move_to.set_changed();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Local separation
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SepScratch {
    /// (entity, team, position, flying)
    entries: Vec<(Entity, Team, Vec3, bool)>,
    pushes: Vec<Vec3>,
    buckets: HashMap<(i32, i32), Vec<usize>>,
}

fn bucket_of(pos: Vec3) -> (i32, i32) {
    ((pos.x / SEPARATION_DIST).floor() as i32, (pos.z / SEPARATION_DIST).floor() as i32)
}

fn separate_units(
    time: Res<Time>,
    nav: Res<NavGrid>,
    mut query: Query<(Entity, &Team, &Unit, &mut Transform)>,
    mut scratch: Local<SepScratch>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    let scratch = &mut *scratch;
    scratch.entries.clear();
    scratch.pushes.clear();
    for bucket in scratch.buckets.values_mut() {
        bucket.clear();
    }

    for (entity, team, unit, transform) in &query {
        scratch
            .entries
            .push((entity, *team, transform.translation, is_flying_kind(unit.kind)));
    }
    if scratch.entries.len() < 2 {
        return;
    }
    scratch.pushes.resize(scratch.entries.len(), Vec3::ZERO);

    for (i, (_, _, pos, _)) in scratch.entries.iter().enumerate() {
        scratch.buckets.entry(bucket_of(*pos)).or_default().push(i);
    }

    let max_push = SEPARATION_SPEED * dt;

    for i in 0..scratch.entries.len() {
        let (_, team_i, pos_i, flying_i) = scratch.entries[i];
        let (bx, bz) = bucket_of(pos_i);
        let mut push = Vec3::ZERO;

        for ox in -1..=1 {
            for oz in -1..=1 {
                let Some(bucket) = scratch.buckets.get(&(bx + ox, bz + oz)) else {
                    continue;
                };
                for &j in bucket {
                    if j == i {
                        continue;
                    }
                    let (_, team_j, pos_j, flying_j) = scratch.entries[j];
                    // Two different traffic layers: a flyer and a ground unit
                    // sharing an XZ cell are stacked, not collided, so they
                    // never push each other. Flyers jostle flyers, walkers
                    // jostle walkers.
                    if flying_i != flying_j {
                        continue;
                    }
                    let mut delta = Vec3::new(pos_i.x - pos_j.x, 0.0, pos_i.z - pos_j.z);
                    let dist = delta.length();
                    if dist >= SEPARATION_DIST {
                        continue;
                    }
                    if dist < 1e-3 {
                        // Exactly stacked: deterministic tie-break direction.
                        let a = i as f32 * 2.399_963;
                        delta = Vec3::new(a.cos(), 0.0, a.sin());
                    } else {
                        delta /= dist;
                    }
                    // Friendlies jostle fully; enemies only lightly (combat.rs
                    // owns their spacing behaviour).
                    let strength = if team_i == team_j { 1.0 } else { 0.4 };
                    push += delta * (SEPARATION_DIST - dist) * 0.5 * strength;
                }
            }
        }

        let len = push.length();
        if len > max_push {
            push *= max_push / len;
        }
        scratch.pushes[i] = push;
    }

    for i in 0..scratch.entries.len() {
        let push = scratch.pushes[i];
        if push.length_squared() < 1e-8 {
            continue;
        }
        let (entity, _, _, flying) = scratch.entries[i];
        let Ok((_, _, _, mut transform)) = query.get_mut(entity) else {
            continue;
        };
        let old = transform.translation;
        let mut candidate = Vec3::new(old.x + push.x, old.y, old.z + push.z);

        // Never shove a unit into a blocked cell — retry one axis at a time.
        // Flyers have no such cells to be shoved into.
        if !flying && nav.is_blocked_world(candidate) {
            let x_only = Vec3::new(old.x + push.x, old.y, old.z);
            let z_only = Vec3::new(old.x, old.y, old.z + push.z);
            if !nav.is_blocked_world(x_only) {
                candidate = x_only;
            } else if !nav.is_blocked_world(z_only) {
                candidate = z_only;
            } else {
                continue;
            }
        }

        candidate.x = candidate.x.clamp(-MAP_HALF + 0.5, MAP_HALF - 0.5);
        candidate.z = candidate.z.clamp(-MAP_HALF + 0.5, MAP_HALF - 0.5);
        transform.translation = candidate;
    }
}

// ---------------------------------------------------------------------------
// 6. Housekeeping
// ---------------------------------------------------------------------------

/// If `MoveTo` was removed by another module (or by us), the path state goes
/// with it.
fn cleanup_orphan_paths(
    mut commands: Commands,
    query: Query<Entity, (With<PathFollow>, Without<MoveTo>)>,
) {
    for entity in &query {
        commands.entity(entity).try_remove::<PathFollow>();
    }
}
