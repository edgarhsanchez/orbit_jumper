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

use crate::modules::RunScore;
use crate::sim::Ship;

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

    /// Salvage cost of the NEXT tier of a slot, if one exists.
    pub fn next_cost(&self, slot: UpgradeSlot) -> Option<u64> {
        let next = self.tier(slot) + 1;
        self.book
            .iter()
            .find(|r| r.slot == slot && r.tier == next)
            .map(|r| r.units as u64 * 10)
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
    }
}

pub struct UpgradesPlugin;

impl Plugin for UpgradesPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ShipUpgrades {
            tiers: HashMap::default(),
            book: Recipe::book(),
        })
        .add_systems(Update, (buy_upgrades, apply_to_new_ships));
    }
}

/// 1-4 buy the next tier: shields, command array, rocket drive, energy
/// collector. Cost comes off the run's salvage value.
fn buy_upgrades(
    keys: Res<ButtonInput<KeyCode>>,
    mut upgrades: ResMut<ShipUpgrades>,
    mut run: ResMut<RunScore>,
    mut ships: Query<&mut Ship>,
) {
    let picks = [
        (KeyCode::Digit1, UpgradeSlot::Shield),
        (KeyCode::Digit2, UpgradeSlot::CommandArray),
        (KeyCode::Digit3, UpgradeSlot::RocketDrive),
        (KeyCode::Digit4, UpgradeSlot::EnergyCollector),
    ];
    for (key, slot) in picks {
        if !keys.just_pressed(key) {
            continue;
        }
        let Some(cost) = upgrades.next_cost(slot) else { continue };
        if run.salvage_value < cost {
            continue;
        }
        run.salvage_value -= cost;
        let next = upgrades.tier(slot) + 1;
        upgrades.tiers.insert(slot, next);
        if let Ok(mut ship) = ships.single_mut() {
            upgrades.apply(&mut ship);
        }
    }
}

/// A respawned vessel inherits the installed upgrades (the pilot keeps
/// their engineering; only the hull was lost).
fn apply_to_new_ships(upgrades: Res<ShipUpgrades>, mut ships: Query<&mut Ship, Added<Ship>>) {
    for mut ship in &mut ships {
        upgrades.apply(&mut ship);
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
