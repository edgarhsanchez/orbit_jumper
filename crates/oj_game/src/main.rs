//! orbit_jumper: launch, orbit, survive.
//!
//! Architecture (docs/design.md is the reference):
//! - Simulation is f64 (`SimPos`/`SimVel`), fixed 60 Hz, deterministic.
//!   Celestials ride Kepler rails from `oj_universe`; the ship integrates.
//! - Rendering is camera-relative f32: every frame each entity's Transform
//!   is `(sim_pos - camera_sim_pos)` cast down. No meshlets by default —
//!   see docs/design.md "Rendering research" for why instancing + LOD is
//!   the right tool at our object counts (meshlet stays a feature flag).
//! - UI is bevy_pf XAML (assets/ui/hud.xaml) bound to a `Bindable`
//!   view-model with per-property notification.

use bevy::prelude::*;
use bevy_pf::prelude::*;
use oj_orbits::Vec3d;
use oj_universe::{SectorCoord, SystemId, Universe};

mod command;
mod modules;
mod sim;
mod ui;
mod upgrades;
mod weapons;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "orbit jumper".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(PfUiPlugin)
        .add_plugins((sim::SimPlugin, ui::HudPlugin))
        .add_plugins((modules::StudyPlugin, modules::ScorePlugin, modules::SalvagePlugin))
        .add_plugins((bevy::picking::mesh_picking::MeshPickingPlugin, command::CommandPlugin))
        .add_plugins((upgrades::UpgradesPlugin, weapons::WeaponsPlugin))
        .run();
}

/// The one resource every system agrees on: which universe this is.
#[derive(Resource)]
pub struct GameUniverse {
    pub universe: Universe,
    /// The system the player currently occupies.
    pub current: SystemId,
}

impl Default for GameUniverse {
    fn default() -> Self {
        let universe = Universe::new(0xC0FFEE);
        // Find the first sector with a galaxy so a fresh run always starts
        // somewhere real. Deterministic, so every player's "first system"
        // is the same one — the tutorial neighborhood.
        let mut sector = SectorCoord { x: 0, y: 0, z: 0 };
        for x in 0..256 {
            sector = SectorCoord { x, y: 0, z: 0 };
            if universe.galaxy_at(sector).is_some() {
                break;
            }
        }
        Self {
            universe,
            current: SystemId { sector, index: 0 },
        }
    }
}

/// f64 simulation position, meters, system-local frame (sun at origin).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SimPos(pub Vec3d);

/// f64 simulation velocity, m/s.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SimVel(pub Vec3d);
