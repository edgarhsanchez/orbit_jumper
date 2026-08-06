//! Orbital mechanics for orbit_jumper.
//!
//! Two-tier physics, chosen for "realistic yet performant":
//!
//! - **Celestial bodies ride rails**: closed-form Kepler propagation from
//!   orbital elements. Deterministic (any peer computes the same universe at
//!   the same tick — the foundation the P2P design leans on), O(1) per body
//!   per query, and exactly the "real orbits" WPF of physics: ellipses obey
//!   Kepler's laws because they ARE Kepler orbits.
//! - **Ships integrate**: semi-implicit (symplectic) Euler under the
//!   dominant body's gravity plus whatever the game adds (thrust,
//!   force-field weapons). Patched-conic sphere-of-influence decides which
//!   body dominates.
//!
//! Everything is `f64`: a solar system is ~1e12 m across and `f32` runs out
//! of millimeters at ~1e7 m. Rendering re-origins to `f32` around the
//! camera; simulation never leaves `f64`.

use serde::{Deserialize, Serialize};

/// Gravitational constant, m^3 kg^-1 s^-2.
pub const G: f64 = 6.674e-11;

/// Minimal 3D f64 vector. Deliberately not glam: the sim is f64-only and
/// this crate stays decoupled from any engine's math-crate version.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec3d {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3d {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }
    pub fn normalized(self) -> Self {
        let l = self.length();
        if l == 0.0 { Self::ZERO } else { self / l }
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn distance(self, o: Self) -> f64 {
        (self - o).length()
    }
}

impl std::ops::Add for Vec3d {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3d {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f64> for Vec3d {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}
impl std::ops::Div<f64> for Vec3d {
    type Output = Self;
    fn div(self, s: f64) -> Self {
        Self::new(self.x / s, self.y / s, self.z / s)
    }
}
impl std::ops::Neg for Vec3d {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}
impl std::ops::AddAssign for Vec3d {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

/// Classical orbital elements of a closed (elliptic) orbit around a parent
/// body, propagated on rails.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KeplerOrbit {
    /// Parent's standard gravitational parameter mu = G * M, m^3/s^2.
    pub mu: f64,
    /// Semi-major axis, m. Must be positive (elliptic only).
    pub semi_major: f64,
    /// Eccentricity in [0, 1).
    pub eccentricity: f64,
    /// Inclination, rad.
    pub inclination: f64,
    /// Right ascension of the ascending node, rad.
    pub raan: f64,
    /// Argument of periapsis, rad.
    pub arg_periapsis: f64,
    /// Mean anomaly at t = 0, rad.
    pub mean_anomaly_epoch: f64,
}

impl KeplerOrbit {
    /// A circular orbit of radius `r` in the parent's equatorial plane.
    pub fn circular(mu: f64, r: f64, phase: f64) -> Self {
        Self {
            mu,
            semi_major: r,
            eccentricity: 0.0,
            inclination: 0.0,
            raan: 0.0,
            arg_periapsis: 0.0,
            mean_anomaly_epoch: phase,
        }
    }

    /// Orbital period, s (Kepler's third law).
    pub fn period(&self) -> f64 {
        std::f64::consts::TAU * (self.semi_major.powi(3) / self.mu).sqrt()
    }

    /// Mean motion, rad/s.
    pub fn mean_motion(&self) -> f64 {
        (self.mu / self.semi_major.powi(3)).sqrt()
    }

    /// Solve Kepler's equation M = E - e sin E for the eccentric anomaly by
    /// Newton iteration. Converges in a handful of steps for e < 0.99.
    fn eccentric_anomaly(&self, mean_anomaly: f64) -> f64 {
        let m = mean_anomaly.rem_euclid(std::f64::consts::TAU);
        let e = self.eccentricity;
        // Standard starter: M for near-circular, pi for eccentric.
        let mut ea = if e < 0.8 { m } else { std::f64::consts::PI };
        for _ in 0..12 {
            let f = ea - e * ea.sin() - m;
            let d = 1.0 - e * ea.cos();
            let step = f / d;
            ea -= step;
            if step.abs() < 1e-12 {
                break;
            }
        }
        ea
    }

    /// Position and velocity in the parent's inertial frame at time `t`.
    pub fn state_at(&self, t: f64) -> (Vec3d, Vec3d) {
        let e = self.eccentricity;
        let a = self.semi_major;
        let m = self.mean_anomaly_epoch + self.mean_motion() * t;
        let ea = self.eccentric_anomaly(m);
        let (sin_ea, cos_ea) = ea.sin_cos();

        // Perifocal frame: x toward periapsis.
        let b = a * (1.0 - e * e).sqrt();
        let px = a * (cos_ea - e);
        let py = b * sin_ea;
        let r = a * (1.0 - e * cos_ea);
        // dE/dt = n a / r; v = d/dE (perifocal position) * dE/dt.
        let v_scale = self.mean_motion() * a * a / r;
        let vx = -v_scale * sin_ea;
        let vy = v_scale * (1.0 - e * e).sqrt() * cos_ea;

        // Rotate perifocal -> inertial: Rz(raan) * Rx(inc) * Rz(argp).
        let rot = |x: f64, y: f64| -> Vec3d {
            let (sw, cw) = self.arg_periapsis.sin_cos();
            let (si, ci) = self.inclination.sin_cos();
            let (so, co) = self.raan.sin_cos();
            let (x1, y1) = (cw * x - sw * y, sw * x + cw * y);
            let (y2, z2) = (ci * y1, si * y1);
            Vec3d::new(co * x1 - so * y2, so * x1 + co * y2, z2)
        };
        (rot(px, py), rot(vx, vy))
    }

