//! The shield made visible — and made a weapon.
//!
//! Every ship wears its force field as a BUBBLE whose glow tracks the
//! shield points behind it: full shield is an unmistakable aura, a
//! drained one barely shimmers. And once the Shield slot is crafted
//! (tier 1+), `N` turns defense into offense: the NOVA dumps the whole
//! shield into an expanding wave that consumes energy while it grows.
//! Hostile shields soak the punch until they are consumed — how hard
//! the wave burns through them is the shield weapon skill (the Shield
//! slot tier). Celestial objects carry LEVELS: a wave from a strong
//! enough pilot (rating = 2x shield tier + pilot level) shatters any
//! body of lower level — moons, planets, even the sun — and the wreck
//! pays out: suns burst into harvestable energy (more than any planet
//! of the same level), planets into debris that scales with their
//! level.

use bevy::light::NotShadowCaster;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use oj_orbits::Vec3d;
use oj_universe::SunClass;

use crate::modules::{RunScore, Tumble, Wreck, debris_material};
use crate::sim::{
    CelestialBody, DT, OnRailsAround, RENDER_SCALE, Ship, SunBody, SystemScoped,
};
use crate::upgrades::{ShipUpgrades, pilot_level};
use crate::weapons::Hull;
use crate::{SimPos, SimVel};
use oj_materials::{Element, UpgradeSlot};

/// Every destructible-by-nova object carries a level; the wave shatters
/// anything of LOWER level than the pilot's nova rating.
#[derive(Component)]
pub struct ObjectLevel(pub u32);

/// A hostile's nova screen: soaks wave punch until consumed. Only the
/// nova cares — bolts and lasers burn hull as they always did.
#[derive(Component)]
pub struct NovaShield(pub f64);

/// The visible force field: glow follows shield points.
#[derive(Component)]
pub struct ShieldBubble;

/// The expanding wave. Sim-space radius; visual entity tracks it.
#[derive(Component)]
pub struct NovaWave {
    radius: f64,
    max_radius: f64,
    /// m of sim radius per real second.
    speed: f64,
    /// Shield-burn per object; the shield weapon skill made concrete.
    punch: f64,
    /// Destroys celestials below this level.
    rating: u32,
    hit: HashSet<Entity>,
}

const NOVA_COOLDOWN: f64 = 12.0;
const NOVA_FIRE_COST: f64 = 30.0;
/// Energy drained per real second while the wave expands.
const NOVA_DRAIN: f64 = 12.0;

#[derive(Resource, Default)]
struct NovaCd(f64);

/// The pilot's destruction rating: shield weapon skill plus experience.
pub fn nova_rating(shield_tier: u8, pilot: u32) -> u32 {
    shield_tier as u32 * 2 + pilot
}

/// Wave punch: scales with the Shield tier (the skill) and with how
/// much shield was banked when it fired (the ammo).
pub fn nova_punch(shield_tier: u8, shield_points: f64) -> f64 {
    (15.0 + 15.0 * shield_tier as f64) * (0.5 + shield_points / 100.0)
}

/// A sun's level by class: even the gentlest star outranks any planet.
pub fn sun_level(class: SunClass) -> u32 {
    match class {
        SunClass::M => 16,
        SunClass::K => 20,
        SunClass::G => 24,
        SunClass::F => 28,
        SunClass::A => 32,
        SunClass::B => 38,
        SunClass::O => 44,
        SunClass::NeutronStar => 52,
        SunClass::Magnetar => 60,
        SunClass::BlackHole => 72,
    }
}

/// A planet's level grows with its size; moons ride the same curve.
pub fn body_level(radius: f64) -> u32 {
    (2.0 + (radius / 3.0e8).sqrt() * 3.0).min(14.0) as u32
}

/// Energy released by a destroyed body. A sun at any level out-pays a
/// planet at the same level — that is what a fusion furnace is.
pub fn destruction_energy(level: u32, is_sun: bool) -> f64 {
    if is_sun { 30.0 + 5.0 * level as f64 } else { 6.0 + 1.5 * level as f64 }
}

/// Debris pieces scattered by a destroyed body: strictly more per level.
pub fn destruction_debris(level: u32) -> u32 {
    2 + level * 2
}

