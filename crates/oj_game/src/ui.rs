//! HUD: bevy_pf XAML bound to a view-model with per-property notification —
//! the generated setters only re-apply the bindings whose values changed.
//!
//! Visual design: "orbit_jumper OS" holographic glass, generated with
//! Google Stitch (project 8423485048351977019, screens "HUD: Vessel
//! Systems" + "HUD: Galaxy Map") and translated to bevy_pf XAML. Flat
//! colors, 1px cyan borders, technical uppercase labels. Palette:
//! bg #10131A, text #E0E2EB, cyan #00E5FF, energy #00FFD4,
//! shield #00A2FF, hull #FF7043, amber #FFB454, dim #5A6472.

use bevy::prelude::*;
use bevy_pf::prelude::*;

use oj_materials::{Element, UpgradeSlot};

use crate::command::{CommandHold, NavState};
use crate::modules::{CareerScore, RunScore, Stash, StudyState};
use crate::sim::{CelestialBody, Ship, SunBody};

/// A stash chip: "FE: 12" in the element's color.
#[derive(Reflect, Clone, PartialEq, Default)]
struct StashVm {
    name: String,
    color: String,
}

/// An achievement medal square; gold when unlocked.
#[derive(Reflect, Clone, PartialEq, Default)]
struct MedalVm {
    color: String,
}

/// One upgrade row of the vessel panel.
#[derive(Reflect, Clone, PartialEq, Default)]
struct UpgradeRowVm {
    name: String,
    tier: String,
    cost: String,
    color: String,
    /// Slot index, passed back as the craft command's parameter.
    param: String,
}

/// One destination row of the galaxy map.
#[derive(Reflect, Clone, PartialEq, Default)]
struct MapRowVm {
    /// Sun classification if studied, "???" otherwise.
    name: String,
    dist: String,
    cost: String,
    color: String,
    /// Row index, passed back as the jump command's parameter.
    param: String,
}

/// Element symbol + chip color for the stash readout.
fn element_display(e: Element) -> (&'static str, &'static str) {
    match e {
        Element::Iron => ("FE", "#8A8F98"),
        Element::Titanium => ("TI", "#B8C4D0"),
        Element::Silicon => ("SI", "#7E97B8"),
        Element::Carbon => ("C", "#9AA1AB"),
        Element::Ice => ("ICE", "#9FD0FF"),
        Element::Uranium => ("U", "#7FE08A"),
        Element::Aetherite => ("AE", "#C9A0FF"),
    }
}

#[derive(Reflect, Default, Bindable)]
struct HudVm {
    energy: f64,
    shield: f64,
    hull: f64,
    energy_text: String,
    shield_text: String,
    hull_text: String,
    sun_class: String,
    nav: String,
    speed: String,
    score: String,
    best: String,
    salvage: String,
    rows: Vec<UpgradeRowVm>,
    stash: Vec<StashVm>,
    medals: Vec<MedalVm>,
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

// Design tokens (Stitch "orbit_jumper OS").
// glass panel: Background #F00D131C, BorderBrush #1E3A44, 1px, sharp.
// title cyan #00E5FF; body #E0E2EB; labels/dim #5A6472; grid line #22313C.

const HUD_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12"
           >

  <Border Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1" Padding="10,8" Width="236">
    <StackPanel>
      <TextBlock Text="SYSTEM DETECTED" Foreground="#00E5FF" FontSize="10"/>
      <TextBlock Text="{Binding sun_class}" Foreground="#E0E2EB" FontSize="15" FontWeight="Bold" Margin="0,3,0,0"/>
    </StackPanel>
  </Border>

  <Border Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1" Padding="10,8" Width="236" Margin="0,8,0,0">
    <StackPanel>
      <StackPanel Orientation="Horizontal">
        <TextBlock Text="ENERGY" Foreground="#00FFD4" FontSize="10" Width="150"/>
        <TextBlock Text="{Binding energy_text}" Foreground="#00FFD4" FontSize="10"/>
      </StackPanel>
      <ProgressBar Width="214" Height="5" Maximum="100" Value="{Binding energy}"
                   Foreground="#00FFD4" Background="#0A1420" BorderBrush="#16222E" Margin="0,3,0,0"/>
      <StackPanel Orientation="Horizontal" Margin="0,7,0,0">
        <TextBlock Text="SHIELD" Foreground="#00A2FF" FontSize="10" Width="150"/>
        <TextBlock Text="{Binding shield_text}" Foreground="#00A2FF" FontSize="10"/>
      </StackPanel>
      <ProgressBar Width="214" Height="5" Maximum="100" Value="{Binding shield}"
                   Foreground="#00A2FF" Background="#0A1420" BorderBrush="#16222E" Margin="0,3,0,0"/>
      <StackPanel Orientation="Horizontal" Margin="0,7,0,0">
        <TextBlock Text="HULL" Foreground="#FF7043" FontSize="10" Width="150"/>
        <TextBlock Text="{Binding hull_text}" Foreground="#FF7043" FontSize="10"/>
      </StackPanel>
      <ProgressBar Width="214" Height="5" Maximum="100" Value="{Binding hull}"
                   Foreground="#FF7043" Background="#0A1420" BorderBrush="#16222E" Margin="0,3,0,0"/>
    </StackPanel>
  </Border>

