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
    level: String,
    style: String,
    threat: String,
    heading: String,
}

#[derive(Resource, Clone)]
struct HudModel(Bindable);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiLayoutState>()
            .add_systems(Startup, init_model)
            .add_systems(Update, (update_hud, toggle_panel, log_commands))
            .add_systems(Update, relayout_ui)
            // The touch-key bridge must run AFTER bevy's per-frame input
            // clear and AFTER UI focus computes Interaction, or the
            // just_pressed edge is wiped before any consumer (FixedUpdate
            // weapons, Update toggles) can see it.
            .add_systems(
                PreUpdate,
                sync_touch_keys.after(bevy::ui::UiSystems::Focus),
            );
    }
}

/// Which layout the UI is currently built for. Phone = the window's short
/// side is phone-sized; portrait vs landscape picks the arrangement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UiMode {
    Desktop,
    PhonePortrait,
    PhoneLandscape,
}

impl UiMode {
    fn of(width: f32, height: f32) -> Self {
        if width.min(height) < 500.0 {
            if height > width { Self::PhonePortrait } else { Self::PhoneLandscape }
        } else {
            Self::Desktop
        }
    }
}

#[derive(Resource, Default)]
struct UiLayoutState(Option<(UiMode, crate::sim::ViewMode)>);

/// Marks every UI document root so a relayout can tear the set down.
#[derive(Component)]
struct UiRoot;

/// An on-screen control that presses/releases a key while touched — the
/// bridge that lets every keyboard-driven game system work on a phone
/// without knowing touch exists.
#[derive(Component)]
struct TouchKey(KeyCode);

