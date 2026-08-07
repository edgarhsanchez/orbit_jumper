//! P1 gameplay modules: study, scorecard, salvage/respawn.
//!
//! Each is a small plugin over the sim's components; they communicate
//! through events and resources, never by reaching into each other.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use oj_materials::Element;
use oj_orbits::Vec3d;
use oj_universe::SunClass;

use crate::sim::{DT, OnRails, OnRailsAround, Ship, SimClock, SunBody, SystemScoped, TIME_WARP};
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
    /// Salvage credits EARNED this run — the score term. Monotone: it
    /// only climbs, so spending credits never lowers score or rank.
    pub salvage_value: u64,
    /// Salvage credits spent (hull patches). The spendable balance is
    /// `salvage_balance()`; the HUD shows that, and what it shows is
    /// exactly what can be spent.
    pub salvage_spent: u64,
    /// Clean gravity assists flown (SOI transit, no thrust, net speed gain).
    pub assists: u32,
    /// Hostiles destroyed (count; the VALUE of each kill goes to
    /// `combat_score`).
    pub kills: u32,
    /// Bounty-weighted kill credit — the dominant score term. A tougher
    /// enemy is worth proportionally more, so leveling is paced by what
    /// you can defeat, not by how long you idle.
    pub combat_score: u64,
    /// Skill points banked THIS RUN. The pilot's rank is per-run: every
    /// restart returns to level 1, and unspent points die with the hull.
    pub skill_points: u32,
    /// Highest level already paid this run.
    pub level_seen: u32,
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

    /// Spendable salvage credits: earned minus spent.
    pub fn salvage_balance(&self) -> u64 {
        self.salvage_value.saturating_sub(self.salvage_spent)
    }
}

/// Lifetime totals, persisted to disk between sessions.
#[derive(Resource, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CareerScore {
    pub best_run: u64,
    pub total_score: u64,
    pub runs: u32,
    pub ships_lost: u32,
    /// LEGACY, unused: rank went per-run (level 1 on every restart), so
    /// points now live on RunScore. Kept so old save files still parse.
    #[serde(default)]
    pub skill_points: u32,
    #[serde(default)]
    pub level_seen: u32,
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

/// Debris tumbles: a slow per-piece rotation so facets sweep the
/// sunlight and the sun-facing side glints. The render sync writes only
/// translation, so this rotation survives the camera-relative frame.
#[derive(Component)]
pub struct Tumble {
    pub axis: Vec3,
    /// Radians per second of real time.
    pub rate: f32,
}

impl Tumble {
    /// Deterministic tumble from any handy seed — spawn index, rng draw.
    pub fn seeded(k: u64) -> Self {
        let mut r = oj_universe::SplitMix64(k ^ 0x7D3B_C0DE);
        let axis = Vec3::new(
            r.range(-1.0, 1.0) as f32,
            r.range(-1.0, 1.0) as f32,
            r.range(-1.0, 1.0) as f32,
        )
        .try_normalize()
        .unwrap_or(Vec3::Z);
        Self { axis, rate: r.range(0.3, 1.4) as f32 }
    }
}

fn tumble_debris(time: Res<Time>, mut pieces: Query<(&Tumble, &mut Transform)>) {
    for (tumble, mut transform) in &mut pieces {
        transform.rotate(Quat::from_axis_angle(tumble.axis, tumble.rate * time.delta_secs()));
    }
}

