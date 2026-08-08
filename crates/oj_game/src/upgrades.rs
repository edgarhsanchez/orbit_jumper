//! Upgrades: crafted tiers applied to the vessel.
//!
//! Crafting is MATERIAL-based: every slot has a canonical two-element
//! recipe (quantities scale with tier) drawn from the common space
//! materials the world already drops — ring debris, kill scrap, comet
//! ice — plus ONE skill point from leveling. Both are required: the
//! stash is what you flew for, the point is what you fought for. The
//! full alloy-design loop (oj_materials' property-gated recipes) stays
//! dormant substrate for a later phase; the APPLICATION path
//! (tiers -> ship stats) is unchanged.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use oj_materials::{Element, UpgradeSlot};

use crate::modules::{RunScore, Stash};
use crate::sim::Ship;

/// Skill points banked per pilot level gained.
pub const POINTS_PER_LEVEL: u32 = 2;
/// Every craft costs one point on top of its materials.
pub const CRAFT_POINT_COST: u32 = 1;

/// The vessel's installed tiers.
#[derive(Resource, Default)]
pub struct ShipUpgrades {
    tiers: HashMap<UpgradeSlot, u8>,
}

impl ShipUpgrades {
    pub fn tier(&self, slot: UpgradeSlot) -> u8 {
        self.tiers.get(&slot).copied().unwrap_or(0)
    }
}

/// The canonical element pair a slot is built from. Thematic and
/// grounded in the property table: shields want thermal resistance and
/// low mass, drives want structure, graviton tech wants the exotics.
pub fn slot_elements(slot: UpgradeSlot) -> [Element; 2] {
    use Element as E;
    match slot {
        UpgradeSlot::Shield => [E::Titanium, E::Ice],
        UpgradeSlot::Hull | UpgradeSlot::CargoHold => [E::Iron, E::Titanium],
        UpgradeSlot::LaserWeapon => [E::Silicon, E::Iron],
        UpgradeSlot::MissileRack => [E::Iron, E::Titanium],
        UpgradeSlot::ForceFieldProjector => [E::Aetherite, E::Uranium],
        UpgradeSlot::RocketDrive => [E::Iron, E::Carbon],
        UpgradeSlot::LightDrive => [E::Uranium, E::Silicon],
        UpgradeSlot::GravityDrive => [E::Uranium, E::Aetherite],
        UpgradeSlot::EnergyCollector => [E::Silicon, E::Ice],
        UpgradeSlot::StudySensor | UpgradeSlot::CommandArray => [E::Silicon, E::Carbon],
    }
}

/// Materials for crafting `tier` of `slot`: the slot's element pair,
/// primary-heavy, climbing linearly forever — leveling never caps.
pub fn material_cost(slot: UpgradeSlot, tier: u8) -> [(Element, u32); 2] {
    let [a, b] = slot_elements(slot);
    let t = tier as u32;
    [(a, 2 + t), (b, 1 + t.div_ceil(2))]
}

/// Can the stash and this run's point bank cover the next tier of `slot`?
pub fn can_afford(slot: UpgradeSlot, next_tier: u8, stash: &Stash, skill_points: u32) -> bool {
    skill_points >= CRAFT_POINT_COST
        && material_cost(slot, next_tier)
            .iter()
            .all(|(e, n)| stash.0.get(e).copied().unwrap_or(0) >= *n)
}

/// Salvage credits per point of missing hull. Repair is maintenance,
/// not engineering: it costs the CR the tractor hauls in — the number
/// the HUD's SALVAGE readout shows — never materials or skill points.
pub const REPAIR_CR_PER_POINT: u64 = 2;

/// Credits to patch `missing` points of hull.
pub fn repair_cost(missing: f64) -> u64 {
    (missing.ceil() as u64).max(1) * REPAIR_CR_PER_POINT
}

/// Patch the hull back to full out of the run's salvage balance. No-op
/// when the hull is whole or the credits are short — all or nothing,
/// like crafting. Spending never lowers score: the score term is
/// credits EARNED, and this only moves the spent counter.
pub fn try_repair(run: &mut RunScore, ship: &mut Ship) -> bool {
    let missing = ship.hull_max - ship.hull;
    if missing < 0.5 {
        return false;
    }
    let cost = repair_cost(missing);
    if run.salvage_balance() < cost {
        return false;
    }
    run.salvage_spent += cost;
    ship.hull = ship.hull_max;
    info!("hull repaired for {cost} CR ({} CR left)", run.salvage_balance());
    true
}

