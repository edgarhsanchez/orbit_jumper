//! Visual effects: explosions, shield flares, impact flashes, engine
//! fire. Hand-rolled particles — each effect is a handful of short-lived
//! emissive meshes with per-entity materials so they can fade and shrink
//! independently. Everything lives in sim space (SimPos) so the
//! camera-relative sync places it like any other world object.

use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::sim::SystemScoped;
use crate::{SimPos, SimVel};

/// A piece of an effect: fades, shrinks and dies.
#[derive(Component)]
pub struct FxParticle {
    pub ttl: f64,
    pub ttl_max: f64,
    /// Uniform scale at birth; shrinks toward zero across the lifetime.
    pub base_scale: f32,
    /// Emissive color at birth; fades with the particle.
    pub glow: LinearRgba,
}

/// Engine exhaust layers flicker: scale and glow jitter per frame.
#[derive(Component)]
pub struct FlameFlicker {
    /// Phase offset so multiple layers don't pulse in sync.
    pub phase: f32,
    pub base_scale: Vec3,
}

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (tick_particles, flicker_flames));
    }
}

/// Spawn a fireball: a bright core flash, a ring of hot debris, and
/// lingering embers. `scale` ~ the victim's visual size in render units.
pub fn spawn_explosion(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3d,
    vel: Vec3d,
    scale: f32,
) {
    let mut rng = oj_universe::SplitMix64(
        (pos.x.to_bits() ^ pos.y.to_bits()).wrapping_mul(0x9E3779B97F4A7C15),
    );
    // Core flash: one big soft sphere, gone fast.
    let flash_glow = LinearRgba::rgb(8.0, 5.0, 2.2);
    commands.spawn((
        SystemScoped,
        FxParticle { ttl: 0.22, ttl_max: 0.22, base_scale: scale * 2.4, glow: flash_glow },
        SimPos(pos),
        SimVel(vel),
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(2).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.85, 0.5, 0.9),
            emissive: flash_glow,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        Transform::from_scale(Vec3::splat(scale * 2.4)),
        bevy::picking::Pickable::IGNORE,
    ));
    // Hot debris: jagged chunks thrown outward, tumbling glow.
    for _ in 0..10 {
        let dir = Vec3d::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-0.4, 0.4));
        let dir = if dir.length() < 1e-3 { Vec3d::new(1.0, 0.0, 0.0) } else { dir.normalized() };
        let speed = rng.range(1.2e8, 3.2e8);
        let hot = rng.range(0.0, 1.0) > 0.5;
        let glow = if hot {
            LinearRgba::rgb(6.0, 2.2, 0.5)
        } else {
            LinearRgba::rgb(3.0, 0.8, 0.25)
        };
        let ttl = rng.range(0.5, 1.1);
        let s = scale * rng.range(0.10, 0.28) as f32;
        commands.spawn((
            SystemScoped,
            FxParticle { ttl, ttl_max: ttl, base_scale: s, glow },
            SimPos(pos),
            SimVel(vel + dir * speed),
            Mesh3d(meshes.add(Cuboid::new(1.0, 1.4, 1.0).mesh())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgba(1.0, 0.6, 0.3, 0.85),
                emissive: glow,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })),
            Transform::from_scale(Vec3::splat(s)),
            bevy::picking::Pickable::IGNORE,
        ));
    }
    // Smoke/ember puffs: slower, dimmer, longest lived.
    for _ in 0..5 {
        let dir = Vec3d::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.0);
        let dir = if dir.length() < 1e-3 { Vec3d::new(0.0, 1.0, 0.0) } else { dir.normalized() };
        let glow = LinearRgba::rgb(0.9, 0.35, 0.12);
        let ttl = rng.range(1.0, 1.7);
        let s = scale * rng.range(0.3, 0.55) as f32;
        commands.spawn((
            SystemScoped,
            FxParticle { ttl, ttl_max: ttl, base_scale: s, glow },
            SimPos(pos),
            SimVel(vel + dir * rng.range(3.0e7, 9.0e7)),
            Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(1).unwrap())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgba(0.5, 0.3, 0.2, 0.5),
                emissive: glow,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })),
            Transform::from_scale(Vec3::splat(s)),
            bevy::picking::Pickable::IGNORE,
        ));
    }
}

