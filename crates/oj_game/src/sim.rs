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
        app.init_asset::<crate::fx::SunMaterial>();
        app.init_resource::<GameUniverse>()
            .insert_resource(ShipStyle::load())
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
            .init_resource::<ViewMode>()
            .init_resource::<CameraRig>()
            .add_systems(
                Update,
                (
                    sync_render_transforms,
                    orient_ship,
                    flame_visibility,
                    camera_zoom,
                    toggle_view,
                    tint_rings,
                    drive_camera.after(sync_render_transforms).after(orient_ship),
                ),
            );
        #[cfg(debug_assertions)]
        app.add_systems(Update, debug_teleport);
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
#[derive(Component, Clone)]
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

/// Where the pilot is looking from. Tactical is the oblique overview
/// where planning happens (rings, map, yard); Cockpit is flying it.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ViewMode {
    #[default]
    Tactical,
    Cockpit,
}

/// Camera state: tactical zoom and viewing angle persist across mode
/// flips. `pitch` is elevation above the orbital plane, radians —
/// steep is map-like, shallow is the cinematic skim the screenshots
/// show.
#[derive(Resource)]
pub struct CameraRig {
    pub zoom: f32,
    pub pitch: f32,
    /// Azimuth around the ship, radians — the pan pad swings it.
    pub yaw: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self { zoom: 600.0, pitch: 1.0, yaw: 0.0 }
    }
}

/// Everything owned by the current solar system — torn down by a jump,
/// rebuilt by [`spawn_bodies`]. The ship, camera, and UI are NOT scoped.
#[derive(Component)]
pub struct SystemScoped;

/// A clickable ride orbit around a celestial body. Click+hold the ring
/// to transfer into orbit at exactly `ride_r`.
#[derive(Component)]
pub struct OrbitRing {
    pub body: Entity,
    /// Orbit radius, sim meters.
    pub ride_r: f64,
}

/// Shared ring materials: dim at rest, bright under the pointer.
#[derive(Resource)]
pub struct OrbitRingMaterials {
    pub dim: Handle<StandardMaterial>,
    pub bright: Handle<StandardMaterial>,
    pub unreachable: Handle<StandardMaterial>,
}

/// Candidate ride orbits for a body, innermost first: fixed multiples of
/// the physical radius, capped inside the sphere of influence. Bigger
/// bodies therefore carry bigger (and more) rings — a giant's outer ring
/// is grabbable from far away even when the body itself is a distant dot.
pub fn orbit_rings(radius: f64, soi: f64) -> Vec<f64> {
    const MULTS: [f64; 4] = [3.0, 8.0, 20.0, 50.0];
    let cap = soi * 0.6;
    let mut rings: Vec<f64> = MULTS
        .iter()
        .map(|m| radius * m)
        .filter(|r| *r <= cap)
        .collect();
    if rings.is_empty() {
        rings.push((radius * 1.5).min(cap).max(radius * 1.2));
    }
    rings
}

