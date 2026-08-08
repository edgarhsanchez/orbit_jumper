//! Alien raiders: the hostile half of "fighting off alien ships".
//!
//! Raiders spawn in-system on a cadence that scales with pilot level —
//! the universe answers a stronger pilot with bigger hunting packs, so
//! combat pressure levels infinitely alongside the player. They seek the
//! ship, fire plasma bolts (shield first, then hull — the existing death
//! and salvage loop takes over from there), and die through the same
//! `Hull` reaper as practice drones, dropping a bounty in exotic
//! elements. True player-vs-player rides the future net phase; raiders
//! are the fight the game has today.

use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::modules::{CareerScore, RunScore};
use crate::sim::{BodyVel, DT, Ship, SystemScoped, TIME_WARP, ViewMode};
use crate::upgrades::pilot_level;
use crate::weapons::{Bounty, Hull};
use crate::{SimPos, SimVel};

/// A hostile ship. Combat stats are set at spawn from the pilot level of
/// the moment, so every wave is harder than the last: more damage per
/// bolt, faster bolts, shorter cooldowns, quicker approaches.
#[derive(Component)]
pub struct AlienShip {
    fire_cooldown: f64,
    cooldown: f64,
    damage: f64,
    bolt_speed: f64,
    approach: f64,
    /// Bolts per trigger pull — dreadnoughts volley.
    volley: u32,
    /// Per-ship rhythm offset: weave timing, so no two raiders jink in
    /// step.
    phase: f64,
    /// Seconds between mines for a layer; 0 = never lays.
    mine_every: f64,
    mine_cd: f64,
}

/// Elite raider: a rarer, heavier variant that appears at higher pilot
/// levels. Marker drives the contacts label and nothing else — the stats
/// are already baked into `AlienShip`.
#[derive(Component)]
pub struct Elite;

/// The boss. One at a time, every few pilot levels, announced in the
/// threat line. Same AI loop as a raider — what changes is mass: a dozen
/// raiders' worth of hull, volley fire, and a bounty to match. Arrives
/// with a two-raider escort wing.
#[derive(Component)]
pub struct Dreadnought;

/// The weaver: a fast, lightly-armed raider that seeds proximity mines
/// across the fight. Kill it first or the arena shrinks around you.
#[derive(Component)]
pub struct MineLayer;

/// The carrier: slow, heavy, and a mothership — it hatches kamikaze
/// interceptors on a clock until it dies. Kill it first or the sky
/// fills with darts.
#[derive(Component)]
pub struct Carrier {
    hatch_every: f64,
    hatch_cd: f64,
    level: u32,
}

/// A hatched kamikaze: seeks like a raider, but its weapon is ITSELF —
/// it detonates on proximity, shield-first like everything else.
#[derive(Component)]
pub struct Interceptor {
    damage: f64,
    ttl: f64,
}

/// Interceptors detonate inside this range, m.
const INTERCEPT_TRIGGER: f64 = 7.0e8;

/// A proximity mine: arms after a delay, drifts, detonates on the
/// player's hull — shields first, like everything else that hits.
#[derive(Component)]
pub struct SpaceMine {
    armed_in: f64,
    ttl: f64,
    damage: f64,
}

/// Mines trigger inside this range, m.
const MINE_TRIGGER: f64 = 6.0e8;

/// A plasma bolt in flight.
#[derive(Component)]
pub struct AlienBolt {
    damage: f64,
    ttl: f64,
}

/// Seconds between spawn waves: 20 at level 1, tightening toward a
/// 6-second floor as the pilot climbs — the bullet-hell curve.
fn spawn_period(level: u32) -> f64 {
    (20.0 / (1.0 + 0.12 * level.saturating_sub(1) as f64)).max(6.0)
}

/// How many hostiles the system keeps in the air at once: 1 at level 1,
/// climbing two per three levels to a dozen-ship swarm.
fn pack_cap(level: u32) -> usize {
    ((1 + level * 2 / 3) as usize).min(12)
}

/// Hostiles per spawn wave — depth arrives in gangs, not single file.
fn wave_size(level: u32) -> usize {
    ((1 + level / 5) as usize).min(3)
}

/// Raiders open fire inside this range, m.
const FIRE_RANGE: f64 = 8.0e9;
/// Bolt proximity fuse, m.
const BOLT_FUSE: f64 = 5.0e8;

#[derive(Resource)]
struct RaiderClock(f64);

/// When the next dreadnought is owed (pilot level threshold).
#[derive(Resource)]
struct BossClock {
    next_level: u32,
}

/// One dot of a predicted raider path.
#[derive(Component)]
pub struct TrajDot;