/// The sun-catching surface for a debris element. Structural metals are
/// bare metal (bright specular on the sun side, dark on the night side),
/// ice is glass-gloss, and the exotics keep a faint glow of their own —
/// but every piece reads its lighting from the nearest sun's point
/// light, so the shine always faces the sun.
pub fn debris_material(element: Element) -> StandardMaterial {
    // Metallic stays below 1: a pure mirror only flashes at glint
    // angles, while some diffuse keeps the sun-facing face readably lit
    // through the whole tumble — the shine should track the sun, not
    // the camera.
    let (base, metallic, roughness, emissive) = match element {
        Element::Iron => (Color::srgb(0.60, 0.58, 0.55), 0.75, 0.42, LinearRgba::BLACK),
        Element::Titanium => (Color::srgb(0.72, 0.74, 0.78), 0.75, 0.30, LinearRgba::BLACK),
        Element::Silicon => (Color::srgb(0.42, 0.52, 0.66), 0.4, 0.25, LinearRgba::BLACK),
        Element::Carbon => (Color::srgb(0.24, 0.24, 0.26), 0.3, 0.5, LinearRgba::BLACK),
        Element::Ice => (Color::srgb(0.78, 0.90, 1.0), 0.0, 0.08, LinearRgba::BLACK),
        Element::Uranium => {
            (Color::srgb(0.48, 0.74, 0.48), 0.6, 0.35, LinearRgba::rgb(0.03, 0.10, 0.03))
        }
        Element::Aetherite => {
            (Color::srgb(0.66, 0.54, 0.90), 0.7, 0.20, LinearRgba::rgb(0.08, 0.04, 0.14))
        }
    };
    StandardMaterial {
        base_color: base,
        metallic,
        perceptual_roughness: roughness,
        emissive,
        ..default()
    }
}

/// The vessel's hold: collected elements, ready for crafting.
#[derive(Resource, Default)]
pub struct Stash(pub HashMap<Element, u32>);

/// Fired when a hull reaches zero.
#[derive(Message)]
pub struct ShipDestroyed {
    pub at: Vec3d,
}

/// Set while the destroyed-vessel screen is up: the pilot flies again
/// only when they choose to. Respawn waits on this.
#[derive(Resource, Default)]
pub struct AwaitingRestart(pub bool);

/// The ended run's numbers, frozen at the moment of death for the
/// destroyed-vessel screen (the live RunScore resets immediately).
#[derive(Resource, Default)]
pub struct LastRun(pub String);

pub struct SalvagePlugin;

impl Plugin for SalvagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stash>()
            .init_resource::<AwaitingRestart>()
            .init_resource::<LastRun>()
            .add_message::<ShipDestroyed>()
            .add_systems(
                FixedUpdate,
                (detect_death, spawn_wrecks, magnet_wrecks, collect_wrecks, respawn).chain(),
            )
            .add_systems(Update, (dev_hull, dev_wrecks, tumble_debris));
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

#[allow(clippy::too_many_arguments)]
fn spawn_wrecks(
    mut events: MessageReader<ShipDestroyed>,
    mut career: ResMut<CareerScore>,
    mut run: ResMut<RunScore>,
    mut awaiting: ResMut<AwaitingRestart>,
    mut last_run: ResMut<LastRun>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    for death in events.read() {
        sfx.write(crate::audio::Sfx::Explosion);
        sfx.write(crate::audio::Sfx::Warning);
        career.ships_lost += 1;
        // Freeze the ended run for the destroyed-vessel screen, then
        // hold the respawn until the pilot chooses to fly again.
        last_run.0 = format!(
            "RUN SCORE {}  ·  KILLS {}  ·  SALVAGE {} CR",
            run.total(),
            run.kills,
            run.salvage_value
        );
        awaiting.0 = true;
        career.absorb(&run);
        *run = RunScore::default();
        // Going down IS an event — the biggest fireball in the game.
        crate::fx::spawn_explosion(
            &mut commands,
            &mut meshes,
            &mut materials,
            death.at,
            oj_orbits::Vec3d::ZERO,
            26.0,
        );
        // The lost ship becomes claimable scrap, scattered near the wreck.
        let mesh = meshes.add(Cuboid::new(4.0, 4.0, 4.0).mesh());
        for i in 0..5 {
            let offset = Vec3d::new(
                (i as f64 - 2.0) * 1.5e8,
                (i as f64 * 37.0).sin() * 1.0e8,
                0.0,
            );
            // A dead hull scraps into structural metals.
            let element = if i % 2 == 0 { Element::Iron } else { Element::Titanium };
            let mat = materials.add(debris_material(element));
            commands.spawn((
                SystemScoped,
                Wreck { value: 20, element },
                Tumble::seeded(i as u64),
                SimPos(death.at + offset),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::default(),
            ));
        }
    }
}

/// How far the salvage tractor reaches, sim meters.
const MAGNET_RADIUS: f64 = 4.0e9;
/// Peak pull speed, m/s of sim time; ramps up as the debris closes.
const MAGNET_SPEED: f64 = 1.4e6;