/// Repair from an exclusive-world context — the vessel panel's REPAIR
/// button. Salvage credits are run currency, not persisted, so there is
/// nothing to save here.
pub fn repair_from_world(world: &mut World) {
    world.resource_scope(|world, mut run: Mut<RunScore>| {
        let mut ships = world.query::<&mut Ship>();
        if let Ok(mut ship) = ships.single_mut(world) {
            try_repair(&mut run, &mut ship);
        }
    });
}

/// The pilot's engineering, persisted between sessions: installed gear
/// tiers and the material stash. Skill points are persisted currency
/// (CareerScore); spending them on goods that evaporated at exit would
/// be a progression trap, so the goods persist too.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Loadout {
    #[serde(default)]
    pub tiers: Vec<(UpgradeSlot, u8)>,
    #[serde(default)]
    pub stash: Vec<(Element, u32)>,
}

impl Loadout {
    pub fn load() -> Self {
        // Through the storage seam so the web build persists too
        // (localStorage); native keeps the same file as before.
        crate::storage::load("loadout")
            .and_then(|s| ron::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// Write the current gear + stash wherever the platform persists.
pub fn save_loadout(upgrades: &ShipUpgrades, stash: &Stash) {
    let loadout = Loadout {
        tiers: upgrades.tiers.iter().map(|(k, v)| (*k, *v)).collect(),
        stash: stash.0.iter().map(|(k, v)| (*k, *v)).collect(),
    };
    if let Ok(text) = ron::to_string(&loadout) {
        crate::storage::save("loadout", &text);
    }
}

impl ShipUpgrades {
    /// Apply installed tiers to the ship's stats. Idempotent: recomputes
    /// from a tier-0 baseline every call.
    pub fn apply(&self, ship: &mut Ship) {
        let base = Ship::default();
        ship.shield_tier = base.shield_tier.saturating_add(self.tier(UpgradeSlot::Shield));
        ship.hull_max = base.hull_max + 25.0 * self.tier(UpgradeSlot::Hull) as f64;
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
        // The pilot's engineering survives the session: gear tiers and
        // stash reload from the loadout file. Starter armament floor: a
        // tier-1 laser, so combat is reachable from the very first run.
        let loadout = Loadout::load();
        let mut tiers: HashMap<UpgradeSlot, u8> = loadout.tiers.into_iter().collect();
        let laser = tiers.entry(UpgradeSlot::LaserWeapon).or_insert(1);
        if *laser == 0 {
            *laser = 1;
        }
        app.insert_resource(ShipUpgrades { tiers })
            .insert_resource(Stash(loadout.stash.into_iter().collect()))
            .add_systems(Startup, dev_salvage)
            .add_systems(Update, (award_level_points, buy_upgrades, apply_to_new_ships));
    }
}

/// Reaching a new pilot level banks skill points — the level-up IS the
/// reward: points buy gear tiers outright, no salvage needed.
fn award_level_points(
    mut run: ResMut<RunScore>,
    mut flash: ResMut<crate::achievements::LastUnlock>,
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    // Rank is PER-RUN: level comes from this run's score alone, so every
    // restart — death or relaunch — starts back at level 1.
    let level = pilot_level(run.total());
    let points = points_due(run.level_seen, level);
    if points == 0 {
        return;
    }
    run.skill_points += points;
    run.level_seen = level;
    flash.text = format!("LEVEL {level} — +{points} SKILL POINTS · [TAB] SPEND");
    flash.ttl = 6.0;
    sfx.write(crate::audio::Sfx::OrbitLock);
    info!("level up: {level}, +{points} skill points ({} banked)", run.skill_points);
}

/// Digits 1-8 craft the next tier of each slot from the stash + one
/// skill point — the same recipe the vessel panel's CRAFT buttons use.
#[allow(clippy::too_many_arguments)]
fn buy_upgrades(
    keys: Res<ButtonInput<KeyCode>>,
    mut upgrades: ResMut<ShipUpgrades>,
    mut stash: ResMut<Stash>,
    mut run: ResMut<RunScore>,
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
        if keys.just_pressed(key) && try_craft(&mut upgrades, &mut stash, &mut run, slot) {
            save_loadout(&upgrades, &stash);
            if let Ok(mut ship) = ships.single_mut() {
                upgrades.apply(&mut ship);
            }
            sfx.write(crate::audio::Sfx::Salvage);
        }
    }
    // 9: patch the hull (the panel's REPAIR button, as a key).
    if keys.just_pressed(KeyCode::Digit9)
        && let Ok(mut ship) = ships.single_mut()
        && try_repair(&mut run, &mut ship)
    {
        sfx.write(crate::audio::Sfx::Salvage);
    }
}

/// The one crafting rule, shared by every entry point: the next tier of
/// `slot` costs its material recipe out of the stash PLUS one skill
/// point. Both or nothing — no partial spends. The caller re-applies
/// tiers to the ship on success.
fn try_craft(
    upgrades: &mut ShipUpgrades,
    stash: &mut Stash,
    run: &mut RunScore,
    slot: UpgradeSlot,
) -> bool {
    // Tier 255 is the hard ceiling of the u8 — refusing here beats
    // wrapping a maxed slot back to zero while still taking payment.
    let Some(next) = upgrades.tier(slot).checked_add(1) else { return false };
    if !can_afford(slot, next, stash, run.skill_points) {
        return false;
    }
    for (element, n) in material_cost(slot, next) {
        if let Some(have) = stash.0.get_mut(&element) {
            *have -= n;
        }
    }
    run.skill_points -= CRAFT_POINT_COST;
    upgrades.tiers.insert(slot, next);
    info!("crafted {slot:?} tier {next}");
    true
}

/// Craft from an exclusive-world context — the entry point the XAML
/// CRAFT buttons call.
pub fn buy_from_world(world: &mut World, slot: UpgradeSlot) {
    world.resource_scope(|world, mut upgrades: Mut<ShipUpgrades>| {
        world.resource_scope(|world, mut stash: Mut<Stash>| {
            world.resource_scope(|world, mut run: Mut<RunScore>| {
                if try_craft(&mut upgrades, &mut stash, &mut run, slot) {
                    save_loadout(&upgrades, &stash);
                    let mut ships = world.query::<&mut Ship>();
                    if let Ok(mut ship) = ships.single_mut(world) {
                        upgrades.apply(&mut ship);
                    }
                }
            });
        });
    });
}

/// Dev hooks: OJ_SALVAGE=500 seeds run salvage; OJ_STASH=20 seeds that
/// many of EVERY element, so crafting paths can be exercised without a
/// farming session. No-op on wasm and in normal runs.
fn dev_salvage(
    mut run: ResMut<RunScore>,
    mut stash: ResMut<Stash>,
    mut upgrades: ResMut<ShipUpgrades>,
) {
    if let Ok(v) = std::env::var("OJ_SALVAGE")
        && let Ok(v) = v.parse::<u64>()
    {
        run.salvage_value = v;
    }
    if let Ok(v) = std::env::var("OJ_SP")
        && let Ok(v) = v.parse::<u32>()
    {
        run.skill_points = v;
        run.level_seen = 99; // no fanfare over the seeded points
    }
    if let Ok(v) = std::env::var("OJ_STASH")
        && let Ok(v) = v.parse::<u32>()
    {
        use Element as E;
        for e in [E::Iron, E::Titanium, E::Silicon, E::Carbon, E::Ice, E::Uranium, E::Aetherite] {
            stash.0.insert(e, v);
        }
    }
    // OJ_SHIELD_TIER=n: install the Shield slot at that tier, so nova
    // paths can be exercised without a crafting session.
    if let Ok(v) = std::env::var("OJ_SHIELD_TIER")
        && let Ok(v) = v.parse::<u8>()
    {
        upgrades.tiers.insert(UpgradeSlot::Shield, v);
    }
}

/// A respawned vessel inherits the installed upgrades (the pilot keeps
/// their engineering; only the hull was lost).
fn apply_to_new_ships(upgrades: Res<ShipUpgrades>, mut ships: Query<&mut Ship, Added<Ship>>) {
    for mut ship in &mut ships {
        let fresh_full = ship.hull == 100.0;
        upgrades.apply(&mut ship);
        // A fresh hull launches at its plated ceiling; a mid-run resume
        // (save.rs restores a lower value afterwards) keeps its damage.
        if fresh_full {
            ship.hull = ship.hull_max;
        }
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
        // March a slot far past any bookkeeping: material costs must
        // exist and never decrease at every tier.
        let mut last = 0u32;
        for tier in 1..=200u8 {
            let total: u32 = material_cost(UpgradeSlot::Shield, tier).iter().map(|(_, n)| n).sum();
            assert!(total > 0, "tier {tier} costs nothing");
            assert!(total >= last, "cost regressed at tier {tier}: {total} < {last}");
            last = total;
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

    /// The crafting contract: a craft needs BOTH the material recipe and
    /// a skill point; it spends exactly those; a missing ingredient or an
    /// empty bank is a no-op — one click improves ONE thing.
    #[test]
    fn crafting_spends_materials_and_points_or_nothing() {
        let mut upgrades = ShipUpgrades::default();
        let mut stash = Stash::default();
        let mut run = RunScore::default();
        let slot = UpgradeSlot::Shield; // recipe: Titanium + Ice
        let [(a, na), (b, nb)] = material_cost(slot, 1);

        // No materials, no points: nothing happens.
        assert!(!try_craft(&mut upgrades, &mut stash, &mut run, slot));
        assert_eq!(upgrades.tier(slot), 0);

        // Materials but no points: still nothing.
        stash.0.insert(a, na + 3);
        stash.0.insert(b, nb + 1);
        assert!(!try_craft(&mut upgrades, &mut stash, &mut run, slot));
        assert_eq!(upgrades.tier(slot), 0);

        // Both: the craft lands and spends exactly the recipe + 1 point.
        run.skill_points = 2;
        assert!(try_craft(&mut upgrades, &mut stash, &mut run, slot));
        assert_eq!(upgrades.tier(slot), 1);
        assert_eq!(stash.0[&a], 3, "primary spent exactly");
        assert_eq!(stash.0[&b], 1, "secondary spent exactly");
        assert_eq!(run.skill_points, 2 - CRAFT_POINT_COST);

        // Tier 2 costs more than the leftovers: no partial spend.
        assert!(!try_craft(&mut upgrades, &mut stash, &mut run, slot));
        assert_eq!(stash.0[&a], 3);
        assert_eq!(run.skill_points, 2 - CRAFT_POINT_COST);
    }

    /// The two payoff-gear slots do what their rows claim: HULL PLATING
    /// raises the ceiling 25 per tier, LIGHT DRIVE compounds the jump
    /// discount without ever making travel free.
    #[test]
    fn hull_plating_and_light_drive_pay_off() {
        let mut upgrades = ShipUpgrades::default();
        let mut ship = Ship::default();
        upgrades.apply(&mut ship);
        assert_eq!(ship.hull_max, 100.0);
        upgrades.tiers.insert(UpgradeSlot::Hull, 3);
        upgrades.apply(&mut ship);
        assert_eq!(ship.hull_max, 175.0);

        let d = 4.0e17;
        let base = crate::travel::jump_cost(d, 0);
        let t1 = crate::travel::jump_cost(d, 1);
        let t5 = crate::travel::jump_cost(d, 5);
        assert!(t1 < base && t5 < t1, "discount must compound");
        assert!(t5 > 0.0, "travel is never free");
    }

    /// The repair contract: a patch spends the salvage credits the HUD
    /// shows — or nothing — and spending never lowers score or rank.
    /// (Regression: repair used to want stash metals while the UI
    /// headlined an unspendable CR number; 600 CR could not buy a patch.)
    #[test]
    fn repair_spends_salvage_credits_or_nothing() {
        let mut run = RunScore::default();
        let mut ship = Ship::default();
        ship.hull = 40.0;

        // Short balance: no-op, nothing spent.
        run.salvage_value = 10;
        assert!(!try_repair(&mut run, &mut ship));
        assert_eq!(ship.hull, 40.0);
        assert_eq!(run.salvage_balance(), 10);

        // Funded (the reported scenario — 600 CR banked): patch to full,
        // spend exactly the shown cost, score unmoved.
        run.salvage_value = 600;
        let score_before = run.total();
        let cost = repair_cost(100.0 - ship.hull);
        assert!(cost <= 600, "600 CR must always cover a full patch");
        assert!(try_repair(&mut run, &mut ship));
        assert_eq!(ship.hull, 100.0);
        assert_eq!(run.salvage_balance(), 600 - cost);
        assert_eq!(run.total(), score_before, "repair must not move score");

        // Whole hull: no-op regardless of balance.
        assert!(!try_repair(&mut run, &mut ship));
        assert_eq!(run.salvage_balance(), 600 - cost);
    }
}