/// The trajectory dot pool: spawned once, repositioned every frame.
#[derive(Resource, Default)]
struct TrajPool {
    dots: Vec<Entity>,
}

/// Prediction: steps ahead at this sim-second spacing (8 x 800 s ~ 13
/// real seconds of relative motion at warp 600).
const TRAJ_STEPS: usize = 8;
const TRAJ_STEP_SIM_S: f64 = 800.0;
const MAX_TRACKED: usize = 10;

pub struct AliensPlugin;

impl Plugin for AliensPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RaiderClock(8.0))
            .insert_resource(BossClock { next_level: 5 })
            .init_resource::<TrajPool>()
            .add_systems(
                FixedUpdate,
                (spawn_raiders, alien_ai, run_carriers, fly_interceptors, fly_bolts, fly_mines)
                    .chain(),
            )
            .add_systems(Update, project_trajectories);
    }
}

/// Keep the system stocked with raiders, harder with pilot level.
#[allow(clippy::too_many_arguments)]
fn spawn_raiders(
    mut clock: ResMut<RaiderClock>,
    mut boss_clock: ResMut<BossClock>,
    aliens: Query<(), With<AlienShip>>,
    bosses: Query<(), With<Dreadnought>>,
    ships: Query<(&SimPos, &SimVel), With<Ship>>,
    run: Res<RunScore>,
    career: Res<CareerScore>,
    game: Res<crate::GameUniverse>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    clock.0 -= DT;
    if clock.0 > 0.0 {
        return;
    }
    let level = pilot_level(run.total());
    clock.0 = spawn_period(level);
    let Ok((ship_pos, ship_vel)) = ships.single() else { return };

    // Arrive from a seed-random bearing, well outside weapons range,
    // velocity matched so the approach is deliberate, not a flyby.
    let mut rng = oj_universe::SplitMix64(
        game.current.index as u64 ^ (career.total_score + run.total()) ^ 0xA11E7,
    );
    let bearing = rng.range(0.0, std::f64::consts::TAU);
    let dist = rng.range(1.6e10, 2.4e10);
    let pos = ship_pos.0 + Vec3d::new(bearing.cos() * dist, bearing.sin() * dist, 0.0);

    // Every few pilot levels the pack yields to a DREADNOUGHT: one at a
    // time, a dozen raiders' worth of hull, volley fire, boss bounty.
    if level >= boss_clock.next_level && bosses.iter().count() == 0 {
        // Owed-boss spacing tightens with depth: every 5 levels early,
        // every 2 past level 24 — capitals become a fact of life.
        boss_clock.next_level = level + (5u32.saturating_sub(level / 8)).max(2);
        spawn_hostile(
            &mut commands,
            &mut meshes,
            &mut materials,
            pos,
            ship_vel.0,
            HostileSpec::dreadnought(level),
        );
        // A capital ship travels with a wing: two raiders on its flanks,
        // so the boss fight opens as a furball, not a duel.
        for side in [-1.0, 1.0] {
            let flank = Vec3d::new(-bearing.sin(), bearing.cos(), 0.0) * (3.0e9 * side);
            spawn_hostile(
                &mut commands,
                &mut meshes,
                &mut materials,
                pos + flank,
                ship_vel.0,
                HostileSpec::raider(level),
            );
        }
        info!(
            "DREADNOUGHT inbound with escort wing (level {level}); next owed at {}",
            boss_clock.next_level
        );
        return;
    }

    let cap = pack_cap(level);
    let live = aliens.iter().count();
    if live >= cap {
        return;
    }
    let wave = wave_size(level).min(cap - live);

    // Elites appear from level 4, more often the deeper you go; weavers
    // join from level 3 and turn the arena into a minefield; from level
    // 12 even a DREADNOUGHT can arrive as a regular spawn — the larger
    // the pilot flies, the larger what hunts them.
    let elite_chance = if level >= 4 {
        (0.10 + 0.03 * level as f64).min(0.6)
    } else {
        0.0
    };
    // Dev hook: OJ_WEAVER=1 forces every regular spawn to a weaver, so
    // the minefield path can be exercised on demand.
    let weaver_chance = if std::env::var("OJ_WEAVER").is_ok() {
        1.0
    } else if level >= 3 {
        0.20
    } else {
        0.0
    };
    let mut heavy_chance = if level >= 12 && bosses.iter().count() == 0 {
        (0.05 + 0.01 * (level - 12) as f64).min(0.15)
    } else {
        0.0
    };
    // Dev hook: OJ_CARRIER=1 forces every regular spawn to a carrier.
    let carrier_chance = if std::env::var("OJ_CARRIER").is_ok() {
        1.0
    } else if level >= 8 {
        0.10
    } else {
        0.0
    };
    for n in 0..wave {
        // Each ship in the wave gets its own bearing — a gang closing
        // from several directions, not a queue on one vector.
        let bearing = rng.range(0.0, std::f64::consts::TAU);
        let dist = rng.range(1.6e10, 2.4e10);
        let pos = ship_pos.0 + Vec3d::new(bearing.cos() * dist, bearing.sin() * dist, 0.0);
        let roll = rng.range(0.0, 1.0);
        let (spec, kind) = if roll < heavy_chance {
            // One capital per wave is plenty.
            heavy_chance = 0.0;
            (HostileSpec::dreadnought(level), "dreadnought")
        } else if roll < heavy_chance + carrier_chance {
            (HostileSpec::carrier(level), "carrier")
        } else if roll < heavy_chance + carrier_chance + elite_chance {
            (HostileSpec::elite(level), "elite")
        } else if roll < heavy_chance + carrier_chance + elite_chance + weaver_chance {
            (HostileSpec::weaver(level), "weaver")
        } else {
            (HostileSpec::raider(level), "raider")
        };
        spawn_hostile(&mut commands, &mut meshes, &mut materials, pos, ship_vel.0, spec);
        info!("{kind} inbound (level {level}, wave {wave}, pack {}/{cap})", live + n + 1);
    }
}

