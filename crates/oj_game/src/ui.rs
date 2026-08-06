//! HUD: bevy_pf XAML bound to a view-model with per-property notification —
//! the generated setters only re-apply the bindings whose values changed.

use bevy::prelude::*;
use bevy_pf::prelude::*;

use oj_materials::{Element, UpgradeSlot};

use crate::command::{CommandHold, NavState};
use crate::modules::{CareerScore, RunScore, Stash, StudyState, displayed_sun_class};
use crate::sim::{CelestialBody, Ship, SunBody};

/// A generic icon row (stash items, achievement medals): name, count or
/// spare text, and the fill colour the DataTemplate binds onto an Ellipse.
#[derive(Reflect, Clone, PartialEq, Default)]
struct StashVm {
    name: String,
    count: f64,
    color: String,
    desc: String,
}

fn element_display(e: Element) -> (&'static str, &'static str) {
    match e {
        Element::Iron => ("iron", "#8A8F98"),
        Element::Titanium => ("titanium", "#B8C4D0"),
        Element::Silicon => ("silicon", "#7E97B8"),
        Element::Carbon => ("carbon", "#5A5F68"),
        Element::Ice => ("ice", "#9FD0FF"),
        Element::Uranium => ("uranium", "#7FE08A"),
        Element::Aetherite => ("aetherite", "#C9A0FF"),
    }
}

/// One row of the galaxy map: a candidate destination.
#[derive(Reflect, Clone, PartialEq, Default)]
struct MapRowVm {
    /// Sun label — the studied class, or "?" for the unknown.
    name: String,
    /// Distance and jump cost.
    detail: String,
    color: String,
    /// Row index, passed back as the jump command's parameter.
    param: String,
}

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
    row_shield: String,
    row_range: String,
    row_drive: String,
    row_collector: String,
    row_gravity: String,
    stash: Vec<StashVm>,
    achievements: Vec<StashVm>,
    feed: Vec<String>,
    map: Vec<MapRowVm>,
    map_status: String,
}

#[derive(Resource, Clone)]
struct HudModel(Bindable);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, (update_hud, toggle_panel, log_commands));
    }
}

/// Every XAML command activation, in the log — cheap and permanently
/// useful for driving the UI from automation.
fn log_commands(mut msgs: MessageReader<bevy_pf::binding::PfCommandInvoked>) {
    for m in msgs.read() {
        info!("ui command: {} ({:?})", m.command, m.parameter);
    }
}

fn toggle_panel(
    keys: Res<ButtonInput<KeyCode>>,
    vessel: Option<Res<VesselPanel>>,
    map: Option<Res<MapPanel>>,
    mut vis: Query<&mut Visibility>,
) {
    let flip = |vis: &mut Query<&mut Visibility>, entity: bevy::prelude::Entity| {
        if let Ok(mut v) = vis.get_mut(entity) {
            *v = match *v {
                Visibility::Hidden => Visibility::Inherited,
                _ => Visibility::Hidden,
            };
        }
    };
    let hide = |vis: &mut Query<&mut Visibility>, entity: bevy::prelude::Entity| {
        if let Ok(mut v) = vis.get_mut(entity) {
            *v = Visibility::Hidden;
        }
    };
    // The panels overlap on screen, so opening one closes the other.
    if keys.just_pressed(KeyCode::Tab)
        && let Some(vessel) = &vessel
    {
        if let Some(map) = &map {
            hide(&mut vis, map.0);
        }
        flip(&mut vis, vessel.0);
    }
    if keys.just_pressed(KeyCode::KeyM)
        && let Some(map) = &map
    {
        if let Some(vessel) = &vessel {
            hide(&mut vis, vessel.0);
        }
        flip(&mut vis, map.0);
    }
}

