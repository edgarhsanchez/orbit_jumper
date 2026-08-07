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

/// A hostile ship.
#[derive(Component)]
pub struct AlienShip {
    fire_cooldown: f64,
}

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
    aliens: Query<(), With<AlienShip>>,
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
    let pack_cap = (1 + level / 3).min(6) as usize;
    if aliens.iter().count() >= pack_cap {
        return;
    }

    // Arrive from a seed-random bearing, well outside weapons range,
    // velocity matched so the approach is deliberate, not a flyby.
    let mut rng = oj_universe::SplitMix64(
        game.current.index as u64 ^ (career.total_score + run.total()) ^ 0xA11E7,
    );
    let bearing = rng.range(0.0, std::f64::consts::TAU);
    let dist = rng.range(1.6e10, 2.4e10);
    let pos = ship_pos.0 + Vec3d::new(bearing.cos() * dist, bearing.sin() * dist, 0.0);

    let hp = 30.0 * (1.0 + level as f64 * 0.15);
    let bounty = 40 + level as u64 * 8;
    let hull_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.9, 0.5),
        metallic: 0.35,
        emissive: LinearRgba::rgb(0.15, 1.1, 0.3),
        ..default()
    });
    let fin_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.75, 0.3, 0.85),
        emissive: LinearRgba::rgb(0.7, 0.1, 0.9),
        ..default()
    });
    commands
        .spawn((
            SystemScoped,
            AlienShip { fire_cooldown: 3.0 },
            Hull { hp },
            Bounty(bounty),
            SimPos(pos),
            SimVel(ship_vel.0),
            BodyVel::default(),
            // Inverted dart: broad base forward — unmistakably not ours.
            Mesh3d(meshes.add(Cone::new(7.0, 14.0).mesh().resolution(3))),
            MeshMaterial3d(hull_mat),
            Transform::default(),
        ))
        .with_children(|alien| {
            alien.spawn((
                Mesh3d(meshes.add(Cuboid::new(18.0, 3.0, 1.4).mesh())),
                MeshMaterial3d(fin_mat),
                Transform::from_xyz(0.0, 4.0, 0.0),
            ));
        });
    info!("raider inbound (level {level}, pack cap {pack_cap})");
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
        // sideways so raiders orbit the fight instead of ramming.
        let desired = if dist > FIRE_RANGE * 0.5 {
            ship_vel.0 + dir * 6.0e5
        } else {
            let tangent = Vec3d::new(-dir.y, dir.x, 0.0);
            ship_vel.0 + tangent * 4.0e5 + dir * 5.0e4
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

        // Fire.
        alien.fire_cooldown -= DT;
        if alien.fire_cooldown <= 0.0 && dist < FIRE_RANGE {
            alien.fire_cooldown = 2.5;
            let lead = ship_pos.0 + (ship_vel.0 - vel.0) * (dist / 1.2e6);
            let aim = (lead - pos.0).normalized();
            commands.spawn((
                SystemScoped,
                AlienBolt { damage: 7.0, ttl: 25.0 },
                SimPos(pos.0 + aim * 4.0e8),
                SimVel(vel.0 + aim * 1.2e6),
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
