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

use bevy::picking::events::{Click, Pointer};
use bevy::prelude::*;
use oj_materials::UpgradeSlot;
use oj_orbits::{Vec3d, gravity_accel};

use crate::modules::{RunScore, Wreck};
use crate::sim::{BodyVel, CelestialBody, DT, OnRails, RENDER_SCALE, Ship, SystemScoped, TIME_WARP};
use crate::travel::SystemChanged;
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

/// Salvage value a destroyed hull scatters; defaults to drone scrap.
#[derive(Component)]
pub struct Bounty(pub u64);

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

/// The designated target: click any hostile hull to lock it. Locked
/// targets get laser priority and missile guidance; clicking the locked
/// vessel again releases the lock.
#[derive(Resource, Default)]
pub struct TargetLock(pub Option<Entity>);

/// The in-world lock indicator ring.
#[derive(Component)]
struct LockMarker;

/// A short-lived laser beam flash between ship and victim.
#[derive(Component)]
struct LaserBeam {
    ttl: f64,
}

pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cooldowns>()
            .init_resource::<TargetLock>()
            .add_observer(lock_target)
            .add_systems(Startup, (spawn_drones, spawn_lock_marker))
            .add_systems(Update, (respawn_drones, drive_lock_marker, fade_beams))
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
    spawn_drone_set(&mut commands, &game, &mut meshes, &mut materials);
}

/// A jump tears the old drones down with the rest of the system; fresh
/// practice targets spawn on the new start ring.
fn respawn_drones(
    mut changed: MessageReader<SystemChanged>,
    mut commands: Commands,
    game: Res<GameUniverse>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if changed.read().next().is_none() {
        return;
    }
    spawn_drone_set(&mut commands, &game, &mut meshes, &mut materials);
}

fn spawn_drone_set(
    commands: &mut Commands,
    game: &GameUniverse,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let Some(system) = game.universe.system(game.current) else { return };
    let mu = oj_orbits::G * system.sun.mass;
    let r = system
        .planets
        .first()
        .map(|p| p.orbit.semi_major * 0.6)
        .unwrap_or(1.0e11);
    let mesh = meshes.add(Sphere::new(5.0).mesh().ico(2).unwrap());
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
            SystemScoped,
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

/// Click a hostile hull (or any of its mesh children) to lock it; click
/// the locked vessel again to release. Presses on rings and celestials
/// never reach here — they carry no Hull anywhere in their ancestry.
fn lock_target(
    ev: On<Pointer<Click>>,
    hulls: Query<(), With<Hull>>,
    parents: Query<&ChildOf>,
    mut lock: ResMut<TargetLock>,
) {
    let mut e = ev.entity;
    loop {
        if hulls.get(e).is_ok() {
            lock.0 = if lock.0 == Some(e) { None } else { Some(e) };
            info!("target lock: {:?}", lock.0);
            return;
        }
        match parents.get(e) {
            Ok(child_of) => e = child_of.parent(),
            Err(_) => return,
        }
    }
}

fn spawn_lock_marker(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        LockMarker,
        SimPos(Vec3d::ZERO),
        Mesh3d(meshes.add(Torus::new(15.0, 16.5).mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.35, 0.4, 0.9),
            emissive: LinearRgba::rgb(2.4, 0.4, 0.5),
            unlit: true,
            ..default()
        })),
        // The torus lies in XZ by default; stand it into the orbital
        // plane so it reads as a ring around the vessel from above.
        Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        Visibility::Hidden,
        bevy::picking::Pickable::IGNORE,
    ));
}

/// Ride the locked vessel with a pulsing ring; release the lock the
/// moment the target stops existing (killed, despawned with the system).
fn drive_lock_marker(
    time: Res<Time>,
    mut lock: ResMut<TargetLock>,
    targets: Query<&SimPos, (With<Hull>, Without<LockMarker>)>,
    mut markers: Query<(&mut SimPos, &mut Transform, &mut Visibility), With<LockMarker>>,
) {
    let Ok((mut pos, mut transform, mut vis)) = markers.single_mut() else { return };
    let target_pos = lock.0.and_then(|e| targets.get(e).ok());
    match target_pos {
        Some(tp) => {
            pos.0 = tp.0;
            let pulse = 1.0 + 0.18 * (time.elapsed_secs() * 5.0).sin();
            transform.scale = Vec3::splat(pulse);
            *vis = Visibility::Inherited;
        }
        None => {
            lock.0 = None;
            *vis = Visibility::Hidden;
        }
    }
}

