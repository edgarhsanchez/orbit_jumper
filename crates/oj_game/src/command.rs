//! The orbit command — the game's core traversal verb.
//!
//! Click and HOLD a celestial body: if the ship is within command range
//! (upgradeable), a hold timer fills and the ship begins a guided transfer
//! into orbit around the target. Propulsion is deliberately weak against
//! interplanetary distances, so getting anywhere means hitching rides:
//! orbit a planet and it carries you around the system; dive past one in
//! free flight and its gravity — real, integrated — slings you out faster.
//! Assists are detected and scored.

use bevy::picking::events::{Pointer, Press, Release};
use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::modules::RunScore;
use crate::sim::{BodyVel, CelestialBody, DT, OrbitRing, Ship, TIME_WARP, orbit_rings};
use crate::{SimPos, SimVel};

/// Seconds of hold to issue an orbit command.
const HOLD_SECONDS: f64 = 1.2;

/// Ship navigation mode.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub enum NavState {
    /// Manual flight; gravity and arrow thrust only.
    #[default]
    Free,
    /// Guided burn toward a circular orbit of `target` at `ride_r`.
    Transfer { target: Entity, ride_r: f64 },
    /// Captured; light station-keeping holds the orbit while the body
    /// carries the ship around the system — the hitched ride.
    Orbiting { body: Entity, ride_r: f64 },
}

/// The in-progress click-and-hold, if any.
#[derive(Resource, Default)]
pub struct CommandHold {
    pub target: Option<Entity>,
    /// The ride orbit being commanded, sim meters (the clicked ring's
    /// radius, or the innermost ring for a click on the body itself).
    pub ride_r: f64,
    pub progress: f64,
    pub out_of_range: bool,
}

/// Slingshot detection state.
#[derive(Resource, Default)]
pub struct AssistTracker {
    in_soi_of: Option<Entity>,
    entry_speed: f64,
}

pub struct CommandPlugin;

impl Plugin for CommandPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandHold>()
            .init_resource::<AssistTracker>()
            .add_observer(on_press)
            .add_observer(on_release)
            .add_systems(
                FixedUpdate,
                (tick_hold, guide_nav, track_assists).chain(),
            );
    }
}

fn on_press(
    ev: On<Pointer<Press>>,
    rings: Query<&OrbitRing>,
    celestials: Query<&CelestialBody>,
    mut hold: ResMut<CommandHold>,
) {
    // A press on a ring commands THAT orbit; a press on the body itself
    // commands its innermost ring.
    let picked = if let Ok(ring) = rings.get(ev.entity) {
        Some((ring.body, ring.ride_r))
    } else {
        celestials
            .get(ev.entity)
            .ok()
            .map(|b| (ev.entity, orbit_rings(b.radius, b.soi)[0]))
    };
    if let Some((target, ride_r)) = picked {
        hold.target = Some(target);
        hold.ride_r = ride_r;
        hold.progress = 0.0;
        hold.out_of_range = false;
    }
}

fn on_release(_ev: On<Pointer<Release>>, mut hold: ResMut<CommandHold>) {
    hold.target = None;
    hold.progress = 0.0;
}

fn tick_hold(
    mut hold: ResMut<CommandHold>,
    bodies: Query<(&CelestialBody, &SimPos)>,
    mut ships: Query<(&Ship, &SimPos, &mut NavState)>,
) {
    let Some(target) = hold.target else { return };
    let (Ok((_, body_pos)), Ok((ship, ship_pos, mut nav))) =
        (bodies.get(target), ships.single_mut())
    else {
        hold.target = None;
        return;
    };
    // Range is measured to the ORBIT, not the body's center: a giant's
    // outer ring can pass close enough to leap onto while the body
    // itself sits far outside command range.
    let d = ship_pos.0.distance(body_pos.0);
    let to_ring = (d - hold.ride_r).abs().min(d);
    if to_ring > ship.command_range {
        hold.out_of_range = true;
        hold.progress = 0.0;
        return;
    }
    hold.out_of_range = false;
    hold.progress += DT / HOLD_SECONDS;
    if hold.progress >= 1.0 {
        *nav = NavState::Transfer { target, ride_r: hold.ride_r };
        hold.target = None;
        hold.progress = 0.0;
    }
}

