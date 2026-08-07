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
            .add_systems(
                Update,
                (sync_render_transforms, orient_ship, flame_visibility, camera_zoom),
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

fn spawn_current_system(
    mut commands: Commands,
    game: Res<GameUniverse>,
    game_style: Res<ShipStyle>,
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
        Transform::from_xyz(0.0, 0.0, 600.0).looking_at(Vec3::ZERO, Vec3::Y),
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
    for _ in 0..520 {
        let x = sky_rng.range(-4200.0, 4200.0) as f32;
        let y = sky_rng.range(-4200.0, 4200.0) as f32;
        let z = sky_rng.range(-2400.0, -1200.0) as f32;
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
pub fn spawn_bodies(
    commands: &mut Commands,
    system: &SolarSystem,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
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
    commands.insert_resource(OrbitRingMaterials {
        dim: ring_dim.clone(),
        bright: ring_bright,
    });

    // Sun at the origin of the system-local frame.
    let sun_radius = system.sun.radius.max(2.0e9);
    let sun_entity = commands.spawn((
        SystemScoped,
        SunBody {
            class: system.sun.class,
            hazard_radius: system.sun.hazard_radius,
        },
        CelestialBody {
            mu,
            radius: sun_radius,
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
    )).id();
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
                CelestialBody {
                    mu: oj_orbits::G * planet.mass,
                    radius: planet_radius,
                    soi: planet_soi,
                    name: format!("Planet {}", i + 1),
                },
                SimPos::default(),
                BodyVel::default(),
                Mesh3d(meshes.add(
                    Sphere::new((planet_radius * RENDER_SCALE) as f32).mesh().ico(3).unwrap(),
                )),
                MeshMaterial3d({
                    let (pr, pg, pb) =
                        PLANET_PALETTE[(system.id.index as usize + i) % PLANET_PALETTE.len()];
                    materials.add(StandardMaterial {
                        base_color: Color::srgb(pr, pg, pb),
                        perceptual_roughness: 0.9,
                        ..default()
                    })
                }),
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
                CelestialBody {
                    mu: oj_orbits::G * moon.mass,
                    radius: moon_radius,
                    soi: moon_soi,
                    name: format!("Planet {} moon {}", i + 1, (b'a' + m as u8) as char),
                },
                SimPos::default(),
                BodyVel::default(),
                Mesh3d(meshes.add(
                    Sphere::new((moon_radius * RENDER_SCALE) as f32).mesh().ico(2).unwrap(),
                )),
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
    mut rings: Query<&mut MeshMaterial3d<StandardMaterial>, With<OrbitRing>>,
) {
    if let Ok(mut m) = rings.get_mut(over.entity) {
        m.0 = mats.bright.clone();
    }
}

fn ring_hover_off(
    out: On<bevy::picking::events::Pointer<bevy::picking::events::Out>>,
    mats: Res<OrbitRingMaterials>,
    mut rings: Query<&mut MeshMaterial3d<StandardMaterial>, With<OrbitRing>>,
) {
    if let Ok(mut m) = rings.get_mut(out.entity) {
        m.0 = mats.dim.clone();
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
    let hull_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(hull_rgb.0, hull_rgb.1, hull_rgb.2),
        metallic: 0.3,
        perceptual_roughness: 0.4,
        emissive: LinearRgba::rgb(hull_glow.0, hull_glow.1, hull_glow.2),
        ..default()
    });
    let wing_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(acc_rgb.0, acc_rgb.1, acc_rgb.2),
        metallic: 0.4,
        emissive: LinearRgba::rgb(acc_glow.0, acc_glow.1, acc_glow.2),
        ..default()
    });
    // Frame silhouettes: hull mesh + wing span vary per frame.
    let (hull_mesh, wing_size, nozzle_y) = match style.frame % SHIP_FRAMES.len() {
        // DART: the classic needle.
        0 => (meshes.add(Cone::new(5.0, 18.0).mesh().resolution(16)), (16.0, 5.0), -10.0),
        // LANCE: longer, slimmer, narrow wings.
        1 => (meshes.add(Cone::new(3.6, 26.0).mesh().resolution(16)), (11.0, 7.0), -14.0),
        // HAMMER: broad wedge with wide wings.
        _ => (meshes.add(Cone::new(8.0, 14.0).mesh().resolution(6)), (22.0, 6.0), -8.0),
    };
    let flame_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.6, 0.15),
        emissive: LinearRgba::rgb(14.0, 5.0, 0.8),
        unlit: true,
        ..default()
    });
    commands
        .spawn((
            Ship::default(),
            crate::command::NavState::Free,
            OriginAnchor,
            SimPos(pos),
            SimVel(vel),
            Mesh3d(hull_mesh),
            MeshMaterial3d(hull_mat.clone()),
            Transform::default(),
        ))
        .with_children(|ship| {
            // Swept wings.
            ship.spawn((
                Mesh3d(meshes.add(Cuboid::new(wing_size.0, wing_size.1, 1.6).mesh())),
                MeshMaterial3d(wing_mat.clone()),
                Transform::from_xyz(0.0, -6.0, 0.0),
            ));
            // Engine nozzle block.
            ship.spawn((
                Mesh3d(meshes.add(Cuboid::new(6.0, 4.0, 3.0).mesh())),
                MeshMaterial3d(wing_mat),
                Transform::from_xyz(0.0, nozzle_y, 0.0),
            ));
            // Exhaust flame, pointing aft; bloom does the glow.
            ship.spawn((
                EngineFlame,
                Mesh3d(meshes.add(Cone::new(3.2, 12.0).mesh().resolution(10))),
                MeshMaterial3d(flame_mat),
                Transform::from_xyz(0.0, nozzle_y - 7.0, 0.0)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::PI)),
                Visibility::Hidden,
            ));
        });
}

