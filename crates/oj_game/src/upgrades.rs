//! Upgrades: recipe-book tiers applied to the vessel.
//!
//! v1 acquisition: salvage value buys the next tier directly (cost from the
//! recipe book's unit counts), on hotkeys. The full loop — element
//! inventory, alloy design, property-gated crafting — replaces the
//! purchase step later; the APPLICATION path (tiers -> ship stats) is the
//! part that stays.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use oj_materials::{Recipe, UpgradeSlot};

use crate::modules::{CareerScore, RunScore};
use crate::sim::Ship;

/// Skill points banked per pilot level gained.
pub const POINTS_PER_LEVEL: u32 = 2;

/// The vessel's installed tiers.
#[derive(Resource, Default)]
pub struct ShipUpgrades {
    tiers: HashMap<UpgradeSlot, u8>,
    book: Vec<Recipe>,
}

impl ShipUpgrades {
    pub fn tier(&self, slot: UpgradeSlot) -> u8 {
        self.tiers.get(&slot).copied().unwrap_or(0)
    }

    /// Salvage cost of the NEXT tier of a slot. Tiers within the recipe
    /// book use its unit counts; beyond it the cost curve continues
    /// procedurally — leveling never caps.
    pub fn next_cost(&self, slot: UpgradeSlot) -> Option<u64> {
        let next = self.tier(slot).checked_add(1)?;
        if let Some(r) = self.book.iter().find(|r| r.slot == slot && r.tier == next) {
            return Some(r.units as u64 * 10);
        }
        // Beyond the book: exponential climb from the book's ceiling.
        let over = next.saturating_sub(8) as f64;
        Some((400.0 * 1.35f64.powf(over)) as u64)
    }

    /// Apply installed tiers to the ship's stats. Idempotent: recomputes
    /// from a tier-0 baseline every call.
    pub fn apply(&self, ship: &mut Ship) {
        let base = Ship::default();
        ship.shield_tier = base.shield_tier + self.tier(UpgradeSlot::Shield);
        ship.command_range =
            base.command_range * (1.0 + self.tier(UpgradeSlot::CommandArray) as f64);
        ship.thrust = base.thrust * 1.2f64.powi(self.tier(UpgradeSlot::RocketDrive) as i32);
        ship.energy_max =
            base.energy_max * (1.0 + 0.5 * self.tier(UpgradeSlot::EnergyCollector) as f64);
        ship.orbit_boost =
            base.orbit_boost + 0.25 * self.tier(UpgradeSlot::GravityDrive) as f64;
    }
}

pub struct UpgradesPlugin;

impl Plugin for UpgradesPlugin {
    fn build(&self, app: &mut App) {
        // Starter armament: a tier-1 laser, so combat is reachable before
        // the first salvage run.
        let mut tiers = HashMap::default();
        tiers.insert(UpgradeSlot::LaserWeapon, 1);
        app.insert_resource(ShipUpgrades {
            tiers,
            book: Recipe::book(),
        })
        .add_systems(Startup, dev_salvage)
        .add_systems(Update, (award_level_points, buy_upgrades, apply_to_new_ships));
    }
}

/// Reaching a new pilot level banks skill points — the level-up IS the
/// reward: points buy gear tiers outright, no salvage needed.
fn award_level_points(
    run: Res<RunScore>,
    mut career: ResMut<CareerScore>,
    mut flash: ResMut<crate::achievements::LastUnlock>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let level = pilot_level(career.total_score + run.total());
    let points = points_due(career.level_seen, level);
    if points == 0 {
        return;
    }
    career.skill_points += points;
    career.level_seen = level;
    career.save();
    flash.text = format!("LEVEL {level} — +{points} SKILL POINTS · [TAB] SPEND");
    flash.ttl = 6.0;
    sfx.write(crate::audio::Sfx::OrbitLock);
    info!("level up: {level}, +{points} skill points ({} banked)", career.skill_points);
}

/// Digits 1-8 buy the next tier of each slot. A banked skill point pays
/// for any tier outright; otherwise the cost comes off run salvage.
fn buy_upgrades(
    keys: Res<ButtonInput<KeyCode>>,
    mut upgrades: ResMut<ShipUpgrades>,
    mut run: ResMut<RunScore>,
    mut career: ResMut<CareerScore>,
    mut ships: Query<&mut Ship>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    let picks = [
        (KeyCode::Digit1, UpgradeSlot::Shield),
        (KeyCode::Digit2, UpgradeSlot::CommandArray),
        (KeyCode::Digit3, UpgradeSlot::RocketDrive),
        (KeyCode::Digit4, UpgradeSlot::EnergyCollector),
        (KeyCode::Digit5, UpgradeSlot::GravityDrive),
        (KeyCode::Digit6, UpgradeSlot::LaserWeapon),
        (KeyCode::Digit7, UpgradeSlot::MissileRack),
        (KeyCode::Digit8, UpgradeSlot::ForceFieldProjector),
    ];
    for (key, slot) in picks {
        if keys.just_pressed(key)
            && try_buy(&mut upgrades, &mut run, &mut career, &mut ships, slot)
        {
            sfx.write(crate::audio::Sfx::Salvage);
        }
    }
}

