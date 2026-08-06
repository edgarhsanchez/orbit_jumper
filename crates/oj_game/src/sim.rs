//! The simulation: rails, gravity, energy, and the sun that kills you.

use bevy::prelude::*;
use oj_orbits::{KeplerOrbit, Vec3d, circular_speed, gravity_accel, integrate_step};
use oj_universe::{SolarSystem, SunClass};

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
                    place_child_rails,
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

/// A body on rails around the system origin (the sun).
#[derive(Component)]
pub struct OnRails(pub KeplerOrbit);

/// A body on rails around another railed body (moons, ring debris).
/// Positions/velocities compose: parent state + local orbit state.
#[derive(Component)]
pub struct OnRailsAround {
    pub orbit: KeplerOrbit,
    pub parent: Entity,
}

/// Rails-derived velocity, m/s, updated with SimPos each tick. Zero for
/// the sun. Guidance and assist logic read this instead of re-deriving.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BodyVel(pub Vec3d);

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
    /// Orbital overspeed factor while riding a body: 1.0 = natural circular
    /// speed; above it the gravity drive pushes the ship faster than the
    /// orbit wants, paying continuous energy for the centripetal deficit.
    pub orbit_boost: f64,
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
            orbit_boost: 1.0,
        }
    }
}

/// Marks the entity the floating origin follows (the ship, for now).
#[derive(Component)]
pub struct OriginAnchor;

/// Everything owned by the current solar system — torn down by a jump,
/// rebuilt by [`spawn_bodies`]. The ship, camera, and UI are NOT scoped.
#[derive(Component)]
pub struct SystemScoped;

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
    spawn_bodies(&mut commands, &system, &mut meshes, &mut materials);

    // The ship starts in a comfortable circular orbit of the sun.
    let mu = oj_orbits::G * system.sun.mass;
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
        Mesh3d(meshes.add(Cone::new(6.0, 20.0).mesh().resolution(16))),
        // Slight emissive: a bare-metal hull with no environment map
        // renders black in space (verified by screenshot, 2026-08-06).
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.85, 0.9),
            metallic: 0.2,
            emissive: LinearRgba::rgb(2.0, 2.6, 3.6),
            ..default()
        })),
        Transform::default(),
    ));

    // Camera works in RENDER units (sim / 1e7): the ship rides at the
    // render origin; orbits live in the XY plane, so the view is
    // top-down. The far plane must reach the sun from anywhere in the
    // system (~1e5 render units), nowhere near the 1000-unit default.
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            far: 5.0e6,
            ..default()
        }),
        // The sun is the main light; ambient keeps night sides readable.
        AmbientLight {
            color: Color::srgb(0.7, 0.8, 1.0),
            brightness: 300.0,
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 600.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    info!(
        "system {:?}: {:?} sun, {} planets",
        game.current,
        system.sun.class,
        system.planets.len()
    );
}