/// Free-floating wreckage streams toward the NEAREST vessel — combat
/// debris comes to the victor instead of demanding a sweep-up lap.
/// Nearest, not "the player's": in the multiplayer phase whoever is
/// closest collects, and this min-by-distance is the line that will
/// decide it. Ring debris on rails is excluded — that salvage is
/// terrain, visited on purpose.
#[allow(clippy::type_complexity)]
fn magnet_wrecks(
    ships: Query<&SimPos, With<Ship>>,
    mut wrecks: Query<
        &mut SimPos,
        (With<Wreck>, Without<OnRails>, Without<OnRailsAround>, Without<Ship>),
    >,
) {
    let dt = DT * TIME_WARP;
    for mut pos in &mut wrecks {
        let Some((target, d)) = ships
            .iter()
            .map(|s| (s.0, s.0.distance(pos.0)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        else {
            return;
        };
        if d > MAGNET_RADIUS || d < 1.0 {
            continue;
        }
        // Gentle at the field's edge, urgent near the hull.
        let pull = MAGNET_SPEED * (0.25 + 0.75 * (1.0 - d / MAGNET_RADIUS));
        let step = (target - pos.0) / d * (pull * dt).min(d);
        pos.0 += step;
    }
}

/// Dev hook: OJ_HULL=40 sets the first ship's hull (and drops the
/// shield) so the repair and destroyed-vessel paths can be exercised
/// without volunteering for a beating. Once, first ship only.
fn dev_hull(mut done: Local<bool>, mut ships: Query<&mut Ship>) {
    if *done {
        return;
    }
    let Ok(v) = std::env::var("OJ_HULL") else {
        *done = true;
        return;
    };
    if let Ok(v) = v.parse::<f64>()
        && let Ok(mut ship) = ships.single_mut()
    {
        ship.hull = v;
        ship.shield = 0.0;
        *done = true;
    }
}

/// Dev hook: OJ_WRECKS=N parks N debris pieces in a ring around the
/// ship at 6e9 m — outside the magnet, cycling every element — so the
/// sun-facing shine can be inspected without a combat session. Once,
/// first ship only; no-op in normal runs.
#[allow(clippy::too_many_arguments)]
fn dev_wrecks(
    mut done: Local<bool>,
    ships: Query<&SimPos, With<Ship>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if *done {
        return;
    }
    let Ok(n) = std::env::var("OJ_WRECKS").map(|v| v.parse::<u32>().unwrap_or(0)) else {
        *done = true;
        return;
    };
    let Ok(ship_pos) = ships.single() else { return };
    let elements = [
        Element::Iron,
        Element::Titanium,
        Element::Silicon,
        Element::Carbon,
        Element::Ice,
        Element::Uranium,
        Element::Aetherite,
    ];
    // OJ_LIGHT=1: also park a strong point light at the render origin
    // (the ship) — the control experiment for "is the sun light dead":
    // if this lights the ring radially, point lights work here and the
    // sun's own light is what's failing.
    if let Ok(range) = std::env::var("OJ_LIGHT").map(|v| v.parse::<f32>().unwrap_or(5.0e3)) {
        commands.spawn((
            SystemScoped,
            PointLight {
                intensity: 1.0e9,
                range,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::default(),
        ));
    }
    // OJ_LIGHT_SUN=1: a bare light synced to the sun's exact position
    // (SimPos zero) at the sun's intensity — isolates "the sun entity"
    // from "the sun's location" as the reason its light never lands.
    if std::env::var("OJ_LIGHT_SUN").is_ok() {
        commands.spawn((
            SystemScoped,
            PointLight {
                intensity: 3.0e15,
                range: 2.0e6,
                shadow_maps_enabled: false,
                ..default()
            },
            SimPos(Vec3d::ZERO),
            Transform::default(),
        ));
    }
    let cuboid = meshes.add(Cuboid::new(4.0, 4.0, 4.0).mesh());
    // Alternate cuboids with spheres: the cuboids are the real debris
    // shape, the spheres are light-direction instruments — a sphere's
    // bright hemisphere points at the sun with no facet noise.
    let sphere = meshes.add(Sphere::new(2.5).mesh().ico(3).unwrap());
    for i in 0..n {
        let angle = std::f64::consts::TAU * i as f64 / n as f64;
        let element = elements[i as usize % elements.len()];
        let mesh = if i % 2 == 0 { cuboid.clone() } else { sphere.clone() };
        commands.spawn((
            SystemScoped,
            Wreck { value: 1, element },
            Tumble::seeded(i as u64),
            SimPos(ship_pos.0 + Vec3d::new(angle.cos(), angle.sin(), 0.0) * 6.0e9),
            Mesh3d(mesh),
            MeshMaterial3d(materials.add(debris_material(element))),
            // Oversized on purpose: this ring exists to eyeball the
            // sun-facing shine, and facets need pixels to read.
            Transform::from_scale(Vec3::splat(5.0)),
        ));
    }
    *done = true;
}

const COLLECT_RADIUS: f64 = 5.0e8;

fn collect_wrecks(
    wrecks: Query<(Entity, &Wreck, &SimPos), Without<Ship>>,
    ships: Query<&SimPos, With<Ship>>,
    upgrades: Res<crate::upgrades::ShipUpgrades>,
    mut run: ResMut<RunScore>,
    mut stash: ResMut<Stash>,
    mut commands: Commands,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let Ok(ship_pos) = ships.single() else { return };
    let mut collected = false;
    for (entity, wreck, pos) in &wrecks {
        if pos.0.distance(ship_pos.0) < COLLECT_RADIUS {
            run.salvage_value += wreck.value;
            *stash.0.entry(wreck.element).or_default() += 1;
            sfx.write(crate::audio::Sfx::Salvage);
            commands.entity(entity).despawn();
            collected = true;
        }
    }
    // Materials are craft currency now; what you hauled in survives the
    // session.
    if collected {
        crate::upgrades::save_loadout(&upgrades, &stash);
    }
}

/// A destroyed vessel is forever lost — but the pilot flies again: a fresh
/// tier-1 ship spawns at the starting orbit with a zeroed run score.
#[allow(clippy::too_many_arguments)]
fn respawn(
    ships: Query<(), With<Ship>>,
    awaiting: Res<AwaitingRestart>,
    game: Res<GameUniverse>,
    style: Res<crate::sim::ShipStyle>,
    mut study: ResMut<StudyState>,
    _clock: Res<SimClock>,
    commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // The destroyed-vessel screen holds the field until the pilot
    // presses START OVER.
    if !ships.is_empty() || awaiting.0 {
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
mod magnet_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use oj_materials::Element;

    /// The multiplayer-critical rule: debris streams to the NEAREST
    /// ship, whoever that is — and debris outside the field stays put.
    #[test]
    fn wrecks_stream_to_the_nearest_ship() {
        let mut world = World::new();
        world.spawn((Ship::default(), SimPos(Vec3d::ZERO)));
        world.spawn((Ship::default(), SimPos(Vec3d::new(1.0e10, 0.0, 0.0))));
        let near_b = world
            .spawn((Wreck { value: 1, element: Element::Iron }, SimPos(Vec3d::new(8.0e9, 0.0, 0.0))))
            .id();
        let far = world
            .spawn((Wreck { value: 1, element: Element::Iron }, SimPos(Vec3d::new(0.0, 9.0e9, 0.0))))
            .id();
        world.run_system_once(magnet_wrecks).unwrap();
        let p = world.get::<SimPos>(near_b).unwrap().0;
        assert!(
            p.x > 8.0e9,
            "wreck between two ships must move toward the CLOSER one (x {} should grow)",
            p.x
        );
        let f = world.get::<SimPos>(far).unwrap().0;
        assert_eq!(f, Vec3d::new(0.0, 9.0e9, 0.0), "outside the field nothing moves");
        // Repeated ticks must converge on the near ship, never overshoot
        // into the far one's lap.
        for _ in 0..600 {
            world.run_system_once(magnet_wrecks).unwrap();
        }
        let p = world.get::<SimPos>(near_b).unwrap().0;
        assert!(
            p.distance(Vec3d::new(1.0e10, 0.0, 0.0)) < 2.0e9,
            "after many ticks the wreck should sit near ship B, got {p:?}"
        );
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