fn try_buy(
    upgrades: &mut ShipUpgrades,
    run: &mut RunScore,
    career: &mut CareerScore,
    ships: &mut Query<&mut Ship>,
    slot: UpgradeSlot,
) -> bool {
    let Some(cost) = upgrades.next_cost(slot) else { return false };
    // Skill points first: they exist to be spent, and one point buys a
    // tier no matter how deep the salvage curve has climbed.
    if career.skill_points > 0 {
        career.skill_points -= 1;
        career.save();
    } else if run.salvage_value >= cost {
        run.salvage_value -= cost;
    } else {
        return false;
    }
    let next = upgrades.tier(slot) + 1;
    upgrades.tiers.insert(slot, next);
    if let Ok(mut ship) = ships.single_mut() {
        upgrades.apply(&mut ship);
    }
    true
}

/// Buy from an exclusive-world context — the entry point XAML button
/// commands call.
pub fn buy_from_world(world: &mut World, slot: UpgradeSlot) {
    world.resource_scope(|world, mut upgrades: Mut<ShipUpgrades>| {
        world.resource_scope(|world, mut run: Mut<RunScore>| {
            world.resource_scope(|world, mut career: Mut<CareerScore>| {
                let mut ships = world.query::<&mut Ship>();
                let Some(cost) = upgrades.next_cost(slot) else { return };
                if career.skill_points > 0 {
                    career.skill_points -= 1;
                    career.save();
                } else if run.salvage_value >= cost {
                    run.salvage_value -= cost;
                } else {
                    return;
                }
                let next = upgrades.tier(slot) + 1;
                upgrades.tiers.insert(slot, next);
                if let Ok(mut ship) = ships.single_mut(world) {
                    upgrades.apply(&mut ship);
                }
            });
        });
    });
}

/// "name Tn — next: cost" row text for the vessel panel.
pub fn row(upgrades: &ShipUpgrades, label: &str, slot: UpgradeSlot) -> String {
    match upgrades.next_cost(slot) {
        Some(cost) => format!("{label} T{} — next {} salvage", upgrades.tier(slot), cost),
        None => format!("{label} T{} — maxed", upgrades.tier(slot)),
    }
}

/// Dev hook: OJ_SALVAGE=500 starts the run with salvage in hand, so
/// upgrade-gated paths (weapon crafts, UI unlock states) can be exercised
/// without a farming session. No-op on wasm and in normal runs.
fn dev_salvage(mut run: ResMut<RunScore>) {
    if let Ok(v) = std::env::var("OJ_SALVAGE")
        && let Ok(v) = v.parse::<u64>()
    {
        run.salvage_value = v;
    }
}

/// A respawned vessel inherits the installed upgrades (the pilot keeps
/// their engineering; only the hull was lost).
fn apply_to_new_ships(upgrades: Res<ShipUpgrades>, mut ships: Query<&mut Ship, Added<Ship>>) {
    for mut ship in &mut ships {
        upgrades.apply(&mut ship);
    }
}

/// Pilot level: an infinite, sublinear curve over lifetime + current
/// score. Score is combat-dominant (bounty-weighted kills), so climbing
/// this curve means defeating the progressively harder enemies each
/// level summons — level N costs 2000·(N-1)² points, and only high-tier
/// bounties pay that fast.
pub fn pilot_level(lifetime_score: u64) -> u32 {
    ((lifetime_score as f64 / 2000.0).sqrt() as u32) + 1
}

/// Skill points owed when the pilot stands at `level` with everything
/// through `level_seen` already paid. Level 1 is the floor (nobody is
/// paid for existing), and old saves carrying `level_seen` 0 back-pay
/// the levels the veteran already earned.
pub fn points_due(level_seen: u32, level: u32) -> u32 {
    level.saturating_sub(level_seen.max(1)) * POINTS_PER_LEVEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leveling_is_infinite() {
        let mut upgrades = ShipUpgrades { tiers: HashMap::default(), book: Recipe::book() };
        // March a slot far past the recipe book: a next cost must always
        // exist and never decrease.
        let mut last = 0u64;
        for tier in 0..200u8 {
            upgrades.tiers.insert(UpgradeSlot::Shield, tier);
            let cost = upgrades.next_cost(UpgradeSlot::Shield).expect("cost at every tier");
            assert!(cost >= last, "cost regressed at tier {tier}: {cost} < {last}");
            last = cost;
        }
        // The level curve is monotone and unbounded in practice.
        assert!(pilot_level(0) == 1);
        assert!(pilot_level(2_000_000) > pilot_level(20_000));
        assert!(pilot_level(2_000_000_000) > pilot_level(2_000_000));
    }

    /// Each level pays exactly once; fresh pilots get nothing for
    /// existing; veteran saves are back-paid on first sight.
    #[test]
    fn skill_points_pay_per_level_once() {
        assert_eq!(points_due(0, 1), 0, "level 1 is the starting state");
        assert_eq!(points_due(1, 1), 0);
        assert_eq!(points_due(1, 2), POINTS_PER_LEVEL);
        assert_eq!(points_due(0, 4), 3 * POINTS_PER_LEVEL, "veteran back-pay");
        assert_eq!(points_due(4, 4), 0, "no double pay");
        assert_eq!(points_due(4, 6), 2 * POINTS_PER_LEVEL);
    }
}

/// One-line HUD summary of installed tiers.
pub fn summary(upgrades: &ShipUpgrades) -> String {
    format!(
        "shield T{}  range T{}  drive T{}  collector T{}   [1-4 to buy]",
        upgrades.tier(UpgradeSlot::Shield),
        upgrades.tier(UpgradeSlot::CommandArray),
        upgrades.tier(UpgradeSlot::RocketDrive),
        upgrades.tier(UpgradeSlot::EnergyCollector),
    )
}
