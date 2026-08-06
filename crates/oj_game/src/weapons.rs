//! Weapons: laser (hitscan), missiles (integrated seekers), and force
//! wells — projectiles that carry a signed gravity pocket, pulling toward
//! or pushing away from their center. Wells act through the same
//! acceleration bookkeeping as everything else, so they bend missiles,
//! shove drones, brake or boost your own ship, and (by design) make
//! terrible neighbors near a delicate orbit.
//!
//! Tiers gate and scale everything: no missile rack, no missiles; a
//! stronger projector digs a deeper well. Practice drones orbit near the
//! starting ring so there is something to shoot.

use bevy::prelude::*;
use oj_materials::UpgradeSlot;
use oj_orbits::{Vec3d, gravity_accel};

use crate::modules::{RunScore, Wreck};
use crate::sim::{BodyVel, CelestialBody, DT, OnRails, Ship, TIME_WARP};
use crate::upgrades::ShipUpgrades;
use crate::{GameUniverse, SimPos, SimVel};

const LASER_RANGE: f64 = 6.0e9;
const LASER_COOLDOWN: f64 = 0.5;
const MISSILE_COOLDOWN: f64 = 2.0;
const WELL_COOLDOWN: f64 = 5.0;

/// Something weapons can destroy.
#[derive(Component)]
pub struct Hull {
    pub hp: f64,
}

/// A practice drone on rails.
#[derive(Component)]
pub struct TargetDrone;

/// A seeking missile.
#[derive(Component)]
pub struct Missile {
    pub target: Option<Entity>,
    pub damage: f64,
    pub ttl: f64,
}

/// A force-field projectile: a traveling gravity pocket. Positive
/// `strength` pulls toward the center; negative pushes away.
#[derive(Component)]
pub struct ForceWell {
    /// Effective mu (m^3/s^2), signed.
    pub strength: f64,
    pub radius: f64,
    pub ttl: f64,
}

#[derive(Resource, Default)]
struct Cooldowns {
    laser: f64,
    missile: f64,
    well: f64,
}

pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cooldowns>()
            .add_systems(Startup, spawn_drones)
            .add_systems(
                FixedUpdate,
                (fire_weapons, fly_missiles, fly_drones, apply_wells, reap_hulls).chain(),
            );
    }
}

fn spawn_drones(
    mut commands: Commands,
    game: Res<GameUniverse>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(system) = game.universe.system(game.current) else { return };
    let mu = oj_orbits::G * system.sun.mass;
    let r = system
        .planets
        .first()
        .map(|p| p.orbit.semi_major * 0.6)
        .unwrap_or(1.0e11);
    let mesh = meshes.add(Sphere::new(5.0e7).mesh().ico(2).unwrap());
    let mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.4, 0.35),
        emissive: LinearRgba::rgb(0.6, 0.1, 0.1),
        ..default()
    });
    // Free-flying (not railed) so force wells can genuinely drag them —
    // an attractor can pull a drone clean out of its orbit.
    for i in 0..4 {
        let radius = r * (0.96 + 0.02 * i as f64);
        let phase = 0.02 * i as f64;
        let pos = Vec3d::new(radius * phase.cos(), radius * phase.sin(), 0.0);
        let v = (mu / radius).sqrt();
        let vel = Vec3d::new(-v * phase.sin(), v * phase.cos(), 0.0);
        commands.spawn((
            TargetDrone,
            Hull { hp: 30.0 },
            SimPos(pos),
            SimVel(vel),
            BodyVel::default(),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::default(),
        ));
    }
}