/// Everything that varies between a raider, an elite, a weaver and a
/// dreadnought.
struct HostileSpec {
    hp: f64,
    /// The pilot level this hostile was spawned against — its object
    /// level for the nova's shield-consumption fight.
    level: u32,
    bounty: u64,
    damage: f64,
    cooldown: f64,
    bolt_speed: f64,
    approach: f64,
    scale: f32,
    elite: bool,
    boss: bool,
    /// Seconds between mines; 0 = not a layer.
    mine_every: f64,
    /// Seconds between hatched interceptors; 0 = not a carrier.
    hatch_every: f64,
    /// Contact detonation damage; 0 = not a kamikaze.
    kamikaze: f64,
}

impl HostileSpec {
    /// Baseline raider at a pilot level — every stat climbs with it.
    fn raider(level: u32) -> Self {
        let l = level as f64;
        Self {
            hp: 30.0 * (1.0 + l * 0.15),
            level,
            bounty: 40 + level as u64 * 8,
            damage: 6.0 * (1.0 + l * 0.10),
            cooldown: (2.6 - 0.06 * l).max(1.2),
            bolt_speed: 1.2e6 * (1.0 + l * 0.03).min(2.0),
            approach: 6.0e5 * (1.0 + l * 0.02).min(1.8),
            scale: 1.0,
            elite: false,
            boss: false,
            mine_every: 0.0,
            hatch_every: 0.0,
            kamikaze: 0.0,
        }
    }

    /// The carrier: a slow flying hangar. Weak gun, huge hull, and a
    /// hatch that keeps the interceptors coming.
    fn carrier(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 6.0,
            bounty: base.bounty * 8,
            damage: base.damage * 0.8,
            cooldown: base.cooldown * 2.2,
            approach: base.approach * 0.45,
            scale: 2.6,
            hatch_every: 8.0,
            ..base
        }
    }

    /// A hatched dart: unarmed but for itself — fast, fragile, and
    /// aimed straight at the hull.
    fn interceptor(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 0.35,
            bounty: 10 + level as u64 * 2,
            damage: 0.0,
            cooldown: 1.0e9,
            approach: base.approach * 2.6,
            scale: 0.6,
            kamikaze: 14.0 * (1.0 + level as f64 * 0.08),
            ..base
        }
    }

    fn elite(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 2.5,
            bounty: base.bounty * 3,
            damage: base.damage * 1.6,
            cooldown: base.cooldown * 0.8,
            bolt_speed: base.bolt_speed * 1.15,
            approach: base.approach * 1.15,
            scale: 1.45,
            elite: true,
            boss: false,
            ..base
        }
    }

    /// The weaver: quick, fragile, barely armed — its weapon is the
    /// minefield it drags behind it.
    fn weaver(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 0.7,
            bounty: base.bounty + level as u64 * 4,
            damage: base.damage * 0.5,
            cooldown: base.cooldown * 1.8,
            approach: base.approach * 1.3,
            scale: 1.1,
            mine_every: 5.0,
            ..base
        }
    }

    fn dreadnought(level: u32) -> Self {
        let base = Self::raider(level);
        Self {
            hp: base.hp * 12.0,
            bounty: base.bounty * 15,
            damage: base.damage * 2.2,
            cooldown: base.cooldown * 1.4,
            approach: base.approach * 0.7,
            scale: 3.4,
            boss: true,
            ..base
        }
    }
}

