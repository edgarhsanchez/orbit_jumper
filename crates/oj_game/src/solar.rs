//! The solar arm: how a ship refuels outside an orbit.
//!
//! Press [P] and a segmented boom telescopes out toward the nearest sun,
//! ending in an octagonal collector whose sun side lights up as it
//! drinks. While the arm is out, energy climbs fast and WEAPONS ARE
//! OFFLINE — stow it (press [P] again) before firing. At full charge the
//! arm stows itself. Riding an orbit still trickle-charges; free flight
//! has no other source: deploying the arm is the deliberate, vulnerable
//! act of refueling.

use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::sim::{RENDER_SCALE, Ship, SunBody};
use crate::{SimPos, SimVel};

/// Boom length at full extension, render units.
const ARM_LEN: f32 = 26.0;
/// Seconds to fully extend or stow.
const DEPLOY_S: f64 = 1.1;
/// Energy per real second while deployed, scaled by the sun's class.
const BASE_RATE: f64 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ArmPhase {
    #[default]
    Stowed,
    Deploying,
    Deployed,
    Retracting,
}

/// The arm's state; `extent` runs 0 (stowed) to 1 (deployed).
#[derive(Resource, Default)]
pub struct SolarArm {
    pub phase: ArmPhase,
    pub extent: f64,
}

impl SolarArm {
    /// Weapons are hot only when the arm is fully away.
    pub fn weapons_free(&self) -> bool {
        matches!(self.phase, ArmPhase::Stowed)
    }
}

/// Root of the arm rig (positioned at the ship, aimed at the sun).
#[derive(Component)]
struct ArmRig;

/// The telescoping boom (scaled by extent).
#[derive(Component)]
struct ArmBoom;

/// The octagonal collector head (rides the boom tip).
#[derive(Component)]
struct ArmHead;

/// The sun-facing glow face of the collector.
#[derive(Component)]
struct ArmFace;

pub struct SolarPlugin;

impl Plugin for SolarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SolarArm>()
            .add_systems(Startup, spawn_rig)
            .add_systems(Update, (drive_arm, place_rig).chain());
    }
}

fn spawn_rig(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let boom_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.58, 0.62),
        metallic: 0.7,
        perceptual_roughness: 0.35,
        ..default()
    });
    let head_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.16, 0.22),
        metallic: 0.5,
        ..default()
    });
    let face_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.85, 0.4),
        emissive: LinearRgba::rgb(3.5, 2.6, 0.8),
        unlit: true,
        ..default()
    });
    commands
        .spawn((
            ArmRig,
            SimPos(Vec3d::ZERO),
            Transform::default(),
            Visibility::Hidden,
            bevy::picking::Pickable::IGNORE,
        ))
        .with_children(|rig| {
            // Telescoping boom: two nested segments read as machinery.
            rig.spawn((
                ArmBoom,
                Mesh3d(meshes.add(Cuboid::new(0.9, 1.0, 0.9).mesh())),
                MeshMaterial3d(boom_mat.clone()),
                Transform::default(),
                bevy::picking::Pickable::IGNORE,
            ));
            rig.spawn((
                ArmBoom,
                Mesh3d(meshes.add(Cuboid::new(0.5, 1.0, 0.5).mesh())),
                MeshMaterial3d(boom_mat),
                Transform::default(),
                bevy::picking::Pickable::IGNORE,
            ));
            // Octagonal collector: an 8-sided prism, flat to the sun.
            rig.spawn((
                ArmHead,
                Mesh3d(meshes.add(Cylinder::new(4.2, 1.1).mesh().resolution(8))),
                MeshMaterial3d(head_mat),
                Transform::default(),
                bevy::picking::Pickable::IGNORE,
            ));
            // The face that shines on the collecting side.
            rig.spawn((
                ArmFace,
                Mesh3d(meshes.add(Circle::new(3.6).mesh().resolution(8))),
                MeshMaterial3d(face_mat),
                Transform::default(),
                bevy::picking::Pickable::IGNORE,
            ));
        });
}

