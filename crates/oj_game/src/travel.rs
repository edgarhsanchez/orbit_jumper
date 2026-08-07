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

/// Headless cross-frame exercise of the jump: real plugins, real frames,
/// no renderer. The transient (teardown + respawn) is exactly what a
/// settled-screenshot check would miss — the suite tests it on purpose.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameUniverse;
    use crate::sim::SystemScoped;
    use crate::weapons::TargetDrone;

    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.add_message::<bevy::input::mouse::MouseWheel>();
        app.add_plugins((
            crate::sim::SimPlugin,
            crate::modules::StudyPlugin,
            crate::modules::ScorePlugin,
            crate::modules::SalvagePlugin,
            crate::command::CommandPlugin,
            crate::upgrades::UpgradesPlugin,
            crate::weapons::WeaponsPlugin,
            TravelPlugin,
        ));
        app
    }

    fn scoped_count(app: &mut App) -> usize {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<SystemScoped>>()
            .iter(world)
            .count()
    }

    fn fill_tank(app: &mut App) {
        let world = app.world_mut();
        let mut ships = world.query::<&mut Ship>();
        let mut ship = ships.single_mut(world).unwrap();
        ship.energy_max = 1.0e9;
        ship.energy = 1.0e9;
    }

    #[test]
    fn jump_tears_down_rebuilds_and_revisits_deterministically() {
        let mut app = headless_app();
        app.update(); // startup: bodies, ship, drones

        let start = app.world().resource::<GameUniverse>().current;
        let scoped_at_start = scoped_count(&mut app);
        assert!(scoped_at_start > 0, "startup spawned nothing system-scoped");

        let (target, dist) = {
            let game = app.world().resource::<GameUniverse>();
            let nearby = nearby_systems(game, 1);
            assert!(!nearby.is_empty(), "tutorial sector holds only one system");
            nearby[0]
        };

        // An empty tank must refuse the jump.
        {
            let world = app.world_mut();
            let mut ships = world.query::<&mut Ship>();
            ships.single_mut(world).unwrap().energy = 0.0;
        }
        app.world_mut().resource_mut::<PendingJump>().0 = Some(target);
        app.update();
        assert_eq!(
            app.world().resource::<GameUniverse>().current,
            start,
            "jump went through with no energy"
        );

        // Funded, the jump lands: current flips, the old system is gone,
        // the new one is populated, the ship paid and was reset.
        fill_tank(&mut app);
        app.world_mut().resource_mut::<PendingJump>().0 = Some(target);
        app.update();
        app.update(); // drone respawn consumes SystemChanged by here

        assert_eq!(app.world().resource::<GameUniverse>().current, target);
        assert!(scoped_count(&mut app) > 0, "destination spawned nothing");
        {
            let world = app.world_mut();
            let mut suns = world.query::<&crate::sim::SunBody>();
            assert_eq!(suns.iter(world).count(), 1, "expected exactly one sun");
            let mut drones = world.query_filtered::<Entity, With<TargetDrone>>();
            assert_eq!(drones.iter(world).count(), 4, "drones did not respawn");
            let mut ships = world.query::<(&Ship, &crate::SimPos)>();
            let (ship, pos) = ships.single(world).unwrap();
            let cost = jump_cost(dist);
            assert!(
                ship.energy <= 1.0e9 - cost + 1.0,
                "jump cost not charged: {} left, cost {}",
                ship.energy,
                cost
            );
            assert!(pos.0.y.abs() < 1.0, "ship not reset to the start orbit");
        }

        // Jumping home rebuilds the SAME system: the seed, not saved
        // state, is the level format.
        fill_tank(&mut app);
        app.world_mut().resource_mut::<PendingJump>().0 = Some(start);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<GameUniverse>().current, start);
        assert_eq!(
            scoped_count(&mut app),
            scoped_at_start,
            "revisited system differs from the original"
        );
    }
}