pub struct NovaPlugin;

impl Plugin for NovaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NovaCd>()
            .add_systems(FixedUpdate, (fire_nova, expand_novas).chain())
            .add_systems(Update, (glow_bubbles, dev_shield));
    }
}

/// The bubble's look follows the shield behind it: alpha and emissive
/// climb with shield points, and a slow breath keeps it alive.
fn glow_bubbles(
    time: Res<Time>,
    view: Res<crate::sim::ViewMode>,
    ships: Query<&Ship>,
    bubbles: Query<(&ChildOf, &MeshMaterial3d<StandardMaterial>, Entity), With<ShieldBubble>>,
    mut transforms: Query<&mut Transform, With<ShieldBubble>>,
    mut visibility: Query<&mut Visibility, With<ShieldBubble>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (parent, mat, entity) in &bubbles {
        // In cockpit view the camera sits INSIDE the bubble — a
        // screen-filling dome, not a force field. First person hides it.
        if let Ok(mut vis) = visibility.get_mut(entity) {
            *vis = if *view == crate::sim::ViewMode::Cockpit {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        }
        if *view == crate::sim::ViewMode::Cockpit {
            continue;
        }
        let Ok(ship) = ships.get(parent.parent()) else { continue };
        let charge = (ship.shield / 100.0).clamp(0.0, 1.0);
        if let Some(mut m) = materials.get_mut(&mat.0) {
            m.base_color = Color::srgba(0.35, 0.85, 1.0, 0.028 + 0.16 * charge as f32);
            let glow = 0.02 + 0.75 * charge as f32;
            m.emissive = LinearRgba::rgb(glow * 0.35, glow * 0.8, glow);
        }
        if let Ok(mut tf) = transforms.get_mut(entity) {
            let breath = 1.0 + 0.035 * (time.elapsed_secs() * 1.7).sin() * charge as f32;
            tf.scale = Vec3::splat(breath);
        }
    }
}

/// `N`: dump the shield into a nova. Needs the Shield slot crafted
/// (tier 1+) — that tier IS the shield weapon skill.
#[allow(clippy::too_many_arguments)]
fn fire_nova(
    keys: Res<ButtonInput<KeyCode>>,
    mut cd: ResMut<NovaCd>,
    upgrades: Res<ShipUpgrades>,
    run: Res<RunScore>,
    mut ships: Query<(&mut Ship, &SimPos, &SimVel)>,
    mut flash: ResMut<crate::achievements::LastUnlock>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    cd.0 = (cd.0 - DT).max(0.0);
    if !keys.just_pressed(KeyCode::KeyN) {
        return;
    }
    let tier = upgrades.tier(UpgradeSlot::Shield);
    let Ok((mut ship, pos, vel)) = ships.single_mut() else { return };
    if tier == 0 {
        flash.text = "NOVA NEEDS SHIELD PLATING — CRAFT THE SHIELD SLOT".into();
        flash.ttl = 4.0;
        return;
    }
    if cd.0 > 0.0 || ship.energy < NOVA_FIRE_COST {
        flash.text = if cd.0 > 0.0 {
            format!("NOVA RECHARGING — {:.0}s", cd.0)
        } else {
            "NOVA NEEDS 30 ENERGY".into()
        };
        flash.ttl = 3.0;
        return;
    }
    cd.0 = NOVA_COOLDOWN;
    ship.energy -= NOVA_FIRE_COST;
    let punch = nova_punch(tier, ship.shield);
    let rating = nova_rating(tier, pilot_level(run.total()));
    // The shield IS the ammo: the bubble empties into the wave.
    ship.shield = 0.0;
    let max_radius = 3.0e9 + 1.0e9 * tier as f64;
    sfx.write(crate::audio::Sfx::Explosion);
    flash.text = format!("NOVA — PUNCH {:.0} · RATING {rating}", punch);
    flash.ttl = 4.0;
    info!("nova fired: punch {punch:.0}, rating {rating}, reach {max_radius:.1e}");
    commands.spawn((
        SystemScoped,
        NovaWave {
            radius: 5.0e7,
            max_radius,
            speed: max_radius / 2.0,
            punch,
            rating,
            hit: HashSet::default(),
        },
        SimPos(pos.0),
        SimVel(vel.0),
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().ico(4).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.4, 0.9, 1.0, 0.16),
            emissive: LinearRgba::rgb(0.6, 1.6, 2.0),
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            ..default()
        })),
        Transform::default(),
        NotShadowCaster,
        bevy::picking::Pickable::IGNORE,
    ));
}