/// Opt an entire UI subtree out of pointer picking.
fn ignore_picking_recursive(world: &mut World, entity: Entity) {
    world.entity_mut(entity).insert(bevy::picking::Pickable::IGNORE);
    let kids: Vec<Entity> = world
        .get::<Children>(entity)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for kid in kids {
        ignore_picking_recursive(world, kid);
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
  <TextBlock Text="[Tab] vessel  [M] galaxy map" Foreground="#5A5F68" Margin="0,4,0,0"/>
</StackPanel>
"##;

const MAP_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Center" VerticalAlignment="Center" Margin="340,12,0,0"
        Background="#D810141C" BorderBrush="#3A4552" BorderThickness="1"
        CornerRadius="8" Padding="12" Width="420">
  <StackPanel>
    <TextBlock Text="GALAXY MAP" Foreground="#9FD0FF" FontSize="15"/>
    <TextBlock Text="{Binding map_status}" Foreground="#C8E06E" Margin="0,4,0,6"/>
    <ItemsControl ItemsSource="{Binding map}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Orientation="Horizontal" Margin="0,3,0,0">
            <Ellipse Width="13" Height="13" Fill="{Binding color}"/>
            <TextBlock Text="{Binding name}" Width="130" Margin="8,0,0,0"/>
            <TextBlock Text="{Binding detail}" Width="170" Foreground="#8A8F98"/>
            <Button Content="jump" Command="{Binding jump}" CommandParameter="{Binding param}"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>
    <TextBlock Text="studied suns are labeled; ? is a gamble. [M] close" Foreground="#5A5F68" Margin="0,8,0,0"/>
  </StackPanel>
</Border>
"##;

const PANEL_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Right" VerticalAlignment="Top" Margin="340,12,0,0"
        Background="#D810141C" BorderBrush="#3A4552" BorderThickness="1"
        CornerRadius="8" Padding="12" Width="320">
  <StackPanel>
    <TextBlock Text="VESSEL" Foreground="#9FD0FF" FontSize="15"/>
    <TextBlock Text="{Binding salvage, StringFormat=salvage: {0}}" Foreground="#C9A96E" Margin="0,4,0,6"/>

    <StackPanel Orientation="Horizontal" Margin="0,3,0,0">
      <Rectangle Width="14" Height="14" RadiusX="3" RadiusY="3" Fill="#6EC1E0"/>
      <TextBlock Text="{Binding row_shield}" Width="200" Margin="8,0,8,0"/>
      <Button Content="craft" Command="{Binding buy_shield}"/>
    </StackPanel>
    <StackPanel Orientation="Horizontal" Margin="0,3,0,0">
      <Rectangle Width="14" Height="14" RadiusX="3" RadiusY="3" Fill="#B8A0E0"/>
      <TextBlock Text="{Binding row_range}" Width="200" Margin="8,0,8,0"/>
      <Button Content="craft" Command="{Binding buy_range}"/>
    </StackPanel>
    <StackPanel Orientation="Horizontal" Margin="0,3,0,0">
      <Rectangle Width="14" Height="14" RadiusX="3" RadiusY="3" Fill="#E0876E"/>
      <TextBlock Text="{Binding row_drive}" Width="200" Margin="8,0,8,0"/>
      <Button Content="craft" Command="{Binding buy_drive}"/>
    </StackPanel>
    <StackPanel Orientation="Horizontal" Margin="0,3,0,0">
      <Rectangle Width="14" Height="14" RadiusX="3" RadiusY="3" Fill="#C8E06E"/>
      <TextBlock Text="{Binding row_collector}" Width="200" Margin="8,0,8,0"/>
      <Button Content="craft" Command="{Binding buy_collector}"/>
    </StackPanel>
    <StackPanel Orientation="Horizontal" Margin="0,3,0,0">
      <Rectangle Width="14" Height="14" RadiusX="3" RadiusY="3" Fill="#C9A0FF"/>
      <TextBlock Text="{Binding row_gravity}" Width="200" Margin="8,0,8,0"/>
      <Button Content="craft" Command="{Binding buy_gravity}"/>
    </StackPanel>

    <TextBlock Text="STASH" Foreground="#9FD0FF" FontSize="15" Margin="0,10,0,2"/>
    <ItemsControl ItemsSource="{Binding stash}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Orientation="Horizontal" Margin="0,2,0,0">
            <Ellipse Width="13" Height="13" Fill="{Binding color}"/>
            <TextBlock Text="{Binding name}" Width="110" Margin="8,0,0,0"/>
            <TextBlock Text="{Binding count}" Foreground="#8A8F98"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>
    <TextBlock Text="ACHIEVEMENTS" Foreground="#9FD0FF" FontSize="15" Margin="0,10,0,2"/>
    <ItemsControl ItemsSource="{Binding achievements}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Orientation="Horizontal" Margin="0,2,0,0">
            <Ellipse Width="13" Height="13" Fill="{Binding color}"/>
            <TextBlock Text="{Binding name}" Width="110" Margin="8,0,0,0"/>
            <TextBlock Text="{Binding desc}" Foreground="#8A8F98" FontSize="11"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>

    <TextBlock Text="FEED (network soon)" Foreground="#9FD0FF" FontSize="15" Margin="0,10,0,2"/>
    <ItemsControl ItemsSource="{Binding feed}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <TextBlock Text="{Binding}" Foreground="#8A8F98" FontSize="11" Margin="0,1,0,0"/>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>
    <TextBlock Text="[Tab] close" Foreground="#5A5F68" Margin="0,8,0,0"/>
  </StackPanel>
</Border>
"##;

/// Root entity of the vessel panel, for the Tab toggle.
#[derive(Resource)]
struct VesselPanel(Entity);

/// Root entity of the galaxy map, for the M toggle.
#[derive(Resource)]
struct MapPanel(Entity);

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
        row_shield: String::new(),
        row_range: String::new(),
        row_drive: String::new(),
        row_collector: String::new(),
        row_gravity: String::new(),
        stash: Vec::new(),
        achievements: Vec::new(),
        feed: Vec::new(),
        map: Vec::new(),
        map_status: String::new(),
    });
    // XAML button commands route into the world through the model.
    for (name, slot) in [
        ("buy_shield", UpgradeSlot::Shield),
        ("buy_range", UpgradeSlot::CommandArray),
        ("buy_drive", UpgradeSlot::RocketDrive),
        ("buy_collector", UpgradeSlot::EnergyCollector),
        ("buy_gravity", UpgradeSlot::GravityDrive),
    ] {
        vm.on_command(name, move |world, _| {
            crate::upgrades::buy_from_world(world, slot);
        });
    }
    // The map's jump buttons pass their row index; the row list resource
    // maps it back to a SystemId for `perform_jump` to consume.
    vm.on_command("jump", |world, param| {
        let Some(index) = param.and_then(|p| p.trim().parse::<usize>().ok()) else { return };
        let target = world
            .resource::<crate::travel::MapRows>()
            .0
            .get(index)
            .copied();
        if let Some(target) = target {
            world.resource_mut::<crate::travel::PendingJump>().0 = Some(target);
        }
    });
    commands.insert_resource(HudModel(vm.clone()));
    commands.queue(move |world: &mut World| {
        enum Panel {
            Hud,
            Vessel,
            Map,
        }
        for (xaml, panel) in [
            (HUD_XAML, Panel::Hud),
            (PANEL_XAML, Panel::Vessel),
            (MAP_XAML, Panel::Map),
        ] {
            let scene = bevy_pf::XamlScene::parse(xaml).expect("ui xaml is valid");
            let root = world.spawn(DataContext(vm.clone())).id();
            if let Err(e) = bevy_pf::instantiate_document(world, root, &scene.document()) {
                error!("ui failed to instantiate: {e}");
            }
            // Instantiation replaces root components; re-attach the context.
            world.entity_mut(root).insert(DataContext(vm.clone()));
            match panel {
                Panel::Hud => {
                    // The HUD is a passive readout, but its root spans the
                    // window and stack rows stretch full-width: with default
                    // Pickable they swallow clicks aimed at panels and the
                    // 3D scene beneath (found by click-path tracing). The
                    // whole tree opts out of picking.
                    ignore_picking_recursive(world, root);
                }
                Panel::Vessel => {
                    world.entity_mut(root).insert(Visibility::Hidden);
                    world.insert_resource(VesselPanel(root));
                }
                Panel::Map => {
                    world.entity_mut(root).insert(Visibility::Hidden);
                    world.insert_resource(MapPanel(root));
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
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
    stash: Res<Stash>,
    achieved: Res<crate::achievements::Unlocked>,
    flash: Res<crate::achievements::LastUnlock>,
    game: Res<crate::GameUniverse>,
    atlas: Res<crate::travel::SunAtlas>,
    mut map_rows: ResMut<crate::travel::MapRows>,
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
    model.0.set_nav(if flash.ttl > 0.0 { flash.text.clone() } else { nav_text });
    model.0.set_speed((vel.0.length() / 100.0).round() / 10.0);
    // Equality-checked setters: an unchanged bar costs nothing downstream.
    model.0.set_energy((ship.energy * 10.0).round() / 10.0);
    model.0.set_shield((ship.shield * 10.0).round() / 10.0);
    model.0.set_hull((ship.hull * 10.0).round() / 10.0);
    model.0.set_score(run.total() as f64);
    model.0.set_salvage(run.salvage_value as f64);
    model.0.set_upgrades(crate::upgrades::summary(&ship_upgrades));
    let u = &ship_upgrades;
    model.0.set_row_shield(crate::upgrades::row(u, "shield", UpgradeSlot::Shield));
    model.0.set_row_range(crate::upgrades::row(u, "command range", UpgradeSlot::CommandArray));
    model.0.set_row_drive(crate::upgrades::row(u, "rocket drive", UpgradeSlot::RocketDrive));
    model.0.set_row_collector(crate::upgrades::row(u, "collector", UpgradeSlot::EnergyCollector));
    model.0.set_row_gravity(crate::upgrades::row(u, "gravity drive", UpgradeSlot::GravityDrive));
    let medals: Vec<StashVm> = crate::achievements::Achievement::ALL
        .iter()
        .map(|a| StashVm {
            name: a.name().into(),
            count: 0.0,
            color: if achieved.0.contains(a) { "#E0C36E".into() } else { "#3A4048".into() },
            desc: a.description().into(),
        })
        .collect();
    model.0.set_achievements(medals);
    let feed: Vec<String> = crate::achievements::read_feed()
        .iter()
        .rev()
        .take(6)
        .map(|r| match r {
            oj_protocol::GlobalRecord::AchievementUnlocked { achievement, .. } => {
                format!("you unlocked {achievement}")
            }
            oj_protocol::GlobalRecord::ScoreFinal { run_score, .. } => {
                format!("run ended: {run_score}")
            }
        })
        .collect();
    model.0.set_feed(feed);
    let mut items: Vec<StashVm> = stash
        .0
        .iter()
        .map(|(e, n)| {
            let (name, color) = element_display(*e);
            StashVm { name: name.into(), count: *n as f64, color: color.into(), desc: String::new() }
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    model.0.set_stash(items);
    model.0.set_best(career.best_run as f64);
    if let Ok(sun) = suns.single() {
        model.0.set_sun_class(displayed_sun_class(sun.class, study.revealed));
    }

    // Galaxy map: the six nearest systems, studied suns labeled from the
    // atlas, priced by distance. Row order backs the jump parameter.
    let nearby = crate::travel::nearby_systems(&game, 6);
    map_rows.0 = nearby.iter().map(|(id, _)| *id).collect();
    let rows: Vec<MapRowVm> = nearby
        .iter()
        .enumerate()
        .map(|(i, (id, dist))| {
            let cost = crate::travel::jump_cost(*dist);
            let affordable = ship.energy >= cost;
            let name = match atlas.0.get(id) {
                Some(class) => format!("{class:?} sun"),
                None => "? unstudied".into(),
            };
            MapRowVm {
                name,
                detail: format!("{:.1} ly — {:.0} energy", dist / crate::travel::LY, cost),
                color: match (atlas.0.contains_key(id), affordable) {
                    (_, false) => "#7A3A3A".into(),
                    (true, true) => "#E0C36E".into(),
                    (false, true) => "#3A4048".into(),
                },
                param: i.to_string(),
            }
        })
        .collect();
    model.0.set_map(rows);
    model.0.set_map_status(format!(
        "system {:?} of sector ({}, {}, {}) — energy {:.0}",
        game.current.index,
        game.current.sector.x,
        game.current.sector.y,
        game.current.sector.z,
        ship.energy
    ));
}
