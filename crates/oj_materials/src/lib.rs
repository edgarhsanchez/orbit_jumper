//! Material science: the economy under every upgrade.
//!
//! Planets and wreckage yield ELEMENTS (keyed by the system's
//! `ResourceProfile`). Elements combine into ALLOYS whose properties are a
//! weighted blend plus a discovery bonus. UPGRADES gate on alloy property
//! thresholds plus quantities — so "better shields" is literally "find or
//! salvage something with higher thermal resistance", and a magnetar-grade
//! shield exists only if somebody engineered the alloy for it.

use serde::{Deserialize, Serialize};

/// The physical properties the game cares about, each roughly [0, 10].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Properties {
    /// Survives heat/radiation — shield and sun-orbit gating.
    pub thermal_resistance: f64,
    /// Structural strength — hull integrity per unit mass.
    pub tensile_strength: f64,
    /// Moves energy — weapon output and charge rate.
    pub energy_conductivity: f64,
    /// kg per unit — everything costs delta-v to push around.
    pub density: f64,
    /// Responds to gravity manipulation — force-field tech gating.
    pub graviton_affinity: f64,
}

impl Properties {
    fn blend(a: Self, b: Self, w: f64) -> Self {
        let l = |x: f64, y: f64| x * (1.0 - w) + y * w;
        Self {
            thermal_resistance: l(a.thermal_resistance, b.thermal_resistance),
            tensile_strength: l(a.tensile_strength, b.tensile_strength),
            energy_conductivity: l(a.energy_conductivity, b.energy_conductivity),
            density: l(a.density, b.density),
            graviton_affinity: l(a.graviton_affinity, b.graviton_affinity),
        }
    }
}

/// Base elements. Which ones a planet yields follows its resource profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Element {
    Iron,
    Titanium,
    Silicon,
    Carbon,
    Ice,
    Uranium,
    /// Exotic-crystal systems only; the graviton-tech gate.
    Aetherite,
}

impl Element {
    pub fn properties(self) -> Properties {
        match self {
            Self::Iron => Properties {
                thermal_resistance: 3.0,
                tensile_strength: 4.0,
                energy_conductivity: 2.0,
                density: 7.9,
                graviton_affinity: 0.0,
            },
            Self::Titanium => Properties {
                thermal_resistance: 5.0,
                tensile_strength: 7.0,
                energy_conductivity: 1.5,
                density: 4.5,
                graviton_affinity: 0.0,
            },
            Self::Silicon => Properties {
                thermal_resistance: 4.0,
                tensile_strength: 2.0,
                energy_conductivity: 6.0,
                density: 2.3,
                graviton_affinity: 0.0,
            },
            Self::Carbon => Properties {
                thermal_resistance: 6.0,
                tensile_strength: 8.0,
                energy_conductivity: 3.0,
                density: 2.2,
                graviton_affinity: 0.0,
            },
            Self::Ice => Properties {
                thermal_resistance: 1.0,
                tensile_strength: 1.0,
                energy_conductivity: 0.5,
                density: 0.9,
                graviton_affinity: 0.0,
            },
            Self::Uranium => Properties {
                thermal_resistance: 4.0,
                tensile_strength: 3.0,
                energy_conductivity: 8.0,
                density: 19.0,
                graviton_affinity: 1.0,
            },
            Self::Aetherite => Properties {
                thermal_resistance: 7.0,
                tensile_strength: 3.0,
                energy_conductivity: 7.0,
                density: 1.2,
                graviton_affinity: 9.0,
            },
        }
    }

    /// What a resource profile mines as.
    pub fn from_profile(profile: oj_universe::ResourceProfile) -> &'static [Element] {
        use oj_universe::ResourceProfile as P;
        match profile {
            P::Ferrous => &[Element::Iron, Element::Titanium],
            P::Silicate => &[Element::Silicon, Element::Iron],
            P::Icy => &[Element::Ice, Element::Silicon],
            P::Carbonaceous => &[Element::Carbon, Element::Ice],
            P::Radioactive => &[Element::Uranium, Element::Iron],
            P::ExoticCrystal => &[Element::Aetherite, Element::Silicon],
        }
    }
}

/// A two-element alloy. `mix` in (0,1) is B's share. Alloying is where
/// engineering lives: the blend gains a synergy bonus when the pair is
/// complementary (one strong where the other is weak).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Alloy {
    pub a: Element,
    pub b: Element,
    pub mix: f64,
}

impl Alloy {
    pub fn properties(&self) -> Properties {
        let pa = self.a.properties();
        let pb = self.b.properties();
        let mut p = Properties::blend(pa, pb, self.mix.clamp(0.0, 1.0));
        // Synergy: reward covering each other's weaknesses. Peak bonus at a
        // 50/50 mix of dissimilar elements; zero for self-alloys.
        let dissimilarity = (pa.thermal_resistance - pb.thermal_resistance).abs()
            + (pa.tensile_strength - pb.tensile_strength).abs()
            + (pa.energy_conductivity - pb.energy_conductivity).abs();
        let balance = 1.0 - (self.mix - 0.5).abs() * 2.0;
        let bonus = dissimilarity * 0.05 * balance.max(0.0);
        p.thermal_resistance += bonus;
        p.tensile_strength += bonus;
        p.energy_conductivity += bonus;
        p
    }
}

/// The ship systems an upgrade can improve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UpgradeSlot {
    Shield,
    Hull,
    LaserWeapon,
    MissileRack,
    ForceFieldProjector,
    RocketDrive,
    LightDrive,
    GravityDrive,
    EnergyCollector,
    StudySensor,
    CargoHold,
}

