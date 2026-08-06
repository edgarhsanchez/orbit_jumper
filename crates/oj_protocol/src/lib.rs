//! The multiplayer seam: every message peers could ever exchange, as plain
//! serde types with no transport attached. The netcode phase (see
//! docs/design.md) picks transports; game code only ever sees these.
//!
//! Anti-cheat posture for a P2P economy: events that MATTER globally
//! (achievements, wreck claims, scores) are SIGNED by their author and
//! carry the deterministic context (universe seed, system id, tick) that
//! lets any peer re-validate plausibility before gossiping them on.

use oj_orbits::Vec3d;
use oj_universe::SystemId;
use serde::{Deserialize, Serialize};

/// Stable player identity: an Ed25519-style public key (32 bytes). The
/// crypto impl arrives with the net phase; the type is fixed now so every
/// record is born attributable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub [u8; 32]);

/// Simulation tick (fixed timestep, 60 Hz).
pub type Tick = u64;

/// Dynamic ship state inside one system session. Everything else about the
/// system is derivable from the universe seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShipSnapshot {
    pub player: PlayerId,
    pub tick: Tick,
    pub position: Vec3d,
    pub velocity: Vec3d,
    pub energy: f64,
    pub shield: f64,
    pub hull: f64,
}

/// In-session events, unreliable-ordered channel material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionEvent {
    FiredLaser { from: Vec3d, dir: Vec3d },
    LaunchedMissile { from: Vec3d, vel: Vec3d, target: Option<PlayerId> },
    ForceField { at: Vec3d, strength: f64, attract: bool },
    ShipExploded { player: PlayerId, at: Vec3d },
    WreckSpawned { wreck: u64, at: Vec3d },
    WreckClaimed { wreck: u64, by: PlayerId },
}

/// Globally-persisted records: signed, gossiped, replayable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GlobalRecord {
    AchievementUnlocked {
        player: PlayerId,
        achievement: String,
        tick: Tick,
        system: SystemId,
    },
    ScoreFinal {
        player: PlayerId,
        run_score: u64,
        tick: Tick,
    },
}

/// A signed envelope. `signature` is validated by the net layer; game code
/// treats presence of a verified envelope as authenticity. Stored as bytes
/// rather than [u8; 64] because serde's array impls stop at 32.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signed<T> {
    pub author: PlayerId,
    pub payload: T,
    pub signature: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_roundtrip_through_ron() {
        let snap = ShipSnapshot {
            player: PlayerId([7; 32]),
            tick: 123,
            position: Vec3d::new(1.0, 2.0, 3.0),
            velocity: Vec3d::new(-1.0, 0.5, 0.0),
            energy: 0.8,
            shield: 1.0,
            hull: 1.0,
        };
        let text = ron::to_string(&snap).unwrap();
        let back: ShipSnapshot = ron::from_str(&text).unwrap();
        assert_eq!(snap, back);
    }
}
