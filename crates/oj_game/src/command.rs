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
    /// Guided burn toward the CLICKED POINT on a ring: `aim` is the
    /// angle of that point in the body's frame, `bias` the route side
    /// the flight planner chose around obstacles.
    Transfer { target: Entity, ride_r: f64, aim: f64, bias: f64 },
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
/// Approach rate: close the gap in ~this much SIM time whatever the
/// scale — a leap between rings must feel like seconds, not
/// orbital-mechanics hours (measured: a v_circ-capped descent took
/// minutes of real time). The gap shrinking slows the approach
/// naturally, so arrival is smooth; burns still price energy.
const TRANSFER_CLOSE_SIM_S: f64 = 6000.0;

/// Feedback state for the last press on a ring that could NOT be
/// commanded — the HUD says why until the button lifts.
#[derive(Resource, Default)]
pub struct CommandHold {
    pub target: Option<Entity>,
    pub out_of_range: bool,
    pub no_energy: bool,
}

/// A frozen view of one celestial for the flight planner.
#[derive(Clone, Copy)]
pub struct BodySnap {
    pub mu: f64,
    pub radius: f64,
    pub pos: Vec3d,
    pub vel: Vec3d,
    pub is_target: bool,
}

/// Sim ticks each route candidate is flown forward.
const PLAN_TICKS: u32 = 2600;
/// Planner work per frame — small enough that the progress bar is a
/// visible moment, large enough that a plan lands in under a second.
const PLAN_TICKS_PER_FRAME: u32 = 90;
/// A plan must leave this fraction of the tank untouched.
const PLAN_ENERGY_MARGIN: f64 = 0.85;

/// An in-flight flight-plan calculation: the guided transfer flown
/// forward with the SAME steering law the executor uses — the plan is
/// the flight. Three route candidates (straight, bend left, bend right)
/// race; the cheapest clean arrival wins, which is where gravity
/// assists get picked up for free: the candidate that falls past a body
/// spends less.
pub struct PlanJob {
    pub target: Entity,
    pub ride_r: f64,
    pub aim: f64,
    pub progress: f64,
    bodies: Vec<BodySnap>,
    start_bodies: Vec<BodySnap>,
    candidates: [f64; 3],
    results: [Option<f64>; 3],
    current: usize,
    pos: Vec3d,
    vel: Vec3d,
    spent: f64,
    tick: u32,
    start_pos: Vec3d,
    start_vel: Vec3d,
    thrust: f64,
    energy: f64,
}

#[derive(Resource, Default)]
pub struct FlightPlanner(pub Option<PlanJob>);

/// Standing per-ring reachability: can the tank buy a transfer there
/// RIGHT NOW? Refreshed one ring per tick with a coarse forward flight;
/// unreachable rings render gray and refuse the click. Permissive on
/// uncertainty — the full planner still has the final word.
#[derive(Resource, Default)]
pub struct RingReach {
    pub flags: bevy::platform::collections::HashMap<Entity, bool>,
    cursor: usize,
}