/// Drive `ButtonInput<KeyCode>` from on-screen control state. Transition
/// edges only, so `just_pressed` semantics (panel toggles, missile
/// triggers) behave exactly like a physical key tap.
fn sync_touch_keys(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    controls: Query<(&Interaction, &TouchKey), Changed<Interaction>>,
) {
    for (interaction, key) in &controls {
        match interaction {
            Interaction::Pressed => keys.press(key.0),
            _ => keys.release(key.0),
        }
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
      <TextBlock Text="{Binding threat}" Foreground="#FF5459" FontSize="10"/>
      <TextBlock Text="VELOCITY" Foreground="#5A6472" FontSize="10" Margin="0,6,0,0"/>
      <TextBlock Text="{Binding speed}" Foreground="#E0E2EB" FontSize="17" FontWeight="Bold"/>
      <Rectangle Width="214" Height="1" Fill="#22313C" Margin="0,7,0,7"/>
      <StackPanel Orientation="Horizontal">
        <StackPanel Width="76">
          <TextBlock Text="SCORE" Foreground="#5A6472" FontSize="10"/>
          <TextBlock Text="{Binding score}" Foreground="#E0E2EB" FontSize="12"/>
        </StackPanel>
        <StackPanel Width="76">
          <TextBlock Text="BEST" Foreground="#5A6472" FontSize="10"/>
          <TextBlock Text="{Binding best}" Foreground="#E0E2EB" FontSize="12"/>
        </StackPanel>
        <StackPanel>
          <TextBlock Text="PILOT" Foreground="#5A6472" FontSize="10"/>
          <TextBlock Text="{Binding level}" Foreground="#00E5FF" FontSize="12"/>
        </StackPanel>
      </StackPanel>
      <TextBlock Text="SALVAGE" Foreground="#5A6472" FontSize="10" Margin="0,5,0,0"/>
      <TextBlock Text="{Binding salvage}" Foreground="#FFB454" FontSize="12"/>
    </StackPanel>
  </Border>

  <TextBlock Text="[TAB] VESSEL  [M] MAP  [S] STUDY  [F] COCKPIT  [E/Q] VERT" Foreground="#3A4650" FontSize="10" Margin="2,8,0,0"/>
</StackPanel>
"##;

const PANEL_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Left" VerticalAlignment="Top" Margin="@PM"
        Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1"
        Padding="12,10" Width="@PW">
  <StackPanel>
    <TextBlock Text="VESSEL" Foreground="#00E5FF" FontSize="14"/>
    <Rectangle Width="@PS" Height="1" Fill="#00E5FF" Margin="0,5,0,8"/>

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

    <TextBlock Text="SHIP YARD" Foreground="#5A6472" FontSize="10" Margin="0,12,0,3"/>
    <TextBlock Text="{Binding style}" Foreground="#00E5FF" FontSize="11"/>
    <StackPanel Orientation="Horizontal" Margin="0,4,0,0">
      <Button Content="FRAME" Command="style_frame" Background="#0B1B22" BorderBrush="#00E5FF"
              Foreground="#00E5FF" FontSize="10" Padding="8,3" Margin="0,0,6,0"/>
      <Button Content="PAINT" Command="style_paint" Background="#0B1B22" BorderBrush="#00E5FF"
              Foreground="#00E5FF" FontSize="10" Padding="8,3" Margin="0,0,6,0"/>
      <Button Content="ACCENT" Command="style_accent" Background="#0B1B22" BorderBrush="#00E5FF"
              Foreground="#00E5FF" FontSize="10" Padding="8,3"/>
    </StackPanel>

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

    <Rectangle Width="@PS" Height="1" Fill="#22313C" Margin="0,12,0,6"/>
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

// Cockpit docs (Stitch screen 'HUD: Cockpit View', project
// 8423485048351977019): bars top-left, heading tape top-center, threat
// chip top-right, reticle center, console strip bottom. Sensors, meta
// and management stay in the tactical overview.
const COCKPIT_TAPE_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Center" VerticalAlignment="Top" Margin="0,10,0,0"
        Background="#D80B111A" BorderBrush="#1E3A44" BorderThickness="1" Padding="14,4">
  <TextBlock Text="{Binding heading}" Foreground="#00E5FF" FontSize="12"/>
</Border>
"##;

const COCKPIT_RETICLE_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Center" VerticalAlignment="Center">
  <Ellipse Width="54" Height="54" Stroke="#8000E5FF" StrokeThickness="1" Fill="#00000000"/>
</StackPanel>
"##;

const COCKPIT_THREAT_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,10,12,0">
  <TextBlock Text="{Binding threat}" Foreground="#FF5459" FontSize="11"/>
</StackPanel>
"##;

const COCKPIT_CONSOLE_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Center" VerticalAlignment="Bottom" Margin="0,0,0,8"
        Background="#D80B111A" BorderBrush="#1E3A44" BorderThickness="1" Padding="18,6">
  <StackPanel>
    <TextBlock Text="{Binding speed}" Foreground="#E0E2EB" FontSize="16" FontWeight="Bold"
               HorizontalAlignment="Center"/>
    <TextBlock Text="{Binding nav}" Foreground="#00E5FF" FontSize="10" Margin="0,2,0,0"/>
  </StackPanel>
</Border>
"##;

const HUD_COCKPIT_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12"
           >
  <Border Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1" Padding="10,8" Width="236">
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
  <TextBlock Text="[F] TACTICAL  [E/Q] VERT  [Z] LASER  [X] MSL  [C/V] WELLS" Foreground="#3A4650" FontSize="10" Margin="2,8,0,0"/>
</StackPanel>
"##;

const MAP_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Left" VerticalAlignment="Top" Margin="@MM"
        Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1"
        Padding="12,10" Width="@MW">
  <StackPanel>
    <StackPanel Orientation="Horizontal">
      <TextBlock Text="GALAXY MAP" Foreground="#00E5FF" FontSize="14" Width="150"/>
      <TextBlock Text="{Binding map_status}" Foreground="#5A6472" FontSize="10" Margin="0,4,0,0"/>
    </StackPanel>
    <Rectangle Width="@MS" Height="1" Fill="#00E5FF" Margin="0,5,0,6"/>

    <StackPanel Orientation="Horizontal" Margin="0,2,0,2">
      <TextBlock Text="DESTINATION" Foreground="#5A6472" FontSize="10" Width="@C1"/>
      <TextBlock Text="DIST" Foreground="#5A6472" FontSize="10" Width="@C2"/>
      <TextBlock Text="COST" Foreground="#5A6472" FontSize="10" Width="@C3"/>
      <TextBlock Text="ACTION" Foreground="#5A6472" FontSize="10"/>
    </StackPanel>
    <Rectangle Width="@MS" Height="1" Fill="#22313C"/>

    <ItemsControl ItemsSource="{Binding map}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Margin="0,0,0,0">
            <StackPanel Orientation="Horizontal" Margin="0,6,0,6">
              <TextBlock Text="{Binding name}" Foreground="{Binding color}" FontSize="11" Width="@C1"/>
              <TextBlock Text="{Binding dist}" Foreground="#E0E2EB" FontSize="11" Width="@C2"/>
              <TextBlock Text="{Binding cost}" Foreground="#FFB454" FontSize="11" Width="@C3"/>
              <Button Content="JUMP" Command="{Binding jump}" CommandParameter="{Binding param}"
                      Background="#0B1B22" BorderBrush="#00E5FF" Foreground="#00E5FF"
                      FontSize="10" Padding="9,2"/>
            </StackPanel>
            <Rectangle Width="@MS" Height="1" Fill="#1A2530"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>

    <TextBlock Text="[S] STUDY REVEALS SUN CLASS — ??? IS A GAMBLE   [M] CLOSE"
               Foreground="#3A4650" FontSize="10" Margin="0,8,0,0"/>
  </StackPanel>
</Border>
"##;

// Touch controls: chunky targets, same glass language. Buttons press
// virtual keys (TouchKey), so game systems stay input-agnostic.
const TOUCH_TOPBAR_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,8,8,0"
            Orientation="Horizontal">
  <Button x:Name="btn_vessel" Content="VESSEL" Background="#D80B1B22" BorderBrush="#00E5FF"
          Foreground="#00E5FF" FontSize="12" Padding="12,10" Margin="0,0,6,0"/>
  <Button x:Name="btn_map" Content="MAP" Background="#D80B1B22" BorderBrush="#00E5FF"
          Foreground="#00E5FF" FontSize="12" Padding="12,10" Margin="0,0,6,0"/>
  <Button x:Name="btn_study" Content="STUDY" Background="#D80B1B22" BorderBrush="#FFB454"
          Foreground="#FFB454" FontSize="12" Padding="12,10" Margin="0,0,6,0"/>
  <Button x:Name="btn_view" Content="VIEW" Background="#D80B1B22" BorderBrush="#B48CFF"
          Foreground="#B48CFF" FontSize="12" Padding="12,10"/>
</StackPanel>
"##;

const TOUCH_TOPBAR_COCKPIT_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,8,8,0"
            Orientation="Horizontal">
  <Button x:Name="btn_view" Content="VIEW" Background="#D80B1B22" BorderBrush="#B48CFF"
          Foreground="#B48CFF" FontSize="12" Padding="12,10"/>
</StackPanel>
"##;

const TOUCH_THRUST_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Left" VerticalAlignment="Bottom" Margin="@TM">
  <StackPanel Orientation="Horizontal">
    <Button x:Name="btn_up" Content="PRO" Width="66" Background="#D80B1B22" BorderBrush="#00FFD4"
            Foreground="#00FFD4" FontSize="13" Padding="0,14" Margin="0,0,6,0"/>
    <Button x:Name="btn_down" Content="RET" Width="66" Background="#D80B1B22" BorderBrush="#00FFD4"
            Foreground="#00FFD4" FontSize="13" Padding="0,14"/>
  </StackPanel>
  <StackPanel Orientation="Horizontal" Margin="0,6,0,0">
    <Button x:Name="btn_left" Content="IN" Width="66" Background="#D80B1B22" BorderBrush="#00FFD4"
            Foreground="#00FFD4" FontSize="13" Padding="0,14" Margin="0,0,6,0"/>
    <Button x:Name="btn_right" Content="OUT" Width="66" Background="#D80B1B22" BorderBrush="#00FFD4"
            Foreground="#00FFD4" FontSize="13" Padding="0,14"/>
  </StackPanel>
  <StackPanel Orientation="Horizontal" Margin="0,6,0,0">
    <Button x:Name="btn_climb" Content="VERT+" Width="66" Background="#D80B1B22" BorderBrush="#B48CFF"
            Foreground="#B48CFF" FontSize="13" Padding="0,14" Margin="0,0,6,0"/>
    <Button x:Name="btn_dive" Content="VERT-" Width="66" Background="#D80B1B22" BorderBrush="#B48CFF"
            Foreground="#B48CFF" FontSize="13" Padding="0,14"/>
  </StackPanel>
</StackPanel>
"##;

const TOUCH_WEAPONS_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Bottom" Margin="0,0,8,8">
  <StackPanel Orientation="Horizontal">
    <Button x:Name="btn_las" Content="LAS" Width="66" Background="#D80B1B22" BorderBrush="#FF7043"
            Foreground="#FF7043" FontSize="13" Padding="0,14" Margin="0,0,6,0"/>
    <Button x:Name="btn_msl" Content="MSL" Width="66" Background="#D80B1B22" BorderBrush="#FF7043"
            Foreground="#FF7043" FontSize="13" Padding="0,14"/>
  </StackPanel>
  <StackPanel Orientation="Horizontal" Margin="0,6,0,0">
    <Button x:Name="btn_pull" Content="PULL" Width="66" Background="#D80B1B22" BorderBrush="#B48CFF"
            Foreground="#B48CFF" FontSize="13" Padding="0,14" Margin="0,0,6,0"/>
    <Button x:Name="btn_push" Content="PUSH" Width="66" Background="#D80B1B22" BorderBrush="#B48CFF"
            Foreground="#B48CFF" FontSize="13" Padding="0,14"/>
  </StackPanel>
</StackPanel>
"##;

/// btn_* name -> virtual key, for wiring TouchKey after instantiation.
const TOUCH_KEYS: [(&str, KeyCode); 14] = [
    ("btn_view", KeyCode::KeyF),
    ("btn_climb", KeyCode::KeyE),
    ("btn_dive", KeyCode::KeyQ),
    ("btn_vessel", KeyCode::Tab),
    ("btn_map", KeyCode::KeyM),
    ("btn_study", KeyCode::KeyS),
    ("btn_up", KeyCode::ArrowUp),
    ("btn_down", KeyCode::ArrowDown),
    ("btn_left", KeyCode::ArrowLeft),
    ("btn_right", KeyCode::ArrowRight),
    ("btn_las", KeyCode::KeyZ),
    ("btn_msl", KeyCode::KeyX),
    ("btn_pull", KeyCode::KeyC),
    ("btn_push", KeyCode::KeyV),
];

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

fn init_model(mut commands: Commands) {
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
    // Ship-yard styling: cycle an index, persist, rebuild the visuals.
    for (name, which) in [("style_frame", 0usize), ("style_paint", 1), ("style_accent", 2)] {
        vm.on_command(name, move |world, _| {
            {
                let mut style = world.resource_mut::<crate::sim::ShipStyle>();
                match which {
                    0 => style.frame = (style.frame + 1) % crate::sim::SHIP_FRAMES.len(),
                    1 => style.paint = (style.paint + 1) % crate::sim::SHIP_PAINTS.len(),
                    _ => style.accent = (style.accent + 1) % crate::sim::SHIP_ACCENTS.len(),
                }
                style.save();
            }
            crate::sim::restyle_ship(world);
        });
    }
    commands.insert_resource(HudModel(vm));
}

/// Per-mode sizing substituted into the XAML templates.
struct Metrics {
    panel_margin: String,
    panel_w: i32,
    map_margin: String,
    map_w: i32,
    /// Map columns: destination / dist / cost.
    cols: (i32, i32, i32),
    touch_controls: bool,
    /// Bottom-left thrust cluster margin (clears the HUD in landscape).
    thrust_margin: String,
}

impl Metrics {
    fn for_mode(mode: UiMode, win_w: f32) -> Self {
        match mode {
            UiMode::Desktop => Self {
                panel_margin: "340,12,0,0".into(),
                panel_w: 330,
                map_margin: "340,12,0,0".into(),
                map_w: 470,
                cols: (190, 90, 90),
                touch_controls: false,
                thrust_margin: "8,0,0,8".into(),
            },
            // Landscape phone: desktop arrangement (it fits), plus touch
            // controls in the corners.
            UiMode::PhoneLandscape => Self {
                touch_controls: true,
                thrust_margin: "260,0,0,8".into(),
                ..Self::for_mode(UiMode::Desktop, win_w)
            },
            // Portrait: panels become near-full-width overlays with
            // compressed map columns; controls on.
            UiMode::PhonePortrait => {
                let w = (win_w as i32 - 16).clamp(280, 400);
                Self {
                    panel_margin: "8,64,0,0".into(),
                    panel_w: w.min(330),
                    map_margin: "8,64,0,0".into(),
                    map_w: w,
                    cols: (118, 56, 56),
                    touch_controls: true,
                    thrust_margin: "8,0,0,8".into(),
                }
            }
        }
    }

    fn fill(&self, template: &str) -> String {
        template
            .replace("@PM", &self.panel_margin)
            .replace("@PW", &self.panel_w.to_string())
            .replace("@PS", &(self.panel_w - 26).to_string())
            .replace("@MM", &self.map_margin)
            .replace("@MW", &self.map_w.to_string())
            .replace("@MS", &(self.map_w - 26).to_string())
            .replace("@C1", &self.cols.0.to_string())
            .replace("@C2", &self.cols.1.to_string())
            .replace("@C3", &self.cols.2.to_string())
            .replace("@TM", &self.thrust_margin)
    }
}

/// Build (or rebuild) every UI document for the current window shape.
/// Runs each frame; tears down and re-instantiates only when the MODE
/// flips (desktop <-> phone, portrait <-> landscape).
fn relayout_ui(world: &mut World) {
    let Some(model) = world.get_resource::<HudModel>().map(|m| m.0.clone()) else { return };
    let mut windows = world.query::<&Window>();
    let Some((w, h)) = windows.iter(world).next().map(|w| (w.width(), w.height())) else {
        return;
    };
    let mode = UiMode::of(w, h);
    let view = *world.resource::<crate::sim::ViewMode>();
    if world.resource::<UiLayoutState>().0 == Some((mode, view)) {
        return;
    }
    world.resource_mut::<UiLayoutState>().0 = Some((mode, view));
    info!("ui layout: {mode:?} / {view:?} ({w:.0}x{h:.0})");

    // Tear down the previous layout's documents.
    let old: Vec<Entity> = world
        .query_filtered::<Entity, With<UiRoot>>()
        .iter(world)
        .collect();
    for e in old {
        world.entity_mut(e).despawn();
    }

    let m = Metrics::for_mode(mode, w);
    enum Doc {
        Hud,
        Vessel,
        Map,
        Controls,
    }
    // The audit that decides what lives where: tactical carries the
    // management surface (sensors, meta, yard, map); the cockpit carries
    // only flight and fight.
    let cockpit = view == crate::sim::ViewMode::Cockpit;
    let mut docs = if cockpit {
        world.remove_resource::<VesselPanel>();
        world.remove_resource::<MapPanel>();
        // The cockpit console is a control surface on every device —
        // the Stitch design puts thrust and weapons ON the console.
        vec![
            (m.fill(HUD_COCKPIT_XAML), Doc::Hud),
            (m.fill(COCKPIT_TAPE_XAML), Doc::Hud),
            (m.fill(COCKPIT_RETICLE_XAML), Doc::Hud),
            (m.fill(COCKPIT_THREAT_XAML), Doc::Hud),
            (m.fill(COCKPIT_CONSOLE_XAML), Doc::Hud),
            (m.fill(TOUCH_THRUST_XAML), Doc::Controls),
            (m.fill(TOUCH_WEAPONS_XAML), Doc::Controls),
        ]
    } else {
        vec![
            (m.fill(HUD_XAML), Doc::Hud),
            (m.fill(PANEL_XAML), Doc::Vessel),
            (m.fill(MAP_XAML), Doc::Map),
        ]
    };
    if m.touch_controls {
        let topbar = if cockpit { TOUCH_TOPBAR_COCKPIT_XAML } else { TOUCH_TOPBAR_XAML };
        docs.push((m.fill(topbar), Doc::Controls));
        if !cockpit {
            docs.push((m.fill(TOUCH_THRUST_XAML), Doc::Controls));
            docs.push((m.fill(TOUCH_WEAPONS_XAML), Doc::Controls));
        }
    }
    for (xaml, doc) in docs {
        let scene = bevy_pf::XamlScene::parse(&xaml).expect("ui xaml is valid");
        let root = world.spawn(DataContext(model.clone())).id();
        if let Err(e) = bevy_pf::instantiate_document(world, root, &scene.document()) {
            error!("ui failed to instantiate: {e}");
        }
        // Instantiation replaces root components; re-attach the context.
        world.entity_mut(root).insert((DataContext(model.clone()), UiRoot));
        match doc {
            Doc::Hud => {
                // The HUD is a passive readout, but its root spans the
                // window and stack rows stretch full-width: with default
                // Pickable they swallow clicks aimed at panels and the
                // 3D scene beneath (found by click-path tracing). The
                // whole tree opts out of picking.
                ignore_picking_recursive(world, root);
            }
            Doc::Vessel => {
                world.entity_mut(root).insert(Visibility::Hidden);
                world.insert_resource(VesselPanel(root));
            }
            Doc::Map => {
                world.entity_mut(root).insert(Visibility::Hidden);
                world.insert_resource(MapPanel(root));
            }
            Doc::Controls => {
                // Wire each named button to its virtual key.
                let buttons: Vec<(Entity, KeyCode)> = world
                    .get::<XamlNames>(root)
                    .map(|names| {
                        TOUCH_KEYS
                            .iter()
                            .filter_map(|(n, k)| names.get(n).map(|e| (e, *k)))
                            .collect()
                    })
                    .unwrap_or_default();
                for (button, key) in buttons {
                    world.entity_mut(button).insert(TouchKey(key));
                }
            }
        }
    }
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
    // Grouped: bevy caps a system at 16 parameters.
    (game, atlas, style_res, raider_q, mut map_rows): (
        Res<crate::GameUniverse>,
        Res<crate::travel::SunAtlas>,
        Res<crate::sim::ShipStyle>,
        Query<(), With<crate::aliens::AlienShip>>,
        ResMut<crate::travel::MapRows>,
    ),
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
            NavState::Transfer { target, .. } => format!(">> TRANSFER: {}", name(target)),
            NavState::Orbiting { body, .. } => format!(">> ORBIT LOCK: {}", name(body)),
        }
    };
    model.0.set_nav(if flash.ttl > 0.0 {
        format!("++ {}", flash.text.to_uppercase())
    } else {
        nav_text
    });
    model.0.set_speed(format!("{:.1} KM/S", vel.0.length() / 1000.0));
    let hdg = (90.0 - vel.0.y.atan2(vel.0.x).to_degrees()).rem_euclid(360.0);
    model.0.set_heading(format!("<<  HDG {hdg:03.0}\u{00B0}  >>"));
    // Equality-checked setters: an unchanged readout costs nothing downstream.
    model.0.set_energy((ship.energy / ship.energy_max * 100.0).round());
    model.0.set_shield(ship.shield.round());
    model.0.set_hull(ship.hull.round());
    model.0.set_energy_text(format!("{:.0}/{:.0}", ship.energy, ship.energy_max));
    model.0.set_shield_text(format!("{:.0}/100", ship.shield));
    model.0.set_hull_text(format!("{:.0}/100", ship.hull));
    model.0.set_score(format!("{}", run.total()));
    model.0.set_best(format!("{}", career.best_run));
    model.0.set_level(format!(
        "LVL {}",
        crate::upgrades::pilot_level(career.total_score + run.total())
    ));
    model.0.set_salvage(format!("{} CR", run.salvage_value));

    model.0.set_style(style_res.label());
    let raiders = raider_q.iter().count();
    model.0.set_threat(if raiders > 0 {
        format!("!! {raiders} RAIDER{} IN-SYSTEM", if raiders == 1 { "" } else { "S" })
    } else {
        String::new()
    });
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