/// Beam flashes decay fast — the laser is hitscan; this is muzzle light.
fn fade_beams(mut beams: Query<(Entity, &mut LaserBeam)>, mut commands: Commands) {
    for (entity, mut beam) in &mut beams {
        beam.ttl -= 1.0 / 60.0;
        if beam.ttl <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fire_weapons(
    keys: Res<ButtonInput<KeyCode>>,
    mut cd: ResMut<Cooldowns>,
    upgrades: Res<ShipUpgrades>,
    lock: Res<TargetLock>,
    arm: Res<crate::solar::SolarArm>,
    mut run: ResMut<RunScore>,
    mut ships: Query<(&mut Ship, &SimPos, &SimVel)>,
    mut drones: Query<(Entity, &SimPos, &mut Hull)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    cd.laser = (cd.laser - DT).max(0.0);
    cd.missile = (cd.missile - DT).max(0.0);
    cd.well = (cd.well - DT).max(0.0);
    // A deployed solar arm sits in the weapon train: stow it first.
    if !arm.weapons_free() {
        return;
    }
    let Ok((mut ship, pos, vel)) = ships.single_mut() else { return };

    // Target priority: the locked vessel when it is in reach, otherwise
    // whatever is nearest — locking is a promise, not a handcuff.
    let prefer = |max_range: f64, drones: &Query<(Entity, &SimPos, &mut Hull)>| {
        lock.0
            .and_then(|e| drones.get(e).ok().map(|(e, p, _)| (e, p.0.distance(pos.0))))
            .filter(|(_, d)| *d < max_range)
            .or_else(|| {
                drones
                    .iter()
                    .map(|(e, p, _)| (e, p.0.distance(pos.0)))
                    .filter(|(_, d)| *d < max_range)
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            })
            .map(|(e, _)| e)
    };

    // Laser: hitscan on the locked target, else the nearest hull in range.
    let laser_tier = upgrades.tier(UpgradeSlot::LaserWeapon);
    if keys.pressed(KeyCode::KeyZ) && laser_tier > 0 && cd.laser == 0.0 && ship.energy >= 5.0 {
        let victim = prefer(LASER_RANGE, &drones);
        if let Some(victim) = victim {
            let victim_pos = drones.get(victim).map(|(_, p, _)| p.0).unwrap_or(pos.0);
            if let Ok((_, _, mut hull)) = drones.get_mut(victim) {
                ship.energy -= 5.0;
                cd.laser = LASER_COOLDOWN;
                hull.hp -= 10.0 * 1.5f64.powi(laser_tier as i32 - 1);
                run.score_hit();
                sfx.write(crate::audio::Sfx::Laser);
            }
            // The beam itself, in two layers: a white-hot core and a
            // wider red glow sheath, plus an impact flash where it
            // lands. Hitscan needs light or combat reads as nothing.
            let to_target = victim_pos - pos.0;
            let len = (to_target.length() * RENDER_SCALE) as f32;
            let dir3 = Vec3::new(to_target.x as f32, to_target.y as f32, to_target.z as f32)
                .normalize_or_zero();
            let rotation = Quat::from_rotation_arc(Vec3::Y, dir3);
            for (w, color, glow, a) in [
                (0.5, (1.0, 0.85, 0.8), LinearRgba::rgb(9.0, 3.5, 3.0), 0.95),
                (1.5, (1.0, 0.25, 0.25), LinearRgba::rgb(3.5, 0.4, 0.4), 0.35),
            ] {
                commands.spawn((
                    SystemScoped,
                    LaserBeam { ttl: 0.12 },
                    SimPos(pos.0 + to_target * 0.5),
                    Mesh3d(meshes.add(Cuboid::new(w, 1.0, w).mesh())),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgba(color.0, color.1, color.2, a),
                        emissive: glow,
                        unlit: true,
                        alpha_mode: AlphaMode::Blend,
                        ..default()
                    })),
                    Transform { rotation, scale: Vec3::new(1.0, len, 1.0), ..default() },
                    bevy::picking::Pickable::IGNORE,
                ));
            }
            crate::fx::spawn_impact_flash(
                &mut commands,
                &mut meshes,
                &mut materials,
                victim_pos,
                Vec3d::ZERO,
                6.0,
                LinearRgba::rgb(7.0, 1.6, 1.2),
            );
        }
    }

    // Missile: launched prograde; seeks the locked vessel, else nearest.
    let missile_tier = upgrades.tier(UpgradeSlot::MissileRack);
    if keys.just_pressed(KeyCode::KeyX) && missile_tier > 0 && cd.missile == 0.0 && ship.energy >= 10.0 {
        ship.energy -= 10.0;
        cd.missile = MISSILE_COOLDOWN;
        sfx.write(crate::audio::Sfx::Missile);
        let target = prefer(f64::INFINITY, &drones);
        commands.spawn((
            SystemScoped,
            Missile {
                target,
                damage: 40.0 * 1.4f64.powi(missile_tier as i32 - 1),
                ttl: 60.0,
            },
            SimPos(pos.0 + vel.0.normalized() * 3.0e8),
            SimVel(vel.0 + vel.0.normalized() * 2000.0),
            Mesh3d(meshes.add(Cone::new(2.0, 8.0).mesh().resolution(8))),
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
            sfx.write(crate::audio::Sfx::Missile);
            commands.spawn((
                SystemScoped,
                ForceWell {
                    strength: sign * 8.0e14 * ff_tier as f64,
                    radius: 4.0e9,
                    ttl: 25.0,
                },
                SimPos(pos.0 + vel.0.normalized() * 5.0e8),
                SimVel(vel.0 * 1.02),
                Mesh3d(meshes.add(Sphere::new(15.0).mesh().ico(3).unwrap())),
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
    celestials: Query<(&CelestialBody, &SimPos), (Without<Missile>, Without<Hull>)>,
    targets: Query<(&SimPos, &BodyVel), (With<Hull>, Without<Missile>)>,
    mut missiles: Query<(Entity, &mut Missile, &mut SimPos, &mut SimVel), Without<Hull>>,
    mut drones: Query<(Entity, &SimPos, &mut Hull), Without<Missile>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
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

        // Proximity fuse — detonation gets a real fireball.
        if let Some(target) = missile.target
            && let Ok((_, tpos, mut hull)) = drones.get_mut(target)
            && tpos.0.distance(pos.0) < 2.0e8
        {
            hull.hp -= missile.damage;
            sfx.write(crate::audio::Sfx::Explosion);
            crate::fx::spawn_explosion(
                &mut commands,
                &mut meshes,
                &mut materials,
                pos.0,
                Vec3d::ZERO,
                9.0,
            );
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
    hulls: Query<(Entity, &Hull, &SimPos, Option<&Bounty>)>,
    mut run: ResMut<RunScore>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    for (entity, hull, pos, bounty) in &hulls {
        if hull.hp > 0.0 {
            continue;
        }
        run.kills += 1;
        // Kill credit scales with the victim's bounty, which scales with
        // its difficulty tier — bosses and elites pay what they cost.
        let bounty_value = bounty.map_or(25, |b| b.0);
        run.combat_score += bounty_value * 12;
        // The kill is an event; give it a fireball to match its worth —
        // drones pop, raiders blow, a dreadnought goes up like a depot.
        let blast = if bounty_value >= 1000 {
            34.0
        } else if bounty_value >= 100 {
            16.0
        } else {
            10.0
        };
        sfx.write(crate::audio::Sfx::Explosion);
        crate::fx::spawn_explosion(
            &mut commands,
            &mut meshes,
            &mut materials,
            pos.0,
            Vec3d::ZERO,
            blast,
        );
        commands.entity(entity).despawn();
        let mesh = meshes.add(Cuboid::new(3.0, 3.0, 3.0).mesh());
        let mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.5, 0.45),
            ..default()
        });
        // Bountied hulls (raiders) scrap into exotics; drones into basics.
        let value = bounty.map_or(15, |b| b.0 / 2);
        for i in 0..2 {
            commands.spawn((
                SystemScoped,
                Wreck {
                    value,
                    element: match (bounty.is_some(), i) {
                        (true, 0) => oj_materials::Element::Aetherite,
                        (true, _) => oj_materials::Element::Titanium,
                        (false, 0) => oj_materials::Element::Silicon,
                        (false, _) => oj_materials::Element::Iron,
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