fn spawn_hostile(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3d,
    vel: Vec3d,
    spec: HostileSpec,
) {
    let hp = spec.hp;
    let bounty = spec.bounty;
    // Elites run hot-blooded crimson; the dreadnought burns amber. A
    // pilot should read the threat tier from the silhouette color alone.
    let (hull_color, hull_glow, fin_color, fin_glow) = if spec.boss {
        (
            Color::srgb(0.85, 0.55, 0.2),
            LinearRgba::rgb(1.4, 0.7, 0.1),
            Color::srgb(0.9, 0.35, 0.2),
            LinearRgba::rgb(1.2, 0.3, 0.1),
        )
    } else if spec.elite {
        (
            Color::srgb(0.95, 0.3, 0.35),
            LinearRgba::rgb(1.5, 0.2, 0.25),
            Color::srgb(0.85, 0.5, 0.2),
            LinearRgba::rgb(1.0, 0.5, 0.1),
        )
    } else if spec.hatch_every > 0.0 {
        // Carriers run violet-slate: big, slow, and worth killing first.
        (
            Color::srgb(0.55, 0.48, 0.75),
            LinearRgba::rgb(0.7, 0.4, 1.3),
            Color::srgb(0.4, 0.35, 0.6),
            LinearRgba::rgb(0.5, 0.3, 1.0),
        )
    } else if spec.kamikaze > 0.0 {
        // Interceptors burn hot magenta: the streak IS the warning.
        (
            Color::srgb(1.0, 0.35, 0.75),
            LinearRgba::rgb(2.0, 0.4, 1.2),
            Color::srgb(0.9, 0.2, 0.5),
            LinearRgba::rgb(1.6, 0.2, 0.8),
        )
    } else if spec.mine_every > 0.0 {
        // Weavers run pale ice-blue: read the color, hunt the layer.
        (
            Color::srgb(0.65, 0.85, 1.0),
            LinearRgba::rgb(0.5, 1.0, 1.6),
            Color::srgb(0.4, 0.6, 0.95),
            LinearRgba::rgb(0.3, 0.5, 1.4),
        )
    } else {
        (
            Color::srgb(0.45, 0.9, 0.5),
            LinearRgba::rgb(0.15, 1.1, 0.3),
            Color::srgb(0.75, 0.3, 0.85),
            LinearRgba::rgb(0.7, 0.1, 0.9),
        )
    };
    let hull_mat = materials.add(StandardMaterial {
        base_color: hull_color,
        metallic: 0.35,
        emissive: hull_glow,
        ..default()
    });
    let fin_mat = materials.add(StandardMaterial {
        base_color: fin_color,
        emissive: fin_glow,
        ..default()
    });
    let core_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.2, 0.18),
        metallic: 0.65,
        perceptual_roughness: 0.6,
        ..default()
    });
    let glow_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.3, 1.0),
        emissive: LinearRgba::rgb(2.6, 0.5, 3.2),
        unlit: true,
        ..default()
    });
    let mut root = commands.spawn((
        SystemScoped,
        AlienShip {
            fire_cooldown: 3.0,
            cooldown: spec.cooldown,
            damage: spec.damage,
            bolt_speed: spec.bolt_speed,
            approach: spec.approach,
            volley: if spec.boss { 3 } else { 1 },
            // Seed the weave rhythm from where the ship arrived, so
            // every raider jinks on its own clock.
            phase: (pos.x.to_bits() ^ pos.y.to_bits()) as f64 % 6.28,
            mine_every: spec.mine_every,
            mine_cd: spec.mine_every,
        },
        Hull { hp },
        // The nova fight: this screen soaks wave punch until consumed,
        // and the spawn level is the hostile's object level.
        crate::nova::NovaShield(hp * 0.4),
        crate::nova::ObjectLevel(spec.level),
        Bounty(bounty),
        SimPos(pos),
        SimVel(vel),
        BodyVel::default(),
        // Angular strike frame: a 3-sided prism core — nothing about
        // it reads friendly or aerodynamic. Tier picks the mass.
        Mesh3d(meshes.add(Cone::new(6.5, 13.0).mesh().resolution(3))),
        MeshMaterial3d(hull_mat),
        Transform::from_scale(Vec3::splat(spec.scale)),
    ));
    if spec.elite {
        root.insert(Elite);
    }
    if spec.boss {
        root.insert(Dreadnought);
    }
    if spec.mine_every > 0.0 {
        root.insert(MineLayer);
    }
    if spec.hatch_every > 0.0 {
        root.insert(Carrier {
            hatch_every: spec.hatch_every,
            hatch_cd: spec.hatch_every * 0.5,
            level: spec.level,
        });
    }
    if spec.kamikaze > 0.0 {
        root.insert(Interceptor { damage: spec.kamikaze, ttl: 75.0 });
    }
    root.with_children(|alien| {
            // Jagged fin trio at 120-degree spacing.
            for k in 0..3 {
                let a = std::f32::consts::TAU * k as f32 / 3.0;
                alien.spawn((
                    Mesh3d(meshes.add(Cuboid::new(11.0, 2.2, 0.8).mesh())),
                    MeshMaterial3d(fin_mat.clone()),
                    Transform::from_xyz(a.cos() * 3.0, 2.0 - k as f32 * 2.4, a.sin() * 3.0)
                        .with_rotation(Quat::from_rotation_y(a) * Quat::from_rotation_z(0.45)),
                ));
            }
            // Reactor block + spike antennas + eye pods.
            alien.spawn((
                Mesh3d(meshes.add(Cuboid::new(4.2, 3.2, 3.2).mesh())),
                MeshMaterial3d(core_mat.clone()),
                Transform::from_xyz(0.0, -7.5, 0.0),
            ));
            alien.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.3, 7.0, 0.3).mesh())),
                MeshMaterial3d(core_mat),
                Transform::from_xyz(-2.0, 5.0, 1.2).with_rotation(Quat::from_rotation_z(-0.5)),
            ));
            for side in [-1.0f32, 1.0] {
                alien.spawn((
                    Mesh3d(meshes.add(Sphere::new(0.9).mesh().ico(1).unwrap())),
                    MeshMaterial3d(glow_mat.clone()),
                    Transform::from_xyz(side * 2.6, 4.6, 0.8),
                ));
            }
            // The dreadnought reads as a different CLASS, not a bigger
            // raider: dorsal spine, twin outrigger pods, extra eye row.
            if spec.boss {
                alien.spawn((
                    Mesh3d(meshes.add(Cuboid::new(1.4, 22.0, 1.4).mesh())),
                    MeshMaterial3d(fin_mat.clone()),
                    Transform::from_xyz(0.0, -4.0, 2.2),
                ));
                for side in [-1.0f32, 1.0] {
                    alien.spawn((
                        Mesh3d(meshes.add(Cuboid::new(3.0, 9.0, 3.0).mesh())),
                        MeshMaterial3d(fin_mat.clone()),
                        Transform::from_xyz(side * 7.5, -3.0, 0.0)
                            .with_rotation(Quat::from_rotation_z(side * 0.12)),
                    ));
                    alien.spawn((
                        Mesh3d(meshes.add(Sphere::new(1.1).mesh().ico(1).unwrap())),
                        MeshMaterial3d(glow_mat.clone()),
                        Transform::from_xyz(side * 4.6, -0.5, 1.4),
                    ));
                }
            }
        });
}