/// A lumpy, banded planetoid mesh: an ico-sphere with seeded radial
/// displacement (nothing in nature is a perfect sphere) and per-vertex
/// shading bands (nothing is one flat color). StandardMaterial picks the
/// vertex colors up automatically.
fn planetoid_mesh(radius: f32, seed: u64, roughness: f32, shades: [(f32, f32, f32); 3]) -> Mesh {
    use bevy::mesh::{Mesh, VertexAttributeValues};
    let mut rng = oj_universe::SplitMix64(seed);
    // A handful of random plane-wave directions gives smooth value noise
    // without a noise crate: n(p) = mean(sin(p . k_i + phi_i)).
    let waves: Vec<(Vec3, f32, f32)> = (0..5)
        .map(|_| {
            let dir = Vec3::new(
                rng.range(-1.0, 1.0) as f32,
                rng.range(-1.0, 1.0) as f32,
                rng.range(-1.0, 1.0) as f32,
            )
            .normalize_or(Vec3::X);
            (dir, rng.range(1.5, 4.5) as f32, rng.range(0.0, 6.28) as f32)
        })
        .collect();
    let noise = |p: Vec3| -> f32 {
        waves
            .iter()
            .map(|(d, f, ph)| (p.dot(*d) * f + ph).sin())
            .sum::<f32>()
            / waves.len() as f32
    };

    let mut mesh = Sphere::new(radius).mesh().ico(4).unwrap();
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).cloned()
    else {
        return mesh;
    };
    let mut new_pos = Vec::with_capacity(positions.len());
    let mut colors = Vec::with_capacity(positions.len());
    for p in &positions {
        let v = Vec3::from_array(*p);
        let unit = v / radius;
        let n = noise(unit);
        let bumped = v * (1.0 + roughness * n);
        new_pos.push(bumped.to_array());
        // Bands: latitude + noise chooses among three shades.
        let band = (unit.z * 2.2 + noise(unit * 1.7) * 1.4).sin() * 0.5 + 0.5;
        let (a, b, c) = if band < 0.4 {
            (shades[0], shades[1], band / 0.4)
        } else {
            (shades[1], shades[2], (band - 0.4) / 0.6)
        };
        let t = c.clamp(0.0, 1.0);
        colors.push([
            a.0 + (b.0 - a.0) * t,
            a.1 + (b.1 - a.1) * t,
            a.2 + (b.2 - a.2) * t,
            1.0,
        ]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, new_pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.compute_smooth_normals();
    mesh
}

fn spawn_current_system(
    mut commands: Commands,
    game: Res<GameUniverse>,
    game_style: Res<ShipStyle>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sun_materials: ResMut<Assets<crate::fx::SunMaterial>>,
) {
    let Some(system) = game.universe.system(game.current) else {
        error!("current system does not exist; universe misconfigured");
        return;
    };
    spawn_bodies(&mut commands, &system, &mut meshes, &mut materials, &mut sun_materials);

    // The ship starts in a comfortable circular orbit of the sun.
    let mu = oj_orbits::G * system.sun.mass;
    let r = system
        .planets
        .first()
        .map(|p| p.orbit.semi_major * 0.6)
        .unwrap_or(1.0e11);
    let v = circular_speed(mu, r);
    let style = *game_style;
    spawn_ship(commands.reborrow(), &mut meshes, &mut materials, Vec3d::new(r, 0.0, 0.0), Vec3d::new(0.0, v, 0.0), style);

    // Camera works in RENDER units (sim / 1e7): the ship rides at the
    // render origin; orbits live in the XY plane, so the view is
    // top-down. The far plane must reach the sun from anywhere in the
    // system (~1e5 render units), nowhere near the 1000-unit default.
    commands.spawn((
        Camera3d::default(),
        bevy::camera::Hdr,
        // Emissives (sun, engine flames, missiles, stars) glow for free.
        bevy::post_process::bloom::Bloom::NATURAL,
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
        Transform::from_translation(Vec3::new(0.0, -330.0, 510.0))
            .looking_at(Vec3::ZERO, Vec3::Z),
    ));

    // Distant starfield: anchored to the ship's render frame (the ship is
    // always at the origin/screen center, so the sky reads as infinitely
    // far). Below the orbital plane, so everything draws over it.
    let star_mesh = meshes.add(Sphere::new(1.6).mesh().ico(1).unwrap());
    let star_mats = [
        materials.add(StandardMaterial {
            base_color: Color::linear_rgba(1.4, 1.4, 1.6, 1.0),
            unlit: true,
            ..default()
        }),
        materials.add(StandardMaterial {
            base_color: Color::linear_rgba(0.9, 1.0, 1.5, 1.0),
            unlit: true,
            ..default()
        }),
        materials.add(StandardMaterial {
            base_color: Color::linear_rgba(1.5, 1.1, 0.8, 1.0),
            unlit: true,
            ..default()
        }),
    ];
    let mut sky_rng = oj_universe::SplitMix64(0x57A2F1E1D);
    for _ in 0..640 {
        let x = sky_rng.range(-4200.0, 4200.0) as f32;
        let y = sky_rng.range(-4200.0, 4200.0) as f32;
        // Both hemispheres, clear of the play plane: the cockpit horizon
        // needs stars above it, not only the tactical floor below.
        let z = sky_rng.range(900.0, 2600.0) as f32
            * if sky_rng.next_u64() % 2 == 0 { -1.0 } else { 1.0 };
        let scale = sky_rng.range(0.5, 1.8) as f32;
        let mat = star_mats[(sky_rng.next_u64() % 3) as usize].clone();
        commands.spawn((
            Mesh3d(star_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_xyz(x, y, z).with_scale(Vec3::splat(scale)),
        ));
    }
    info!(
        "system {:?}: {:?} sun, {} planets",
        game.current,
        system.sun.class,
        system.planets.len()
    );
}

/// Spawn a system's celestial content: sun, planets, moons, ring debris.
/// Deterministic per system id, so a revisited system looks the same.
/// Class temperature on the shader's 0..1 heat axis.
fn sun_heat(class: SunClass) -> f32 {
    match class {
        SunClass::M => 0.05,
        SunClass::K => 0.18,
        SunClass::G => 0.32,
        SunClass::F => 0.45,
        SunClass::A => 0.6,
        SunClass::B => 0.75,
        SunClass::O => 0.9,
        SunClass::NeutronStar => 0.97,
        SunClass::Magnetar => 1.0,
        SunClass::BlackHole => 0.0,
    }
}

pub fn spawn_bodies(
    commands: &mut Commands,
    system: &SolarSystem,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    sun_materials: &mut Assets<crate::fx::SunMaterial>,
) {
    let mu = oj_orbits::G * system.sun.mass;

    // Ride-orbit ring materials: dim at rest, bright under the pointer.
    let ring_dim = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.9, 1.0, 0.12),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    let ring_bright = materials.add(StandardMaterial {
        base_color: Color::srgba(0.3, 1.0, 1.0, 0.55),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    // Out of energy reach: unmistakably gray, and the click bounces.
    let ring_unreachable = materials.add(StandardMaterial {
        base_color: Color::srgba(0.55, 0.58, 0.62, 0.22),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    commands.insert_resource(OrbitRingMaterials {
        dim: ring_dim.clone(),
        bright: ring_bright,
        unreachable: ring_unreachable,
    });

    // Sun at the origin of the system-local frame.
    let sun_radius = system.sun.radius.max(2.0e9);
    let sun_entity = commands.spawn((
        SystemScoped,
        SunBody {
            class: system.sun.class,
            hazard_radius: system.sun.hazard_radius,
        },
        crate::nova::ObjectLevel(crate::nova::sun_level(system.sun.class)),
        CelestialBody {
            mu,
            radius: sun_radius,
            soi: f64::INFINITY,
            name: format!("{:?}-class sun", system.sun.class),
        },
        SimPos(Vec3d::ZERO),
        BodyVel::default(),
        // The living sun: a shader-driven plasma core with a licking
        // flame shell and breathing corona layered over it — animated
        // entirely on the GPU, class-tinted from deep M-red to O-blue.
        Mesh3d(meshes.add(Sphere::new(200.0).mesh().ico(5).unwrap())),
        MeshMaterial3d(sun_materials.add(crate::fx::SunMaterial::new(
            sun_heat(system.sun.class),
            0.0,
            (system.sun.radius % 97.0e7) as f32 / 97.0e7,
            1.0,
        ))),
        Transform::default(),
    )).id();
    // The sun's light rides its OWN mesh-less entity, synced to the same
    // SimPos. On the sun entity the light was silently culled whenever
    // the sun's mesh left the frustum (one entity, one ViewVisibility,
    // computed from the mesh AABB) — i.e. almost always in tactical
    // view, so nothing in the system ever showed a day side. A bare
    // light entity is visibility-tested on its range sphere instead.
    commands.spawn((
        SystemScoped,
        PointLight {
            color: Color::srgb(1.0, 0.95, 0.85),
            // Bright enough to out-shine the ambient at gameplay orbit
            // distances (tens of thousands of render units), so debris,
            // ships and planets read a real day side facing the sun —
            // without clipping small pieces to white at default exposure.
            intensity: 3.0e13,
            range: 2.0e6,
            radius: 200.0,
            shadow_maps_enabled: false,
            ..default()
        },
        SimPos(Vec3d::ZERO),
        BodyVel::default(),
        Transform::default(),
    ));
    let heat = sun_heat(system.sun.class);
    let seed = (system.sun.radius % 97.0e7) as f32 / 97.0e7;
    let shell_mesh = meshes.add(Sphere::new(200.0).mesh().ico(4).unwrap());
    for (mode, scale, boost) in [(1.0, 1.10f32, 1.0), (2.0, 1.32, 1.0)] {
        let shell = commands
            .spawn((
                Mesh3d(shell_mesh.clone()),
                MeshMaterial3d(sun_materials.add(crate::fx::SunMaterial::new(
                    heat, mode, seed, boost,
                ))),
                Transform::from_scale(Vec3::splat(scale)),
                bevy::picking::Pickable::IGNORE,
            ))
            .id();
        commands.entity(sun_entity).add_child(shell);
    }
    spawn_rings_for(commands, sun_entity, sun_radius, f64::INFINITY, meshes, &ring_dim);

    // Planets on rails. Meshes are per-body at PHYSICAL radius (render
    // units): ride rings are geometry the pilot reasons about, so the
    // sprite must not lie about where the surface is. Palette varies per
    // planet from the seed, so systems feel distinct.
    const PLANET_PALETTE: [(f32, f32, f32); 6] = [
        (0.45, 0.52, 0.62),
        (0.62, 0.42, 0.32),
        (0.66, 0.56, 0.36),
        (0.5, 0.66, 0.72),
        (0.5, 0.58, 0.42),
        (0.56, 0.48, 0.66),
    ];
    let moon_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.58, 0.55),
        perceptual_roughness: 1.0,
        ..default()
    });
    let debris_mesh = meshes.add(Cuboid::new(3.0, 2.0, 2.5).mesh());
    let mut debris_rng = oj_universe::SplitMix64(
        0xDEB215
            ^ (system.id.index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (system.id.sector.x as u64).rotate_left(17)
            ^ (system.id.sector.y as u64).rotate_left(34)
            ^ (system.id.sector.z as u64).rotate_left(51),
    );
    for (i, planet) in system.planets.iter().enumerate() {
        let planet_radius = planet.radius.max(1.0e8);
        let planet_soi = oj_orbits::sphere_of_influence(
            planet.orbit.semi_major,
            planet.mass,
            system.sun.mass,
        );
        let planet_entity = commands
            .spawn((
                SystemScoped,
                OnRails(planet.orbit),
                crate::nova::ObjectLevel(crate::nova::body_level(planet_radius)),
                CelestialBody {
                    mu: oj_orbits::G * planet.mass,
                    radius: planet_radius,
                    soi: planet_soi,
                    name: format!("Planet {}", i + 1),
                },
                SimPos::default(),
                BodyVel::default(),
                Mesh3d({
                    let (pr, pg, pb) =
                        PLANET_PALETTE[(system.id.index as usize + i) % PLANET_PALETTE.len()];
                    meshes.add(planetoid_mesh(
                        (planet_radius * RENDER_SCALE) as f32,
                        system.id.index as u64 ^ (i as u64) << 17 ^ 0x9EA7,
                        0.09,
                        [
                            (pr * 0.55, pg * 0.55, pb * 0.6),
                            (pr, pg, pb),
                            (pr * 1.25, pg * 1.2, pb * 1.05),
                        ],
                    ))
                }),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    perceptual_roughness: 0.9,
                    ..default()
                })),
                Transform::default(),
            ))
            .id();
        spawn_rings_for(commands, planet_entity, planet_radius, planet_soi, meshes, &ring_dim);
        // Moons: commandable bodies riding parented rails.
        for (m, moon) in planet.moons.iter().enumerate() {
            let moon_radius = moon.radius.max(3.0e7);
            let moon_soi = oj_orbits::sphere_of_influence(
                moon.orbit.semi_major,
                moon.mass,
                planet.mass,
            );
            let moon_entity = commands.spawn((
                SystemScoped,
                OnRailsAround { orbit: moon.orbit, parent: planet_entity },
                crate::nova::ObjectLevel(crate::nova::body_level(moon_radius)),
                CelestialBody {
                    mu: oj_orbits::G * moon.mass,
                    radius: moon_radius,
                    soi: moon_soi,
                    name: format!("Planet {} moon {}", i + 1, (b'a' + m as u8) as char),
                },
                SimPos::default(),
                BodyVel::default(),
                Mesh3d(meshes.add(planetoid_mesh(
                    (moon_radius * RENDER_SCALE) as f32,
                    system.id.index as u64 ^ (i as u64) << 9 ^ (m as u64) << 23 ^ 0x30071,
                    0.16,
                    [(0.42, 0.4, 0.38), (0.6, 0.58, 0.55), (0.72, 0.7, 0.66)],
                ))),
                MeshMaterial3d(moon_mat.clone()),
                Transform::default(),
            )).id();
            spawn_rings_for(commands, moon_entity, moon_radius, moon_soi, meshes, &ring_dim);
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
                        inclination: debris_rng.range(-0.4, 0.4),
                        raan: debris_rng.range(0.0, std::f64::consts::TAU),
                        arg_periapsis: 0.0,
                        mean_anomaly_epoch: debris_rng.range(0.0, std::f64::consts::TAU),
                    },
                    parent: planet_entity,
                },
                {
                    let opts = oj_materials::Element::from_profile(planet.resources);
                    let element = opts[(debris_rng.next_u64() % opts.len() as u64) as usize];
                    (
                        crate::modules::Wreck { value: 5, element },
                        MeshMaterial3d(materials.add(crate::modules::debris_material(element))),
                    )
                },
                crate::modules::Tumble::seeded(debris_rng.next_u64()),
                SimPos::default(),
                BodyVel::default(),
                Mesh3d(debris_mesh.clone()),
                Transform::default(),
            ));
        }
    }

    // Comets: sun-grazers on fierce ellipses, seeded per system. They
    // outgas an anti-sunward tail, drop collectible ice along the path,
    // and hit hard if you cross one (comets.rs drives all of that).
    let outer = system
        .planets
        .last()
        .map(|p| p.orbit.semi_major)
        .unwrap_or(2.0e11);
    let mut comet_rng = oj_universe::SplitMix64(
        0xC04E75
            ^ (system.id.index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (system.id.sector.x as u64).rotate_left(13)
            ^ (system.id.sector.y as u64).rotate_left(29)
            ^ (system.id.sector.z as u64).rotate_left(47),
    );
    let comet_count = 2 + (comet_rng.next_u64() % 3) as usize;
    for c in 0..comet_count {
        commands.spawn((
            SystemScoped,
            OnRails(KeplerOrbit {
                mu,
                semi_major: outer * comet_rng.range(0.55, 1.35),
                eccentricity: comet_rng.range(0.72, 0.93),
                inclination: comet_rng.range(-0.3, 0.3),
                raan: comet_rng.range(0.0, std::f64::consts::TAU),
                arg_periapsis: comet_rng.range(0.0, std::f64::consts::TAU),
                mean_anomaly_epoch: comet_rng.range(0.0, std::f64::consts::TAU),
            }),
            crate::comets::Comet::default(),
            SimPos::default(),
            BodyVel::default(),
            Mesh3d(meshes.add(planetoid_mesh(
                6.0,
                system.id.index as u64 ^ (c as u64) << 29 ^ 0x1CE,
                0.22,
                [(0.55, 0.62, 0.7), (0.72, 0.8, 0.88), (0.88, 0.94, 1.0)],
            ))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 0.55,
                ..default()
            })),
            Transform::default(),
            bevy::picking::Pickable::IGNORE,
        ));
    }
}

