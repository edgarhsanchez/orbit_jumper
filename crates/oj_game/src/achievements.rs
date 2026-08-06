//! Achievements: definitions, live condition checks, local persistence,
//! and the record feed that becomes gossip-shared in the net phase.
//!
//! Unlocks append an `oj_protocol::GlobalRecord::AchievementUnlocked` to a
//! local feed file — the exact record the P2P layer will sign and gossip,
//! so "achievements earned by others" is a transport away, not a redesign:
//! the panel already renders the feed, it just only contains us for now.

use std::collections::HashSet;
use bevy::prelude::*;
use oj_protocol::{GlobalRecord, PlayerId};

use crate::command::NavState;
use crate::modules::{CareerScore, RunScore, StudyState};
use crate::sim::Ship;
use crate::upgrades::ShipUpgrades;

/// Every achievement in the game. Conditions are pure checks over live
/// state, so adding one is adding a row here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Achievement {
    FirstContact,
    GravityDancer,
    SlingMaster,
    FirstBlood,
    DroneReaper,
    Astronomer,
    FullTank,
    Icarus,
    Engineer,
    Overdrive,
}

impl Achievement {
    pub const ALL: [Achievement; 10] = [
        Self::FirstContact,
        Self::GravityDancer,
        Self::SlingMaster,
        Self::FirstBlood,
        Self::DroneReaper,
        Self::Astronomer,
        Self::FullTank,
        Self::Icarus,
        Self::Engineer,
        Self::Overdrive,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::FirstContact => "First Contact",
            Self::GravityDancer => "Gravity Dancer",
            Self::SlingMaster => "Sling Master",
            Self::FirstBlood => "First Blood",
            Self::DroneReaper => "Drone Reaper",
            Self::Astronomer => "Astronomer",
            Self::FullTank => "Full Tank",
            Self::Icarus => "Icarus",
            Self::Engineer => "Engineer",
            Self::Overdrive => "Overdrive",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FirstContact => "capture your first orbit",
            Self::GravityDancer => "fly one clean gravity assist",
            Self::SlingMaster => "fly five assists in one run",
            Self::FirstBlood => "destroy a drone",
            Self::DroneReaper => "destroy ten in one run",
            Self::Astronomer => "study a sun to completion",
            Self::FullTank => "fill the energy banks",
            Self::Icarus => "lose a ship",
            Self::Engineer => "craft any upgrade",
            Self::Overdrive => "ride an orbit above natural speed",
        }
    }
}

/// Unlock state, persisted locally.
#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct Unlocked(pub HashSet<Achievement>);

fn unlocked_path() -> std::path::PathBuf {
    std::path::PathBuf::from("orbit_jumper_achievements.ron")
}

/// The record feed: what the gossip layer will carry. Local file for now.
fn feed_path() -> std::path::PathBuf {
    std::path::PathBuf::from("orbit_jumper_feed.ron")
}

pub fn read_feed() -> Vec<GlobalRecord> {
    std::fs::read_to_string(feed_path())
        .ok()
        .and_then(|s| ron::from_str(&s).ok())
        .unwrap_or_default()
}

fn append_feed(record: GlobalRecord) {
    let mut feed = read_feed();
    feed.push(record);
    if let Ok(text) = ron::to_string(&feed) {
        let _ = std::fs::write(feed_path(), text);
    }
}

/// The most recent unlock, for the HUD flash.
#[derive(Resource, Default)]
pub struct LastUnlock {
    pub text: String,
    pub ttl: f64,
}

pub struct AchievementsPlugin;

impl Plugin for AchievementsPlugin {
    fn build(&self, app: &mut App) {
        let unlocked: Unlocked = std::fs::read_to_string(unlocked_path())
            .ok()
            .and_then(|s| ron::from_str(&s).ok())
            .unwrap_or_default();
        app.insert_resource(unlocked)
            .init_resource::<LastUnlock>()
            .add_systems(Update, check_achievements);
    }
}

#[allow(clippy::too_many_arguments)]
fn check_achievements(
    time: Res<Time>,
    mut unlocked: ResMut<Unlocked>,
    mut last: ResMut<LastUnlock>,
    run: Res<RunScore>,
    career: Res<CareerScore>,
    study: Res<StudyState>,
    upgrades: Res<ShipUpgrades>,
    game: Res<crate::GameUniverse>,
    ships: Query<(&Ship, &NavState)>,
) {
    last.ttl = (last.ttl - time.delta_secs_f64()).max(0.0);

    let ship = ships.single().ok();
    let orbiting = matches!(ship, Some((_, NavState::Orbiting { .. })));
    let overspeed = orbiting && ship.map(|(s, _)| s.orbit_boost > 1.0).unwrap_or(false);
    let full = ship.map(|(s, _)| s.energy >= s.energy_max).unwrap_or(false);
    // Any tier beyond the starter laser means something was crafted.
    let engineered = {
        use oj_materials::UpgradeSlot as S;
        [S::Shield, S::CommandArray, S::RocketDrive, S::EnergyCollector, S::GravityDrive]
            .iter()
            .any(|s| upgrades.tier(*s) > 0)
    };

    let conditions: [(Achievement, bool); 10] = [
        (Achievement::FirstContact, orbiting),
        (Achievement::GravityDancer, run.assists >= 1),
        (Achievement::SlingMaster, run.assists >= 5),
        (Achievement::FirstBlood, run.kills >= 1),
        (Achievement::DroneReaper, run.kills >= 10),
        (Achievement::Astronomer, study.revealed),
        (Achievement::FullTank, full),
        (Achievement::Icarus, career.ships_lost >= 1),
        (Achievement::Engineer, engineered),
        (Achievement::Overdrive, overspeed),
    ];

    let mut changed = false;
    for (achievement, met) in conditions {
        if met && unlocked.0.insert(achievement) {
            changed = true;
            last.text = format!("achievement: {}", achievement.name());
            last.ttl = 5.0;
            // The record the gossip layer will sign and share. Identity is
            // a placeholder key until the net phase brings real keys.
            append_feed(GlobalRecord::AchievementUnlocked {
                player: PlayerId([0; 32]),
                achievement: achievement.name().to_string(),
                tick: 0,
                system: game.current,
            });
        }
    }
    if changed && let Ok(text) = ron::to_string(&*unlocked) {
        let _ = std::fs::write(unlocked_path(), text);
    }
}