fn fire_weapons(
    keys: Res<ButtonInput<KeyCode>>,
    mut cd: ResMut<Cooldowns>,
    upgrades: Res<ShipUpgrades>,
    mut run: ResMut<RunScore>,
    mut ships: Query<(&mut Ship, &SimPos, &SimVel)>,
    mut drones: Query<(Entity, &SimPos, &mut Hull), With<TargetDrone>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    cd.laser = (cd.laser - DT).max(0.0);
    cd.missile = (cd.missile - DT).max(0.0);
    cd.well = (cd.well - DT).max(0.0);
    let Ok((mut ship, pos, vel)) = ships.single_mut() else { return };

    // Laser: auto-aim the nearest drone in range; instant.
    let laser_tier = upgrades.tier(UpgradeSlot::LaserWeapon);
    if keys.pressed(KeyCode::KeyZ) && laser_tier > 0 && cd.laser == 0.0 && ship.energy >= 5.0 {
        let nearest = drones
            .iter_mut()
            .map(|(e, p, h)| (e, p.0.distance(pos.0), h))
            .filter(|(_, d, _)| *d < LASER_RANGE)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        if let Some((_, _, mut hull)) = nearest {
            ship.energy -= 5.0;
            cd.laser = LASER_COOLDOWN;
            hull.hp -= 10.0 * 1.5f64.powi(laser_tier as i32 - 1);
            run.score_hit();
        }
    }

    // Missile: launched prograde, seeks the nearest drone.
    let missile_tier = upgrades.tier(UpgradeSlot::MissileRack);
    if keys.just_pressed(KeyCode::KeyX) && missile_tier > 0 && cd.missile == 0.0 && ship.energy >= 10.0 {
        ship.energy -= 10.0;
        cd.missile = MISSILE_COOLDOWN;
        let target = drones
            .iter()
            .map(|(e, p, _)| (e, p.0.distance(pos.0)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(e, _)| e);
        commands.spawn((
            Missile {
                target,
                damage: 40.0 * 1.4f64.powi(missile_tier as i32 - 1),
                ttl: 60.0,
            },
            SimPos(pos.0 + vel.0.normalized() * 3.0e8),
            SimVel(vel.0 + vel.0.normalized() * 2000.0),
            Mesh3d(meshes.add(Cone::new(2.0e7, 8.0e7).mesh().resolution(8))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.8, 0.3),
                emissive: LinearRgba::rgb(2.0, 1.5, 0.3),
                ..default()
            })),
            Transform::default(),
        ));
    }

    // Force well: C pulls, V pushes. Launched gently prograde.
    let ff_tier = upgrades.tier(UpgradeSlot::ForceFieldProjector);
    for (key, sign) in [(KeyCode::KeyC, 1.0), (KeyCode::KeyV, -1.0)] {
        if keys.just_pressed(key) && ff_tier > 0 && cd.well == 0.0 && ship.energy >= 25.0 {
            ship.energy -= 25.0;
            cd.well = WELL_COOLDOWN;
            commands.spawn((
                ForceWell {
                    strength: sign * 8.0e14 * ff_tier as f64,
                    radius: 4.0e9,
                    ttl: 25.0,
                },
                SimPos(pos.0 + vel.0.normalized() * 5.0e8),
                SimVel(vel.0 * 1.02),
                Mesh3d(meshes.add(Sphere::new(1.5e8).mesh().ico(3).unwrap())),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgba(0.5, 0.3, 0.9, 0.6),
                    emissive: if sign > 0.0 {
                        LinearRgba::rgb(0.8, 0.2, 2.0)
                    } else {
                        LinearRgba::rgb(0.2, 1.6, 1.8)
                    },
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })),
                Transform::default(),
            ));
        }
    }
}