fn guide_nav(
    keys: Res<ButtonInput<KeyCode>>,
    bodies: Query<(&CelestialBody, &SimPos, &BodyVel)>,
    mut ships: Query<(&mut Ship, &SimPos, &mut SimVel, &mut NavState)>,
) {
    let Ok((mut ship, pos, mut vel, mut nav)) = ships.single_mut() else { return };
    // Manual thrust overrides guidance — the pilot is always in charge.
    if keys.any_pressed([
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::KeyE,
        KeyCode::KeyQ,
    ]) {
        *nav = NavState::Free;
        return;
    }
    let (target, ride_r, station_keeping) = match *nav {
        NavState::Free => return,
        NavState::Transfer { target, ride_r } => (target, ride_r, false),
        NavState::Orbiting { body, ride_r } => (body, ride_r, true),
    };
    let Ok((body, body_pos, body_vel)) = bodies.get(target) else {
        *nav = NavState::Free;
        return;
    };
    let dt = DT * TIME_WARP;
    let rel_pos = pos.0 - body_pos.0;
    let rel_vel = vel.0 - body_vel.0;
    let r = rel_pos.length().max(body.radius);

    // The commanded ring, clamped never inside the body and never
    // outside its influence.
    let r_target = ride_r.max(body.radius * 1.2).min(body.soi * 0.9);
    let v_circ = (body.mu / r).sqrt();
    let r_hat = rel_pos.normalized();
    // In-plane tangent; degenerate (radial) approaches pick any normal.
    let h = rel_pos.cross(rel_vel);
    let t_hat = if h.length() > 1e-6 {
        h.cross(rel_pos).normalized() * -1.0
    } else {
        Vec3d::new(-r_hat.y, r_hat.x, 0.0).normalized()
    };
    // Bigger bodies mean faster rides for free (v = sqrt(mu/r)); the
    // gravity drive multiplies further. Overspeeding a circular orbit
    // needs continuous inward correction, so the energy price below is
    // the real centripetal deficit, not a fee schedule.
    let v_ride = v_circ * if station_keeping { ship.orbit_boost } else { 1.0 };
    // Approach rate: close the radial gap in ~this much SIM time whatever
    // the scale — a leap between rings must feel like seconds, not
    // orbital-mechanics hours (measured: a v_circ-capped descent took
    // minutes of real time). The gap shrinking slows the approach
    // naturally, so arrival is smooth; burns still price energy.
    const TRANSFER_CLOSE_SIM_S: f64 = 6000.0;
    let v_des = t_hat * v_ride + r_hat * ((r_target - r) / TRANSFER_CLOSE_SIM_S);

    let dv = v_des - rel_vel;
    let a_max = ship.thrust * TIME_WARP * if station_keeping { 0.5 } else { 4.0 };
    let need = dv.length() / dt;
    let a = if need > a_max { dv.normalized() * a_max } else { dv / dt };

    // Guided burns cost energy in proportion to effort; an empty tank
    // drops the ship back to free flight mid-transfer. Plan your energy.
    let cost = a.length() / (ship.thrust * TIME_WARP) * 2.0 * DT;
    if ship.energy < cost {
        *nav = NavState::Free;
        return;
    }
    ship.energy -= cost;
    vel.0 += a * dt;

    if !station_keeping {
        let radius_ok = (r - r_target).abs() / r_target < 0.15;
        let speed_ok = (rel_vel.length() - v_circ).abs() / v_circ < 0.15;
        if radius_ok && speed_ok {
            *nav = NavState::Orbiting { body: target, ride_r };
        }
    }
}

/// A slingshot is: enter a planet's sphere of influence in free flight,
/// leave it faster (sun-frame) without having thrust. Score it.
fn track_assists(
    keys: Res<ButtonInput<KeyCode>>,
    mut tracker: ResMut<AssistTracker>,
    mut run: ResMut<RunScore>,
    bodies: Query<(Entity, &CelestialBody, &SimPos)>,
    ships: Query<(&SimPos, &SimVel, &NavState)>,
) {
    let Ok((pos, vel, nav)) = ships.single() else {
        tracker.in_soi_of = None;
        return;
    };
    let thrusting = keys.any_pressed([
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::KeyE,
        KeyCode::KeyQ,
    ]);
    let speed = vel.0.length();
    // Deepest finite-SOI body containing the ship (planets, not the sun).
    let containing = bodies
        .iter()
        .filter(|(_, b, bp)| b.soi.is_finite() && pos.0.distance(bp.0) < b.soi)
        .min_by(|a, b| {
            let da = pos.0.distance(a.2.0) / a.1.soi;
            let db = pos.0.distance(b.2.0) / b.1.soi;
            da.partial_cmp(&db).unwrap()
        })
        .map(|(e, ..)| e);

    match (tracker.in_soi_of, containing) {
        (None, Some(e)) if *nav == NavState::Free && !thrusting => {
            tracker.in_soi_of = Some(e);
            tracker.entry_speed = speed;
        }
        (Some(_), Some(_)) if thrusting || *nav != NavState::Free => {
            // Thrusting inside the SOI voids the assist — that's a burn,
            // not a slingshot.
            tracker.in_soi_of = None;
        }
        (Some(_), None) => {
            if speed - tracker.entry_speed > 300.0 {
                run.assists += 1;
            }
            tracker.in_soi_of = None;
        }
        _ => {}
    }
}
