//! Visual effects: explosions, shield flares, impact flashes, engine
//! fire. Hand-rolled particles — each effect is a handful of short-lived
//! emissive meshes with per-entity materials so they can fade and shrink
//! independently. Everything lives in sim space (SimPos) so the
//! camera-relative sync places it like any other world object.

use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::{Shader, ShaderRef};
use oj_orbits::Vec3d;

use crate::sim::SystemScoped;
use crate::{SimPos, SimVel};

/// The living-sun shader: fractal plasma, licking flame shell, corona.
/// One material, three modes — all animated in-shader off `globals.time`
/// so a burning star costs zero CPU per frame.
pub const SUN_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("7be0c5a1-9e2f-4b3d-8f6a-52d90c11ab34");

const SUN_WGSL: &str = r#"
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::{globals, view}

struct SunParams {
    // x: heat 0..1 (class tint), y: mode (0 core, 1 flames, 2 corona),
    // z: seed, w: intensity scale
    v: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: SunParams;

fn hash3(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453123);
}

fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash3(i);
    let b = hash3(i + vec3<f32>(1.0, 0.0, 0.0));
    let c = hash3(i + vec3<f32>(0.0, 1.0, 0.0));
    let d = hash3(i + vec3<f32>(1.0, 1.0, 0.0));
    let e = hash3(i + vec3<f32>(0.0, 0.0, 1.0));
    let f1 = hash3(i + vec3<f32>(1.0, 0.0, 1.0));
    let g = hash3(i + vec3<f32>(0.0, 1.0, 1.0));
    let h = hash3(i + vec3<f32>(1.0, 1.0, 1.0));
    let x1 = mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
    let x2 = mix(mix(e, f1, u.x), mix(g, h, u.x), u.y);
    return mix(x1, x2, u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + a * vnoise(q);
        q = q * 2.13 + vec3<f32>(7.7, 3.1, 1.9);
        a = a * 0.5;
    }
    return v;
}

// Rodrigues rotation of p around a unit axis.
fn rot_axis(p: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return p * c + cross(axis, p) * s + axis * dot(axis, p) * (1.0 - c);
}

struct Storm {
    dir: vec3<f32>,
    eye: f32,
}

// Storm cells: seeded vortices wander the photosphere, each twisting
// the plasma around itself. The twist winds up and unwinds (bounded, so
// hours of play never shear the noise into mush) while the cells drift,
// dragging spiral arms across the surface.
fn storm_warp(dir: vec3<f32>, seed: f32, t: f32) -> Storm {
    var d = dir;
    var eye = 0.0;
    for (var i = 0; i < 3; i = i + 1) {
        let fi = f32(i);
        let a = seed * 37.0 + fi * 2.39996;
        let b = seed * 61.0 + fi * 1.71;
        var center = normalize(vec3<f32>(sin(a) * cos(b), sin(b), cos(a) * cos(b)));
        center = normalize(center + 0.35 * vec3<f32>(
            sin(t * 0.043 + fi * 2.1),
            cos(t * 0.037 + fi * 1.3),
            sin(t * 0.029 + fi * 4.2),
        ));
        let fall = exp(-pow(acos(clamp(dot(d, center), -1.0, 1.0)) * 2.6, 2.0));
        let hand = select(1.0, -1.0, (i & 1) == 1);
        let angle = fall * hand * 2.6 * sin(t * (0.21 + 0.07 * fi) + fi * 2.0);
        d = rot_axis(d, center, angle);
        eye = eye + fall;
    }
    return Storm(d, min(eye, 1.0));
}