/// Rebuild the ship's visuals in the current style, preserving flight
/// state and stats — the yard repaints, it does not recommission.
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

/// Point the hull along the velocity vector (the cone's +Y is forward).
fn orient_ship(mut ships: Query<(&SimVel, &mut Transform), With<Ship>>) {
    for (vel, mut transform) in &mut ships {
        if vel.0.length() > 1.0 {
            let angle = (vel.0.y).atan2(vel.0.x) as f32 - std::f32::consts::FRAC_PI_2;
            transform.rotation = Quat::from_rotation_z(angle);
        }
    }
}

/// Show the exhaust while burning: manual thrust or a guided transfer.
fn flame_visibility(
    keys: Res<ButtonInput<KeyCode>>,
    ships: Query<(&Ship, &crate::command::NavState)>,
    mut flames: Query<&mut Visibility, With<EngineFlame>>,
) {
    let Ok((ship, nav)) = ships.single() else { return };
    let thrusting = keys.any_pressed([
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
    ]);
    let burning = ship.energy > 0.0
        && (thrusting || matches!(nav, crate::command::NavState::Transfer { .. }));
    for mut vis in &mut flames {
        *vis = if burning { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Mouse-wheel camera zoom, clamped so the HUD scale stays sane.
fn camera_zoom(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    let scroll: f32 = wheel.read().map(|w| w.y).sum();
    if scroll == 0.0 {
        return;
    }
    for mut transform in &mut cameras {
        let z = (transform.translation.z * (1.0 - scroll * 0.12)).clamp(160.0, 4000.0);
        transform.translation.z = z;
    }
}

/// Dev-only: G teleports the ship next to the first planet, inside its
/// ring set — the fast path to exercising ring hover/click by hand.
#[cfg(debug_assertions)]
#[allow(clippy::type_complexity)]
fn debug_teleport(
    keys: Res<ButtonInput<KeyCode>>,
    planets: Query<(&CelestialBody, &SimPos, &BodyVel), (With<OnRails>, Without<Ship>)>,
    mut ships: Query<(&mut SimPos, &mut SimVel), (With<Ship>, Without<OnRails>)>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    let (Some((body, pos, vel)), Ok((mut spos, mut svel))) =
        (planets.iter().next(), ships.single_mut())
    else {
        return;
    };
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
