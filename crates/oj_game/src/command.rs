//! The orbit command — the game's core traversal verb.
//!
//! Click a ride ring (or the body itself, for its innermost ring): if
//! the ring is within command range (upgradeable), the ship begins a
//! guided transfer into that orbit immediately — one click, one command.
//! Propulsion is deliberately weak against interplanetary distances, so
//! getting anywhere means hitching rides: orbit a planet and it carries
//! you around the system; dive past one in free flight and its gravity —
//! real, integrated — slings you out faster. Assists are detected and
//! scored.

use bevy::picking::events::{Pointer, Press, Release};
use bevy::prelude::*;
use oj_orbits::Vec3d;

use crate::modules::RunScore;
use crate::sim::{BodyVel, CelestialBody, DT, OrbitRing, Ship, TIME_WARP, orbit_rings};
use crate::{SimPos, SimVel};

/// Ship navigation mode.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub enum NavState {
    /// Manual flight; gravity and arrow thrust only.
    #[default]
    Free,
    /// Guided burn toward a circular orbit of `target` at `ride_r`.
    Transfer { target: Entity, ride_r: f64 },
    /// Captured; light station-keeping holds the orbit while the body
    /// carries the ship around the system — the hitched ride. The orbit
    /// is STICKY: it ends only by commanding another ride or [O] EXIT.
    /// `speed` is a signed multiple of circular velocity — positive is
    /// counterclockwise, negative clockwise; the only thing the stick
    /// and arrows steer while riding.
    Orbiting { body: Entity, ride_r: f64, speed: f64 },
}

/// Highest ride-speed multiple the station-keeper will hold.
const ORBIT_SPEED_MAX: f64 = 3.0;
/// Ride-speed change per real second at full stick.
const ORBIT_SPEED_RATE: f64 = 0.9;
/// Riding an orbit recharges the tank, real seconds.
const ORBIT_ENERGY_REGEN: f64 = 1.5;

/// Feedback state for the last press on a ring that could NOT be
/// commanded — the HUD shows "out of range" until the button lifts.
#[derive(Resource, Default)]
pub struct CommandHold {
    pub target: Option<Entity>,
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
            .add_systems(FixedUpdate, (guide_nav, track_assists).chain());
    }
}

fn on_press(
    ev: On<Pointer<Press>>,
    rings: Query<&OrbitRing>,
    celestials: Query<&CelestialBody>,
    body_pos: Query<&SimPos, (With<CelestialBody>, Without<Ship>)>,
    mut ships: Query<(&Ship, &SimPos, &mut NavState), With<Ship>>,
    mut hold: ResMut<CommandHold>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    // Every press, in the log: the cheapest possible probe of whether the
    // picking pipeline is delivering events at all (it has silently died
    // once already — this line is how it gets caught next time).
    info!("pointer press on {:?}", ev.entity);
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
    // Presses that miss every pickable land on the window entity and
    // resolve to nothing — normal, not an error.
    let Some((target, ride_r)) = picked else { return };
    let Ok((ship, ship_pos, mut nav)) = ships.single_mut() else { return };
    // Pressing the ring you already ride is a no-op.
    if matches!(
        *nav,
        NavState::Orbiting { body, ride_r: r, .. } if body == target && r == ride_r
    ) {
        return;
    }
    let Ok(bp) = body_pos.get(target) else { return };
    // Range is measured to the ORBIT, not the body's center: a giant's
    // outer ring can pass close enough to leap onto while the body
    // itself sits far outside command range.
    let d = ship_pos.0.distance(bp.0);
    let to_ring = (d - ride_r).abs().min(d);
    if to_ring <= ship.command_range {
        // One click, one command: the transfer starts NOW.
        *nav = NavState::Transfer { target, ride_r };
        hold.target = None;
        hold.out_of_range = false;
        sfx.write(crate::audio::Sfx::Click);
        info!("orbit commanded: transfer to ride_r {ride_r:.3e}");
    } else {
        // Out of reach: let the HUD say so until the button lifts.
        hold.target = Some(target);
        hold.out_of_range = true;
        info!("orbit out of range: {:.3e} > {:.3e}", to_ring, ship.command_range);
    }
}

fn on_release(_ev: On<Pointer<Release>>, mut hold: ResMut<CommandHold>) {
    hold.target = None;
    hold.out_of_range = false;
}