/// A directional shield flare: an emissive disc at the shield boundary on
/// the struck side, facing the attacker — "the force field lit up where
/// it was hit".
pub fn spawn_shield_flare(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    ship_pos: Vec3d,
    ship_vel: Vec3d,
    strike_dir: Vec3d,
    radius: f32,
) {
    let dir = strike_dir.normalized();
    let offset = dir * (radius as f64 / crate::sim::RENDER_SCALE);
    let dir3 = Vec3::new(dir.x as f32, dir.y as f32, dir.z as f32);
    let glow = LinearRgba::rgb(0.6, 3.2, 4.5);
    commands.spawn((
        SystemScoped,
        FxParticle { ttl: 0.35, ttl_max: 0.35, base_scale: radius, glow },
        SimPos(ship_pos + offset),
        SimVel(ship_vel),
        Mesh3d(meshes.add(Circle::new(1.0).mesh().resolution(24))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.35, 0.8, 1.0, 0.55),
            emissive: glow,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            ..default()
        })),
        Transform {
            // The disc faces its +Z; aim that at the attacker.
            rotation: Quat::from_rotation_arc(Vec3::Z, dir3),
            scale: Vec3::splat(radius),
            ..default()
        },
        bevy::picking::Pickable::IGNORE,
    ));
}

/// A small impact flash — laser hits, bolt strikes on bare hull.
pub fn spawn_impact_flash(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3d,
    vel: Vec3d,
    scale: f32,
    glow: LinearRgba,
) {
    commands.spawn((
        SystemScoped,
        FxParticle { ttl: 0.16, ttl_max: 0.16, base_scale: scale, glow },
        SimPos(pos),
        SimVel(vel),
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(1).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.9, 0.7, 0.9),
            emissive: glow,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        Transform::from_scale(Vec3::splat(scale)),
        bevy::picking::Pickable::IGNORE,
    ));
}

/// Advance every particle: drift, expand-then-shrink, fade to nothing.
fn tick_particles(
    time: Res<Time>,
    mut particles: Query<(
        Entity,
        &mut FxParticle,
        &mut SimPos,
        &SimVel,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs_f64();
    for (entity, mut p, mut pos, vel, mut transform, mat) in &mut particles {
        p.ttl -= dt;
        if p.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        pos.0 += vel.0 * dt * crate::sim::TIME_WARP;
        let life = (p.ttl / p.ttl_max) as f32; // 1 -> 0
        // Quick swell at birth, then decay.
        let swell = 1.0 + 0.6 * (1.0 - life).min(0.25) * 4.0;
        transform.scale = Vec3::splat(p.base_scale * swell * life.max(0.05));
        if let Some(mut m) = materials.get_mut(&mat.0) {
            m.emissive = p.glow * life;
            let a = m.base_color.alpha();
            m.base_color = m.base_color.with_alpha(a.min(life));
        }
    }
}

/// Engine fire is alive: layers pulse and jitter out of phase.
fn flicker_flames(
    time: Res<Time>,
    mut flames: Query<(&FlameFlicker, &mut Transform)>,
) {
    let t = time.elapsed_secs();
    for (f, mut transform) in &mut flames {
        let n = (t * 31.0 + f.phase).sin() * 0.5 + (t * 47.0 + f.phase * 2.3).sin() * 0.3
            + (t * 13.0 + f.phase * 0.7).cos() * 0.2;
        let len = 1.0 + 0.28 * n;
        let width = 1.0 + 0.10 * ((t * 23.0 + f.phase).cos());
        transform.scale = Vec3::new(
            f.base_scale.x * width,
            f.base_scale.y * len,
            f.base_scale.z * width,
        );
    }
}
