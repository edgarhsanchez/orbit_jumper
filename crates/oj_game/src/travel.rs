//! Multi-system travel: the galaxy map and the jump.
//!
//! The map (M) lists the nearest systems of the current galaxy. Jumping
//! costs energy scaled by distance — the launch window question: charge in
//! this system first, or gamble on arriving dry? Arrival tears down every
//! system-scoped entity and derives the destination from the seed, ship
//! reset to a fresh start orbit. Suns you have studied are remembered in
//! the atlas and labeled on the map; everything else is a "?".

use std::collections::HashMap;

use bevy::prelude::*;
use oj_orbits::Vec3d;
use oj_universe::{SunClass, SystemId};

use crate::command::NavState;
use crate::modules::{RunScore, StudyState};
use crate::sim::{Ship, SystemScoped, spawn_bodies};
use crate::{GameUniverse, SimPos, SimVel};

/// Suns whose class the pilot has learned (study completions), by system.
#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct SunAtlas(pub HashMap<SystemId, SunClass>);

/// A jump requested by the map UI, consumed by `perform_jump`.
#[derive(Resource, Default)]
pub struct PendingJump(pub Option<SystemId>);

/// The map rows currently on screen, in display order — the jump command's
/// `CommandParameter` is an index into this.
#[derive(Resource, Default)]
pub struct MapRows(pub Vec<SystemId>);

/// Broadcast after arrival so other modules (drones, etc.) can respawn
/// their system-scoped content.
#[derive(Message)]
pub struct SystemChanged;

pub struct TravelPlugin;

impl Plugin for TravelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SunAtlas>()
            .init_resource::<PendingJump>()
            .init_resource::<MapRows>()
            .add_message::<SystemChanged>()
            .add_systems(Update, (record_studied_suns, perform_jump));
    }
}

/// Completing a study writes the sun's class into the atlas.
fn record_studied_suns(
    study: Res<StudyState>,
    game: Res<GameUniverse>,
    mut atlas: ResMut<SunAtlas>,
) {
    if study.revealed
        && !atlas.0.contains_key(&game.current)
        && let Some(system) = game.universe.system(game.current)
    {
        atlas.0.insert(game.current, system.sun.class);
    }
}

/// One light-year, m.
pub const LY: f64 = 9.46e15;

/// Jump cost in energy for a map distance, m.
pub fn jump_cost(distance: f64) -> f64 {
    30.0 + (distance / LY) * 4.0
}

/// The nearest `count` other systems of the current galaxy, with distance.
pub fn nearby_systems(game: &GameUniverse, count: usize) -> Vec<(SystemId, f64)> {
    let Some(current) = game.universe.system(game.current) else {
        return Vec::new();
    };
    let mut all: Vec<(SystemId, f64)> = game
        .universe
        .systems_in(game.current.sector)
        .filter(|s| s.id != game.current)
        .map(|s| (s.id, s.position.distance(current.position)))
        .collect();
    all.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    all.truncate(count);
    all
}

#[allow(clippy::too_many_arguments)]
fn perform_jump(
    mut pending: ResMut<PendingJump>,
    mut game: ResMut<GameUniverse>,
    mut study: ResMut<StudyState>,
    mut run: ResMut<RunScore>,
    mut changed: MessageWriter<SystemChanged>,
    scoped: Query<Entity, With<SystemScoped>>,
    mut ships: Query<(&mut Ship, &mut SimPos, &mut SimVel, &mut NavState)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(target) = pending.0.take() else { return };
    let Ok((mut ship, mut pos, mut vel, mut nav)) = ships.single_mut() else { return };
    let Some(current) = game.universe.system(game.current) else { return };
    let Some(destination) = game.universe.system(target) else { return };

    let cost = jump_cost(destination.position.distance(current.position));
    if ship.energy < cost {
        return; // the map shows the price; arriving dry is not an option
    }
    ship.energy -= cost;

    // Leaving a system alive counts the sun as survived.
    run.suns_survived += 1;

    for entity in &scoped {
        commands.entity(entity).despawn();
    }
    game.current = target;
    study.progress = 0.0;
    study.revealed = false;

    spawn_bodies(&mut commands, &destination, &mut meshes, &mut materials);

    // Fresh start orbit in the new system.
    let mu = oj_orbits::G * destination.sun.mass;
    let r = destination
        .planets
        .first()
        .map(|p| p.orbit.semi_major * 0.6)
        .unwrap_or(1.0e11);
    pos.0 = Vec3d::new(r, 0.0, 0.0);
    vel.0 = Vec3d::new(0.0, oj_orbits::circular_speed(mu, r), 0.0);
    *nav = NavState::Free;

    changed.write(SystemChanged);
    info!("jumped to {:?}: {:?} sun", target, destination.sun.class);
}