/// Seek the player; fire when in range; keep a fighting distance.
fn alien_ai(
    clock: Res<crate::sim::SimClock>,
    ships: Query<(&SimPos, &SimVel), (With<Ship>, Without<AlienShip>)>,
    #[allow(clippy::type_complexity)]
    mut aliens: Query<
        (
            &mut AlienShip,
            &mut SimPos,
            &mut SimVel,
            &mut BodyVel,
            &mut Transform,
            Option<&Interceptor>,
        ),
        Without<Ship>,
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok((ship_pos, ship_vel)) = ships.single() else { return };
    let dt = DT * TIME_WARP;
    for (mut alien, mut pos, mut vel, mut bvel, mut transform, dart) in &mut aliens {
        let to_ship = ship_pos.0 - pos.0;
        let dist = to_ship.length().max(1.0);
        let dir = to_ship * (1.0 / dist);

        // Close to fighting range, then circle: chase point offsets
        // sideways so raiders orbit the fight instead of ramming. The
        // closing speed is a spawn-time stat — veterans close faster.
        // The circle is not steady: each raider reverses its circling
        // direction on its own irregular clock and jinks radially, so a
        // gunner never gets a free tracking solution.
        // A kamikaze never circles: the terminal run is straight in.
        let desired = if dart.is_some() || dist > FIRE_RANGE * 0.5 {
            ship_vel.0 + dir * alien.approach
        } else {
            let weave = (clock.0 * 9.0e-4 + alien.phase).sin().signum();
            let jink = 1.0 + 0.8 * (clock.0 * 1.7e-3 + alien.phase * 2.0).sin();
            let tangent = Vec3d::new(-dir.y, dir.x, 0.0);
            ship_vel.0 + tangent * (alien.approach * 0.66 * weave) + dir * (5.0e4 * jink)
        };
        let dv = desired - vel.0;
        let a_max = 4.0e4 * TIME_WARP;
        let need = dv.length() / dt;
        let a = if need > a_max { dv.normalized() * a_max } else { dv * (1.0 / dt) };
        vel.0 += a * dt;
        pos.0 += vel.0 * dt;
        bvel.0 = vel.0;
        // Face the prey.
        let angle = (to_ship.y).atan2(to_ship.x) as f32 - std::f32::consts::FRAC_PI_2;
        transform.rotation = Quat::from_rotation_z(angle);

        // Weavers seed the fight with proximity mines while in the
        // brawl: one every few seconds, dropped in their wake.
        if alien.mine_every > 0.0 {
            alien.mine_cd -= DT;
            if alien.mine_cd <= 0.0 && dist < FIRE_RANGE * 1.4 {
                alien.mine_cd = alien.mine_every;
                commands.spawn((
                    SystemScoped,
                    SpaceMine { armed_in: 3.0, ttl: 90.0, damage: 22.0 },
                    SimPos(pos.0),
                    SimVel(vel.0 * 0.15),
                    Mesh3d(meshes.add(Sphere::new(2.6).mesh().ico(1).unwrap())),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.5, 0.4, 0.15),
                        emissive: LinearRgba::rgb(2.6, 1.4, 0.2),
                        unlit: true,
                        ..default()
                    })),
                    Transform::default(),
                    crate::fx::FlameFlicker {
                        phase: alien.phase as f32,
                        base_scale: Vec3::splat(1.0),
                    },
                    bevy::picking::Pickable::IGNORE,
                ));
                info!("weaver laid a mine");
            }
        }

        // Fire. Damage, cadence and bolt speed are the ship's own stats;
        // a dreadnought looses a spread volley instead of one bolt.
        alien.fire_cooldown -= DT;
        if alien.fire_cooldown <= 0.0 && dist < FIRE_RANGE {
            alien.fire_cooldown = alien.cooldown;
            let lead = ship_pos.0 + (ship_vel.0 - vel.0) * (dist / alien.bolt_speed);
            let aim = (lead - pos.0).normalized();
            let volley = alien.volley.max(1);
            for i in 0..volley {
                // Spread the volley in-plane: center bolt true, wings
                // angled a few degrees out.
                let spread = (i as f64 - (volley as f64 - 1.0) / 2.0) * 0.06;
                let (s, c) = spread.sin_cos();
                let aim_i =
                    Vec3d::new(aim.x * c - aim.y * s, aim.x * s + aim.y * c, aim.z).normalized();
                commands.spawn((
                    SystemScoped,
                    AlienBolt { damage: alien.damage, ttl: 25.0 },
                    SimPos(pos.0 + aim_i * 4.0e8),
                    SimVel(vel.0 + aim_i * alien.bolt_speed),
                    Mesh3d(meshes.add(Sphere::new(3.0).mesh().ico(1).unwrap())),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.9, 0.3, 1.0),
                        emissive: LinearRgba::rgb(6.0, 1.2, 8.0),
                        unlit: true,
                        ..default()
                    })),
                    Transform::default(),
                ));
            }
        }
    }
}

