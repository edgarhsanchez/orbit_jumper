//! HUD: bevy_pf XAML bound to a view-model with per-property notification —
//! the generated setters only re-apply the bindings whose values changed.

use bevy::prelude::*;
use bevy_pf::prelude::*;

use crate::command::{CommandHold, NavState};
use crate::modules::{CareerScore, RunScore, StudyState, displayed_sun_class};
use crate::sim::{CelestialBody, Ship, SunBody};

#[derive(Reflect, Default, Bindable)]
struct HudVm {
    energy: f64,
    shield: f64,
    hull: f64,
    sun_class: String,
    score: f64,
    best: f64,
    nav: String,
    speed: f64,
    upgrades: String,
    salvage: f64,
}

#[derive(Resource, Clone)]
struct HudModel(Bindable);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, update_hud);
    }
}

const HUD_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12">
  <TextBlock Text="{Binding sun_class, StringFormat=sun: {0}}" Foreground="#9FD0FF"/>
  <StackPanel Orientation="Horizontal" Margin="0,6,0,0">
    <TextBlock Text="energy" Width="60" Foreground="#C8E06E"/>
    <ProgressBar Width="180" Height="10" Maximum="100" Value="{Binding energy}"/>
  </StackPanel>
  <StackPanel Orientation="Horizontal" Margin="0,4,0,0">
    <TextBlock Text="shield" Width="60" Foreground="#6EC1E0"/>
    <ProgressBar Width="180" Height="10" Maximum="100" Value="{Binding shield}"/>
  </StackPanel>
  <StackPanel Orientation="Horizontal" Margin="0,4,0,0">
    <TextBlock Text="hull" Width="60" Foreground="#E0876E"/>
    <ProgressBar Width="180" Height="10" Maximum="100" Value="{Binding hull}"/>
  </StackPanel>
  <TextBlock Text="{Binding nav}" Foreground="#B8A0E0" Margin="0,8,0,0"/>
  <TextBlock Text="{Binding speed, StringFormat=speed: {0} km/s}" Foreground="#8FBCB0"/>
  <TextBlock Text="{Binding score, StringFormat=score: {0}}" Foreground="#E0D06E" Margin="0,8,0,0"/>
  <TextBlock Text="{Binding best, StringFormat=best: {0}}" Foreground="#8A8F98"/>
  <TextBlock Text="{Binding salvage, StringFormat=salvage: {0}}" Foreground="#C9A96E"/>
  <TextBlock Text="{Binding upgrades}" Foreground="#7E97B8" Margin="0,4,0,0"/>
</StackPanel>
"##;

fn spawn_hud(mut commands: Commands) {
    let vm = Bindable::new(HudVm {
        energy: 100.0,
        shield: 100.0,
        hull: 100.0,
        sun_class: "unknown (hold S to study)".into(),
        score: 0.0,
        best: 0.0,
        nav: "free flight".into(),
        speed: 0.0,
        upgrades: String::new(),
        salvage: 0.0,
    });
    commands.insert_resource(HudModel(vm.clone()));
    commands.queue(move |world: &mut World| {
        let scene = bevy_pf::XamlScene::parse(HUD_XAML).expect("hud xaml is valid");
        let root = world.spawn(DataContext(vm.clone())).id();
        if let Err(e) = bevy_pf::instantiate_document(world, root, &scene.document()) {
            error!("hud failed to instantiate: {e}");
        }
        // Instantiation replaces root components; re-attach the context.
        world.entity_mut(root).insert(DataContext(vm));
    });
}

fn update_hud(
    model: Option<Res<HudModel>>,
    ships: Query<(&Ship, &crate::SimVel, &NavState)>,
    suns: Query<&SunBody>,
    bodies: Query<&CelestialBody>,
    hold: Res<CommandHold>,
    study: Res<StudyState>,
    run: Res<RunScore>,
    career: Res<CareerScore>,
    ship_upgrades: Res<crate::upgrades::ShipUpgrades>,
) {
    let (Some(model), Ok((ship, vel, nav))) = (model, ships.single()) else {
        return;
    };
    let name = |e: bevy::prelude::Entity| {
        bodies.get(e).map(|b| b.name.clone()).unwrap_or_else(|_| "?".into())
    };
    let nav_text = if let Some(target) = hold.target {
        if hold.out_of_range {
            format!("{}: OUT OF COMMAND RANGE", name(target))
        } else {
            format!("commanding {}: {:.0}%", name(target), hold.progress * 100.0)
        }
    } else {
        match *nav {
            NavState::Free => "free flight — click+hold a body to orbit it".into(),
            NavState::Transfer { target } => format!("transferring to {}", name(target)),
            NavState::Orbiting { body } => format!("orbiting {} (riding along)", name(body)),
        }
    };
    model.0.set_nav(nav_text);
    model.0.set_speed((vel.0.length() / 100.0).round() / 10.0);
    // Equality-checked setters: an unchanged bar costs nothing downstream.
    model.0.set_energy((ship.energy * 10.0).round() / 10.0);
    model.0.set_shield((ship.shield * 10.0).round() / 10.0);
    model.0.set_hull((ship.hull * 10.0).round() / 10.0);
    model.0.set_score(run.total() as f64);
    model.0.set_salvage(run.salvage_value as f64);
    model.0.set_upgrades(crate::upgrades::summary(&ship_upgrades));
    model.0.set_best(career.best_run as f64);
    if let Ok(sun) = suns.single() {
        model.0.set_sun_class(displayed_sun_class(sun.class, study.revealed));
    }
}