// Class palette: deep M-red through G-gold to O-blue-white.
fn heat_color(heat: f32, t: f32) -> vec3<f32> {
    let cool = mix(vec3<f32>(0.7, 0.12, 0.02), vec3<f32>(1.0, 0.45, 0.08), t);
    let mid  = mix(vec3<f32>(1.0, 0.5, 0.1),  vec3<f32>(1.0, 0.92, 0.55), t);
    let hot  = mix(vec3<f32>(0.55, 0.7, 1.0), vec3<f32>(0.92, 0.97, 1.0), t);
    if heat < 0.5 {
        return mix(cool, mid, heat * 2.0);
    }
    return mix(mid, hot, (heat - 0.5) * 2.0);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let heat = params.v.x;
    let mode = params.v.y;
    let seed = params.v.z;
    let boost = params.v.w;
    let t = globals.time;
    let n_dir = normalize(in.world_normal);
    let v_dir = normalize(view.world_position - in.world_position.xyz);
    let facing = clamp(dot(n_dir, v_dir), 0.0, 1.0);
    let rim = 1.0 - facing;

    // Plasma coordinates: the unit direction, storm-swirled, drifting
    // and churning. The warp feeds every mode, so flame tongues spiral
    // with the same storms the surface shows.
    let storm = storm_warp(n_dir, seed, t);
    let p = storm.dir * 3.2 + vec3<f32>(seed * 17.0);
    let churn = fbm(p + vec3<f32>(t * 0.11, -t * 0.07, t * 0.05));
    let cells = fbm(p * 2.7 - vec3<f32>(t * 0.05, t * 0.09, -t * 0.04));
    let plasma = churn * 0.65 + cells * 0.35;

    if mode < 0.5 {
        // CORE: banded plasma surface, granulation-darkened, edge-dimmed
        // like a real photosphere.
        let band = smoothstep(0.25, 0.85, plasma);
        // Storm eyes run hotter and brighter than the quiet surface.
        var col = heat_color(heat, min(band + 0.25 * storm.eye, 1.0));
        let granule = smoothstep(0.35, 0.0, abs(cells - 0.5)) * 0.35;
        col = col * (1.0 - granule);
        let limb = mix(1.0, 0.55, rim * rim);
        let glow = (2.5 + 9.0 * band + 4.0 * storm.eye) * boost * limb;
        return vec4<f32>(col * glow, 1.0);
    } else if mode < 1.5 {
        // FLAMES: noise eroded by the silhouette rim — tongues that lick
        // outward and churn. Additive, so overlap builds white heat.
        let tongue = fbm(p * 1.7 + vec3<f32>(-t * 0.23, t * 0.17, t * 0.13));
        let ring = pow(rim, 1.6);
        let flame = smoothstep(0.45, 0.8, tongue * (0.35 + ring));
        let col = heat_color(heat, 0.35 + 0.5 * flame);
        let a = flame * (0.35 + 0.65 * ring);
        return vec4<f32>(col * (3.5 * boost) * a, a * 0.85);
    }
    // CORONA: pure fresnel halo with a slow breathing pulse.
    let pulse = 0.8 + 0.2 * sin(t * 0.7 + seed * 6.28);
    let halo = pow(rim, 2.6) * pulse;
    let col = heat_color(heat, 0.75);
    return vec4<f32>(col * (1.6 * boost) * halo, halo * 0.5);
}
"#;

/// The nebula backdrop shader: domain-warped fbm clouds in two seeded
/// hues drifting almost imperceptibly. Evaluated per fragment on two
/// huge sky quads — zero CPU per frame, and every system's seed paints
/// a different sky.
pub const NEBULA_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("3f8a1c2e-6b4d-4e9a-9c7f-0d5e88a21b57");

const NEBULA_WGSL: &str = r#"
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::globals

struct NebulaParams {
    // x: seed, y: palette shift 0..1, z: brightness, w: cloud scale
    v: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: NebulaParams;

fn hash3(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453123);
}

fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash3(i);
    let b = hash3(i + vec3<f32>(1.0, 0.0, 0.0));
    let c = hash3(i + vec3<f32>(0.0, 1.0, 0.0));
    let d = hash3(i + vec3<f32>(1.0, 1.0, 0.0));
    let e = hash3(i + vec3<f32>(0.0, 0.0, 1.0));
    let f1 = hash3(i + vec3<f32>(1.0, 0.0, 1.0));
    let g = hash3(i + vec3<f32>(0.0, 1.0, 1.0));
    let h = hash3(i + vec3<f32>(1.0, 1.0, 1.0));
    let x1 = mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
    let x2 = mix(mix(e, f1, u.x), mix(g, h, u.x), u.y);
    return mix(x1, x2, u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 5; i = i + 1) {
        v = v + a * vnoise(q);
        q = q * 2.17 + vec3<f32>(5.2, 1.3, 8.7);
        a = a * 0.5;
    }
    return v;
}

// IQ cosine palette: seeded, always a plausible deep-space pair.
fn palette(t: f32, shift: f32) -> vec3<f32> {
    return vec3<f32>(0.5) + vec3<f32>(0.5) * cos(6.28318 * (vec3<f32>(1.0, 0.7, 0.4) * t
        + vec3<f32>(0.0, 0.15, 0.20) + shift));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let seed = params.v.x;
    let shift = params.v.y;
    let bright = params.v.z;
    let scale = params.v.w;
    // Barely-drifting domain-warped clouds; the sky breathes over
    // minutes, not seconds.
    let t = globals.time * 0.004;
    let p = vec3<f32>(in.uv * scale, seed * 17.31);
    let q = fbm(p + vec3<f32>(t, -t * 0.7, 0.0));
    let r = fbm(p + 2.6 * vec3<f32>(q, q * 0.8, 0.1) + vec3<f32>(1.7, 9.2, 3.3));
    let d = fbm(p + 3.2 * vec3<f32>(r, r, 0.2));
    // Sparse clouds with real dark gaps between them.
    let dens = smoothstep(0.42, 0.85, d);
    let wisp = smoothstep(0.62, 0.92, r) * 0.6;
    let col_a = palette(0.15 + q * 0.35, shift);
    let col_b = palette(0.55 + r * 0.3, shift + 0.33);
    let col = (col_a * dens + col_b * wisp) * bright;
    let alpha = clamp((dens * 0.42 + wisp * 0.25) * bright, 0.0, 0.6);
    return vec4<f32>(col * 0.35, alpha);
}
"#;

/// Uniform block for [`NEBULA_SHADER`].
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct NebulaMaterial {
    #[uniform(0)]
    pub params: Vec4,
}

impl NebulaMaterial {
    pub fn new(seed: f32, shift: f32, brightness: f32, scale: f32) -> Self {
        Self { params: Vec4::new(seed, shift, brightness, scale) }
    }
}

impl Material for NebulaMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(NEBULA_SHADER)
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Uniform block for [`SUN_SHADER`]; see `SunParams` in the WGSL.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct SunMaterial {
    #[uniform(0)]
    pub params: Vec4,
}

impl SunMaterial {
    pub fn new(heat: f32, mode: f32, seed: f32, boost: f32) -> Self {
        Self { params: Vec4::new(heat, mode, seed, boost) }
    }
}

impl Material for SunMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SUN_SHADER)
    }
    fn alpha_mode(&self) -> AlphaMode {
        // Core is opaque; flame/corona shells are additive.
        if self.params.y < 0.5 { AlphaMode::Opaque } else { AlphaMode::Add }
    }
}

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
        let _ = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(&SUN_SHADER, Shader::from_wgsl(SUN_WGSL, "oj_sun.wgsl"));
        let _ = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(&NEBULA_SHADER, Shader::from_wgsl(NEBULA_WGSL, "oj_nebula.wgsl"));
        app.add_plugins(MaterialPlugin::<SunMaterial>::default())
            .add_plugins(MaterialPlugin::<NebulaMaterial>::default())
            .add_systems(Update, (tick_particles, flicker_flames));
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
