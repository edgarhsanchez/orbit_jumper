//! Alien raiders: the hostile half of "fighting off alien ships".
//!
//! Raiders spawn in-system on a cadence that scales with pilot level —
//! the universe answers a stronger pilot with bigger hunting packs, so
//! combat pressure levels infinitely alongside the player. They seek the
//! ship, fire plasma bolts (shield first, then hull — the existing death
//! and salvage loop takes over from there), and die through the same
//! `Hull` reaper as practice drones, dropping a bounty in exotic
//! elements. True player-vs-player rides the future net phase; raiders
//! are the fight the game has today.

use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::modules::{CareerScore, RunScore};
use crate::sim::{BodyVel, DT, Ship, SystemScoped, TIME_WARP, ViewMode};
use crate::upgrades::pilot_level;
use crate::weapons::{Bounty, Hull};
use crate::{SimPos, SimVel};

/// A hostile ship. Combat stats are set at spawn from the pilot level of
/// the moment, so every wave is harder than the last: more damage per
/// bolt, faster bolts, shorter cooldowns, quicker approaches.
#[derive(Component)]
pub struct AlienShip {
    fire_cooldown: f64,
    cooldown: f64,
    damage: f64,
    bolt_speed: f64,
    approach: f64,
    /// Bolts per trigger pull — dreadnoughts volley.
    volley: u32,
}

/// Elite raider: a rarer, heavier variant that appears at higher pilot
/// levels. Marker drives the contacts label and nothing else — the stats
/// are already baked into `AlienShip`.
#[derive(Component)]
pub struct Elite;

/// The boss. One at a time, every few pilot levels, announced in the
/// threat line. Same AI loop as a raider — what changes is mass: a dozen
/// raiders' worth of hull, volley fire, and a bounty to match.
#[derive(Component)]
pub struct Dreadnought;

/// A plasma bolt in flight.
#[derive(Component)]
pub struct AlienBolt {
    damage: f64,
    ttl: f64,
}

/// Seconds (real) between spawn checks.
const SPAWN_PERIOD: f64 = 20.0;
/// Raiders open fire inside this range, m.
const FIRE_RANGE: f64 = 8.0e9;
/// Bolt proximity fuse, m.
const BOLT_FUSE: f64 = 5.0e8;

#[derive(Resource)]
struct RaiderClock(f64);

/// When the next dreadnought is owed (pilot level threshold).
#[derive(Resource)]
struct BossClock {
    next_level: u32,
}

/// One dot of a predicted raider path.
#[derive(Component)]
pub struct TrajDot;

/// The trajectory dot pool: spawned once, repositioned every frame.
#[derive(Resource, Default)]
struct TrajPool {
    dots: Vec<Entity>,
}

/// Prediction: steps ahead at this sim-second spacing (8 x 800 s ~ 13
/// real seconds of relative motion at warp 600).
const TRAJ_STEPS: usize = 8;
const TRAJ_STEP_SIM_S: f64 = 800.0;
const MAX_TRACKED: usize = 6;

pub struct AliensPlugin;

impl Plugin for AliensPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RaiderClock(8.0))
            .insert_resource(BossClock { next_level: 5 })
            .init_resource::<TrajPool>()
            .add_systems(
                FixedUpdate,
                (spawn_raiders, alien_ai, fly_bolts).chain(),
            )
            .add_systems(Update, project_trajectories);
    }
}