/// Coarse feasibility flight for one ring per tick: the same steering
/// law at 4x time steps, direct route only, energy-bounded.
fn assess_rings(
    mut reach: ResMut<RingReach>,
    rings: Query<(Entity, &OrbitRing)>,
    celestials: Query<(Entity, &CelestialBody, &SimPos, &BodyVel)>,
    ships: Query<(&Ship, &SimPos, &SimVel), With<Ship>>,
) {
    let Ok((ship, ship_pos, ship_vel)) = ships.single() else { return };
    let n = rings.iter().count();
    if n == 0 {
        return;
    }
    reach.cursor = (reach.cursor + 1) % n;
    let Some((ring_entity, ring)) = rings.iter().nth(reach.cursor) else { return };
    let Ok((te, tb, tp, tv)) = celestials.get(ring.body) else { return };
    let mut snaps: Vec<BodySnap> = celestials
        .iter()
        .map(|(e, b, p, v)| BodySnap {
            mu: b.mu,
            radius: b.radius,
            pos: p.0,
            vel: v.0,
            is_target: e == te,
        })
        .collect();
    let _ = (tb, tv);
    let Some(ti) = snaps.iter().position(|b| b.is_target) else { return };
    let rel = ship_pos.0 - tp.0;
    let aim = rel.y.atan2(rel.x);
    let dt = DT * TIME_WARP * 4.0;
    let mut pos = ship_pos.0;
    let mut vel = ship_vel.0;
    let mut spent = 0.0;
    let budget = ship.energy * PLAN_ENERGY_MARGIN;
    for _ in 0..300 {
        // The sky drifts here too, or the sim hovers at a false
        // equilibrium against a frozen planet.
        for b in &mut snaps {
            b.pos += b.vel * dt;
        }
        let target = snaps[ti];
        let v_des = transfer_v_des(pos, &snaps, &target, ring.ride_r, aim, 0.0);
        let dv = v_des - vel;
        let a_max = ship.thrust * TIME_WARP * 4.0;
        let need = dv.length() / dt;
        let a = if need > a_max { dv.normalized() * a_max } else { dv * (1.0 / dt) };
        spent += a.length() / (ship.thrust * TIME_WARP) * 2.0 * (DT * 4.0);
        if spent > budget {
            break;
        }
        vel += a * dt;
        for b in &snaps {
            vel += oj_orbits::gravity_accel(b.mu, b.pos, pos, b.radius) * dt;
        }
        pos += vel * dt;
        let r_now = (pos - target.pos).length();
        let v_circ = (target.mu / ring.ride_r).sqrt();
        let rel_v = (vel - target.vel).length();
        if (r_now - ring.ride_r).abs() / ring.ride_r < 0.2
            && (rel_v - v_circ).abs() / v_circ < 0.2
        {
            break;
        }
    }
    reach.flags.insert(ring_entity, spent <= budget);
}