/// Toggle on [P]; animate extension; charge while deployed; auto-stow at
/// full tank.
fn drive_arm(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut arm: ResMut<SolarArm>,
    suns: Query<&SunBody>,
    mut ships: Query<&mut Ship>,
) {
    let dt = time.delta_secs_f64();
    if keys.just_pressed(KeyCode::KeyP) {
        arm.phase = match arm.phase {
            ArmPhase::Stowed | ArmPhase::Retracting => ArmPhase::Deploying,
            ArmPhase::Deploying | ArmPhase::Deployed => ArmPhase::Retracting,
        };
    }
    match arm.phase {
        ArmPhase::Deploying => {
            arm.extent = (arm.extent + dt / DEPLOY_S).min(1.0);
            if arm.extent >= 1.0 {
                arm.phase = ArmPhase::Deployed;
            }
        }
        ArmPhase::Retracting => {
            arm.extent = (arm.extent - dt / DEPLOY_S).max(0.0);
            if arm.extent <= 0.0 {
                arm.phase = ArmPhase::Stowed;
            }
        }
        ArmPhase::Deployed => {
            if let Ok(mut ship) = ships.single_mut() {
                // Better suns pour faster — the class already prices the
                // hazard of being near them.
                let rate = suns
                    .single()
                    .map(|s| BASE_RATE + 2.5 * s.class.harvest_rate())
                    .unwrap_or(BASE_RATE);
                ship.energy = (ship.energy + rate * dt).min(ship.energy_max);
                // Full tank: the arm knows its job is done.
                if ship.energy >= ship.energy_max {
                    arm.phase = ArmPhase::Retracting;
                }
            }
        }
        ArmPhase::Stowed => {}
    }
}

/// Keep the rig at the ship, aimed at the nearest sun, extended to
/// `extent`; light the collector face only while drinking.
#[allow(clippy::type_complexity)]
fn place_rig(
    arm: Res<SolarArm>,
    ships: Query<(&SimPos, &SimVel), With<Ship>>,
    suns: Query<&SimPos, (With<SunBody>, Without<Ship>, Without<ArmRig>)>,
    mut rigs: Query<
        (&mut SimPos, &mut Transform, &mut Visibility),
        (With<ArmRig>, Without<Ship>, Without<SunBody>),
    >,
    mut parts: Query<
        (Option<&ArmBoom>, Option<&ArmHead>, Option<&ArmFace>, &mut Transform),
        (Without<ArmRig>, Without<Ship>, Without<SunBody>),
    >,
) {
    let Ok((mut rig_pos, mut rig_tf, mut vis)) = rigs.single_mut() else { return };
    if arm.extent <= 0.001 {
        *vis = Visibility::Hidden;
        return;
    }
    *vis = Visibility::Inherited;
    let (Ok((ship_pos, _)), Ok(sun_pos)) = (ships.single(), suns.single()) else { return };
    rig_pos.0 = ship_pos.0;
    let dir = (sun_pos.0 - ship_pos.0).normalized();
    let dir3 = Vec3::new(dir.x as f32, dir.y as f32, dir.z as f32).normalize_or(Vec3::Y);
    rig_tf.rotation = Quat::from_rotation_arc(Vec3::Y, dir3);

    let len = ARM_LEN * arm.extent as f32;
    let mut boom_index = 0;
    for (boom, head, face, mut tf) in &mut parts {
        if boom.is_some() {
            // Two nested segments: inner one covers the outer half.
            let (frac_start, frac_len) = if boom_index == 0 { (0.0, 0.55) } else { (0.5, 0.5) };
            boom_index += 1;
            let seg = len * frac_len;
            tf.scale = Vec3::new(1.0, seg.max(0.01), 1.0);
            tf.translation = Vec3::new(0.0, len * frac_start + seg / 2.0, 0.0);
        } else if head.is_some() {
            // The prism lies along Y already (cylinder axis is Y); tip it
            // so its flat faces meet the sun line.
            tf.translation = Vec3::new(0.0, len + 0.6, 0.0);
        } else if face.is_some() {
            // Sun side of the head: just past it along the sun line.
            tf.translation = Vec3::new(0.0, len + 1.3, 0.0);
            tf.rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        }
    }
    let _ = RENDER_SCALE;
}