/// Spawn a body's ride-orbit rings as render-space children: the parent's
/// per-frame Transform positions them, so they follow the body for free
/// and despawn with it (recursive despawn on jumps).
fn spawn_rings_for(
    commands: &mut Commands,
    body: Entity,
    radius: f64,
    soi: f64,
    meshes: &mut Assets<Mesh>,
    dim: &Handle<StandardMaterial>,
) {
    for ride_r in orbit_rings(radius, soi) {
        let r_render = (ride_r * RENDER_SCALE) as f32;
        // Thick enough to hover and click at every scale.
        let thickness = (r_render * 0.02).clamp(2.0, 16.0);
        let ring = commands
            .spawn((
                OrbitRing { body, ride_r },
                Mesh3d(meshes.add(
                    Torus {
                        minor_radius: thickness,
                        major_radius: r_render,
                    }
                    .mesh()
                    .major_resolution(96)
                    .minor_resolution(6),
                )),
                MeshMaterial3d(dim.clone()),
                // The torus lies in XZ; orbits live in XY.
                Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            ))
            .observe(ring_hover_on)
            .observe(ring_hover_off)
            .id();
        commands.entity(body).add_child(ring);
    }
}

fn ring_hover_on(
    over: On<bevy::picking::events::Pointer<bevy::picking::events::Over>>,
    mats: Res<OrbitRingMaterials>,
    reach: Res<crate::command::RingReach>,
    mut rings: Query<&mut MeshMaterial3d<StandardMaterial>, With<OrbitRing>>,
) {
    // An unreachable ring does not light up: the gray IS the answer.
    if reach.flags.get(&over.entity).copied() == Some(false) {
        return;
    }
    if let Ok(mut m) = rings.get_mut(over.entity) {
        m.0 = mats.bright.clone();
    }
}