/// Missiles integrate like ships: celestial gravity plus seek thrust.
fn fly_missiles(
    celestials: Query<(&CelestialBody, &SimPos), (Without<Missile>, Without<TargetDrone>)>,
    targets: Query<(&SimPos, &BodyVel), (With<TargetDrone>, Without<Missile>)>,
    mut missiles: Query<(Entity, &mut Missile, &mut SimPos, &mut SimVel), Without<TargetDrone>>,
    mut drones: Query<(Entity, &SimPos, &mut Hull), (With<TargetDrone>, Without<Missile>)>,
    mut commands: Commands,
) {
    let dt = DT * TIME_WARP;
    for (entity, mut missile, mut pos, mut vel) in &mut missiles {
        missile.ttl -= DT;
        if missile.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let mut accel = Vec3d::ZERO;
        for (body, body_pos) in &celestials {
            accel += gravity_accel(body.mu, body_pos.0, pos.0, body.radius);
        }
        if let Some(target) = missile.target
            && let Ok((tpos, tvel)) = targets.get(target)
        {
            // Proportional pursuit of the intercept point.
            let to_target = tpos.0 - pos.0;
            let closing = to_target.normalized() * 3.0e5 + tvel.0 - vel.0;
            accel += closing.normalized() * 4.0e5;
        }
        oj_orbits::integrate_step(&mut pos.0, &mut vel.0, accel, dt);

        // Proximity fuse.
        if let Some(target) = missile.target
            && let Ok((_, tpos, mut hull)) = drones.get_mut(target)
            && tpos.0.distance(pos.0) < 2.0e8
        {
            hull.hp -= missile.damage;
            commands.entity(entity).despawn();
        }
    }
}

/// Drones integrate under celestial gravity like any free body; their
/// BodyVel mirrors SimVel so missiles lead them correctly.
fn fly_drones(
    celestials: Query<(&CelestialBody, &SimPos), Without<TargetDrone>>,
    mut drones: Query<(&mut SimPos, &mut SimVel, &mut BodyVel), With<TargetDrone>>,
) {
    let dt = DT * TIME_WARP;
    for (mut pos, mut vel, mut bvel) in &mut drones {
        let mut accel = Vec3d::ZERO;
        for (body, body_pos) in &celestials {
            accel += gravity_accel(body.mu, body_pos.0, pos.0, body.radius);
        }
        oj_orbits::integrate_step(&mut pos.0, &mut vel.0, accel, dt);
        bvel.0 = vel.0;
    }
}

/// Wells accelerate everything that flies free: ships, missiles, drones'
/// wrecks — anything with a SimVel. Attraction and repulsion are the same
/// formula with opposite signs, softened at the well core.
fn apply_wells(
    mut wells: Query<(Entity, &mut ForceWell, &mut SimPos, &mut SimVel), Without<Hull>>,
    mut movers: Query<(&mut SimVel, &SimPos), (Without<ForceWell>, Without<OnRails>)>,
    mut commands: Commands,
) {
    let dt = DT * TIME_WARP;
    for (entity, mut well, mut wpos, mut wvel) in &mut wells {
        well.ttl -= DT;
        if well.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        // The well itself drifts ballistically (unaffected by celestials:
        // it is a field projection, not matter).
        let drift = wvel.0;
        wpos.0 += drift * dt;
        for (mut vel, pos) in &mut movers {
            let d = pos.0.distance(wpos.0);
            if d > well.radius {
                continue;
            }
            let a = gravity_accel(well.strength.abs(), wpos.0, pos.0, well.radius * 0.05);
            let signed = if well.strength >= 0.0 { a } else { -a };
            vel.0 += signed * dt;
        }
        let _ = &mut wvel;
    }
}

/// Zero-HP hulls become salvage.
fn reap_hulls(
    hulls: Query<(Entity, &Hull, &SimPos)>,
    mut run: ResMut<RunScore>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, hull, pos) in &hulls {
        if hull.hp > 0.0 {
            continue;
        }
        run.kills += 1;
        commands.entity(entity).despawn();
        let mesh = meshes.add(Cuboid::new(3.0e7, 3.0e7, 3.0e7).mesh());
        let mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.5, 0.45),
            ..default()
        });
        for i in 0..2 {
            commands.spawn((
                Wreck {
                    value: 15,
                    element: if i == 0 {
                        oj_materials::Element::Silicon
                    } else {
                        oj_materials::Element::Iron
                    },
                },
                SimPos(pos.0 + Vec3d::new(1.0e8 * (i as f64 - 0.5), 5.0e7, 0.0)),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::default(),
            ));
        }
    }
}