/// Keep the system stocked with raiders, harder with pilot level.
#[allow(clippy::too_many_arguments)]
fn spawn_raiders(
    mut clock: ResMut<RaiderClock>,
    mut boss_clock: ResMut<BossClock>,
    aliens: Query<(), With<AlienShip>>,
    bosses: Query<(), With<Dreadnought>>,
    ships: Query<(&SimPos, &SimVel), With<Ship>>,
    run: Res<RunScore>,
    career: Res<CareerScore>,
    game: Res<crate::GameUniverse>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    clock.0 -= DT;
    if clock.0 > 0.0 {
        return;
    }
    clock.0 = SPAWN_PERIOD;
    let Ok((ship_pos, ship_vel)) = ships.single() else { return };

    let level = pilot_level(career.total_score + run.total());

    // Arrive from a seed-random bearing, well outside weapons range,
    // velocity matched so the approach is deliberate, not a flyby.
    let mut rng = oj_universe::SplitMix64(
        game.current.index as u64 ^ (career.total_score + run.total()) ^ 0xA11E7,
    );
    let bearing = rng.range(0.0, std::f64::consts::TAU);
    let dist = rng.range(1.6e10, 2.4e10);
    let pos = ship_pos.0 + Vec3d::new(bearing.cos() * dist, bearing.sin() * dist, 0.0);

    // Every few pilot levels the pack yields to a DREADNOUGHT: one at a
    // time, a dozen raiders' worth of hull, volley fire, boss bounty.
    if level >= boss_clock.next_level && bosses.iter().count() == 0 {
        boss_clock.next_level = level + 5;
        spawn_hostile(
            &mut commands,
            &mut meshes,
            &mut materials,
            pos,
            ship_vel.0,
            HostileSpec::dreadnought(level),
        );
        info!("DREADNOUGHT inbound (level {level}); next owed at {}", boss_clock.next_level);
        return;
    }

    let pack_cap = (1 + level / 3).min(6) as usize;
    if aliens.iter().count() >= pack_cap {
        return;
    }

    // Elites appear from level 6, more often the deeper you go.
    let elite_chance = if level >= 6 {
        (0.12 + 0.02 * level as f64).min(0.5)
    } else {
        0.0
    };
    let elite = rng.range(0.0, 1.0) < elite_chance;
    let spec =
        if elite { HostileSpec::elite(level) } else { HostileSpec::raider(level) };
    spawn_hostile(&mut commands, &mut meshes, &mut materials, pos, ship_vel.0, spec);
    info!(
        "raider inbound (level {level}, pack cap {pack_cap}, elite {elite})"
    );
}

/// Everything that varies between a raider, an elite and a dreadnought.
struct HostileSpec {
    hp: f64,
    bounty: u64,
    damage: f64,
    cooldown: f64,
    bolt_speed: f64,
    approach: f64,
    scale: f32,
    elite: bool,
    boss: bool,
}

impl HostileSpec {
    /// Baseline raider at a pilot level — every stat climbs with it.
    fn raider(level: u32) -> Self {
        let l = level as f64;
        Self {
            hp: 30.0 * (1.0 + l * 0.15),
            bounty: 40 + level as u64 * 8,
            damage: 6.0 * (1.0 + l * 0.10),
            cooldown: (2.6 - 0.06 * l).max(1.2),
            bolt_speed: 1.2e6 * (1.0 + l * 0.03).min(2.0),
            approach: 6.0e5 * (1.0 + l * 0.02).min(1.8),
            scale: 1.0,
            elite: false,
            boss: false,
        }
    }

    fn elite(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 2.5,
            bounty: base.bounty * 3,
            damage: base.damage * 1.6,
            cooldown: base.cooldown * 0.8,
            bolt_speed: base.bolt_speed * 1.15,
            approach: base.approach * 1.15,
            scale: 1.45,
            elite: true,
            boss: false,
        }
    }

    fn dreadnought(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 12.0,
            bounty: base.bounty * 15,
            damage: base.damage * 2.2,
            cooldown: base.cooldown * 1.4,
            bolt_speed: base.bolt_speed,
            approach: base.approach * 0.7,
            scale: 3.4,
            elite: false,
            boss: true,
        }
    }
}