fn ring_hover_off(
    out: On<bevy::picking::events::Pointer<bevy::picking::events::Out>>,
    mats: Res<OrbitRingMaterials>,
    reach: Res<crate::command::RingReach>,
    mut rings: Query<&mut MeshMaterial3d<StandardMaterial>, With<OrbitRing>>,
) {
    if let Ok(mut m) = rings.get_mut(out.entity) {
        m.0 = if reach.flags.get(&out.entity).copied() == Some(false) {
            mats.unreachable.clone()
        } else {
            mats.dim.clone()
        };
    }
}

/// Keep every ring's base tint in line with the standing reachability
/// verdict; the hover brighten stays untouched for reachable rings.
fn tint_rings(
    reach: Res<crate::command::RingReach>,
    mats: Res<OrbitRingMaterials>,
    mut rings: Query<(Entity, &mut MeshMaterial3d<StandardMaterial>), With<OrbitRing>>,
) {
    for (entity, mut m) in &mut rings {
        let reachable = reach.flags.get(&entity).copied().unwrap_or(true);
        if !reachable {
            if m.0 != mats.unreachable {
                m.0 = mats.unreachable.clone();
            }
        } else if m.0 == mats.unreachable {
            m.0 = mats.dim.clone();
        }
    }
}

/// The engine exhaust cone; visible while burning.
#[derive(Component)]
pub struct EngineFlame;

/// Hull silhouettes the yard can build.
pub const SHIP_FRAMES: [&str; 3] = ["DART", "LANCE", "HAMMER"];
/// Paint schemes: (name, base color, glow).
pub const SHIP_PAINTS: [(&str, (f32, f32, f32), (f32, f32, f32)); 5] = [
    ("STEEL", (0.75, 0.82, 0.9), (0.35, 0.5, 0.7)),
    ("EMBER", (0.9, 0.55, 0.35), (0.9, 0.3, 0.08)),
    ("VIRIDIAN", (0.45, 0.85, 0.6), (0.1, 0.7, 0.35)),
    ("VIOLET", (0.7, 0.55, 0.95), (0.45, 0.2, 0.9)),
    ("AURUM", (0.95, 0.8, 0.4), (0.9, 0.6, 0.1)),
];
/// Accent (wing/nozzle) schemes.
pub const SHIP_ACCENTS: [(&str, (f32, f32, f32), (f32, f32, f32)); 4] = [
    ("CYAN", (0.25, 0.75, 0.85), (0.05, 0.5, 0.6)),
    ("CRIMSON", (0.85, 0.3, 0.35), (0.6, 0.05, 0.1)),
    ("ICE", (0.8, 0.9, 1.0), (0.4, 0.55, 0.8)),
    ("SOL", (0.95, 0.75, 0.3), (0.8, 0.5, 0.05)),
];

