//! Deterministic infinite universe.
//!
//! Nothing is stored and nothing is random at runtime: every galaxy, system,
//! sun, and planet is a pure function of (universe seed, coordinates). Any
//! two machines with the same seed derive the same universe — the property
//! the whole multiplayer design leans on, because peers only ever need to
//! exchange DYNAMIC state (ships, wreckage, projectiles), never the map.
//!
//! Hierarchy: infinite i64 sector grid -> some sectors hold a galaxy ->
//! a galaxy holds systems -> a system holds one sun and its planets, with
//! real orbital elements from `oj_orbits`.
//!
//! SplitMix64 is the hash: tiny, well-distributed, and the same generator
//! the bevy_pf_vector benchmark suite standardized on.

use oj_orbits::{KeplerOrbit, Vec3d};

fn crate_g() -> f64 {
    oj_orbits::G
}
use serde::{Deserialize, Serialize};

/// SplitMix64: seed -> stream of well-mixed u64s.
#[derive(Debug, Clone)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo)
    }
    pub fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next_u64() % (hi - lo) as u64) as u32
    }
}

/// Mix coordinates into a child seed. Order-dependent on purpose.
fn mix(seed: u64, parts: &[u64]) -> u64 {
    let mut s = SplitMix64(seed);
    let mut acc = s.next_u64();
    for &p in parts {
        let mut m = SplitMix64(acc ^ p);
        acc = m.next_u64();
    }
    acc
}

/// Address of a sector in the infinite grid. One sector is a cube of
/// [`SECTOR_SIZE`] meters; most are empty dark space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SectorCoord {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

/// 10^18 m (~100 ly) per sector edge.
pub const SECTOR_SIZE: f64 = 1.0e18;

/// One galaxy in ~[`GALAXY_DENSITY`] sectors.
pub const GALAXY_DENSITY: u64 = 8;

/// Stable id of a solar system: (sector, galaxy-local index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SystemId {
    pub sector: SectorCoord,
    pub index: u32,
}

/// Spectral class of a sun, ordered cool -> hostile. Class decides the
/// energy available in orbit, the shielding needed to survive it, and how
/// hard it is to study from afar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SunClass {
    /// Red dwarf: dim, gentle, a beginner's charging station.
    M,
    K,
    G,
    F,
    A,
    B,
    /// Blue giant: enormous output, brutal radiation.
    O,
    /// Collapsed exotic: extreme gravity well, lethal emission bursts.
    NeutronStar,
    /// Magnetar: worst periodic damage in the game.
    Magnetar,
    /// No light to harvest, but a gravity assist like nothing else.
    BlackHole,
}

impl SunClass {
    fn roll(rng: &mut SplitMix64) -> Self {
        // Rough main-sequence frequencies with a thin exotic tail.
        match rng.next_u64() % 1000 {
            0..=550 => Self::M,
            551..=720 => Self::K,
            721..=830 => Self::G,
            831..=890 => Self::F,
            891..=940 => Self::A,
            941..=975 => Self::B,
            976..=990 => Self::O,
            991..=995 => Self::NeutronStar,
            996..=997 => Self::Magnetar,
            _ => Self::BlackHole,
        }
    }

    /// Shield tier required to hold a close orbit without hull damage.
    pub fn required_shield_tier(self) -> u8 {
        match self {
            Self::M => 0,
            Self::K => 1,
            Self::G => 2,
            Self::F => 3,
            Self::A => 4,
            Self::B => 5,
            Self::O => 6,
            Self::NeutronStar => 7,
            Self::Magnetar => 8,
            Self::BlackHole => 6, // no radiation, but see `tidal_hazard`
        }
    }

    /// Periodic damage per second at the reference close-orbit radius for a
    /// ship with insufficient shielding. Scales with proximity in-sim.
    pub fn hazard_dps(self) -> f64 {
        match self {
            Self::M => 1.0,
            Self::K => 2.0,
            Self::G => 4.0,
            Self::F => 7.0,
            Self::A => 12.0,
            Self::B => 20.0,
            Self::O => 35.0,
            Self::NeutronStar => 60.0,
            Self::Magnetar => 120.0,
            Self::BlackHole => 0.0,
        }
    }

    /// Energy harvest rate multiplier while in orbit.
    pub fn harvest_rate(self) -> f64 {
        match self {
            Self::M => 0.6,
            Self::K => 0.8,
            Self::G => 1.0,
            Self::F => 1.3,
            Self::A => 1.7,
            Self::B => 2.4,
            Self::O => 3.5,
            Self::NeutronStar => 5.0,
            Self::Magnetar => 8.0,
            Self::BlackHole => 0.0,
        }
    }

    /// Seconds of `study` needed before the class is revealed. Exotics
    /// disguise themselves: a magnetar reads like a B star until studied.
    pub fn study_seconds(self) -> f64 {
        match self {
            Self::M | Self::K | Self::G => 3.0,
            Self::F | Self::A => 5.0,
            Self::B | Self::O => 8.0,
            Self::NeutronStar | Self::Magnetar => 14.0,
            Self::BlackHole => 10.0,
        }
    }

