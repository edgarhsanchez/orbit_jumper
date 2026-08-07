//! Comets: sun-grazing wanderers on fierce ellipses. Near the sun they
//! outgas — a glowing anti-sunward tail plus the occasional chunk of
//! collectible ice left drifting along the path — and a ship that
//! crosses one takes a real hit: shields first, then hull, with the
//! reactor paying for the field and the impact shoving the ship off its
//! line. Spawned per system in `spawn_bodies`; driven here.

use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::modules::Wreck;
use crate::sim::{BodyVel, DT, Ship, SunBody, SystemScoped};
use crate::{SimPos, SimVel};

/// How close counts as "passing through" the coma, sim meters.
const STRIKE_RANGE: f64 = 2.5e8;
/// Raw impact damage; the shield absorbs first, like bolt fire.
const STRIKE_DAMAGE: f64 = 30.0;
/// Real seconds between hits from the same comet — one pass, one hit,
/// not a shredder while the volumes overlap.
const STRIKE_COOLDOWN: f64 = 3.0;
/// Motes shed between persistent ice chunks.
const MOTES_PER_CHUNK: u32 = 18;

#[derive(Component, Default)]
pub struct Comet {
    /// Outgassing accumulator: fractional motes owed to the tail.
    shed: f64,
    /// Ticks down after a strike.
    cooldown: f64,
    /// Motes shed since the last chunk dropped.
    since_chunk: u32,
}

/// A shed chunk: collectible ice for a while, then gone.
#[derive(Component)]
pub struct CometChunk {
    pub ttl: f64,
}

pub struct CometPlugin;

impl Plugin for CometPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (shed_tails, comet_strikes, expire_chunks));
    }
}

/// Split a raw hit into (absorbed-by-shield, through-to-hull).
fn split_damage(raw: f64, shield: f64) -> (f64, f64) {
    let absorbed = raw.min(shield);
    (absorbed, raw - absorbed)
}

/// Outgassing: the rate climbs as the comet dives sunward. Motes blow
/// anti-sunward — a comet's tail points away from the sun, not behind
/// the nucleus — and every so often a chunk of ice drops and stays.
fn shed_tails(
    mut comets: Query<(&mut Comet, &SimPos, &BodyVel)>,
    suns: Query<&SimPos, (With<SunBody>, Without<Comet>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(sun_pos) = suns.single() else { return };
    for (mut comet, pos, vel) in &mut comets {
        let to_sun = sun_pos.0 - pos.0;
        let r = to_sun.length().max(1.0);
        let anti_sun = -(to_sun / r);
        let rate = (14.0 * (3.0e10 / r)).clamp(2.0, 16.0);
        comet.shed += rate * DT;
        let mut rng = oj_universe::SplitMix64(
            pos.0.x.to_bits() ^ pos.0.y.to_bits().rotate_left(21),
        );
        while comet.shed >= 1.0 {
            comet.shed -= 1.0;
            let jitter = Vec3d::new(
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-0.3, 0.3),
            ) * 8.0e4;
            let blow = anti_sun * rng.range(1.5e5, 5.0e5) + jitter;
            let ttl = rng.range(2.0, 4.0);
            let s = rng.range(2.0, 4.5) as f32;
            let glow = LinearRgba::rgb(1.6, 3.0, 4.4);
            commands.spawn((
                SystemScoped,
                crate::fx::FxParticle { ttl, ttl_max: ttl, base_scale: s, glow },
                SimPos(pos.0),
                SimVel(vel.0 + blow),
                Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(1).unwrap())),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgba(0.7, 0.88, 1.0, 0.5),
                    emissive: glow,
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })),
                Transform::from_scale(Vec3::splat(s)),
                bevy::picking::Pickable::IGNORE,
            ));
            comet.since_chunk += 1;
            if comet.since_chunk >= MOTES_PER_CHUNK {
                comet.since_chunk = 0;
                commands.spawn((
                    SystemScoped,
                    Wreck { value: 8, element: oj_materials::Element::Ice },
                    CometChunk { ttl: 60.0 },
                    SimPos(pos.0),
                    Mesh3d(meshes.add(Cuboid::new(2.5, 2.0, 2.2).mesh())),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.75, 0.85, 0.95),
                        perceptual_roughness: 0.4,
                        ..default()
                    })),
                    Transform::default(),
                ));
            }
        }
    }
}

/// Passing through a comet hurts: the field takes what it can (costing
/// the reactor), the rest burns to the hull, and the snowball shoves
/// the ship off its line.
#[allow(clippy::type_complexity)]
fn comet_strikes(
    mut comets: Query<(&mut Comet, &SimPos, &BodyVel), Without<Ship>>,
    mut ships: Query<
        (&mut Ship, &SimPos, &mut SimVel),
        (With<crate::sim::OriginAnchor>, Without<Comet>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let Ok((mut ship, ship_pos, mut ship_vel)) = ships.single_mut() else {
        for (mut comet, _, _) in &mut comets {
            comet.cooldown = (comet.cooldown - DT).max(0.0);
        }
        return;
    };
    for (mut comet, pos, vel) in &mut comets {
        comet.cooldown = (comet.cooldown - DT).max(0.0);
        let d = ship_pos.0.distance(pos.0);
        if comet.cooldown > 0.0 || d > STRIKE_RANGE {
            continue;
        }
        comet.cooldown = STRIKE_COOLDOWN;
        let strike_dir = if d > 1.0 {
            (pos.0 - ship_pos.0) / d
        } else {
            Vec3d::new(1.0, 0.0, 0.0)
        };
        let (absorbed, through) = split_damage(STRIKE_DAMAGE, ship.shield);
        sfx.write(crate::audio::Sfx::Explosion);
        if absorbed > 0.0 {
            ship.shield -= absorbed;
            ship.energy = (ship.energy - absorbed * 0.6).max(0.0);
            sfx.write(crate::audio::Sfx::ShieldHit);
            crate::fx::spawn_shield_flare(
                &mut commands,
                &mut meshes,
                &mut materials,
                ship_pos.0,
                ship_vel.0,
                strike_dir,
                18.0,
            );
        }
        if through > 0.0 {
            ship.hull = (ship.hull - through).max(0.0);
            sfx.write(crate::audio::Sfx::HullHit);
        }
        crate::fx::spawn_explosion(
            &mut commands,
            &mut meshes,
            &mut materials,
            ship_pos.0 + strike_dir * (12.0 / crate::sim::RENDER_SCALE),
            vel.0,
            12.0,
        );
        ship_vel.0 += strike_dir * -1.8e4;
        info!(
            "comet strike: shield {:.0}, hull {:.0}, energy {:.0}",
            ship.shield, ship.hull, ship.energy
        );
    }
}

fn expire_chunks(mut chunks: Query<(Entity, &mut CometChunk)>, mut commands: Commands) {
    for (entity, mut chunk) in &mut chunks {
        chunk.ttl -= DT;
        if chunk.ttl <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strike order the whole game uses: shields soak first, the
    /// remainder burns through to the hull.
    #[test]
    fn comet_damage_splits_shield_first() {
        assert_eq!(split_damage(30.0, 100.0), (30.0, 0.0));
        assert_eq!(split_damage(30.0, 12.0), (12.0, 18.0));
        assert_eq!(split_damage(30.0, 0.0), (0.0, 30.0));
    }
}