/// The pilot's ship styling, persisted across sessions.
#[derive(Resource, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct ShipStyle {
    pub frame: usize,
    pub paint: usize,
    pub accent: usize,
}

fn style_path() -> std::path::PathBuf {
    std::path::PathBuf::from("orbit_jumper_style.ron")
}

impl ShipStyle {
    pub fn load() -> Self {
        std::fs::read_to_string(style_path())
            .ok()
            .and_then(|s| ron::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(text) = ron::to_string(self) {
            let _ = std::fs::write(style_path(), text);
        }
    }
    pub fn label(&self) -> String {
        format!(
            "{} · {} / {}",
            SHIP_FRAMES[self.frame % SHIP_FRAMES.len()],
            SHIP_PAINTS[self.paint % SHIP_PAINTS.len()].0,
            SHIP_ACCENTS[self.accent % SHIP_ACCENTS.len()].0,
        )
    }
}

/// Spawn the player's vessel: hull cone, swept wings, engine nozzle and
/// a bloom-lit exhaust flame (hidden until burning). The assembly is
/// render-space children of the hull, so orientation carries everything.
pub fn spawn_ship(
    mut commands: Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3d,
    vel: Vec3d,
    style: ShipStyle,
) {
    let (_, hull_rgb, hull_glow) = SHIP_PAINTS[style.paint % SHIP_PAINTS.len()];
    let (_, acc_rgb, acc_glow) = SHIP_ACCENTS[style.accent % SHIP_ACCENTS.len()];
    // Part materials: painted hull plate, accent plate, dark machinery,
    // strong accent glow (running lights), canopy glass.
    let hull_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(hull_rgb.0, hull_rgb.1, hull_rgb.2),
        metallic: 0.35,
        perceptual_roughness: 0.45,
        emissive: LinearRgba::rgb(hull_glow.0 * 0.25, hull_glow.1 * 0.25, hull_glow.2 * 0.25),
        ..default()
    });
    let acc_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(acc_rgb.0, acc_rgb.1, acc_rgb.2),
        metallic: 0.4,
        perceptual_roughness: 0.5,
        emissive: LinearRgba::rgb(acc_glow.0 * 0.3, acc_glow.1 * 0.3, acc_glow.2 * 0.3),
        ..default()
    });
    let mach_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.22, 0.24, 0.28),
        metallic: 0.6,
        perceptual_roughness: 0.7,
        ..default()
    });
    let glow_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(acc_rgb.0, acc_rgb.1, acc_rgb.2),
        emissive: LinearRgba::rgb(acc_glow.0 * 4.0, acc_glow.1 * 4.0, acc_glow.2 * 4.0),
        unlit: true,
        ..default()
    });
    let canopy_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.85, 1.0),
        emissive: LinearRgba::rgb(0.8, 1.6, 2.2),
        metallic: 0.2,
        ..default()
    });
    // Fire in three layers, hottest innermost. Alpha-blended so the
    // plume overlaps softly; bloom turns the emissives into glow.
    let flame_outer = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.45, 0.1, 0.55),
        emissive: LinearRgba::rgb(9.0, 2.8, 0.4),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let flame_mid = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.75, 0.25, 0.75),
        emissive: LinearRgba::rgb(14.0, 7.0, 1.2),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let flame_core = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.98, 0.85, 0.95),
        emissive: LinearRgba::rgb(22.0, 18.0, 10.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    // Space needs no streamlining: hulls are stepped segment stacks,
    // trusses, plates and pods — machinery you could believe in.
    // Parts: (size, material id, translation, z-rotation). Material ids:
    // 0 hull, 1 accent, 2 machinery, 3 glow, 4 canopy.
    let frame = style.frame % SHIP_FRAMES.len();
    let mut parts: Vec<((f32, f32, f32), usize, (f32, f32, f32), f32)> = vec![
        // Spine truss + cross-braces.
        ((1.2, 23.0, 1.2), 2, (0.0, -1.0, 0.0), 0.0),
        ((4.4, 0.8, 0.8), 2, (0.0, 2.0, 0.0), 0.0),
        ((4.4, 0.8, 0.8), 2, (0.0, -5.0, 0.0), 0.0),
        // Cockpit canopy near the nose.
        ((2.2, 3.0, 1.6), 4, (0.0, 6.0, 1.0), 0.0),
        // Engine block + twin nozzles.
        ((5.0, 3.6, 3.0), 2, (0.0, -9.0, 0.0), 0.0),
        ((1.6, 2.6, 1.6), 2, (-1.7, -11.4, 0.0), 0.0),
        ((1.6, 2.6, 1.6), 2, (1.7, -11.4, 0.0), 0.0),
        // Running-light strips.
        ((0.35, 12.0, 0.3), 3, (-1.9, 0.0, 1.0), 0.0),
        ((0.35, 12.0, 0.3), 3, (1.9, 0.0, 1.0), 0.0),
        // Antenna mast + sensor pod.
        ((0.25, 5.5, 0.25), 2, (2.3, 3.0, 1.8), 0.35),
        ((0.9, 0.9, 0.9), 3, (3.1, 5.2, 2.6), 0.0),
        // Radiator fins, angled.
        ((4.6, 2.8, 0.18), 1, (-3.6, -4.0, 0.7), 0.5),
        ((4.6, 2.8, 0.18), 1, (3.6, -4.0, 0.7), -0.5),
    ];
    match frame {
        // DART: stepped needle, layered swept wing plates.
        0 => parts.extend([
            ((4.6, 5.0, 2.6), 0, (0.0, 3.0, 0.0), 0.0),
            ((3.4, 4.2, 2.1), 0, (0.0, 7.4, 0.0), 0.0),
            ((1.9, 4.0, 1.5), 0, (0.0, 11.2, 0.0), 0.0),
            ((7.0, 3.8, 0.5), 1, (-5.0, -4.0, 0.0), 0.35),
            ((7.0, 3.8, 0.5), 1, (5.0, -4.0, 0.0), -0.35),
            ((4.4, 2.4, 0.5), 1, (-6.0, -6.4, 0.5), 0.5),
            ((4.4, 2.4, 0.5), 1, (6.0, -6.4, 0.5), -0.5),
        ]),
        // LANCE: long truss hull with outrigger pods.
        1 => parts.extend([
            ((3.2, 7.0, 2.0), 0, (0.0, 3.5, 0.0), 0.0),
            ((2.4, 6.0, 1.7), 0, (0.0, 9.5, 0.0), 0.0),
            ((1.4, 5.0, 1.2), 0, (0.0, 14.5, 0.0), 0.0),
            ((9.0, 1.1, 1.1), 2, (0.0, -2.0, 0.0), 0.0),
            ((1.8, 5.0, 1.8), 1, (-5.4, -2.0, 0.0), 0.0),
            ((1.8, 5.0, 1.8), 1, (5.4, -2.0, 0.0), 0.0),
            ((0.9, 0.9, 2.6), 3, (-5.4, 0.8, 0.0), 0.0),
            ((0.9, 0.9, 2.6), 3, (5.4, 0.8, 0.0), 0.0),
        ]),
        // HAMMER: broad decks, quad engines.
        _ => parts.extend([
            ((9.2, 4.2, 2.8), 0, (0.0, 2.0, 0.0), 0.0),
            ((6.6, 3.6, 2.4), 0, (0.0, 5.8, 0.0), 0.0),
            ((3.6, 3.0, 1.8), 0, (0.0, 9.0, 0.0), 0.0),
            ((10.0, 4.6, 0.6), 1, (-7.0, -3.0, 0.0), 0.2),
            ((10.0, 4.6, 0.6), 1, (7.0, -3.0, 0.0), -0.2),
            ((1.6, 2.6, 1.6), 2, (-4.4, -11.0, 0.0), 0.0),
            ((1.6, 2.6, 1.6), 2, (4.4, -11.0, 0.0), 0.0),
        ]),
    }
    // Seeded surface greebles: boxes of machinery, different every
    // paint/frame combination.
    let mut grng = oj_universe::SplitMix64(
        0x9EEB1E ^ (style.frame as u64) << 8 ^ (style.paint as u64) << 16 ^ (style.accent as u64),
    );
    for _ in 0..7 {
        let w = grng.range(0.5, 1.5) as f32;
        parts.push((
            (w, grng.range(0.6, 2.2) as f32, grng.range(0.3, 0.9) as f32),
            2,
            (
                grng.range(-2.2, 2.2) as f32,
                grng.range(-8.0, 4.0) as f32,
                grng.range(0.9, 1.4) as f32,
            ),
            grng.range(-0.3, 0.3) as f32,
        ));
    }

    let mats = [&hull_mat, &acc_mat, &mach_mat, &glow_mat, &canopy_mat];
    commands
        .spawn((
            Ship::default(),
            crate::command::NavState::Free,
            OriginAnchor,
            SimPos(pos),
            SimVel(vel),
            Transform::default(),
            Visibility::default(),
        ))
        .with_children(|ship| {
            for (size, mat, at, rot) in parts {
                ship.spawn((
                    Mesh3d(meshes.add(Cuboid::new(size.0, size.1, size.2).mesh())),
                    MeshMaterial3d(mats[mat].clone()),
                    Transform::from_xyz(at.0, at.1, at.2)
                        .with_rotation(Quat::from_rotation_z(rot)),
                ));
            }
            // The force field made visible: a translucent bubble whose
            // glow follows the shield points (nova.rs drives it). One
            // material per ship, mutated in place each frame.
            ship.spawn((
                crate::nova::ShieldBubble,
                Mesh3d(meshes.add(Sphere::new(24.0).mesh().ico(4).unwrap())),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgba(0.35, 0.85, 1.0, 0.1),
                    emissive: LinearRgba::rgb(0.2, 0.5, 0.65),
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    ..default()
                })),
                Transform::default(),
                bevy::light::NotShadowCaster,
                bevy::picking::Pickable::IGNORE,
            ));
            // Exhaust fire in layers, each flickering out of phase (the
            // fx module drives scale jitter) so the fire reads as alive.
            for (mat, radius, len, y, phase) in [
                (flame_outer, 3.2, 12.5, -17.2, 0.0f32),
                (flame_mid, 2.1, 9.5, -15.9, 2.1),
                (flame_core, 1.1, 6.5, -14.6, 4.4),
            ] {
                ship.spawn((
                    EngineFlame,
                    crate::fx::FlameFlicker {
                        phase,
                        base_scale: Vec3::ONE,
                    },
                    Mesh3d(meshes.add(Cone::new(radius, len).mesh().resolution(10))),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(0.0, y, 0.0)
                        .with_rotation(Quat::from_rotation_z(std::f32::consts::PI)),
                    Visibility::Hidden,
                ));
            }
        });
}

