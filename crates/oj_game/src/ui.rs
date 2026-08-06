//! HUD: bevy_pf XAML bound to a view-model with per-property notification —
//! the generated setters only re-apply the bindings whose values changed.

use bevy::prelude::*;
use bevy_pf::prelude::*;

use crate::sim::Ship;

#[derive(Reflect, Default, Bindable)]
struct HudVm {
    energy: f64,
    shield: f64,
    hull: f64,
    sun_class: String,
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
</StackPanel>
"##;

fn spawn_hud(mut commands: Commands, game: Res<crate::GameUniverse>) {
    let vm = Bindable::new(HudVm {
        energy: 100.0,
        shield: 100.0,
        hull: 100.0,
        sun_class: game
            .universe
            .system(game.current)
            .map(|s| format!("{:?} (unstudied)", s.sun.class))
            .unwrap_or_else(|| "??".into()),
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

fn update_hud(model: Option<Res<HudModel>>, ships: Query<&Ship>) {
    let (Some(model), Ok(ship)) = (model, ships.single()) else {
        return;
    };
    // Equality-checked setters: an unchanged bar costs nothing downstream.
    model.0.set_energy((ship.energy * 10.0).round() / 10.0);
    model.0.set_shield((ship.shield * 10.0).round() / 10.0);
    model.0.set_hull((ship.hull * 10.0).round() / 10.0);
}
