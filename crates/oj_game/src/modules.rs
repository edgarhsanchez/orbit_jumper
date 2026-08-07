//! P1 gameplay modules: study, scorecard, salvage/respawn.
//!
//! Each is a small plugin over the sim's components; they communicate
//! through events and resources, never by reaching into each other.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use oj_materials::Element;
use oj_orbits::Vec3d;
use oj_universe::SunClass;

use crate::sim::{DT, Ship, SimClock, SunBody, SystemScoped};
use crate::{GameUniverse, SimPos};

// ---------------------------------------------------------------------------
// Study
// ---------------------------------------------------------------------------

/// Progress of studying the current system's sun. Holding S near a sun
/// fills it; completion reveals the class (and the shield tier needed)
/// in the HUD. Skipping the study and diving in is the gamble the design
/// doc promises.
#[derive(Resource, Default)]
pub struct StudyState {
    pub progress: f64,
    pub revealed: bool,
}

pub struct StudyPlugin;

impl Plugin for StudyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StudyState>()
            .add_systems(FixedUpdate, run_study);
    }
}

fn run_study(
    keys: Res<ButtonInput<KeyCode>>,
    mut study: ResMut<StudyState>,
    suns: Query<&SunBody>,
    ships: Query<&Ship>,
) {
    if study.revealed || !keys.pressed(KeyCode::KeyS) {
        return;
    }
    let (Ok(sun), Ok(_ship)) = (suns.single(), ships.single()) else {
        return;
    };
    // Sensor tier shortens study time later; tier 1 for now.
    let needed = sun.class.study_seconds();
    study.progress += DT;
    if study.progress >= needed {
        study.revealed = true;
    }
}

// ---------------------------------------------------------------------------
// Scorecard
// ---------------------------------------------------------------------------

/// Per-run stats, reset on death.
#[derive(Resource, Default, Clone)]
pub struct RunScore {
    pub seconds_survived: f64,
    pub energy_harvested: f64,
    pub suns_survived: u32,
    pub salvage_value: u64,
    /// Clean gravity assists flown (SOI transit, no thrust, net speed gain).
    pub assists: u32,
    /// Hostiles destroyed (count; the VALUE of each kill goes to
    /// `combat_score`).
    pub kills: u32,
    /// Bounty-weighted kill credit — the dominant score term. A tougher
    /// enemy is worth proportionally more, so leveling is paced by what
    /// you can defeat, not by how long you idle.
    pub combat_score: u64,
    hits: u32,
    /// Last-seen ship energy, for attributing positive deltas to harvest.
    last_energy: Option<f64>,
}

impl RunScore {
    pub fn total(&self) -> u64 {
        // Survival pays real seconds, not warped sim seconds — the old
        // 1 point per SIM second (600/s wall-clock) drowned every other
        // source and let a parked ship out-level a fighting one.
        (self.seconds_survived as u64)
            + (self.energy_harvested as u64)
            + self.suns_survived as u64 * 500
            + self.salvage_value * 10
            + self.assists as u64 * 1000
            + self.combat_score
            + self.kills as u64 * 50
            + self.hits as u64 * 2
    }

    pub fn score_hit(&mut self) {
        self.hits += 1;
    }
}

/// Lifetime totals, persisted to disk between sessions.
#[derive(Resource, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CareerScore {
    pub best_run: u64,
    pub total_score: u64,
    pub runs: u32,
    pub ships_lost: u32,
}

fn career_path() -> std::path::PathBuf {
    // Local persistence first; the global board arrives with the net phase.
    std::path::PathBuf::from("orbit_jumper_career.ron")
}

impl CareerScore {
    pub fn load() -> Self {
        std::fs::read_to_string(career_path())
            .ok()
            .and_then(|s| ron::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(text) = ron::to_string(self) {
            let _ = std::fs::write(career_path(), text);
        }
    }
    pub fn absorb(&mut self, run: &RunScore) {
        let score = run.total();
        self.best_run = self.best_run.max(score);
        self.total_score += score;
        self.runs += 1;
        self.save();
    }
}

pub struct ScorePlugin;

impl Plugin for ScorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RunScore>()
            .insert_resource(CareerScore::load())
            .add_systems(FixedUpdate, tick_score);
    }
}

fn tick_score(mut run: ResMut<RunScore>, ships: Query<&Ship>) {
    let Ok(ship) = ships.single() else {
        run.last_energy = None;
        return;
    };
    run.seconds_survived += DT;
    if let Some(last) = run.last_energy {
        let delta = ship.energy - last;
        if delta > 0.0 {
            run.energy_harvested += delta;
        }
    }
    run.last_energy = Some(ship.energy);
}

// ---------------------------------------------------------------------------
// Salvage + death/respawn
// ---------------------------------------------------------------------------

/// A piece of wreckage or debris: salvage value plus the element it
/// yields into the stash when collected.
#[derive(Component)]
pub struct Wreck {
    pub value: u64,
    pub element: Element,
}

/// The vessel's hold: collected elements, ready for crafting.
#[derive(Resource, Default)]
pub struct Stash(pub HashMap<Element, u32>);

/// Fired when a hull reaches zero.
#[derive(Message)]
pub struct ShipDestroyed {
    pub at: Vec3d,
}

pub struct SalvagePlugin;