/// Rebuild the ship's visuals in the current style, preserving flight
/// state and stats — the yard repaints, it does not recommission.
#[allow(clippy::type_complexity)]
pub fn restyle_ship(world: &mut World) {
    let mut ships = world.query_filtered::<(Entity, &Ship, &SimPos, &crate::SimVel, &crate::command::NavState), ()>();
    let Some((entity, ship, pos, vel, nav)) = ships.iter(world).next().map(|(e, s, p, v, n)| {
        (e, s.clone(), *p, *v, *n)
    }) else {
        return;
    };
    world.entity_mut(entity).despawn();
    let style = *world.resource::<ShipStyle>();
    world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
        world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
            spawn_ship(world.commands(), &mut meshes, &mut materials, pos.0, vel.0, style);
        });
    });
    world.flush();
    // Restore the pilot's stats and nav mode onto the fresh hull.
    let mut ships = world.query_filtered::<(&mut Ship, &mut crate::command::NavState), ()>();
    if let Some((mut fresh, mut fresh_nav)) = ships.iter_mut(world).next() {
        *fresh = ship;
        *fresh_nav = nav;
    }
}

/// Point the hull along the velocity vector in FULL 3D (the cone's +Y
/// is forward): yaw follows the in-plane heading, pitch follows climbs
/// and dives — vertical burns visibly raise the nose.
fn orient_ship(mut ships: Query<(&SimVel, &mut Transform), With<Ship>>) {
    for (vel, mut transform) in &mut ships {
        if vel.0.length() > 1.0 {
            let dir = vel.0.normalized();
            let dir = Vec3::new(dir.x as f32, dir.y as f32, dir.z as f32);
            transform.rotation = Quat::from_rotation_arc(Vec3::Y, dir);
        }
    }
}