/// Spawn a system's celestial content: sun, planets, moons, ring debris.
/// Deterministic per system id, so a revisited system looks the same.
pub fn spawn_bodies(
    commands: &mut Commands,
    system: &SolarSystem,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let mu = oj_orbits::G * system.sun.mass;

    // Sun at the origin of the system-local frame.
    commands.spawn((
        SystemScoped,
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
        BodyVel::default(),
        Mesh3d(meshes.add(Sphere::new(200.0).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            emissive: LinearRgba::rgb(8.0, 6.5, 3.0),
            base_color: Color::srgb(1.0, 0.9, 0.5),
            ..default()
        })),
        PointLight {
            color: Color::srgb(1.0, 0.95, 0.85),
            intensity: 3.0e13,
            range: 2.0e6,
            radius: 200.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::default(),
    ));

    // Planets on rails. One shared mesh: automatic instancing batches them.
    let planet_mesh = meshes.add(Sphere::new(40.0).mesh().ico(3).unwrap());
    let planet_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.5, 0.6),
        perceptual_roughness: 0.9,
        ..default()
    });
    let moon_mesh = meshes.add(Sphere::new(12.0).mesh().ico(2).unwrap());
    let moon_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.58, 0.55),
        perceptual_roughness: 1.0,
        ..default()
    });
    let debris_mesh = meshes.add(Cuboid::new(3.0, 2.0, 2.5).mesh());
    let debris_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.4, 0.38, 0.35),
        ..default()
    });
    let mut debris_rng = oj_universe::SplitMix64(
        0xDEB215
            ^ (system.id.index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (system.id.sector.x as u64).rotate_left(17)
            ^ (system.id.sector.y as u64).rotate_left(34)
            ^ (system.id.sector.z as u64).rotate_left(51),
    );
    for (i, planet) in system.planets.iter().enumerate() {
        let planet_entity = commands
            .spawn((
                SystemScoped,
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
                BodyVel::default(),
                Mesh3d(planet_mesh.clone()),
                MeshMaterial3d(planet_mat.clone()),
                Transform::default(),
            ))
            .id();
        // Moons: commandable bodies riding parented rails.
        for (m, moon) in planet.moons.iter().enumerate() {
            commands.spawn((
                SystemScoped,
                OnRailsAround { orbit: moon.orbit, parent: planet_entity },
                CelestialBody {
                    mu: oj_orbits::G * moon.mass,
                    radius: moon.radius.max(3.0e7),
                    soi: oj_orbits::sphere_of_influence(
                        moon.orbit.semi_major,
                        moon.mass,
                        planet.mass,
                    ),
                    name: format!("Planet {} moon {}", i + 1, (b'a' + m as u8) as char),
                },
                SimPos::default(),
                BodyVel::default(),
                Mesh3d(moon_mesh.clone()),
                MeshMaterial3d(moon_mat.clone()),
                Transform::default(),
            ));
        }
        // Ring debris: salvage on parented rails, density from the seed.
        let count = (planet.debris_density * 30.0) as u32;
        for _ in 0..count {
            let r = planet.radius.max(1.0e8) * debris_rng.range(4.0, 14.0);
            commands.spawn((
                SystemScoped,
                OnRailsAround {
                    orbit: KeplerOrbit {
                        mu: oj_orbits::G * planet.mass,
                        semi_major: r,
                        eccentricity: debris_rng.range(0.0, 0.1),
                        inclination: debris_rng.range(-0.15, 0.15),
                        raan: debris_rng.range(0.0, std::f64::consts::TAU),
                        arg_periapsis: 0.0,
                        mean_anomaly_epoch: debris_rng.range(0.0, std::f64::consts::TAU),
                    },
                    parent: planet_entity,
                },
                crate::modules::Wreck {
                    value: 5,
                    element: {
                        let opts = oj_materials::Element::from_profile(planet.resources);
                        opts[(debris_rng.next_u64() % opts.len() as u64) as usize]
                    },
                },
                SimPos::default(),
                BodyVel::default(),
                Mesh3d(debris_mesh.clone()),
                MeshMaterial3d(debris_mat.clone()),
                Transform::default(),
            ));
        }
    }
}

fn place_celestials(clock: Res<SimClock>, mut bodies: Query<(&OnRails, &mut SimPos, &mut BodyVel)>) {
    for (rails, mut pos, mut vel) in &mut bodies {
        let (p, v) = rails.0.state_at(clock.0);
        pos.0 = p;
        vel.0 = v;
    }
}

/// Runs after [`place_celestials`]: children read their parent's fresh
/// state. Filters keep the two SimPos borrows disjoint.
fn place_child_rails(
    clock: Res<SimClock>,
    parents: Query<(&SimPos, &BodyVel), Without<OnRailsAround>>,
    mut children: Query<(&OnRailsAround, &mut SimPos, &mut BodyVel), With<OnRailsAround>>,
) {
    for (rails, mut pos, mut vel) in &mut children {
        let Ok((ppos, pvel)) = parents.get(rails.parent) else { continue };
        let (p, v) = rails.orbit.state_at(clock.0);
        pos.0 = ppos.0 + p;
        vel.0 = pvel.0 + v;
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
/// scene so f32 depth stays sane. MESHES ARE AUTHORED IN RENDER UNITS
/// (sim meters / 1e7): only translations pass through this scale, so a
/// mesh authored in meters renders 1e7x too large (found by screenshot —
/// the camera sat inside the ship's own cone).
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