/// The transfer's desired velocity, shared VERBATIM by the planner and
/// the executor: pursue the clicked point, bend away from every body en
/// route (`bias` picks the side), settle tangentially at the end.
fn transfer_v_des(
    pos: Vec3d,
    bodies: &[BodySnap],
    target: &BodySnap,
    ride_r: f64,
    aim: f64,
    bias: f64,
) -> Vec3d {
    let aim_point = target.pos + Vec3d::new(aim.cos(), aim.sin(), 0.0) * ride_r;
    let rel_pos = pos - target.pos;
    let r = rel_pos.length().max(target.radius);
    let v_circ = (target.mu / ride_r).sqrt();
    let to_point = aim_point - pos;
    let d_point = to_point.length().max(1.0);

    // Every body bends the course away from itself; near-misses cost
    // lateral speed, collisions are what the planner rejects outright.
    // The TARGET only guards its surface — its repulsion must not fence
    // off its own rings.
    let mut avoid = Vec3d::ZERO;
    for b in &bodies[..] {
        if b.is_target && d_point < ride_r * 2.0 {
            continue;
        }
        let away = pos - b.pos;
        let dist = away.length().max(1.0);
        let danger = if b.is_target { b.radius * 1.5 } else { b.radius * 3.0 };
        if dist < danger * 3.0 {
            let s = (danger / dist).min(1.0);
            let lateral = Vec3d::new(-away.y, away.x, 0.0).normalized() * bias;
            avoid += (away * (1.0 / dist) + lateral * 0.6) * (v_circ * 1.5 * s * s);
        }
    }

    let near_ring = (r - ride_r).abs() < ride_r * 0.35;
    if !near_ring {
        // Transit: fly AT the clicked point in the body's frame. Twice
        // the closing rate of the terminal phase, so a cross-system leap
        // still lands inside the plan budget; never slower than
        // near-circular speed so gravity keeps mattering.
        let pace = (d_point / (TRANSFER_CLOSE_SIM_S * 0.5)).max(v_circ * 0.9);
        target.vel + to_point * (pace / d_point) + avoid
    } else {
        // Terminal: settle onto the ring at circular speed, circling
        // TOWARD the clicked point the short way around — the click
        // decides the direction the ride begins in.
        let r_hat = rel_pos.normalized();
        let theta = rel_pos.y.atan2(rel_pos.x);
        let mut delta = aim - theta;
        while delta > std::f64::consts::PI {
            delta -= std::f64::consts::TAU;
        }
        while delta < -std::f64::consts::PI {
            delta += std::f64::consts::TAU;
        }
        let sense = if delta >= 0.0 { 1.0 } else { -1.0 };
        let t_hat = Vec3d::new(-r_hat.y * sense, r_hat.x * sense, 0.0);
        target.vel + t_hat * v_circ + r_hat * ((ride_r - r) / TRANSFER_CLOSE_SIM_S) + avoid
    }
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
            .init_resource::<FlightPlanner>()
            .init_resource::<RingReach>()
            .add_observer(on_press)
            .add_observer(on_release)
            .add_systems(
                FixedUpdate,
                (assess_rings, step_planner, guide_nav, track_assists).chain(),
            );
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn on_press(
    mut ev: On<Pointer<Press>>,
    rings: Query<&OrbitRing>,
    celestials: Query<(Entity, &CelestialBody, &SimPos, &BodyVel)>,
    transforms: Query<&GlobalTransform>,
    reach: Res<RingReach>,
    mut planner: ResMut<FlightPlanner>,
    ships: Query<(&Ship, &SimPos, &SimVel, &NavState), With<Ship>>,
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
        // Rings are CHILDREN of their body and picking events bubble up
        // the hierarchy — without this, the same press re-fires on the
        // body and overwrites this plan with one for the innermost ring
        // (the "every click flies to the orbit closest to the sun" bug).
        ev.propagate(false);
        Some((ring.body, ring.ride_r))
    } else {
        celestials
            .get(ev.entity)
            .ok()
            .map(|(_, b, _, _)| (ev.entity, orbit_rings(b.radius, b.soi)[0]))
    };
    // Presses that miss every pickable land on the window entity and
    // resolve to nothing — normal, not an error.
    let Some((target, ride_r)) = picked else { return };
    let Ok((ship, ship_pos, ship_vel, nav)) = ships.single() else { return };
    // Pressing the ring you already ride is a no-op.
    if matches!(
        *nav,
        NavState::Orbiting { body, ride_r: r, .. } if body == target && r == ride_r
    ) {
        return;
    }
    let Ok((_, _, bp, _)) = celestials.get(target) else { return };
    // Grayed-out ring: the standing assessment says the tank cannot buy
    // this transfer. Say so instead of planning a doomed flight.
    if reach.flags.get(&ev.entity).copied() == Some(false) {
        hold.target = Some(target);
        hold.no_energy = true;
        sfx.write(crate::audio::Sfx::Warning);
        info!("orbit rejected: insufficient energy for transfer");
        return;
    }
    // Range is measured to the ORBIT, not the body's center: a giant's
    // outer ring can pass close enough to leap onto while the body
    // itself sits far outside command range.
    let d = ship_pos.0.distance(bp.0);
    let to_ring = (d - ride_r).abs().min(d);
    if to_ring > ship.command_range {
        hold.target = Some(target);
        hold.out_of_range = true;
        info!("orbit out of range: {:.3e} > {:.3e}", to_ring, ship.command_range);
        return;
    }
    // WHERE the ring was clicked picks the point the ship flies to —
    // and with it the direction everything happens in. The hit is in
    // render space; render axes are sim axes, so the angle carries over.
    let aim = ev
        .hit
        .position
        .and_then(|hit| transforms.get(target).ok().map(|tf| hit - tf.translation()))
        .map(|rel| (rel.y as f64).atan2(rel.x as f64))
        .unwrap_or_else(|| {
            let rel = ship_pos.0 - bp.0;
            rel.y.atan2(rel.x)
        });
    // Freeze the sky and hand the request to the flight planner: three
    // route candidates flown forward before any burn is committed.
    let bodies: Vec<BodySnap> = celestials
        .iter()
        .map(|(e, b, p, v)| BodySnap {
            mu: b.mu,
            radius: b.radius,
            pos: p.0,
            vel: v.0,
            is_target: e == target,
        })
        .collect();
    planner.0 = Some(PlanJob {
        target,
        ride_r,
        aim,
        progress: 0.0,
        start_bodies: bodies.clone(),
        bodies,
        candidates: [0.0, 1.0, -1.0],
        results: [None; 3],
        current: 0,
        pos: ship_pos.0,
        vel: ship_vel.0,
        spent: 0.0,
        tick: 0,
        start_pos: ship_pos.0,
        start_vel: ship_vel.0,
        thrust: ship.thrust,
        energy: ship.energy,
    });
    hold.target = None;
    hold.out_of_range = false;
    hold.no_energy = false;
    sfx.write(crate::audio::Sfx::Click);
    info!("flight plan requested: ride_r {ride_r:.3e}, aim {aim:.2}");
}