/// Cockpit targeting: lay each raider's predicted path out as fading
/// dots. Prediction is in the SHIP-RELATIVE frame anchored at the
/// ship's current position — where the raider will be relative to you,
/// which is the question a gunner is actually asking.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn project_trajectories(
    view: Res<ViewMode>,
    ships: Query<(&SimPos, &SimVel), (With<Ship>, Without<AlienShip>, Without<TrajDot>)>,
    aliens: Query<(&SimPos, &SimVel), (With<AlienShip>, Without<TrajDot>)>,
    mut pool: ResMut<TrajPool>,
    mut dots: Query<(&mut SimPos, &mut Visibility), With<TrajDot>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Lazily build the pool: MAX_TRACKED paths of TRAJ_STEPS dots each,
    // alpha fading with distance into the future.
    if pool.dots.is_empty() {
        let mesh = meshes.add(Sphere::new(1.6).mesh().ico(1).unwrap());
        let mats: Vec<Handle<StandardMaterial>> = (0..TRAJ_STEPS)
            .map(|i| {
                let a = 0.75 * (1.0 - i as f32 / TRAJ_STEPS as f32) + 0.1;
                materials.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 0.35, 0.9, a),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                })
            })
            .collect();
        for _ in 0..MAX_TRACKED {
            for (i, mat) in mats.iter().enumerate().take(TRAJ_STEPS) {
                let e = commands
                    .spawn((
                        TrajDot,
                        SimPos::default(),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(mat.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                    ))
                    .id();
                pool.dots.push(e);
                let _ = i;
            }
        }
        return;
    }

    let ship = ships.single().ok();
    let mut cursor = 0usize;
    if *view == ViewMode::Cockpit
        && let Some((_ship_pos, ship_vel)) = ship
    {
        for (a_pos, a_vel) in aliens.iter().take(MAX_TRACKED) {
            let rel_vel = a_vel.0 - ship_vel.0;
            for step in 0..TRAJ_STEPS {
                let Some(&dot) = pool.dots.get(cursor) else { break };
                cursor += 1;
                if let Ok((mut pos, mut vis)) = dots.get_mut(dot) {
                    let t = TRAJ_STEP_SIM_S * (step + 1) as f64;
                    pos.0 = a_pos.0 + rel_vel * t;
                    *vis = Visibility::Inherited;
                }
            }
        }
    }
    // Park the rest.
    for &dot in pool.dots.iter().skip(cursor) {
        if let Ok((_, mut vis)) = dots.get_mut(dot) {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
    }
}