  <Border Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1" Padding="10,8" Width="236" Margin="0,8,0,0">
    <StackPanel>
      <TextBlock Text="{Binding nav}" Foreground="#00E5FF" FontSize="10"/>
      <TextBlock Text="VELOCITY" Foreground="#5A6472" FontSize="10" Margin="0,6,0,0"/>
      <TextBlock Text="{Binding speed}" Foreground="#E0E2EB" FontSize="17" FontWeight="Bold"/>
      <Rectangle Width="214" Height="1" Fill="#22313C" Margin="0,7,0,7"/>
      <StackPanel Orientation="Horizontal">
        <StackPanel Width="107">
          <TextBlock Text="SCORE" Foreground="#5A6472" FontSize="10"/>
          <TextBlock Text="{Binding score}" Foreground="#E0E2EB" FontSize="12"/>
        </StackPanel>
        <StackPanel>
          <TextBlock Text="BEST" Foreground="#5A6472" FontSize="10"/>
          <TextBlock Text="{Binding best}" Foreground="#E0E2EB" FontSize="12"/>
        </StackPanel>
      </StackPanel>
      <TextBlock Text="SALVAGE" Foreground="#5A6472" FontSize="10" Margin="0,5,0,0"/>
      <TextBlock Text="{Binding salvage}" Foreground="#FFB454" FontSize="12"/>
    </StackPanel>
  </Border>

  <TextBlock Text="[TAB] VESSEL   [M] MAP   [S] STUDY" Foreground="#3A4650" FontSize="10" Margin="2,8,0,0"/>
</StackPanel>
"##;

const PANEL_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Left" VerticalAlignment="Top" Margin="340,12,0,0"
        Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1"
        Padding="12,10" Width="330">
  <StackPanel>
    <TextBlock Text="VESSEL" Foreground="#00E5FF" FontSize="14"/>
    <Rectangle Width="304" Height="1" Fill="#00E5FF" Margin="0,5,0,8"/>

    <ItemsControl ItemsSource="{Binding rows}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Orientation="Horizontal" Margin="0,4,0,0">
            <Rectangle Width="9" Height="9" Fill="{Binding color}" Margin="0,4,0,0"/>
            <StackPanel Width="150" Margin="9,0,0,0">
              <TextBlock Text="{Binding name}" Foreground="#E0E2EB" FontSize="11"/>
              <TextBlock Text="{Binding tier}" Foreground="#5A6472" FontSize="10"/>
            </StackPanel>
            <TextBlock Text="{Binding cost}" Foreground="#FFB454" FontSize="10" Width="72" Margin="0,4,0,0"/>
            <Button Content="CRAFT" Command="{Binding craft}" CommandParameter="{Binding param}"
                    Background="#0B1B22" BorderBrush="#00E5FF" Foreground="#00E5FF"
                    FontSize="10" Padding="7,2"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>

    <TextBlock Text="STASH" Foreground="#5A6472" FontSize="10" Margin="0,12,0,3"/>
    <ItemsControl ItemsSource="{Binding stash}">
      <ItemsControl.ItemsPanel>
        <ItemsPanelTemplate><StackPanel Orientation="Horizontal"/></ItemsPanelTemplate>
      </ItemsControl.ItemsPanel>
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <Border BorderBrush="#31353C" BorderThickness="1" Padding="6,2" Margin="0,0,6,0">
            <TextBlock Text="{Binding name}" Foreground="{Binding color}" FontSize="10"/>
          </Border>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>