fn spawn_hostile(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3d,
    vel: Vec3d,
    spec: HostileSpec,
) {
    let hp = spec.hp;
    let bounty = spec.bounty;
    // Elites run hot-blooded crimson; the dreadnought burns amber. A
    // pilot should read the threat tier from the silhouette color alone.
    let (hull_color, hull_glow, fin_color, fin_glow) = if spec.boss {
        (
            Color::srgb(0.85, 0.55, 0.2),
            LinearRgba::rgb(1.4, 0.7, 0.1),
            Color::srgb(0.9, 0.35, 0.2),
            LinearRgba::rgb(1.2, 0.3, 0.1),
        )
    } else if spec.elite {
        (
            Color::srgb(0.95, 0.3, 0.35),
            LinearRgba::rgb(1.5, 0.2, 0.25),
            Color::srgb(0.85, 0.5, 0.2),
            LinearRgba::rgb(1.0, 0.5, 0.1),
        )
    } else {
        (
            Color::srgb(0.45, 0.9, 0.5),
            LinearRgba::rgb(0.15, 1.1, 0.3),
            Color::srgb(0.75, 0.3, 0.85),
            LinearRgba::rgb(0.7, 0.1, 0.9),
        )
    };
    let hull_mat = materials.add(StandardMaterial {
        base_color: hull_color,
        metallic: 0.35,
        emissive: hull_glow,
        ..default()
    });
    let fin_mat = materials.add(StandardMaterial {
        base_color: fin_color,
        emissive: fin_glow,
        ..default()
    });
    let core_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.2, 0.18),
        metallic: 0.65,
        perceptual_roughness: 0.6,
        ..default()
    });
    let glow_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.3, 1.0),
        emissive: LinearRgba::rgb(2.6, 0.5, 3.2),
        unlit: true,
        ..default()
    });
    let mut root = commands.spawn((
        SystemScoped,
        AlienShip {
            fire_cooldown: 3.0,
            cooldown: spec.cooldown,
            damage: spec.damage,
            bolt_speed: spec.bolt_speed,
            approach: spec.approach,
            volley: if spec.boss { 3 } else { 1 },
        },
        Hull { hp },
        Bounty(bounty),
        SimPos(pos),
        SimVel(vel),
        BodyVel::default(),
        // Angular strike frame: a 3-sided prism core — nothing about
        // it reads friendly or aerodynamic. Tier picks the mass.
        Mesh3d(meshes.add(Cone::new(6.5, 13.0).mesh().resolution(3))),
        MeshMaterial3d(hull_mat),
        Transform::from_scale(Vec3::splat(spec.scale)),
    ));
    if spec.elite {
        root.insert(Elite);
    }
    if spec.boss {
        root.insert(Dreadnought);
    }
    root.with_children(|alien| {
            // Jagged fin trio at 120-degree spacing.
            for k in 0..3 {
                let a = std::f32::consts::TAU * k as f32 / 3.0;
                alien.spawn((
                    Mesh3d(meshes.add(Cuboid::new(11.0, 2.2, 0.8).mesh())),
                    MeshMaterial3d(fin_mat.clone()),
                    Transform::from_xyz(a.cos() * 3.0, 2.0 - k as f32 * 2.4, a.sin() * 3.0)
                        .with_rotation(Quat::from_rotation_y(a) * Quat::from_rotation_z(0.45)),
                ));
            }
            // Reactor block + spike antennas + eye pods.
            alien.spawn((
                Mesh3d(meshes.add(Cuboid::new(4.2, 3.2, 3.2).mesh())),
                MeshMaterial3d(core_mat.clone()),
                Transform::from_xyz(0.0, -7.5, 0.0),
            ));
            alien.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.3, 7.0, 0.3).mesh())),
                MeshMaterial3d(core_mat),
                Transform::from_xyz(-2.0, 5.0, 1.2).with_rotation(Quat::from_rotation_z(-0.5)),
            ));
            for side in [-1.0f32, 1.0] {
                alien.spawn((
                    Mesh3d(meshes.add(Sphere::new(0.9).mesh().ico(1).unwrap())),
                    MeshMaterial3d(glow_mat.clone()),
                    Transform::from_xyz(side * 2.6, 4.6, 0.8),
                ));
            }
            // The dreadnought reads as a different CLASS, not a bigger
            // raider: dorsal spine, twin outrigger pods, extra eye row.
            if spec.boss {
                alien.spawn((
                    Mesh3d(meshes.add(Cuboid::new(1.4, 22.0, 1.4).mesh())),
                    MeshMaterial3d(fin_mat.clone()),
                    Transform::from_xyz(0.0, -4.0, 2.2),
                ));
                for side in [-1.0f32, 1.0] {
                    alien.spawn((
                        Mesh3d(meshes.add(Cuboid::new(3.0, 9.0, 3.0).mesh())),
                        MeshMaterial3d(fin_mat.clone()),
                        Transform::from_xyz(side * 7.5, -3.0, 0.0)
                            .with_rotation(Quat::from_rotation_z(side * 0.12)),
                    ));
                    alien.spawn((
                        Mesh3d(meshes.add(Sphere::new(1.1).mesh().ico(1).unwrap())),
                        MeshMaterial3d(glow_mat.clone()),
                        Transform::from_xyz(side * 4.6, -0.5, 1.4),
                    ));
                }
            }
        });
}

