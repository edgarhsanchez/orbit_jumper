//! The simulation: rails, gravity, energy, and the sun that kills you.

use bevy::prelude::*;
use oj_orbits::{KeplerOrbit, Vec3d, circular_speed, gravity_accel, integrate_step};
use oj_universe::SunClass;

use crate::{GameUniverse, SimPos, SimVel};

/// Fixed simulation step, seconds.
pub const DT: f64 = 1.0 / 60.0;

/// Sim time acceleration: orbits at real scale are hours; the game runs
/// them at this multiple so play stays brisk. Tuning knob, not physics.
pub const TIME_WARP: f64 = 600.0;

pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameUniverse>()
            .init_resource::<SimClock>()
            .add_systems(Startup, spawn_current_system)
            .add_systems(
                FixedUpdate,
                (
                    tick_clock,
                    place_celestials,
                    ship_controls,
                    integrate_ships,
                    harvest_and_hazard,
                )
                    .chain(),
            )
            .add_systems(Update, sync_render_transforms);
    }
}

/// Simulation time, seconds since session start (warped).
#[derive(Resource, Default)]
pub struct SimClock(pub f64);

fn tick_clock(mut clock: ResMut<SimClock>) {
    clock.0 += DT * TIME_WARP;
}

/// A body on rails.
#[derive(Component)]
pub struct OnRails(pub KeplerOrbit);

/// Any body ships can orbit, slingshot around, or be pulled by. Every one
/// of these exerts real gravity on ships each tick.
#[derive(Component)]
pub struct CelestialBody {
    /// G * mass, m^3/s^2.
    pub mu: f64,
    /// Physical radius, m (also the gravity softening floor).
    pub radius: f64,
    /// Sphere of influence, m; infinite for the system's sun.
    pub soi: f64,
    pub name: String,
}

/// The sun of the current system.
#[derive(Component)]
pub struct SunBody {
    pub class: SunClass,
    pub hazard_radius: f64,
}

/// The player's vessel.
#[derive(Component)]
pub struct Ship {
    pub energy: f64,
    pub energy_max: f64,
    pub shield: f64,
    pub shield_tier: u8,
    pub hull: f64,
    pub thrust: f64,
    /// Max distance at which the orbit command works; upgrades extend it.
    pub command_range: f64,
}

impl Default for Ship {
    fn default() -> Self {
        Self {
            energy: 100.0,
            energy_max: 100.0,
            shield: 100.0,
            shield_tier: 1,
            hull: 100.0,
            thrust: 25.0,
            command_range: 8.0e10,
        }
    }
}

/// Marks the entity the floating origin follows (the ship, for now).
#[derive(Component)]
pub struct OriginAnchor;

fn spawn_current_system(
    mut commands: Commands,
    game: Res<GameUniverse>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(system) = game.universe.system(game.current) else {
        error!("current system does not exist; universe misconfigured");
        return;
    };
    let mu = oj_orbits::G * system.sun.mass;

    // Sun at the origin of the system-local frame.
    commands.spawn((
        SunBody {
            class: system.sun.class,
            hazard_radius: system.sun.hazard_radius,
        },
        CelestialBody {
            mu,
            radius: system.sun.radius.max(2.0e9),
            soi: f64::INFINITY,
            name: format!("{:?}-class sun", system.sun.class),
        },
        SimPos(Vec3d::ZERO),
        Mesh3d(meshes.add(Sphere::new(2.0e9).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            emissive: LinearRgba::rgb(8.0, 6.5, 3.0),
            base_color: Color::srgb(1.0, 0.9, 0.5),
            ..default()
        })),
        Transform::default(),
    ));

    // Planets on rails. One shared mesh: automatic instancing batches them.
    let planet_mesh = meshes.add(Sphere::new(4.0e8).mesh().ico(3).unwrap());
    let planet_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.5, 0.6),
        perceptual_roughness: 0.9,
        ..default()
    });
    for (i, planet) in system.planets.iter().enumerate() {
        commands.spawn((
            OnRails(planet.orbit),
            CelestialBody {
                mu: oj_orbits::G * planet.mass,
                radius: planet.radius.max(1.0e8),
                soi: oj_orbits::sphere_of_influence(
                    planet.orbit.semi_major,
                    planet.mass,
                    system.sun.mass,
                ),
                name: format!("Planet {}", i + 1),
            },
            SimPos::default(),
            Mesh3d(planet_mesh.clone()),
            MeshMaterial3d(planet_mat.clone()),
            Transform::default(),
        ));
    }

    // The ship starts in a comfortable circular orbit of the sun.
    let r = system
        .planets
        .first()
        .map(|p| p.orbit.semi_major * 0.6)
        .unwrap_or(1.0e11);
    let v = circular_speed(mu, r);
    commands.spawn((
        Ship::default(),
        crate::command::NavState::Free,
        OriginAnchor,
        SimPos(Vec3d::new(r, 0.0, 0.0)),
        SimVel(Vec3d::new(0.0, v, 0.0)),
        Mesh3d(meshes.add(Cone::new(6.0e7, 2.0e8).mesh().resolution(16))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.85, 0.9),
            metallic: 0.8,
            ..default()
        })),
        Transform::default(),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, -1.2e9, 6.0e8).looking_at(Vec3::ZERO, Vec3::Z),
    ));
    info!(
        "system {:?}: {:?} sun, {} planets",
        game.current,
        system.sun.class,
        system.planets.len()
    );
}