    <TextBlock Text="ACHIEVEMENTS" Foreground="#5A6472" FontSize="10" Margin="0,12,0,3"/>
    <ItemsControl ItemsSource="{Binding medals}">
      <ItemsControl.ItemsPanel>
        <ItemsPanelTemplate><StackPanel Orientation="Horizontal"/></ItemsPanelTemplate>
      </ItemsControl.ItemsPanel>
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <Border BorderBrush="#31353C" BorderThickness="1" Width="22" Height="22" Margin="0,0,6,0"
                  Background="#0A1118" Padding="5">
            <Rectangle Width="10" Height="10" Fill="{Binding color}"/>
          </Border>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>

    <Rectangle Width="304" Height="1" Fill="#22313C" Margin="0,12,0,6"/>
    <ItemsControl ItemsSource="{Binding feed}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <TextBlock Text="{Binding}" Foreground="#5A6472" FontSize="10" Margin="0,2,0,0"/>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>
    <TextBlock Text="[TAB] CLOSE" Foreground="#3A4650" FontSize="10" Margin="0,10,0,0"/>
  </StackPanel>
</Border>
"##;

const MAP_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Left" VerticalAlignment="Top" Margin="340,12,0,0"
        Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1"
        Padding="12,10" Width="470">
  <StackPanel>
    <StackPanel Orientation="Horizontal">
      <TextBlock Text="GALAXY MAP" Foreground="#00E5FF" FontSize="14" Width="170"/>
      <TextBlock Text="{Binding map_status}" Foreground="#5A6472" FontSize="10" Margin="0,4,0,0"/>
    </StackPanel>
    <Rectangle Width="444" Height="1" Fill="#00E5FF" Margin="0,5,0,6"/>

    <StackPanel Orientation="Horizontal" Margin="0,2,0,2">
      <TextBlock Text="DESTINATION" Foreground="#5A6472" FontSize="10" Width="190"/>
      <TextBlock Text="DIST (LY)" Foreground="#5A6472" FontSize="10" Width="90"/>
      <TextBlock Text="COST (E)" Foreground="#5A6472" FontSize="10" Width="90"/>
      <TextBlock Text="ACTION" Foreground="#5A6472" FontSize="10"/>
    </StackPanel>
    <Rectangle Width="444" Height="1" Fill="#22313C"/>

    <ItemsControl ItemsSource="{Binding map}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Margin="0,0,0,0">
            <StackPanel Orientation="Horizontal" Margin="0,6,0,6">
              <TextBlock Text="{Binding name}" Foreground="{Binding color}" FontSize="11" Width="190"/>
              <TextBlock Text="{Binding dist}" Foreground="#E0E2EB" FontSize="11" Width="90"/>
              <TextBlock Text="{Binding cost}" Foreground="#FFB454" FontSize="11" Width="90"/>
              <Button Content="JUMP" Command="{Binding jump}" CommandParameter="{Binding param}"
                      Background="#0B1B22" BorderBrush="#00E5FF" Foreground="#00E5FF"
                      FontSize="10" Padding="9,2"/>
            </StackPanel>
            <Rectangle Width="444" Height="1" Fill="#1A2530"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>