/// Grow each wave, drain the reactor, and judge everything the front
/// passes: hostile screens soak until consumed, celestials shatter if
/// outranked. Rewards flow back — energy from suns, debris from rock.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn expand_novas(
    mut waves: Query<(Entity, &mut NovaWave, &SimPos, &mut Transform)>,
    mut ships: Query<(&mut Ship, &SimPos), Without<NovaWave>>,
    mut hostiles: Query<
        (Entity, &mut Hull, Option<&mut NovaShield>, &ObjectLevel, &SimPos),
        (Without<NovaWave>, Without<CelestialBody>),
    >,
    celestials: Query<
        (Entity, &CelestialBody, Option<&ObjectLevel>, Option<&SunBody>, &SimPos),
        Without<NovaWave>,
    >,
    riders: Query<(Entity, &OnRailsAround)>,
    sun_lights: Query<Entity, (With<PointLight>, With<SimPos>, Without<CelestialBody>)>,
    mut run: ResMut<RunScore>,
    mut flash: ResMut<crate::achievements::LastUnlock>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (wave_entity, mut wave, wave_pos, mut tf) in &mut waves {
        // The wave feeds on the reactor as it grows; a dry tank ends it.
        let (drained, _ship_pos) = match ships.single_mut() {
            Ok((mut ship, sp)) => {
                let take = (NOVA_DRAIN * DT).min(ship.energy);
                ship.energy -= take;
                (take > 0.0, sp.0)
            }
            Err(_) => (false, Vec3d::ZERO),
        };
        wave.radius += wave.speed * DT;
        tf.scale = Vec3::splat((wave.radius * RENDER_SCALE) as f32);
        if wave.radius >= wave.max_radius || !drained {
            commands.entity(wave_entity).despawn();
            continue;
        }

        // Hostiles inside the front: screen soaks, remainder burns 2x.
        for (entity, mut hull, shield, level, pos) in &mut hostiles {
            if wave.hit.contains(&entity) || pos.0.distance(wave_pos.0) > wave.radius {
                continue;
            }
            wave.hit.insert(entity);
            let mut punch = wave.punch;
            if let Some(mut screen) = shield {
                let absorbed = punch.min(screen.0);
                screen.0 -= absorbed;
                punch -= absorbed;
            }
            if punch > 0.0 {
                hull.hp -= punch * 2.0;
                info!(
                    "nova hit hostile L{}: {:.0} through screen, hull now {:.0}",
                    level.0, punch, hull.hp
                );
            } else {
                info!("nova absorbed by hostile L{} screen", level.0);
            }
        }

        // Celestials: outranked bodies shatter into energy and debris.
        for (entity, body, level, sun, pos) in &celestials {
            if wave.hit.contains(&entity) {
                continue;
            }
            let d = pos.0.distance(wave_pos.0);
            if d - body.radius > wave.radius {
                continue;
            }
            wave.hit.insert(entity);
            let Some(level) = level else { continue };
            let is_sun = sun.is_some();
            if wave.rating < level.0 {
                flash.text =
                    format!("{} IS LEVEL {} — RATING {} TOO LOW", body.name.to_uppercase(), level.0, wave.rating);
                flash.ttl = 4.0;
                info!("nova bounced off {} (L{} vs rating {})", body.name, level.0, wave.rating);
                continue;
            }
            // SHATTERED. Energy back to the tank, debris to the field.
            let energy = destruction_energy(level.0, is_sun);
            if let Ok((mut ship, _)) = ships.single_mut() {
                ship.energy = (ship.energy + energy).min(ship.energy_max);
            }
            let pieces = destruction_debris(level.0);
            let mesh = meshes.add(Cuboid::new(4.0, 3.2, 3.6).mesh());
            let mut rng = oj_universe::SplitMix64(level.0 as u64 ^ pos.0.x.to_bits());
            for i in 0..pieces {
                let a = std::f64::consts::TAU * i as f64 / pieces as f64;
                let element = if is_sun {
                    // A star's corpse is exotic matter.
                    if i % 2 == 0 { Element::Uranium } else { Element::Aetherite }
                } else {
                    [Element::Iron, Element::Silicon, Element::Ice, Element::Carbon]
                        [(rng.next_u64() % 4) as usize]
                };
                commands.spawn((
                    SystemScoped,
                    Wreck { value: 4 + level.0 as u64, element },
                    Tumble::seeded(i as u64 ^ level.0 as u64),
                    SimPos(pos.0 + Vec3d::new(a.cos(), a.sin(), 0.0) * (body.radius * 1.2)),
                    SimVel(Vec3d::new(a.cos(), a.sin(), 0.0) * 2.0e5),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(materials.add(debris_material(element))),
                    Transform::default(),
                ));
            }
            crate::fx::spawn_explosion(
                &mut commands,
                &mut meshes,
                &mut materials,
                pos.0,
                Vec3d::ZERO,
                if is_sun { 60.0 } else { 30.0 },
            );
            sfx.write(crate::audio::Sfx::Explosion);
            run.combat_score += level.0 as u64 * 200;
            flash.text = format!(
                "{} SHATTERED — +{:.0} ENERGY · {} DEBRIS",
                body.name.to_uppercase(),
                energy,
                pieces
            );
            flash.ttl = 6.0;
            info!(
                "nova destroyed {} (L{}): +{energy:.0} energy, {pieces} debris",
                body.name, level.0
            );
            // Tear out the body and everything riding its rails —
            // rings/shells are children (recursive despawn), moons and
            // rail debris reference it through OnRailsAround.
            let mut doomed: HashSet<Entity> = HashSet::default();
            doomed.insert(entity);
            loop {
                let mut grew = false;
                for (rider, rails) in &riders {
                    if doomed.contains(&rails.parent) && doomed.insert(rider) {
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            for e in &doomed {
                commands.entity(*e).despawn();
            }
            // A dead sun takes its light with it.
            if is_sun {
                for light in &sun_lights {
                    commands.entity(light).despawn();
                }
            }
        }
    }
}

/// Dev hooks: OJ_SHIELD=n sets the ship's shield points once (bubble
/// inspection); pairs with OJ_SHIELD_TIER in upgrades.rs.
fn dev_shield(mut done: Local<bool>, mut ships: Query<&mut Ship>) {
    if *done {
        return;
    }
    let Ok(v) = std::env::var("OJ_SHIELD") else {
        *done = true;
        return;
    };
    if let Ok(v) = v.parse::<f64>()
        && let Ok(mut ship) = ships.single_mut()
    {
        ship.shield = v;
        *done = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The destruction economy the design asks for, pinned: suns
    /// out-pay planets at equal level, higher level means more debris,
    /// higher skill means harder punch, and the rating gates who can
    /// shatter what.
    #[test]
    fn nova_economy_scales_as_designed() {
        // A sun at level 16 creates more energy than a planet at 16.
        assert!(destruction_energy(16, true) > destruction_energy(16, false));
        // A planet at level 10 has more debris than one at level 9.
        assert!(destruction_debris(10) > destruction_debris(9));
        // Punch grows with the shield weapon skill and with banked shield.
        assert!(nova_punch(3, 100.0) > nova_punch(1, 100.0));
        assert!(nova_punch(3, 100.0) > nova_punch(3, 20.0));
        // Rating: a fresh pilot cannot shatter any sun; a deep one can.
        assert!(nova_rating(1, 1) < sun_level(SunClass::M));
        assert!(nova_rating(6, 20) >= sun_level(SunClass::M));
        // The heavy exotics outrank everything reachable early.
        assert!(sun_level(SunClass::BlackHole) > sun_level(SunClass::O));
        // Moons undercut planets: level rises with radius.
        assert!(body_level(5.0e7) < body_level(3.0e9));
    }
}