/// A craftable upgrade: property thresholds the chosen alloy must meet,
/// plus a unit cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub slot: UpgradeSlot,
    pub tier: u8,
    pub min: Properties,
    /// Units of alloy consumed.
    pub units: u32,
}

impl Recipe {
    /// The built-in recipe book: tier N of a slot needs progressively more
    /// specialized alloys. Generated, not hand-listed, so tiers stay
    /// monotonic by construction.
    pub fn book() -> Vec<Recipe> {
        let mut out = Vec::new();
        let slots: [(UpgradeSlot, fn(f64) -> Properties); 11] = [
            (UpgradeSlot::Shield, |t: f64| Properties {
                thermal_resistance: 3.0 + t * 0.9,
                density: 10.0 - t * 0.4, // must get LIGHTER as tiers rise
                ..Default::default()
            }),
            (UpgradeSlot::Hull, |t: f64| Properties {
                tensile_strength: 3.0 + t * 0.8,
                ..Default::default()
            }),
            (UpgradeSlot::LaserWeapon, |t: f64| Properties {
                energy_conductivity: 3.5 + t * 0.7,
                ..Default::default()
            }),
            (UpgradeSlot::MissileRack, |t: f64| Properties {
                tensile_strength: 2.0 + t * 0.5,
                energy_conductivity: 1.0 + t * 0.4,
                ..Default::default()
            }),
            (UpgradeSlot::ForceFieldProjector, |t: f64| Properties {
                graviton_affinity: 2.0 + t * 1.0,
                energy_conductivity: 2.0 + t * 0.5,
                ..Default::default()
            }),
            (UpgradeSlot::RocketDrive, |t: f64| Properties {
                thermal_resistance: 2.0 + t * 0.6,
                tensile_strength: 2.0 + t * 0.5,
                ..Default::default()
            }),
            (UpgradeSlot::LightDrive, |t: f64| Properties {
                energy_conductivity: 4.0 + t * 0.8,
                ..Default::default()
            }),
            (UpgradeSlot::GravityDrive, |t: f64| Properties {
                graviton_affinity: 4.0 + t * 1.0,
                ..Default::default()
            }),
            (UpgradeSlot::EnergyCollector, |t: f64| Properties {
                energy_conductivity: 3.0 + t * 0.9,
                ..Default::default()
            }),
            (UpgradeSlot::StudySensor, |t: f64| Properties {
                energy_conductivity: 2.5 + t * 0.5,
                ..Default::default()
            }),
            (UpgradeSlot::CargoHold, |t: f64| Properties {
                tensile_strength: 2.0 + t * 0.6,
                density: 8.0 - t * 0.3,
                ..Default::default()
            }),
        ];
        for (slot, min_at) in slots {
            for tier in 1u8..=8 {
                out.push(Recipe {
                    slot,
                    tier,
                    min: min_at(tier as f64),
                    units: 4 * tier as u32,
                });
            }
        }
        out
    }

    /// Can this alloy build this recipe? Density and other "max"
    /// constraints are encoded as `min.density` meaning MAXIMUM density
    /// (0.0 disables the check).
    pub fn accepts(&self, alloy: &Alloy) -> bool {
        let p = alloy.properties();
        p.thermal_resistance >= self.min.thermal_resistance
            && p.tensile_strength >= self.min.tensile_strength
            && p.energy_conductivity >= self.min.energy_conductivity
            && p.graviton_affinity >= self.min.graviton_affinity
            && (self.min.density == 0.0 || p.density <= self.min.density)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synergy_beats_both_parents_somewhere() {
        let alloy = Alloy { a: Element::Titanium, b: Element::Silicon, mix: 0.5 };
        let p = alloy.properties();
        let ta = Element::Titanium.properties();
        let sa = Element::Silicon.properties();
        // Blended conductivity with bonus should beat titanium alone.
        assert!(p.energy_conductivity > ta.energy_conductivity);
        // Blended tensile with bonus should beat silicon alone.
        assert!(p.tensile_strength > sa.tensile_strength);
    }

    #[test]
    fn self_alloy_gains_nothing() {
        let alloy = Alloy { a: Element::Iron, b: Element::Iron, mix: 0.5 };
        let p = alloy.properties();
        let base = Element::Iron.properties();
        assert!((p.thermal_resistance - base.thermal_resistance).abs() < 1e-9);
    }

    #[test]
    fn recipe_book_tiers_are_strictly_harder() {
        for pair in Recipe::book().windows(2) {
            if pair[0].slot == pair[1].slot {
                assert!(pair[1].units > pair[0].units);
                assert!(
                    pair[1].min.thermal_resistance >= pair[0].min.thermal_resistance
                        && pair[1].min.tensile_strength >= pair[0].min.tensile_strength
                        && pair[1].min.energy_conductivity >= pair[0].min.energy_conductivity
                        && pair[1].min.graviton_affinity >= pair[0].min.graviton_affinity
                );
            }
        }
    }

    #[test]
    fn graviton_tech_requires_aetherite() {
        let book = Recipe::book();
        let ff_t3 = book
            .iter()
            .find(|r| r.slot == UpgradeSlot::ForceFieldProjector && r.tier == 3)
            .unwrap();
        // No aetherite-free alloy reaches graviton 5.0.
        let ordinary = [Element::Iron, Element::Titanium, Element::Silicon, Element::Carbon, Element::Ice, Element::Uranium];
        for a in ordinary {
            for b in ordinary {
                let alloy = Alloy { a, b, mix: 0.5 };
                assert!(!ff_t3.accepts(&alloy), "{a:?}+{b:?} should not unlock tier-3 force fields");
            }
        }
        let exotic = Alloy { a: Element::Aetherite, b: Element::Uranium, mix: 0.4 };
        assert!(ff_t3.accepts(&exotic));
    }
}