impl Plugin for SalvagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stash>()
            .add_message::<ShipDestroyed>()
            .add_systems(
                FixedUpdate,
                (detect_death, spawn_wrecks, collect_wrecks, respawn).chain(),
            );
    }
}

fn detect_death(
    mut events: MessageWriter<ShipDestroyed>,
    ships: Query<(Entity, &Ship, &SimPos)>,
    mut commands: Commands,
) {
    for (entity, ship, pos) in &ships {
        if ship.hull <= 0.0 {
            events.write(ShipDestroyed { at: pos.0 });
            commands.entity(entity).despawn();
        }
    }
}

fn spawn_wrecks(
    mut events: MessageReader<ShipDestroyed>,
    mut career: ResMut<CareerScore>,
    mut run: ResMut<RunScore>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for death in events.read() {
        career.ships_lost += 1;
        career.absorb(&run);
        *run = RunScore::default();
        // The lost ship becomes claimable scrap, scattered near the wreck.
        let mesh = meshes.add(Cuboid::new(4.0, 4.0, 4.0).mesh());
        let mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 0.45, 0.4),
            ..default()
        });
        for i in 0..5 {
            let offset = Vec3d::new(
                (i as f64 - 2.0) * 1.5e8,
                (i as f64 * 37.0).sin() * 1.0e8,
                0.0,
            );
            // A dead hull scraps into structural metals.
            let element = if i % 2 == 0 { Element::Iron } else { Element::Titanium };
            commands.spawn((
                SystemScoped,
                Wreck { value: 20, element },
                SimPos(death.at + offset),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::default(),
            ));
        }
    }
}

const COLLECT_RADIUS: f64 = 5.0e8;

fn collect_wrecks(
    wrecks: Query<(Entity, &Wreck, &SimPos), Without<Ship>>,
    ships: Query<&SimPos, With<Ship>>,
    mut run: ResMut<RunScore>,
    mut stash: ResMut<Stash>,
    mut commands: Commands,
) {
    let Ok(ship_pos) = ships.single() else { return };
    for (entity, wreck, pos) in &wrecks {
        if pos.0.distance(ship_pos.0) < COLLECT_RADIUS {
            run.salvage_value += wreck.value;
            *stash.0.entry(wreck.element).or_default() += 1;
            commands.entity(entity).despawn();
        }
    }
}

/// A destroyed vessel is forever lost — but the pilot flies again: a fresh
/// tier-1 ship spawns at the starting orbit with a zeroed run score.
#[allow(clippy::too_many_arguments)]
fn respawn(
    ships: Query<(), With<Ship>>,
    game: Res<GameUniverse>,
    style: Res<crate::sim::ShipStyle>,
    mut study: ResMut<StudyState>,
    _clock: Res<SimClock>,
    commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !ships.is_empty() {
        return;
    }
    let Some(system) = game.universe.system(game.current) else { return };
    let mu = oj_orbits::G * system.sun.mass;
    let r = system
        .planets
        .first()
        .map(|p| p.orbit.semi_major * 0.6)
        .unwrap_or(1.0e11);
    let v = oj_orbits::circular_speed(mu, r);
    study.progress = 0.0; // knowledge of the sun survives; progress does not
    crate::sim::spawn_ship(
        commands,
        &mut meshes,
        &mut materials,
        Vec3d::new(r, 0.0, 0.0),
        Vec3d::new(0.0, v, 0.0),
        *style,
    );
}

/// Which sun class the HUD should display, honoring the study state.
pub fn displayed_sun_class(class: SunClass, revealed: bool) -> String {
    if revealed {
        format!(
            "{class:?} — shield tier {} required",
            class.required_shield_tier()
        )
    } else {
        "unknown (hold S to study)".to_string()
    }
}

#[cfg(test)]
mod score_tests {
    use super::*;

    /// The invariant the difficulty overhaul exists to hold: an hour of
    /// idling must be worth less than one bounty-weighted kill, so pilot
    /// level is paced by combat, not wall clock.
    #[test]
    fn idling_cannot_outscore_fighting() {
        let mut idle = RunScore::default();
        idle.seconds_survived = 3600.0; // one hour parked
        let mut fight = RunScore::default();
        fight.kills = 1;
        fight.combat_score = 48 * 12; // one level-1 raider bounty
        assert!(
            fight.total() < idle.total() * 2 && idle.total() < fight.total() * 8,
            "one kill ({}) and an idle hour ({}) should be the same order — combat must dominate per minute",
            fight.total(),
            idle.total()
        );
        // Per MINUTE of play the gap is the point: a kill takes seconds.
        let idle_minute = RunScore { seconds_survived: 60.0, ..Default::default() };
        assert!(
            fight.total() > idle_minute.total() * 8,
            "a kill must dwarf a minute of idling"
        );
    }

    /// Boss bounties must pay level-scale points: one dreadnought at
    /// level 6 is worth several levels of raider grinding.
    #[test]
    fn boss_kill_is_level_scale() {
        let raider_bounty = 40 + 6 * 8; // level 6
        let boss_bounty = raider_bounty * 15;
        let mut run = RunScore::default();
        run.combat_score = boss_bounty * 12;
        run.kills = 1;
        // Level 2 costs 2000 lifetime points; the boss alone clears it
        // several times over.
        assert!(run.total() > 2000 * 6, "boss kill = {}", run.total());
    }
}