/// Seek the player; fire when in range; keep a fighting distance.
fn alien_ai(
    ships: Query<(&SimPos, &SimVel), (With<Ship>, Without<AlienShip>)>,
    #[allow(clippy::type_complexity)]
    mut aliens: Query<
        (&mut AlienShip, &mut SimPos, &mut SimVel, &mut BodyVel, &mut Transform),
        Without<Ship>,
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok((ship_pos, ship_vel)) = ships.single() else { return };
    let dt = DT * TIME_WARP;
    for (mut alien, mut pos, mut vel, mut bvel, mut transform) in &mut aliens {
        let to_ship = ship_pos.0 - pos.0;
        let dist = to_ship.length().max(1.0);
        let dir = to_ship * (1.0 / dist);

        // Close to fighting range, then circle: chase point offsets
        // sideways so raiders orbit the fight instead of ramming. The
        // closing speed is a spawn-time stat — veterans close faster.
        let desired = if dist > FIRE_RANGE * 0.5 {
            ship_vel.0 + dir * alien.approach
        } else {
            let tangent = Vec3d::new(-dir.y, dir.x, 0.0);
            ship_vel.0 + tangent * (alien.approach * 0.66) + dir * 5.0e4
        };
        let dv = desired - vel.0;
        let a_max = 4.0e4 * TIME_WARP;
        let need = dv.length() / dt;
        let a = if need > a_max { dv.normalized() * a_max } else { dv * (1.0 / dt) };
        vel.0 += a * dt;
        pos.0 += vel.0 * dt;
        bvel.0 = vel.0;
        // Face the prey.
        let angle = (to_ship.y).atan2(to_ship.x) as f32 - std::f32::consts::FRAC_PI_2;
        transform.rotation = Quat::from_rotation_z(angle);

        // Fire. Damage, cadence and bolt speed are the ship's own stats;
        // a dreadnought looses a spread volley instead of one bolt.
        alien.fire_cooldown -= DT;
        if alien.fire_cooldown <= 0.0 && dist < FIRE_RANGE {
            alien.fire_cooldown = alien.cooldown;
            let lead = ship_pos.0 + (ship_vel.0 - vel.0) * (dist / alien.bolt_speed);
            let aim = (lead - pos.0).normalized();
            let volley = alien.volley.max(1);
            for i in 0..volley {
                // Spread the volley in-plane: center bolt true, wings
                // angled a few degrees out.
                let spread = (i as f64 - (volley as f64 - 1.0) / 2.0) * 0.06;
                let (s, c) = spread.sin_cos();
                let aim_i =
                    Vec3d::new(aim.x * c - aim.y * s, aim.x * s + aim.y * c, aim.z).normalized();
                commands.spawn((
                    SystemScoped,
                    AlienBolt { damage: alien.damage, ttl: 25.0 },
                    SimPos(pos.0 + aim_i * 4.0e8),
                    SimVel(vel.0 + aim_i * alien.bolt_speed),
                    Mesh3d(meshes.add(Sphere::new(3.0).mesh().ico(1).unwrap())),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.9, 0.3, 1.0),
                        emissive: LinearRgba::rgb(6.0, 1.2, 8.0),
                        unlit: true,
                        ..default()
                    })),
                    Transform::default(),
                ));
            }
        }
    }
}

