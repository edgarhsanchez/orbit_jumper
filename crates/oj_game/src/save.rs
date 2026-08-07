//! Continue where you left off: the run autosaves every few seconds and
//! a fresh boot resumes it — current system, position, velocity, vitals,
//! score and rank. Death clears the save: rank is per-run and dies with
//! the hull, so a finished run cannot be resumed. `OJ_FRESH=1` skips the
//! resume for dev sessions that want a clean start.

use bevy::prelude::*;
use oj_orbits::Vec3d;
use oj_universe::SystemId;

use crate::modules::{AwaitingRestart, RunScore};
use crate::sim::{DT, Ship};
use crate::{GameUniverse, SimPos, SimVel, storage};

const KEY: &str = "save";
/// Seconds of real time between autosaves.
const AUTOSAVE_EVERY: f64 = 5.0;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct SaveGame {
    pub system: SystemId,
    pub run: RunScore,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub hull: f64,
    pub shield: f64,
    pub energy: f64,
}

/// A loaded save waiting for the ship to exist before it applies.
#[derive(Resource, Default)]
struct PendingRestore(Option<SaveGame>);

pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        // Boot INTO the saved system: GameUniverse is inserted here,
        // before SimPlugin's init_resource (a no-op once the resource
        // exists), so startup spawns the saved system directly — no
        // fake jump, no jump cost, no suns_survived bump.
        let mut game = GameUniverse::default();
        let pending = if std::env::var("OJ_FRESH").is_ok() {
            None
        } else {
            load_save().filter(|s| game.universe.system(s.system).is_some())
        };
        if let Some(save) = &pending {
            game.current = save.system;
            info!("resuming saved run in {:?}", save.system);
        }
        app.insert_resource(game)
            .insert_resource(PendingRestore(pending))
            .add_systems(FixedUpdate, (apply_restore, autosave).chain());
    }
}

fn load_save() -> Option<SaveGame> {
    ron::from_str(&storage::load(KEY)?).ok()
}

/// Wipe the save. The death path calls this the moment the hull goes.
pub fn clear_save() {
    storage::clear(KEY);
}

/// Once the ship exists, put it back exactly where the save left it.
/// Resumes into free flight: at saved position and velocity a former
/// orbit continues under real gravity; re-click the ring to make the
/// ride sticky again.
fn apply_restore(
    mut pending: ResMut<PendingRestore>,
    mut run: ResMut<RunScore>,
    mut ships: Query<(&mut Ship, &mut SimPos, &mut SimVel)>,
) {
    if pending.0.is_none() {
        return;
    }
    let Ok((mut ship, mut pos, mut vel)) = ships.single_mut() else { return };
    let save = pending.0.take().expect("checked above");
    pos.0 = Vec3d::new(save.pos[0], save.pos[1], save.pos[2]);
    vel.0 = Vec3d::new(save.vel[0], save.vel[1], save.vel[2]);
    ship.hull = save.hull;
    ship.shield = save.shield;
    ship.energy = save.energy;
    *run = save.run;
    info!(
        "run resumed: score {}, hull {:.0}, energy {:.0}",
        run.total(),
        ship.hull,
        ship.energy
    );
}

fn autosave(
    mut acc: Local<f64>,
    awaiting: Res<AwaitingRestart>,
    pending: Res<PendingRestore>,
    game: Res<GameUniverse>,
    run: Res<RunScore>,
    ships: Query<(&Ship, &SimPos, &SimVel)>,
) {
    // Never save over a pending resume, and never save a dead run —
    // the death path just cleared the slot.
    if awaiting.0 || pending.0.is_some() {
        return;
    }
    *acc += DT;
    if *acc < AUTOSAVE_EVERY {
        return;
    }
    *acc = 0.0;
    let Ok((ship, pos, vel)) = ships.single() else { return };
    let save = SaveGame {
        system: game.current,
        run: run.clone(),
        pos: [pos.0.x, pos.0.y, pos.0.z],
        vel: [vel.0.x, vel.0.y, vel.0.z],
        hull: ship.hull,
        shield: ship.shield,
        energy: ship.energy,
    };
    if let Ok(text) = ron::to_string(&save) {
        storage::save(KEY, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The save round-trips through RON with every field intact — the
    /// contract "continue exactly where you left off" depends on.
    #[test]
    fn save_round_trips() {
        let mut run = RunScore::default();
        run.salvage_value = 640;
        run.salvage_spent = 120;
        run.skill_points = 3;
        run.level_seen = 4;
        run.kills = 7;
        run.combat_score = 31_000;
        let save = SaveGame {
            system: SystemId {
                sector: oj_universe::SectorCoord { x: 3, y: 0, z: 0 },
                index: 2,
            },
            run,
            pos: [2.4e11, -1.0e10, 0.0],
            vel: [0.0, 2.9e4, 0.0],
            hull: 62.0,
            shield: 41.0,
            energy: 77.0,
        };
        let text = ron::to_string(&save).unwrap();
        let back: SaveGame = ron::from_str(&text).unwrap();
        assert_eq!(back.system, save.system);
        assert_eq!(back.run.total(), save.run.total());
        assert_eq!(back.run.skill_points, 3);
        assert_eq!(back.run.salvage_balance(), 520);
        assert_eq!(back.hull, 62.0);
        assert_eq!(back.pos[0], 2.4e11);
    }
}