fn guide_nav(
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    bodies: Query<(&CelestialBody, &SimPos, &BodyVel)>,
    mut ships: Query<(&mut Ship, &SimPos, &mut SimVel, &mut NavState)>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let Ok((mut ship, pos, mut vel, mut nav)) = ships.single_mut() else { return };
    let manual = joy.active
        || keys.any_pressed([
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::KeyE,
            KeyCode::KeyQ,
        ]);
    // Manual thrust cancels a TRANSFER — the pilot is always in charge
    // of a burn. An ORBIT is sticky: the same inputs steer the ride
    // instead, and only [O] (or commanding another orbit) releases it.
    if manual && matches!(*nav, NavState::Transfer { .. }) {
        *nav = NavState::Free;
        return;
    }
    if keys.just_pressed(KeyCode::KeyO) && matches!(*nav, NavState::Orbiting { .. }) {
        *nav = NavState::Free;
        return;
    }
    let (target, ride_r, orbit_speed) = match *nav {
        NavState::Free => return,
        NavState::Transfer { target, ride_r } => (target, ride_r, None),
        NavState::Orbiting { body, ride_r, speed } => (body, ride_r, Some(speed)),
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
    // Deterministic in-plane CCW tangent: the SIGN of the ride speed
    // picks the direction, so reversing through zero flips cleanly.
    let t_ccw = Vec3d::new(-r_hat.y, r_hat.x, 0.0).normalized();
    // Approach rate: close the radial gap in ~this much SIM time whatever
    // the scale — a leap between rings must feel like seconds, not
    // orbital-mechanics hours (measured: a v_circ-capped descent took
    // minutes of real time). The gap shrinking slows the approach
    // naturally, so arrival is smooth; burns still price energy.
    const TRANSFER_CLOSE_SIM_S: f64 = 6000.0;

    let v_des = if let Some(speed) = orbit_speed {
        // While riding, the stick and arrows are an orbital throttle:
        // left/right (and stick x) steer CCW/CW absolutely, up/down
        // (and stick y) push along the current direction of travel.
        let absolute = (keys.pressed(KeyCode::ArrowLeft) as i32
            - keys.pressed(KeyCode::ArrowRight) as i32) as f64
            - joy.vec.x as f64;
        let current = if speed >= 0.0 { 1.0 } else { -1.0 };
        let along = (keys.pressed(KeyCode::ArrowUp) as i32
            - keys.pressed(KeyCode::ArrowDown) as i32) as f64
            + joy.vec.y as f64;
        let input = (absolute + along * current).clamp(-1.0, 1.0);
        let new_speed = (speed + input * ORBIT_SPEED_RATE * DT)
            .clamp(-ORBIT_SPEED_MAX, ORBIT_SPEED_MAX);
        *nav = NavState::Orbiting { body: target, ride_r, speed: new_speed };
        // Riding is the recharge state: the ring does the work while
        // the collectors bank energy.
        ship.energy = (ship.energy + ORBIT_ENERGY_REGEN * DT).min(ship.energy_max);
        t_ccw * (v_circ * new_speed) + r_hat * ((r_target - r) / TRANSFER_CLOSE_SIM_S)
    } else {
        // Transfer: burn along the current sense of rotation toward
        // circular speed at the commanded ring.
        let h = rel_pos.cross(rel_vel);
        let t_hat = if h.length() > 1e-6 {
            h.cross(rel_pos).normalized() * -1.0
        } else {
            t_ccw
        };
        t_hat * v_circ + r_hat * ((r_target - r) / TRANSFER_CLOSE_SIM_S)
    };

    let station_keeping = orbit_speed.is_some();
    let dv = v_des - rel_vel;
    let a_max = ship.thrust * TIME_WARP * if station_keeping { 0.5 } else { 4.0 };
    let need = dv.length() / dt;
    let a = if need > a_max { dv.normalized() * a_max } else { dv / dt };

    if station_keeping {
        // The ride itself is free — that is the whole point of hitching.
        vel.0 += a * dt;
    } else {
        // Guided burns cost energy in proportion to effort; an empty
        // tank drops the ship back to free flight mid-transfer.
        let cost = a.length() / (ship.thrust * TIME_WARP) * 2.0 * DT;
        if ship.energy < cost {
            *nav = NavState::Free;
            return;
        }
        ship.energy -= cost;
        vel.0 += a * dt;

        let radius_ok = (r - r_target).abs() / r_target < 0.15;
        let speed_ok = (rel_vel.length() - v_circ).abs() / v_circ < 0.15;
        if radius_ok && speed_ok {
            // Capture keeps the arrival's sense of rotation; the gravity
            // drive's boost is the starting ride speed.
            let h = rel_pos.cross(rel_vel);
            let dir = if h.z >= 0.0 { 1.0 } else { -1.0 };
            *nav = NavState::Orbiting {
                body: target,
                ride_r,
                speed: ship.orbit_boost * dir,
            };
            sfx.write(crate::audio::Sfx::OrbitLock);
        }
    }
}

/// A slingshot is: enter a planet's sphere of influence in free flight,
/// leave it faster (sun-frame) without having thrust. Score it.
fn track_assists(
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    mut tracker: ResMut<AssistTracker>,
    mut run: ResMut<RunScore>,
    bodies: Query<(Entity, &CelestialBody, &SimPos)>,
    ships: Query<(&SimPos, &SimVel, &NavState)>,
) {
    let Ok((pos, vel, nav)) = ships.single() else {
        tracker.in_soi_of = None;
        return;
    };
    let thrusting = joy.active
        || keys.any_pressed([
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