fn place_celestials(clock: Res<SimClock>, mut bodies: Query<(&OnRails, &mut SimPos)>) {
    for (rails, mut pos) in &mut bodies {
        pos.0 = rails.0.state_at(clock.0).0;
    }
}

/// Arrow keys thrust in the orbital plane; costs energy.
fn ship_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut ships: Query<(&mut Ship, &SimVel)>,
) {
    let (mut ship, _vel) = match ships.single_mut() {
        Ok(s) => s,
        Err(_) => return,
    };
    let thrusting = keys.pressed(KeyCode::ArrowUp)
        || keys.pressed(KeyCode::ArrowDown)
        || keys.pressed(KeyCode::ArrowLeft)
        || keys.pressed(KeyCode::ArrowRight);
    if thrusting {
        ship.energy = (ship.energy - 4.0 * DT * TIME_WARP / 60.0).max(0.0);
    }
}

fn integrate_ships(
    keys: Res<ButtonInput<KeyCode>>,
    suns: Query<(&SunBody, &SimPos), Without<Ship>>,
    bodies: Query<(&CelestialBody, &SimPos), Without<Ship>>,
    mut ships: Query<(&Ship, &mut SimPos, &mut SimVel), With<Ship>>,
) {
    let Ok((_sun, sun_pos)) = suns.single() else { return };
    for (ship, mut pos, mut vel) in &mut ships {
        // Every celestial pulls: this is what makes slingshots and planet
        // capture REAL physics rather than scripted moves.
        let mut accel = oj_orbits::Vec3d::ZERO;
        for (body, body_pos) in &bodies {
            accel += gravity_accel(body.mu, body_pos.0, pos.0, body.radius);
        }
        if ship.energy > 0.0 {
            // Thrust in the orbital plane: prograde/retrograde on up/down,
            // radial on left/right. Scaled by warp so it stays effective.
            let prograde = vel.0.normalized();
            let radial = (pos.0 - sun_pos.0).normalized();
            let t = ship.thrust * TIME_WARP;
            if keys.pressed(KeyCode::ArrowUp) {
                accel += prograde * t;
            }
            if keys.pressed(KeyCode::ArrowDown) {
                accel += -prograde * t;
            }
            if keys.pressed(KeyCode::ArrowLeft) {
                accel += -radial * t;
            }
            if keys.pressed(KeyCode::ArrowRight) {
                accel += radial * t;
            }
        }
        let dt = DT * TIME_WARP;
        integrate_step(&mut pos.0, &mut vel.0, accel, dt);
    }
}

/// Near the sun: harvest energy — and, past the hazard radius, take the
/// class's periodic damage (shields first, then hull) unless the shield
/// tier meets the requirement. The core risk/reward loop.
fn harvest_and_hazard(
    suns: Query<(&SunBody, &SimPos), Without<Ship>>,
    mut ships: Query<(&mut Ship, &SimPos), With<Ship>>,
) {
    let Ok((sun, sun_pos)) = suns.single() else { return };
    let dt = DT * TIME_WARP;
    for (mut ship, pos) in &mut ships {
        let r = pos.0.distance(sun_pos.0);
        // Harvest falls off with square of distance from the hazard edge.
        let harvest_edge = sun.hazard_radius * 8.0;
        if r < harvest_edge {
            let closeness = (harvest_edge / r.max(1.0)).min(8.0);
            let rate = sun.class.harvest_rate() * closeness * 0.05;
            ship.energy = (ship.energy + rate * dt).min(ship.energy_max);
        }
        if r < sun.hazard_radius && ship.shield_tier < sun.class.required_shield_tier() {
            let proximity = sun.hazard_radius / r.max(1.0);
            let dps = sun.class.hazard_dps() * proximity * 0.02;
            let dmg = dps * dt;
            if ship.shield > 0.0 {
                ship.shield = (ship.shield - dmg).max(0.0);
            } else {
                ship.hull = (ship.hull - dmg).max(0.0);
                if ship.hull == 0.0 {
                    // Explosion + wreckage arrive with the salvage module.
                    warn!("hull breach: ship lost");
                }
            }
        } else if ship.shield < 100.0 {
            ship.shield = (ship.shield + 0.5 * dt).min(100.0);
        }
    }
}

/// Camera-relative f32 rendering: subtract the anchor's f64 position, cast
/// down, hand bevy an ordinary Transform. Render-space scale compresses the
/// scene so f32 depth stays sane.
const RENDER_SCALE: f64 = 1.0 / 1.0e7;

fn sync_render_transforms(
    anchors: Query<&SimPos, With<OriginAnchor>>,
    mut entities: Query<(&SimPos, &mut Transform)>,
) {
    let Ok(anchor) = anchors.single() else { return };
    let origin = anchor.0;
    for (pos, mut transform) in &mut entities {
        let rel = (pos.0 - origin) * RENDER_SCALE;
        transform.translation = Vec3::new(rel.x as f32, rel.y as f32, rel.z as f32);
    }
}