fn on_release(_ev: On<Pointer<Release>>, mut hold: ResMut<CommandHold>) {
    hold.target = None;
    hold.out_of_range = false;
    hold.no_energy = false;
}

/// Advance the flight-plan calculation a slice per tick; commit the
/// winning route (or reject the request) when every candidate has flown.
fn step_planner(
    mut planner: ResMut<FlightPlanner>,
    mut ships: Query<(&Ship, &mut NavState)>,
    mut flash: ResMut<crate::achievements::LastUnlock>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let Some(job) = planner.0.as_mut() else { return };
    let Ok((ship, mut nav)) = ships.single_mut() else {
        planner.0 = None;
        return;
    };
    let dt = DT * TIME_WARP;
    if !job.bodies.iter().any(|b| b.is_target) {
        planner.0 = None;
        return;
    }
    let mut budget = PLAN_TICKS_PER_FRAME;
    while budget > 0 && job.current < 3 {
        budget -= 1;
        job.tick += 1;
        // The sky moves while the plan flies: drift every body along its
        // frozen velocity, or the commanded frame walks away from a
        // stationary planet and the sim hovers forever at the
        // equilibrium (found the hard way: r stuck at 1.35x ride_r).
        for b in &mut job.bodies {
            b.pos += b.vel * dt;
        }
        let target = *job.bodies.iter().find(|b| b.is_target).unwrap();
        let bias = job.candidates[job.current];
        let v_des =
            transfer_v_des(job.pos, &job.bodies, &target, job.ride_r, job.aim, bias);
        let dv = v_des - job.vel;
        let a_max = job.thrust * TIME_WARP * 4.0;
        let need = dv.length() / dt;
        let a = if need > a_max { dv.normalized() * a_max } else { dv * (1.0 / dt) };
        job.spent += a.length() / (job.thrust * TIME_WARP) * 2.0 * DT;
        job.vel += a * dt;
        // Real gravity: this is where a candidate that falls past a body
        // picks up speed the tank never paid for — the slingshot term.
        for b in &job.bodies {
            job.vel += oj_orbits::gravity_accel(b.mu, b.pos, job.pos, b.radius) * dt;
        }
        job.pos += job.vel * dt;

        // Collision, exhaustion, arrival, or timeout end the candidate.
        let hit = job
            .bodies
            .iter()
            .any(|b| !b.is_target && (job.pos - b.pos).length() < b.radius * 1.3);
        let broke = job.spent > job.energy;
        let rel = job.pos - target.pos;
        let v_circ = (target.mu / job.ride_r).sqrt();
        let rel_v = (job.vel - target.vel).length();
        let arrived = (rel.length() - job.ride_r).abs() / job.ride_r < 0.15
            && (rel_v - v_circ).abs() / v_circ < 0.15;
        if arrived || hit || broke || job.tick >= PLAN_TICKS {
            info!(
                "plan candidate {bias:+.0}: arrived={arrived} hit={hit} broke={broke} tick={} spent={:.1} r={:.3e}/{:.3e} rel_v={:.3e}/{:.3e}",
                job.tick,
                job.spent,
                rel.length(),
                job.ride_r,
                rel_v,
                v_circ,
            );
            if arrived && !hit && !broke {
                job.results[job.current] = Some(job.spent);
            }
            job.current += 1;
            job.tick = 0;
            job.spent = 0.0;
            job.pos = job.start_pos;
            job.vel = job.start_vel;
            job.bodies = job.start_bodies.clone();
        }
    }
    job.progress = ((job.current as f64 * PLAN_TICKS as f64 + job.tick as f64)
        / (3.0 * PLAN_TICKS as f64))
        .min(1.0);
    if job.current < 3 {
        return;
    }
    // Every candidate flown: cheapest clean arrival wins.
    let best = job
        .candidates
        .iter()
        .zip(job.results.iter())
        .filter_map(|(bias, r)| r.map(|energy| (*bias, energy)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    match best {
        Some((bias, energy)) if energy <= ship.energy * PLAN_ENERGY_MARGIN => {
            *nav = NavState::Transfer {
                target: job.target,
                ride_r: job.ride_r,
                aim: job.aim,
                bias,
            };
            sfx.write(crate::audio::Sfx::OrbitLock);
            info!("flight plan committed: bias {bias:+.0}, cost {energy:.1} energy");
        }
        _ => {
            flash.text = "NO VIABLE FLIGHT PLAN — INSUFFICIENT ENERGY".into();
            flash.ttl = 4.0;
            sfx.write(crate::audio::Sfx::Warning);
            info!("flight plan rejected: no candidate within energy margin");
        }
    }
    planner.0 = None;
}

#[allow(clippy::type_complexity)]
fn guide_nav(
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    mut planner: ResMut<FlightPlanner>,
    bodies: Query<(Entity, &CelestialBody, &SimPos, &BodyVel)>,
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
    // Manual thrust cancels a TRANSFER and any plan still computing —
    // the pilot is always in charge of a burn. An ORBIT is sticky: the
    // same inputs steer the ride instead, and only [O] (or commanding
    // another orbit) releases it.
    if manual && planner.0.is_some() {
        planner.0 = None;
    }
    if manual && matches!(*nav, NavState::Transfer { .. }) {
        *nav = NavState::Free;
        return;
    }
    if keys.just_pressed(KeyCode::KeyO) && matches!(*nav, NavState::Orbiting { .. }) {
        *nav = NavState::Free;
        return;
    }
    let (target, ride_r, orbit_speed, aim, bias) = match *nav {
        NavState::Free => return,
        NavState::Transfer { target, ride_r, aim, bias } => (target, ride_r, None, aim, bias),
        NavState::Orbiting { body, ride_r, speed } => (body, ride_r, Some(speed), 0.0, 0.0),
    };
    let Ok((_, body, body_pos, body_vel)) = bodies.get(target) else {
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
        // Transfer: the SAME steering law the flight planner flew —
        // pursue the clicked point, bend around bodies on the chosen
        // side, settle tangentially. The plan is the flight.
        let snaps: Vec<BodySnap> = bodies
            .iter()
            .map(|(e, b, p, v)| BodySnap {
                mu: b.mu,
                radius: b.radius,
                pos: p.0,
                vel: v.0,
                is_target: e == target,
            })
            .collect();
        let target_snap = BodySnap {
            mu: body.mu,
            radius: body.radius,
            pos: body_pos.0,
            vel: body_vel.0,
            is_target: true,
        };
        transfer_v_des(pos.0, &snaps, &target_snap, r_target, aim, bias) - body_vel.0
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

        let v_circ_ride = (body.mu / r_target).sqrt();
        let radius_ok = (r - r_target).abs() / r_target < 0.15;
        let speed_ok = (rel_vel.length() - v_circ_ride).abs() / v_circ_ride < 0.15;
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