    /// Black holes shred by tide instead of radiation.
    pub fn tidal_hazard(self) -> bool {
        matches!(self, Self::BlackHole)
    }
}

/// What a planet's crust is rich in. Drives the material economy: upgrades
/// need materials, materials need mining/salvage in the right systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceProfile {
    Ferrous,
    Silicate,
    Icy,
    Carbonaceous,
    Radioactive,
    ExoticCrystal,
}

impl ResourceProfile {
    fn roll(rng: &mut SplitMix64) -> Self {
        match rng.next_u64() % 100 {
            0..=29 => Self::Silicate,
            30..=54 => Self::Ferrous,
            55..=74 => Self::Icy,
            75..=89 => Self::Carbonaceous,
            90..=96 => Self::Radioactive,
            _ => Self::ExoticCrystal,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sun {
    pub class: SunClass,
    /// kg.
    pub mass: f64,
    /// m.
    pub radius: f64,
    /// Radius inside which hazard damage applies, m.
    pub hazard_radius: f64,
}

/// A moon: a body on rails around its planet (mu = G * planet mass).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Moon {
    /// kg.
    pub mass: f64,
    /// m.
    pub radius: f64,
    /// Orbit around the PLANET, not the sun.
    pub orbit: KeplerOrbit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Planet {
    /// kg.
    pub mass: f64,
    /// m.
    pub radius: f64,
    pub orbit: KeplerOrbit,
    pub resources: ResourceProfile,
    /// Ring/asteroid density in [0,1]: how much salvageable debris and
    /// ambient rock the spawner scatters around this planet.
    pub debris_density: f64,
    pub moons: Vec<Moon>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolarSystem {
    pub id: SystemId,
    /// Galaxy-local position, m.
    pub position: Vec3d,
    pub sun: Sun,
    pub planets: Vec<Planet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Galaxy {
    pub sector: SectorCoord,
    pub system_count: u32,
}

/// The universe: a seed. Everything else is derived.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Universe {
    pub seed: u64,
}

impl Universe {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// Does this sector hold a galaxy?
    pub fn galaxy_at(&self, sector: SectorCoord) -> Option<Galaxy> {
        let h = mix(self.seed, &[sector.x as u64, sector.y as u64, sector.z as u64, 0x6A1A_17ED]);
        if h % GALAXY_DENSITY != 0 {
            return None;
        }
        let mut rng = SplitMix64(h);
        Some(Galaxy {
            sector,
            system_count: rng.range_u32(40, 400),
        })
    }

    /// Derive one solar system of a galaxy.
    pub fn system(&self, id: SystemId) -> Option<SolarSystem> {
        let galaxy = self.galaxy_at(id.sector)?;
        if id.index >= galaxy.system_count {
            return None;
        }
        let h = mix(self.seed, &[
            id.sector.x as u64,
            id.sector.y as u64,
            id.sector.z as u64,
            0x5157EA5 ^ id.index as u64,
        ]);
        let mut rng = SplitMix64(h);

        // Scatter systems in a flattened disc inside the sector.
        let position = Vec3d::new(
            rng.range(0.05, 0.95) * SECTOR_SIZE,
            rng.range(0.05, 0.95) * SECTOR_SIZE,
            rng.range(0.45, 0.55) * SECTOR_SIZE,
        );

        let class = SunClass::roll(&mut rng);
        let sun_mass = match class {
            SunClass::M => rng.range(0.1, 0.5),
            SunClass::K => rng.range(0.5, 0.8),
            SunClass::G => rng.range(0.8, 1.1),
            SunClass::F => rng.range(1.1, 1.5),
            SunClass::A => rng.range(1.5, 2.5),
            SunClass::B => rng.range(2.5, 12.0),
            SunClass::O => rng.range(12.0, 50.0),
            SunClass::NeutronStar => rng.range(1.2, 2.1),
            SunClass::Magnetar => rng.range(1.2, 2.1),
            SunClass::BlackHole => rng.range(4.0, 40.0),
        } * 1.989e30;
        let sun_radius = match class {
            SunClass::NeutronStar | SunClass::Magnetar => 1.2e4,
            SunClass::BlackHole => 2.0 * oj_orbits::G * sun_mass / (2.998e8f64).powi(2),
            _ => rng.range(0.4, 8.0) * 6.96e8,
        };
        let sun = Sun {
            class,
            mass: sun_mass,
            radius: sun_radius,
            hazard_radius: (sun_radius * rng.range(20.0, 60.0)).max(5.0e9),
        };

        let mu = oj_orbits::G * sun_mass;
        let planet_count = rng.range_u32(0, 11);
        let mut planets = Vec::with_capacity(planet_count as usize);
        // Roughly geometric orbit spacing (Titius-Bode-flavored).
        let mut r = rng.range(0.3, 0.7) * 1.496e11;
        for _ in 0..planet_count {
            r *= rng.range(1.4, 2.1);
            let mass = rng.range(1e23, 2e27);
            let orbit = KeplerOrbit {
                mu,
                semi_major: r,
                eccentricity: rng.range(0.0, 0.15),
                inclination: rng.range(-0.06, 0.06),
                raan: rng.range(0.0, std::f64::consts::TAU),
                arg_periapsis: rng.range(0.0, std::f64::consts::TAU),
                mean_anomaly_epoch: rng.range(0.0, std::f64::consts::TAU),
            };
            let radius = (mass / 5500.0 * 3.0 / (4.0 * std::f64::consts::PI)).cbrt();
            let planet_mu = crate_g() * mass;
            let moon_count = rng.range_u32(0, 4);
            let mut moons = Vec::with_capacity(moon_count as usize);
            let mut moon_r = radius * rng.range(6.0, 12.0);
            for _ in 0..moon_count {
                moon_r *= rng.range(1.6, 2.4);
                let moon_mass = mass * rng.range(1e-4, 5e-3);
                moons.push(Moon {
                    mass: moon_mass,
                    radius: (moon_mass / 3300.0 * 3.0 / (4.0 * std::f64::consts::PI)).cbrt(),
                    orbit: KeplerOrbit {
                        mu: planet_mu,
                        semi_major: moon_r,
                        eccentricity: rng.range(0.0, 0.08),
                        inclination: rng.range(-0.1, 0.1),
                        raan: rng.range(0.0, std::f64::consts::TAU),
                        arg_periapsis: rng.range(0.0, std::f64::consts::TAU),
                        mean_anomaly_epoch: rng.range(0.0, std::f64::consts::TAU),
                    },
                });
            }
            planets.push(Planet {
                mass,
                radius,
                orbit,
                resources: ResourceProfile::roll(&mut rng),
                debris_density: rng.next_f64() * rng.next_f64(), // skew low
                moons,
            });
        }

        Some(SolarSystem { id, sun, planets, position })
    }

    /// All systems of the galaxy in `sector` (lazy caller-side; count first).
    pub fn systems_in(&self, sector: SectorCoord) -> impl Iterator<Item = SolarSystem> + '_ {
        let count = self.galaxy_at(sector).map(|g| g.system_count).unwrap_or(0);
        (0..count).filter_map(move |index| self.system(SystemId { sector, index }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some_galaxy_sector(u: &Universe) -> SectorCoord {
        for x in 0..64 {
            let s = SectorCoord { x, y: 0, z: 0 };
            if u.galaxy_at(s).is_some() {
                return s;
            }
        }
        panic!("no galaxy in 64 sectors — density broken");
    }

    #[test]
    fn universe_is_deterministic() {
        let a = Universe::new(42);
        let b = Universe::new(42);
        let sector = some_galaxy_sector(&a);
        let id = SystemId { sector, index: 0 };
        let (sa, sb) = (a.system(id).unwrap(), b.system(id).unwrap());
        assert_eq!(format!("{sa:?}"), format!("{sb:?}"));
    }

    #[test]
    fn different_seeds_differ() {
        let a = Universe::new(1);
        let b = Universe::new(2);
        let sector = some_galaxy_sector(&a);
        if b.galaxy_at(sector).is_some() {
            let id = SystemId { sector, index: 0 };
            let (sa, sb) = (a.system(id), b.system(id));
            assert_ne!(format!("{sa:?}"), format!("{sb:?}"));
        }
    }

    #[test]
    fn galaxy_density_is_sane() {
        let u = Universe::new(7);
        let mut hits = 0;
        for x in 0..1000 {
            if u.galaxy_at(SectorCoord { x, y: 3, z: -2 }).is_some() {
                hits += 1;
            }
        }
        // Expect ~1/8; allow wide slack.
        assert!(hits > 60 && hits < 260, "density off: {hits}/1000");
    }

    #[test]
    fn planets_orbit_inside_the_system() {
        let u = Universe::new(1234);
        let sector = some_galaxy_sector(&u);
        let mut checked = 0;
        for sys in u.systems_in(sector).take(20) {
            for p in &sys.planets {
                assert!(p.orbit.eccentricity < 1.0);
                assert!(p.orbit.semi_major > sys.sun.radius);
                // A planet's year is a real Kepler period.
                assert!(p.orbit.period() > 0.0);
                for m in &p.moons {
                    // Moons orbit outside their planet, well inside its SOI.
                    assert!(m.orbit.semi_major > p.radius);
                    let soi = oj_orbits::sphere_of_influence(
                        p.orbit.semi_major, p.mass, sys.sun.mass);
                    assert!(m.orbit.semi_major < soi,
                        "moon at {} outside SOI {}", m.orbit.semi_major, soi);
                    assert!(m.orbit.period() > 0.0);
                }
                checked += 1;
            }
        }
        assert!(checked > 0, "no planets generated at all");
    }

    #[test]
    fn out_of_range_index_is_none() {
        let u = Universe::new(9);
        let sector = some_galaxy_sector(&u);
        let g = u.galaxy_at(sector).unwrap();
        assert!(u.system(SystemId { sector, index: g.system_count }).is_none());
    }
}