/// Cockpit targeting: lay each raider's predicted path out as fading
/// dots. Prediction is in the SHIP-RELATIVE frame anchored at the
/// ship's current position — where the raider will be relative to you,
/// which is the question a gunner is actually asking.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn project_trajectories(
    view: Res<ViewMode>,
    ships: Query<(&SimPos, &SimVel), (With<Ship>, Without<AlienShip>, Without<TrajDot>)>,
    aliens: Query<(&SimPos, &SimVel), (With<AlienShip>, Without<TrajDot>)>,
    mut pool: ResMut<TrajPool>,
    mut dots: Query<(&mut SimPos, &mut Visibility), With<TrajDot>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Lazily build the pool: MAX_TRACKED paths of TRAJ_STEPS dots each,
    // alpha fading with distance into the future.
    if pool.dots.is_empty() {
        let mesh = meshes.add(Sphere::new(1.6).mesh().ico(1).unwrap());
        let mats: Vec<Handle<StandardMaterial>> = (0..TRAJ_STEPS)
            .map(|i| {
                let a = 0.75 * (1.0 - i as f32 / TRAJ_STEPS as f32) + 0.1;
                materials.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 0.35, 0.9, a),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                })
            })
            .collect();
        for _ in 0..MAX_TRACKED {
            for (i, mat) in mats.iter().enumerate().take(TRAJ_STEPS) {
                let e = commands
                    .spawn((
                        TrajDot,
                        SimPos::default(),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(mat.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                    ))
                    .id();
                pool.dots.push(e);
                let _ = i;
            }
        }
        return;
    }

    let ship = ships.single().ok();
    let mut cursor = 0usize;
    if *view == ViewMode::Cockpit
        && let Some((_ship_pos, ship_vel)) = ship
    {
        for (a_pos, a_vel) in aliens.iter().take(MAX_TRACKED) {
            let rel_vel = a_vel.0 - ship_vel.0;
            for step in 0..TRAJ_STEPS {
                let Some(&dot) = pool.dots.get(cursor) else { break };
                cursor += 1;
                if let Ok((mut pos, mut vis)) = dots.get_mut(dot) {
                    let t = TRAJ_STEP_SIM_S * (step + 1) as f64;
                    pos.0 = a_pos.0 + rel_vel * t;
                    *vis = Visibility::Inherited;
                }
            }
        }
    }
    // Park the rest.
    for &dot in pool.dots.iter().skip(cursor) {
        if let Ok((_, mut vis)) = dots.get_mut(dot) {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
    }
}

/// Bolts fly straight, expire, and burst on the ship: shields first,
/// then hull — the same order the sun burns in.
fn fly_bolts(
    mut bolts: Query<(Entity, &mut AlienBolt, &mut SimPos, &SimVel)>,
    mut ships: Query<(&mut Ship, &SimPos), Without<AlienBolt>>,
    mut commands: Commands,
) {
    let dt = DT * TIME_WARP;
    for (entity, mut bolt, mut pos, vel) in &mut bolts {
        bolt.ttl -= DT;
        if bolt.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        pos.0 += vel.0 * dt;
        if let Ok((mut ship, ship_pos)) = ships.single_mut()
            && ship_pos.0.distance(pos.0) < BOLT_FUSE
        {
            let dmg = bolt.damage;
            if ship.shield > 0.0 {
                ship.shield = (ship.shield - dmg).max(0.0);
            } else {
                ship.hull = (ship.hull - dmg).max(0.0);
            }
            info!(
                "bolt hit: shield {:.0}, hull {:.0}",
                ship.shield, ship.hull
            );
            commands.entity(entity).despawn();
        }
    }
}