/// Show the exhaust while burning: manual thrust or a guided transfer.
fn flame_visibility(
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    ships: Query<(&Ship, &crate::command::NavState)>,
    mut flames: Query<&mut Visibility, With<EngineFlame>>,
) {
    let Ok((ship, nav)) = ships.single() else { return };
    let thrusting = joy.active
        || keys.any_pressed([
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::KeyE,
            KeyCode::KeyQ,
        ]);
    let burning = ship.energy > 0.0
        && (thrusting || matches!(nav, crate::command::NavState::Transfer { .. }));
    for mut vis in &mut flames {
        *vis = if burning { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Zoom, tactical view only: mouse wheel, or hold [-]/[=] for pilots
/// (and test rigs) without one. The zoom-out ceiling scales with the
/// orbit being ridden (or transferred to): a big ride deserves a view
/// wide enough to see where it is taking you.
fn camera_zoom(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    view: Res<ViewMode>,
    ships: Query<&crate::command::NavState, With<Ship>>,
    mut rig: ResMut<CameraRig>,
) {
    let scroll: f32 = wheel.read().map(|w| w.y).sum();
    if *view != ViewMode::Tactical {
        return;
    }
    let ceiling = match ships.single().ok() {
        Some(
            crate::command::NavState::Orbiting { ride_r, .. }
            | crate::command::NavState::Transfer { ride_r, .. },
        ) => 4000.0_f32.max((*ride_r * RENDER_SCALE) as f32 * 2.5),
        _ => 4000.0,
    };
    if scroll != 0.0 {
        rig.zoom = (rig.zoom * (1.0 - scroll * 0.12)).clamp(160.0, ceiling);
    }
    // Held keys triple the zoom per second, frame-rate independent.
    let held = keys.pressed(KeyCode::Minus) as i8 - keys.pressed(KeyCode::Equal) as i8;
    if held != 0 {
        let factor = 3.0f32.powf(time.delta_secs() * held as f32);
        rig.zoom = (rig.zoom * factor).clamp(160.0, ceiling);
    }
    // [ and ] tilt the view: from a near-top-down map to a shallow
    // cinematic skim over the orbital plane.
    let tilt = keys.pressed(KeyCode::BracketRight) as i8 - keys.pressed(KeyCode::BracketLeft) as i8;
    if tilt != 0 {
        rig.pitch = (rig.pitch + tilt as f32 * time.delta_secs() * 0.9).clamp(0.30, 1.50);
    }
}

/// F flips between the tactical overview and the cockpit.
fn toggle_view(keys: Res<ButtonInput<KeyCode>>, mut view: ResMut<ViewMode>) {
    if keys.just_pressed(KeyCode::KeyF) {
        *view = match *view {
            ViewMode::Tactical => ViewMode::Cockpit,
            ViewMode::Cockpit => ViewMode::Tactical,
        };
        info!("view: {:?}", *view);
    }
}

/// Place the camera each frame. Tactical: an OBLIQUE overview — tilted,
/// not straight down, so spheres shade and rings sweep in perspective.
/// Cockpit: just above and behind the hull, looking along the heading —
/// the orbital plane becomes a horizon.
fn drive_camera(
    view: Res<ViewMode>,
    rig: Res<CameraRig>,
    ships: Query<&SimVel, With<Ship>>,
    mut cameras: Query<&mut Transform, (With<Camera3d>, Without<Ship>)>,
) {
    let Ok(mut cam) = cameras.single_mut() else { return };
    let heading = ships
        .single()
        .ok()
        .filter(|v| v.0.length() > 1.0)
        .map(|v| {
            let n = v.0.normalized();
            Vec3::new(n.x as f32, n.y as f32, n.z as f32).normalize_or(Vec3::Y)
        })
        .unwrap_or(Vec3::Y);
    let target = match *view {
        ViewMode::Tactical => {
            let z = rig.zoom;
            let (sin_p, cos_p) = rig.pitch.sin_cos();
            let eye = Quat::from_rotation_z(rig.yaw) * Vec3::new(0.0, -z * cos_p, z * sin_p);
            Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Z)
        }
        ViewMode::Cockpit => {
            // Full-3D chase: the view pitches with climbs and dives. The
            // up reference flips to Y when the nose points near-vertical.
            let up = if heading.z.abs() > 0.85 { Vec3::Y } else { Vec3::Z };
            let eye = -heading * 26.0 + Vec3::Z * 12.0;
            Transform::from_translation(eye).looking_at(heading * 400.0, up)
        }
    };
    // Critically damped-ish chase: fast enough to track a dogfight,
    // soft enough that mode flips glide instead of teleporting.
    cam.translation = cam.translation.lerp(target.translation, 0.18);
    cam.rotation = cam.rotation.slerp(target.rotation, 0.18);
}

/// Dev-only: G teleports the ship next to the first planet, inside its
/// ring set — the fast path to exercising ring hover/click by hand.
#[cfg(debug_assertions)]
#[allow(clippy::type_complexity)]
fn debug_teleport(
    keys: Res<ButtonInput<KeyCode>>,
    planets: Query<(&CelestialBody, &SimPos, &BodyVel), (With<OnRails>, Without<Ship>)>,
    suns: Query<(&CelestialBody, &SimPos), (With<SunBody>, Without<Ship>, Without<OnRails>)>,
    comets: Query<(&SimPos, &BodyVel), (With<crate::comets::Comet>, Without<Ship>)>,
    mut ships: Query<(&mut SimPos, &mut SimVel), (With<Ship>, Without<OnRails>, Without<SunBody>)>,
) {
    if !keys.just_pressed(KeyCode::KeyG)
        && !keys.just_pressed(KeyCode::KeyH)
        && !keys.just_pressed(KeyCode::KeyJ)
    {
        return;
    }
    let Ok((mut spos, mut svel)) = ships.single_mut() else { return };
    // H: the sun's doorstep (shader inspection); J: alongside a comet
    // (tail inspection — close enough that drifting in strikes you);
    // G: first planet.
    if keys.just_pressed(KeyCode::KeyH) {
        let Ok((sun, sun_pos)) = suns.single() else { return };
        let r = sun.radius * 4.0;
        spos.0 = sun_pos.0 + Vec3d::new(r, 0.0, 0.0);
        let v = (sun.mu / r).sqrt();
        svel.0 = Vec3d::new(0.0, v, 0.0);
        info!("debug teleport beside the sun");
        return;
    }
    if keys.just_pressed(KeyCode::KeyJ) {
        let Some((pos, vel)) = comets.iter().next() else { return };
        spos.0 = pos.0 + Vec3d::new(2.0e8, 0.0, 0.0);
        svel.0 = vel.0;
        info!("debug teleport beside a comet");
        return;
    }
    let Some((body, pos, vel)) = planets.iter().next() else { return };
    spos.0 = pos.0 + Vec3d::new(body.radius * 12.0, 0.0, 0.0);
    svel.0 = vel.0;
    info!("debug teleport beside {}", body.name);
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

/// Manual thrust — keys or the virtual stick — costs energy. Outside an
/// orbit the engines burn UNASSISTED and pay double; while riding an
/// orbit the same inputs only trim the ride (guide_nav's job) and the
/// tank is recharging, so no drain at all.
fn ship_controls(
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    mut ships: Query<(&mut Ship, &crate::command::NavState)>,
) {
    let (mut ship, nav) = match ships.single_mut() {
        Ok(s) => s,
        Err(_) => return,
    };
    if matches!(nav, crate::command::NavState::Orbiting { .. }) {
        return;
    }
    let thrusting = joy.active
        || keys.any_pressed([
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::KeyE,
            KeyCode::KeyQ,
        ]);
    if thrusting {
        ship.energy = (ship.energy - 8.0 * DT * TIME_WARP / 60.0).max(0.0);
        debug!("thrust drain: energy {:.1}", ship.energy);
    }
}

fn integrate_ships(
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    suns: Query<(&SunBody, &SimPos), Without<Ship>>,
    bodies: Query<(&CelestialBody, &SimPos), Without<Ship>>,
    mut ships: Query<(&Ship, &crate::command::NavState, &mut SimPos, &mut SimVel), With<Ship>>,
) {
    let Ok((_sun, sun_pos)) = suns.single() else { return };
    for (ship, nav, mut pos, mut vel) in &mut ships {
        // Every celestial pulls: this is what makes slingshots and planet
        // capture REAL physics rather than scripted moves.
        let mut accel = oj_orbits::Vec3d::ZERO;
        for (body, body_pos) in &bodies {
            accel += gravity_accel(body.mu, body_pos.0, pos.0, body.radius);
        }
        // While riding an orbit the same inputs steer the ride speed
        // (guide_nav), never raw thrust — an orbit is sticky.
        let riding = matches!(nav, crate::command::NavState::Orbiting { .. });
        if ship.energy > 0.0 && !riding {
            // Thrust in the orbital plane: prograde/retrograde on up/down,
            // radial on left/right. Scaled by warp so it stays effective.
            let prograde = vel.0.normalized();
            let radial = (pos.0 - sun_pos.0).normalized();
            // Balance: full warp scaling burned thousands of km/s in
            // seconds (measured 37,000 km/s from a 4 s burn). This
            // factor lands at ~45 km/s per real second at base thrust —
            // decisive against ~25 km/s orbits without deleting them.
            let t = ship.thrust * TIME_WARP * 0.005;
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
            // The virtual joystick speaks the same basis, analog: y is
            // prograde/retrograde, x is radial out/in.
            if joy.active {
                accel += prograde * (t * joy.vec.y as f64) + radial * (t * joy.vec.x as f64);
            }
            // Out-of-plane: E climbs above the ecliptic, Q dives below.
            // Space is a volume, not a board.
            let up = oj_orbits::Vec3d::new(0.0, 0.0, 1.0);
            if keys.pressed(KeyCode::KeyE) {
                accel += up * t;
            }
            if keys.pressed(KeyCode::KeyQ) {
                accel += -up * t;
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
        // No ambient trickle: refueling means riding an orbit or
        // deploying the solar arm (solar.rs). Proximity used to pour
        // energy in for free, which made the whole economy cosmetic.
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
            // Slow recovery in REAL seconds. The old warp-scaled rate
            // (+300/s) refilled the field between enemy volleys, which
            // made shields look indestructible — enemy fire has to be
            // able to grind them down faster than they knit.
            ship.shield = (ship.shield + 0.5 * DT).min(100.0);
        }
    }
}

/// Camera-relative f32 rendering: subtract the anchor's f64 position, cast
/// down, hand bevy an ordinary Transform. Render-space scale compresses the
/// scene so f32 depth stays sane. MESHES ARE AUTHORED IN RENDER UNITS
/// (sim meters / 1e7): only translations pass through this scale, so a
/// mesh authored in meters renders 1e7x too large (found by screenshot —
/// the camera sat inside the ship's own cone).
pub(crate) const RENDER_SCALE: f64 = 1.0 / 1.0e7;

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