    /// Recover elements from a state vector (elliptic case). Returns `None`
    /// for non-elliptic (escape) trajectories — the caller decides what an
    /// escaping ship does.
    pub fn from_state(mu: f64, pos: Vec3d, vel: Vec3d) -> Option<Self> {
        let r = pos.length();
        if r == 0.0 {
            return None;
        }
        let v2 = vel.length_squared();
        let energy = v2 / 2.0 - mu / r;
        if energy >= 0.0 {
            return None; // parabolic/hyperbolic
        }
        let a = -mu / (2.0 * energy);
        let h = pos.cross(vel);
        let e_vec = vel.cross(h) / mu - pos / r;
        let e = e_vec.length();
        if e >= 1.0 {
            return None;
        }
        let inc = (h.z / h.length()).acos();
        let n = Vec3d::new(-h.y, h.x, 0.0); // node vector
        let raan = if n.length() < 1e-12 {
            0.0
        } else {
            let mut o = (n.x / n.length()).acos();
            if n.y < 0.0 {
                o = std::f64::consts::TAU - o;
            }
            o
        };
        let argp = if n.length() < 1e-12 || e < 1e-12 {
            0.0
        } else {
            let mut w = (n.dot(e_vec) / (n.length() * e)).clamp(-1.0, 1.0).acos();
            if e_vec.z < 0.0 {
                w = std::f64::consts::TAU - w;
            }
            w
        };
        // True anomaly -> eccentric -> mean, so epoch phase is preserved.
        let nu = if e < 1e-12 {
            let refv = if n.length() < 1e-12 { Vec3d::new(1.0, 0.0, 0.0) } else { n.normalized() };
            let mut t = (refv.dot(pos) / r).clamp(-1.0, 1.0).acos();
            if refv.cross(pos).dot(h) < 0.0 {
                t = std::f64::consts::TAU - t;
            }
            t
        } else {
            let mut t = (e_vec.dot(pos) / (e * r)).clamp(-1.0, 1.0).acos();
            if pos.dot(vel) < 0.0 {
                t = std::f64::consts::TAU - t;
            }
            t
        };
        let ea = 2.0 * ((nu / 2.0).tan() * ((1.0 - e) / (1.0 + e)).sqrt()).atan();
        let m0 = ea - e * ea.sin();
        Some(Self {
            mu,
            semi_major: a,
            eccentricity: e,
            inclination: inc,
            raan,
            arg_periapsis: argp,
            mean_anomaly_epoch: m0,
        })
    }
}

/// Speed of a circular orbit at radius `r`.
pub fn circular_speed(mu: f64, r: f64) -> f64 {
    (mu / r).sqrt()
}

/// Two-body gravitational acceleration exerted at `pos` by a body of
/// parameter `mu` at `body_pos`. Softened at `min_r` so weapon-made
/// singularities cannot produce infinite kicks.
pub fn gravity_accel(mu: f64, body_pos: Vec3d, pos: Vec3d, min_r: f64) -> Vec3d {
    let d = body_pos - pos;
    let r2 = d.length_squared().max(min_r * min_r);
    d.normalized() * (mu / r2)
}

/// Sphere-of-influence radius of a body of mass `m` orbiting a parent of
/// mass `parent_m` at distance `a` (Laplace approximation).
pub fn sphere_of_influence(a: f64, m: f64, parent_m: f64) -> f64 {
    a * (m / parent_m).powf(0.4)
}

/// One semi-implicit Euler step: velocity first, then position. Symplectic,
/// so orbital energy stays bounded over long integrations instead of
/// spiraling — the property that makes cheap integration usable here.
pub fn integrate_step(pos: &mut Vec3d, vel: &mut Vec3d, accel: Vec3d, dt: f64) {
    *vel += accel * dt;
    *pos += *vel * dt;
}

/// Delta-v for a Hohmann transfer between circular orbits r1 -> r2 around
/// `mu`. Returns (burn at r1, burn at r2). The energy price of jumping
/// between orbits — the game's core resource question.
pub fn hohmann_dv(mu: f64, r1: f64, r2: f64) -> (f64, f64) {
    let v1 = circular_speed(mu, r1);
    let v2 = circular_speed(mu, r2);
    let a_t = (r1 + r2) / 2.0;
    let vt1 = (mu * (2.0 / r1 - 1.0 / a_t)).sqrt();
    let vt2 = (mu * (2.0 / r2 - 1.0 / a_t)).sqrt();
    ((vt1 - v1).abs(), (v2 - vt2).abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    const EARTH_MU: f64 = 3.986004418e14;
    const EARTH_R: f64 = 6.371e6;

    #[test]
    fn circular_orbit_stays_circular() {
        let r = EARTH_R + 400e3; // ISS-ish
        let orbit = KeplerOrbit::circular(EARTH_MU, r, 0.0);
        for i in 0..100 {
            let (pos, vel) = orbit.state_at(i as f64 * 60.0);
            assert!((pos.length() - r).abs() < 1.0, "radius drifted: {}", pos.length() - r);
            assert!((vel.length() - circular_speed(EARTH_MU, r)).abs() < 0.01);
        }
    }

    #[test]
    fn period_matches_iss() {
        let orbit = KeplerOrbit::circular(EARTH_MU, EARTH_R + 400e3, 0.0);
        let minutes = orbit.period() / 60.0;
        assert!((minutes - 92.0).abs() < 1.5, "ISS period ~92.5 min, got {minutes}");
    }

    #[test]
    fn eccentric_orbit_respects_apsides() {
        let orbit = KeplerOrbit {
            mu: EARTH_MU,
            semi_major: 26_600e3, // Molniya-like
            eccentricity: 0.74,
            inclination: 1.1,
            raan: 0.5,
            arg_periapsis: 4.7,
            mean_anomaly_epoch: 0.0,
        };
        let peri = orbit.semi_major * (1.0 - orbit.eccentricity);
        let apo = orbit.semi_major * (1.0 + orbit.eccentricity);
        let mut min_r = f64::MAX;
        let mut max_r = 0.0f64;
        let p = orbit.period();
        for i in 0..2000 {
            let (pos, _) = orbit.state_at(p * i as f64 / 2000.0);
            min_r = min_r.min(pos.length());
            max_r = max_r.max(pos.length());
        }
        assert!((min_r - peri).abs() / peri < 1e-3);
        assert!((max_r - apo).abs() / apo < 1e-3);
    }

    #[test]
    fn elements_roundtrip_through_state() {
        let orbit = KeplerOrbit {
            mu: EARTH_MU,
            semi_major: 8.0e6,
            eccentricity: 0.3,
            inclination: 0.7,
            raan: 1.2,
            arg_periapsis: 2.1,
            mean_anomaly_epoch: 0.9,
        };
        let (pos, vel) = orbit.state_at(0.0);
        let rec = KeplerOrbit::from_state(EARTH_MU, pos, vel).expect("elliptic");
        assert!((rec.semi_major - orbit.semi_major).abs() / orbit.semi_major < 1e-9);
        assert!((rec.eccentricity - orbit.eccentricity).abs() < 1e-9);
        assert!((rec.inclination - orbit.inclination).abs() < 1e-9);
        // The recovered elements must reproduce the same state.
        let (rpos, rvel) = rec.state_at(0.0);
        assert!(rpos.distance(pos) < 1.0);
        assert!(rvel.distance(vel) < 1e-3);
    }

    #[test]
    fn escape_velocity_is_not_an_orbit() {
        let r = 7.0e6;
        let v_escape = (2.0 * EARTH_MU / r).sqrt();
        let out = KeplerOrbit::from_state(
            EARTH_MU,
            Vec3d::new(r, 0.0, 0.0),
            Vec3d::new(0.0, v_escape * 1.01, 0.0),
        );
        assert!(out.is_none());
    }

    #[test]
    fn symplectic_integration_holds_energy() {
        let r = 8.0e6;
        let mut pos = Vec3d::new(r, 0.0, 0.0);
        let mut vel = Vec3d::new(0.0, circular_speed(EARTH_MU, r), 0.0);
        let e0 = vel.length_squared() / 2.0 - EARTH_MU / pos.length();
        // Ten orbits at a 1 s step.
        let period = std::f64::consts::TAU * (r.powi(3) / EARTH_MU).sqrt();
        let steps = (10.0 * period) as usize;
        for _ in 0..steps {
            let a = gravity_accel(EARTH_MU, Vec3d::ZERO, pos, 1.0);
            integrate_step(&mut pos, &mut vel, a, 1.0);
        }
        let e1 = vel.length_squared() / 2.0 - EARTH_MU / pos.length();
        assert!((e1 - e0).abs() / e0.abs() < 1e-3, "energy drift {}", (e1 - e0) / e0);
    }

    #[test]
    fn hohmann_leo_to_geo_is_about_3_9_km_s() {
        let (dv1, dv2) = hohmann_dv(EARTH_MU, EARTH_R + 300e3, 42_164e3);
        let total = dv1 + dv2;
        assert!((total - 3900.0).abs() < 100.0, "expected ~3.9 km/s, got {total}");
    }
}