    <TextBlock Text="[S] STUDY REVEALS SUN CLASS — ??? IS A GAMBLE   [M] CLOSE"
               Foreground="#3A4650" FontSize="10" Margin="0,8,0,0"/>
  </StackPanel>
</Border>
"##;

/// Root entity of the vessel panel, for the Tab toggle.
#[derive(Resource)]
struct VesselPanel(Entity);

/// Root entity of the galaxy map, for the M toggle.
#[derive(Resource)]
struct MapPanel(Entity);

/// Craftable slots, in vessel-panel row order; the craft command's
/// parameter indexes this.
const CRAFT_SLOTS: [(UpgradeSlot, &str, &str); 5] = [
    (UpgradeSlot::Shield, "SHIELD PLATING", "#00A2FF"),
    (UpgradeSlot::CommandArray, "COMMAND ARRAY", "#B48CFF"),
    (UpgradeSlot::RocketDrive, "ROCKET DRIVE", "#FF7043"),
    (UpgradeSlot::EnergyCollector, "COLLECTOR", "#00FFD4"),
    (UpgradeSlot::GravityDrive, "GRAVITY DRIVE", "#FF4FD8"),
];

fn spawn_hud(mut commands: Commands) {
    let vm = Bindable::new(HudVm::default());
    // The vessel panel's craft buttons pass their row index.
    vm.on_command("craft", |world, param| {
        let Some(index) = param.and_then(|p| p.trim().parse::<usize>().ok()) else { return };
        let Some((slot, _, _)) = CRAFT_SLOTS.get(index) else { return };
        crate::upgrades::buy_from_world(world, *slot);
    });
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
        bodies
            .get(e)
            .map(|b| b.name.to_uppercase())
            .unwrap_or_else(|_| "?".into())
    };
    let nav_text = if let Some(target) = hold.target {
        if hold.out_of_range {
            format!(">> {}: OUT OF RANGE", name(target))
        } else {
            format!(">> COMMANDING {}: {:.0}%", name(target), hold.progress * 100.0)
        }
    } else {
        match *nav {
            NavState::Free => ">> FREE FLIGHT — CLICK+HOLD A BODY".into(),
            NavState::Transfer { target } => format!(">> TRANSFER: {}", name(target)),
            NavState::Orbiting { body } => format!(">> ORBIT LOCK: {}", name(body)),
        }
    };
    model.0.set_nav(if flash.ttl > 0.0 {
        format!("++ {}", flash.text.to_uppercase())
    } else {
        nav_text
    });
    model.0.set_speed(format!("{:.1} KM/S", vel.0.length() / 1000.0));
    // Equality-checked setters: an unchanged readout costs nothing downstream.
    model.0.set_energy((ship.energy / ship.energy_max * 100.0).round());
    model.0.set_shield(ship.shield.round());
    model.0.set_hull(ship.hull.round());
    model.0.set_energy_text(format!("{:.0}/{:.0}", ship.energy, ship.energy_max));
    model.0.set_shield_text(format!("{:.0}/100", ship.shield));
    model.0.set_hull_text(format!("{:.0}/100", ship.hull));
    model.0.set_score(format!("{}", run.total()));
    model.0.set_best(format!("{}", career.best_run));
    model.0.set_salvage(format!("{} CR", run.salvage_value));

    if let Ok(sun) = suns.single() {
        model.0.set_sun_class(if study.revealed {
            format!(
                "{:?}-CLASS — SHIELD T{}",
                sun.class,
                sun.class.required_shield_tier()
            )
        } else {
            "UNCLASSIFIED".into()
        });
    }

    // Vessel panel rows.
    let rows: Vec<UpgradeRowVm> = CRAFT_SLOTS
        .iter()
        .enumerate()
        .map(|(i, (slot, label, color))| UpgradeRowVm {
            name: (*label).into(),
            tier: format!("TIER {}", ship_upgrades.tier(*slot)),
            cost: match ship_upgrades.next_cost(*slot) {
                Some(c) => format!("{c} CR"),
                None => "MAXED".into(),
            },
            color: (*color).into(),
            param: i.to_string(),
        })
        .collect();
    model.0.set_rows(rows);

    let mut chips: Vec<StashVm> = stash
        .0
        .iter()
        .map(|(e, n)| {
            let (sym, color) = element_display(*e);
            StashVm { name: format!("{sym}: {n}"), color: color.into() }
        })
        .collect();
    chips.sort_by(|a, b| a.name.cmp(&b.name));
    model.0.set_stash(chips);

    let medals: Vec<MedalVm> = crate::achievements::Achievement::ALL
        .iter()
        .map(|a| MedalVm {
            color: if achieved.0.contains(a) { "#FFB454".into() } else { "#1B2733".into() },
        })
        .collect();
    model.0.set_medals(medals);

    let feed: Vec<String> = crate::achievements::read_feed()
        .iter()
        .rev()
        .take(5)
        .map(|r| match r {
            oj_protocol::GlobalRecord::AchievementUnlocked { achievement, .. } => {
                format!("> UNLOCKED: {}", achievement.to_uppercase())
            }
            oj_protocol::GlobalRecord::ScoreFinal { run_score, .. } => {
                format!("> RUN ENDED: {run_score}")
            }
        })
        .collect();
    model.0.set_feed(feed);

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
            let (name, color) = match atlas.0.get(id) {
                Some(class) => (
                    format!("{class:?}-CLASS SUN"),
                    if affordable { "#00E5FF" } else { "#FF5459" },
                ),
                None => ("??? UNSTUDIED".into(), if affordable { "#5A6472" } else { "#FF5459" }),
            };
            MapRowVm {
                name,
                color: color.into(),
                dist: format!("{:.1}", dist / crate::travel::LY),
                cost: format!("{cost:.0}"),
                param: i.to_string(),
            }
        })
        .collect();
    model.0.set_map(rows);
    model.0.set_map_status(format!(
        "SECTOR ({},{},{}) · SYS {} — ENERGY {:.0}",
        game.current.sector.x,
        game.current.sector.y,
        game.current.sector.z,
        game.current.index,
        ship.energy
    ));
}