/// Bolts fly straight, expire, and burst on the ship: shields first,
/// then hull — the same order the sun burns in.
#[allow(clippy::type_complexity)]
fn fly_bolts(
    mut bolts: Query<(Entity, &mut AlienBolt, &mut SimPos, &SimVel)>,
    mut ships: Query<(&mut Ship, &SimPos, &SimVel), (Without<AlienBolt>, With<crate::sim::OriginAnchor>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let dt = DT * TIME_WARP;
    for (entity, mut bolt, mut pos, vel) in &mut bolts {
        bolt.ttl -= DT;
        if bolt.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        pos.0 += vel.0 * dt;
        if let Ok((mut ship, ship_pos, ship_vel)) = ships.single_mut()
            && ship_pos.0.distance(pos.0) < BOLT_FUSE
        {
            let dmg = bolt.damage;
            // Where the bolt came from, for the directional flare.
            let strike_dir = (pos.0 - ship_pos.0).normalized();
            // The field absorbs what it can — costing the reactor — and
            // the REST burns through to the hull. A sliver of regenerated
            // shield must not eat a whole bolt.
            let absorbed = dmg.min(ship.shield);
            let through = dmg - absorbed;
            if absorbed > 0.0 {
                ship.shield -= absorbed;
                ship.energy = (ship.energy - absorbed * 0.6).max(0.0);
                sfx.write(crate::audio::Sfx::ShieldHit);
                // The force field glows on the SIDE it was struck.
                crate::fx::spawn_shield_flare(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    ship_pos.0,
                    ship_vel.0,
                    strike_dir,
                    16.0,
                );
            }
            if through > 0.0 {
                ship.hull = (ship.hull - through).max(0.0);
                sfx.write(crate::audio::Sfx::HullHit);
                // Bare hull: sparks, not glow.
                crate::fx::spawn_impact_flash(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    ship_pos.0 + strike_dir * (10.0 / crate::sim::RENDER_SCALE),
                    ship_vel.0,
                    5.0,
                    LinearRgba::rgb(6.0, 3.0, 0.8),
                );
            }
            info!(
                "bolt hit: shield {:.0}, hull {:.0}, energy {:.0}",
                ship.shield, ship.hull, ship.energy
            );
            commands.entity(entity).despawn();
        }
    }
}

/// Mines drift, arm, wait, and go off in the player's face: shields
/// soak first (costing the reactor), the rest burns the hull, and the
/// blast shoves the ship. Unarmed or expired mines just die quietly.
#[allow(clippy::type_complexity)]
fn fly_mines(
    mut mines: Query<(Entity, &mut SpaceMine, &mut SimPos, &SimVel), Without<Ship>>,
    mut ships: Query<
        (&mut Ship, &SimPos, &mut SimVel),
        (With<crate::sim::OriginAnchor>, Without<SpaceMine>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let dt = DT * TIME_WARP;
    for (entity, mut mine, mut pos, vel) in &mut mines {
        mine.armed_in -= DT;
        mine.ttl -= DT;
        if mine.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        pos.0 += vel.0 * dt;
        if mine.armed_in > 0.0 {
            continue;
        }
        let Ok((mut ship, ship_pos, mut ship_vel)) = ships.single_mut() else { continue };
        let d = ship_pos.0.distance(pos.0);
        if d > MINE_TRIGGER {
            continue;
        }
        let strike_dir = if d > 1.0 {
            (pos.0 - ship_pos.0) / d
        } else {
            Vec3d::new(1.0, 0.0, 0.0)
        };
        let absorbed = mine.damage.min(ship.shield);
        let through = mine.damage - absorbed;
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
                16.0,
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
            pos.0,
            Vec3d::ZERO,
            10.0,
        );
        ship_vel.0 += strike_dir * -1.0e4;
        info!(
            "mine detonation: shield {:.0}, hull {:.0}, energy {:.0}",
            ship.shield, ship.hull, ship.energy
        );
        commands.entity(entity).despawn();
    }
}

/// Carriers hatch interceptors on their clock, capped at three darts in
/// the air per live carrier so a surviving mothership pressures without
/// snowballing.
fn run_carriers(
    mut carriers: Query<(&mut Carrier, &SimPos, &SimVel)>,
    interceptors: Query<(), With<Interceptor>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let live_carriers = carriers.iter().count();
    if live_carriers == 0 {
        return;
    }
    let mut darts = interceptors.iter().count();
    for (mut carrier, pos, vel) in &mut carriers {
        carrier.hatch_cd -= DT;
        if carrier.hatch_cd > 0.0 || darts >= live_carriers * 3 {
            continue;
        }
        carrier.hatch_cd = carrier.hatch_every;
        darts += 1;
        let offset = Vec3d::new(
            (pos.0.x.to_bits() % 7) as f64 - 3.0,
            (pos.0.y.to_bits() % 5) as f64 - 2.0,
            0.0,
        ) * 2.0e8;
        spawn_hostile(
            &mut commands,
            &mut meshes,
            &mut materials,
            pos.0 + offset,
            vel.0,
            HostileSpec::interceptor(carrier.level),
        );
        info!("interceptor away (carrier level {})", carrier.level);
    }
}

/// The dart's whole life: tick down, close, detonate on proximity —
/// shields soak first, the hit shoves the ship, and the dart is spent
/// either way.
#[allow(clippy::type_complexity)]
fn fly_interceptors(
    mut darts: Query<(Entity, &mut Interceptor, &SimPos), Without<Ship>>,
    mut ships: Query<
        (&mut Ship, &SimPos, &mut SimVel),
        (With<crate::sim::OriginAnchor>, Without<Interceptor>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    for (entity, mut dart, pos) in &mut darts {
        dart.ttl -= DT;
        if dart.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let Ok((mut ship, ship_pos, mut ship_vel)) = ships.single_mut() else { continue };
        let d = ship_pos.0.distance(pos.0);
        if d > INTERCEPT_TRIGGER {
            continue;
        }
        let strike_dir = if d > 1.0 {
            (pos.0 - ship_pos.0) / d
        } else {
            Vec3d::new(1.0, 0.0, 0.0)
        };
        let absorbed = dart.damage.min(ship.shield);
        let through = dart.damage - absorbed;
        sfx.write(crate::audio::Sfx::Explosion);
        if absorbed > 0.0 {
            ship.shield -= absorbed;
            sfx.write(crate::audio::Sfx::ShieldHit);
        }
        if through > 0.0 {
            ship.hull = (ship.hull - through).max(0.0);
            sfx.write(crate::audio::Sfx::HullHit);
        }
        crate::fx::spawn_explosion(
            &mut commands,
            &mut meshes,
            &mut materials,
            pos.0,
            Vec3d::ZERO,
            9.0,
        );
        ship_vel.0 += strike_dir * -1.2e4;
        info!(
            "interceptor strike: shield {:.0}, hull {:.0}",
            ship.shield, ship.hull
        );
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::*;

    /// The bullet-hell contract: cadence tightens to a floor, packs grow
    /// to a hard cap, waves thicken — all monotone in pilot level, so a
    /// higher level never means a quieter sky.
    #[test]
    fn bullet_hell_curve_scales_with_level() {
        assert!(spawn_period(1) > spawn_period(5));
        assert!(spawn_period(5) > spawn_period(12));
        assert!((spawn_period(1) - 20.0).abs() < 1e-9, "level 1 keeps the gentle cadence");
        assert!((spawn_period(60) - 6.0).abs() < 1e-9, "floor holds");

        assert_eq!(pack_cap(1), 1, "level 1 still faces one raider");
        assert!(pack_cap(6) > pack_cap(3));
        assert_eq!(pack_cap(40), 12, "swarm cap holds");

        assert_eq!(wave_size(1), 1);
        assert_eq!(wave_size(10), 3);
        assert_eq!(wave_size(50), 3);
    }

    /// The carrier fight's shape: a mothership worth eight raiders that
    /// barely shoots, hatching darts that never shoot at all — their
    /// warhead is the airframe, and it grows with level.
    #[test]
    fn carrier_and_interceptor_specs_hold() {
        let c = HostileSpec::carrier(8);
        let r = HostileSpec::raider(8);
        assert!(c.hp > r.hp * 5.0 && c.approach < r.approach);
        assert!(c.hatch_every > 0.0 && c.kamikaze == 0.0);
        let i = HostileSpec::interceptor(8);
        assert!(i.hp < r.hp && i.approach > r.approach * 2.0);
        assert!(i.damage == 0.0, "darts never fire bolts");
        assert!(HostileSpec::interceptor(20).kamikaze > i.kamikaze);
    }
}
